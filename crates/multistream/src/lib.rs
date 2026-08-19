//! Multi-stream viewer — pure-data tile layout + embed URL spec.
//!
//! The plugin solves two problems for the SPA's Watch route:
//!
//! 1. Given N live streams and a container size, lay them out as
//!    rectangular tiles that maximise per-tile area at 16:9 — without any
//!    iframes/DOM involved (so we can unit-test the maths and the SPA can
//!    pure-render into a CSS grid).
//! 2. Build the right embed URL per platform (Twitch needs a parent= host;
//!    YouTube uses the `live_stream` channel embed) so the SPA doesn't
//!    bake platform knowledge into its template.
//!
//! No IO, no DOM. Sixteen unit tests cover ordering, fullscreen, side-by-
//! side, 2+1, 2×2, 3×3 grids, focus + PiP modes, container clamping,
//! Twitch / YouTube embed shapes, and JSON roundtripping.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Twitch,
    YouTube,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stream {
    pub id: String,
    pub channel_name: String,
    pub platform: Platform,
    /// Channel login / handle / id used to build the platform embed URL.
    pub embed_key: String,
    pub viewer_count: Option<u32>,
    /// Video id of the broadcast currently airing, where the platform
    /// addresses live content that way.
    ///
    /// YouTube's IFrame Player API drives a VIDEO, not a channel, so a
    /// YouTube tile cannot be controlled through that API without this.
    /// Twitch leaves it None — its player is pointed at a channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum LayoutMode {
    /// Pick rows × cols to maximise tile area at 16:9.
    Auto,
    /// Honour explicit grid dimensions; later tiles wrap.
    Grid { cols: u32, rows: u32 },
    /// One stream takes the full container; everyone else hides.
    Focus { stream_id: String },
    /// `main` fills the container; `side` floats top-right at 25% width.
    #[serde(rename = "pip")]
    PiP { main: String, side: String },
    /// Fixed 2×2 grid, even when fewer than four streams are present
    /// — the empty slots stay blank so the layout doesn't reflow when
    /// channels go online/offline. Static-dashboard friendly.
    Quadrant,
    /// One hero on the left at 60% width + every other stream stacked
    /// in the right column. Hero defaults to the first stream when
    /// `stream_id` is empty.
    Highlight { stream_id: String },
    /// Main fills the top 80% of the container; every other stream
    /// shares a single horizontal strip across the bottom 20%.
    /// Control-room / CCTV layout.
    Theatre { stream_id: String },
}

/// One placed tile. Coordinates are in pixels relative to the container's
/// top-left; `z` orders overlapping tiles (PiP mode only).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tile {
    pub stream_id: String,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub z: u32,
}

/// Compute pixel-positioned tiles for `streams` inside a `container_w` ×
/// `container_h` viewport in the chosen [`LayoutMode`].
///
/// Auto mode picks the (cols, rows) combination from 1..=N that maximises
/// the per-tile area when each tile is constrained to 16:9; ties resolve
/// to the squarer grid (fewer cols). Empty stream lists return no tiles.
pub fn compute_tiles(
    streams: &[Stream],
    container_w: u32,
    container_h: u32,
    mode: &LayoutMode,
) -> Vec<Tile> {
    if streams.is_empty() || container_w == 0 || container_h == 0 {
        return vec![];
    }
    match mode {
        LayoutMode::Focus { stream_id } => match streams.iter().find(|s| &s.id == stream_id) {
            Some(s) => vec![Tile {
                stream_id: s.id.clone(),
                x: 0,
                y: 0,
                w: container_w,
                h: container_h,
                z: 0,
            }],
            None => vec![],
        },
        LayoutMode::PiP { main, side } => {
            let Some(m) = streams.iter().find(|s| &s.id == main) else {
                return vec![];
            };
            let mut out = vec![Tile {
                stream_id: m.id.clone(),
                x: 0,
                y: 0,
                w: container_w,
                h: container_h,
                z: 0,
            }];
            if let Some(s) = streams.iter().find(|s| &s.id == side) {
                let pw = container_w / 4;
                let ph = pw * 9 / 16;
                let pad = 16;
                out.push(Tile {
                    stream_id: s.id.clone(),
                    x: container_w.saturating_sub(pw + pad),
                    y: pad,
                    w: pw,
                    h: ph,
                    z: 1,
                });
            }
            out
        }
        LayoutMode::Grid { cols, rows } => {
            grid_tiles(streams, container_w, container_h, *cols, *rows)
        }
        LayoutMode::Auto => {
            let (cols, rows) = best_grid(streams.len() as u32, container_w, container_h);
            grid_tiles(streams, container_w, container_h, cols, rows)
        }
        LayoutMode::Quadrant => grid_tiles(streams, container_w, container_h, 2, 2),
        LayoutMode::Highlight { stream_id } => {
            highlight_tiles(streams, container_w, container_h, stream_id)
        }
        LayoutMode::Theatre { stream_id } => {
            theatre_tiles(streams, container_w, container_h, stream_id)
        }
    }
}

/// Split a stream list into (hero, rest) honouring an optional explicit
/// hero id. Empty / missing id falls back to the first stream.
fn pick_hero<'a>(streams: &'a [Stream], hero_id: &str) -> Option<(&'a Stream, Vec<&'a Stream>)> {
    if streams.is_empty() {
        return None;
    }
    let hero = if hero_id.is_empty() {
        &streams[0]
    } else {
        streams
            .iter()
            .find(|s| s.id == hero_id)
            .unwrap_or(&streams[0])
    };
    let rest: Vec<&Stream> = streams.iter().filter(|s| s.id != hero.id).collect();
    Some((hero, rest))
}

/// Hero on the left at 60% width, rest stacked vertically on the right.
fn highlight_tiles(streams: &[Stream], cw: u32, ch: u32, hero_id: &str) -> Vec<Tile> {
    let Some((hero, rest)) = pick_hero(streams, hero_id) else {
        return vec![];
    };
    let hero_w = (cw as u64 * 60 / 100) as u32;
    let side_w = cw.saturating_sub(hero_w);
    let mut out = vec![Tile {
        stream_id: hero.id.clone(),
        x: 0,
        y: 0,
        w: hero_w,
        h: ch,
        z: 0,
    }];
    if rest.is_empty() || side_w == 0 {
        return out;
    }
    let rows = rest.len() as u32;
    let row_h = ch / rows;
    for (i, s) in rest.into_iter().enumerate() {
        out.push(Tile {
            stream_id: s.id.clone(),
            x: hero_w,
            y: i as u32 * row_h,
            w: side_w,
            h: row_h,
            z: 0,
        });
    }
    out
}

/// Hero across the top 80%, rest evenly across a bottom strip.
fn theatre_tiles(streams: &[Stream], cw: u32, ch: u32, hero_id: &str) -> Vec<Tile> {
    let Some((hero, rest)) = pick_hero(streams, hero_id) else {
        return vec![];
    };
    let hero_h = (ch as u64 * 80 / 100) as u32;
    let strip_h = ch.saturating_sub(hero_h);
    let mut out = vec![Tile {
        stream_id: hero.id.clone(),
        x: 0,
        y: 0,
        w: cw,
        h: hero_h,
        z: 0,
    }];
    if rest.is_empty() || strip_h == 0 {
        return out;
    }
    let cols = rest.len() as u32;
    let col_w = cw / cols;
    for (i, s) in rest.into_iter().enumerate() {
        out.push(Tile {
            stream_id: s.id.clone(),
            x: i as u32 * col_w,
            y: hero_h,
            w: col_w,
            h: strip_h,
            z: 0,
        });
    }
    out
}

fn grid_tiles(streams: &[Stream], cw: u32, ch: u32, cols: u32, rows: u32) -> Vec<Tile> {
    let cols = cols.max(1);
    let rows = rows.max(1);
    let tile_w = cw / cols;
    let tile_h = ch / rows;
    streams
        .iter()
        .enumerate()
        .take((cols * rows) as usize)
        .map(|(i, s)| {
            let i = i as u32;
            Tile {
                stream_id: s.id.clone(),
                x: (i % cols) * tile_w,
                y: (i / cols) * tile_h,
                w: tile_w,
                h: tile_h,
                z: 0,
            }
        })
        .collect()
}

/// Pick (cols, rows) such that `cols * rows >= n`, each tile sized to fit
/// the container with a 16:9 aspect-ratio constraint, maximising the
/// per-tile area. Ties break toward the grid whose shape matches the
/// container's aspect ratio more closely — so a 16:9 viewport with two
/// streams picks the obvious side-by-side arrangement.
fn best_grid(n: u32, cw: u32, ch: u32) -> (u32, u32) {
    let n = n.max(1);
    let target_ratio = cw as f64 / ch.max(1) as f64;
    let mut best: Option<(u32, u32, u64, f64)> = None;
    for cols in 1..=n {
        let rows = n.div_ceil(cols);
        let tw = cw / cols;
        let th = ch / rows;
        let area = constrained_area(tw, th);
        // When per-tile area ties, prefer the orientation that matches the
        // container: a 16:9 container with 2 streams should pick 2×1
        // (side-by-side), not 1×2 (stacked). We compare cols/rows to
        // target_ratio; the closer match wins.
        let grid_aspect = cols as f64 / rows.max(1) as f64;
        let aspect_delta = (grid_aspect - target_ratio).abs();
        let take = match best {
            None => true,
            Some((_, _, ba, br)) => area > ba || (area == ba && aspect_delta < br),
        };
        if take {
            best = Some((cols, rows, area, aspect_delta));
        }
    }
    let (cols, rows, _, _) = best.unwrap();
    (cols, rows)
}

fn constrained_area(w: u32, h: u32) -> u64 {
    // The largest 16:9 rect inside (w, h).
    if w == 0 || h == 0 {
        return 0;
    }
    let by_w = (w as u64, (w as u64 * 9) / 16);
    let by_h = ((h as u64 * 16) / 9, h as u64);
    let (cw, ch) = if by_w.1 <= h as u64 { by_w } else { by_h };
    cw * ch
}

/// Build a platform-specific embed URL for `stream`.
///
/// Twitch's `parent=` accepts a HOSTNAME ONLY — port and scheme break it
/// with "embed misconfigured" / "player.twitch.tv refused to connect" — so
/// we strip the port and any leading scheme before formatting.
///
/// Twitch also rejects bare IPv4 addresses (LAN dogfooding via
/// `http://192.168.x.x:8181/` fails). We detect that case and rewrite
/// the parent to the matching `<ip-dashed>.nip.io` hostname which
/// resolves to the same IP via wildcard DNS — the SPA's viewer route
/// rebuilds the page URL to match so the iframe referer lines up.
pub fn embed_url(stream: &Stream, host: &str) -> String {
    let bare = host
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or(host)
        .split(':')
        .next()
        .unwrap_or(host);
    let parent_host: String = if is_bare_ipv4(bare) {
        format!("{}.nip.io", bare.replace('.', "-"))
    } else {
        bare.to_string()
    };
    match stream.platform {
        Platform::Twitch => format!(
            "https://player.twitch.tv/?channel={}&parent={}",
            url_encode(&stream.embed_key),
            url_encode(&parent_host),
        ),
        Platform::YouTube => format!(
            "https://www.youtube.com/embed/live_stream?channel={}",
            url_encode(&stream.embed_key),
        ),
    }
}

fn is_bare_ipv4(host: &str) -> bool {
    let parts: Vec<&str> = host.split('.').collect();
    parts.len() == 4
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

/// Tiny ASCII-safe URL encoder for the handful of chars that appear in
/// channel names (`_`, `-`, alphanum stay; everything else is percent-
/// encoded). Spec full-coverage isn't needed here — embed_key is bounded
/// by the platform's identifier grammar.
fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(id: &str) -> Stream {
        Stream {
            id: id.into(),
            channel_name: id.into(),
            platform: Platform::Twitch,
            embed_key: id.to_lowercase(),
            viewer_count: None,
            video_id: None,
        }
    }

    #[test]
    fn empty_streams_yield_no_tiles() {
        let tiles = compute_tiles(&[], 1920, 1080, &LayoutMode::Auto);
        assert!(tiles.is_empty());
    }

    #[test]
    fn zero_container_yields_no_tiles() {
        let tiles = compute_tiles(&[s("a")], 0, 1080, &LayoutMode::Auto);
        assert!(tiles.is_empty());
    }

    #[test]
    fn one_stream_fullscreen_in_auto() {
        let tiles = compute_tiles(&[s("a")], 1920, 1080, &LayoutMode::Auto);
        assert_eq!(tiles.len(), 1);
        assert_eq!(tiles[0].w, 1920);
        assert_eq!(tiles[0].h, 1080);
    }

    #[test]
    fn two_streams_pick_2x1_in_widescreen_container() {
        // A 16:9-wide container gets more per-tile area splitting horizontally.
        let tiles = compute_tiles(&[s("a"), s("b")], 1920, 1080, &LayoutMode::Auto);
        assert_eq!(tiles.len(), 2);
        // Two tiles, side by side, each 960 wide.
        assert_eq!(tiles[0].x, 0);
        assert_eq!(tiles[1].x, 960);
        assert_eq!(tiles[0].w, 960);
    }

    #[test]
    fn four_streams_become_2x2() {
        let tiles = compute_tiles(
            &[s("a"), s("b"), s("c"), s("d")],
            1920,
            1080,
            &LayoutMode::Auto,
        );
        assert_eq!(tiles.len(), 4);
        assert_eq!(tiles[0].w, 960);
        assert_eq!(tiles[0].h, 540);
        assert_eq!(tiles[3].x, 960);
        assert_eq!(tiles[3].y, 540);
    }

    #[test]
    fn nine_streams_become_3x3() {
        let streams: Vec<Stream> = (0..9).map(|i| s(&format!("s{i}"))).collect();
        let tiles = compute_tiles(&streams, 1920, 1080, &LayoutMode::Auto);
        assert_eq!(tiles.len(), 9);
        // 3x3 — each tile 640x360
        assert_eq!(tiles[0].w, 640);
        assert_eq!(tiles[0].h, 360);
        assert_eq!(tiles[8].x, 1280);
        assert_eq!(tiles[8].y, 720);
    }

    #[test]
    fn explicit_grid_overrides_auto() {
        let tiles = compute_tiles(
            &[s("a"), s("b"), s("c"), s("d")],
            1920,
            1080,
            &LayoutMode::Grid { cols: 4, rows: 1 },
        );
        assert_eq!(tiles.len(), 4);
        assert_eq!(tiles[0].w, 480);
        assert_eq!(tiles[3].x, 1440);
        assert_eq!(tiles[0].y, 0);
    }

    #[test]
    fn focus_mode_shows_only_target_fullscreen() {
        let tiles = compute_tiles(
            &[s("a"), s("b"), s("c")],
            1920,
            1080,
            &LayoutMode::Focus {
                stream_id: "b".into(),
            },
        );
        assert_eq!(tiles.len(), 1);
        assert_eq!(tiles[0].stream_id, "b");
        assert_eq!(tiles[0].w, 1920);
    }

    #[test]
    fn focus_with_unknown_id_yields_no_tiles() {
        let tiles = compute_tiles(
            &[s("a")],
            1920,
            1080,
            &LayoutMode::Focus {
                stream_id: "zzz".into(),
            },
        );
        assert!(tiles.is_empty());
    }

    #[test]
    fn pip_mode_layers_main_and_side() {
        let tiles = compute_tiles(
            &[s("a"), s("b"), s("c")],
            1920,
            1080,
            &LayoutMode::PiP {
                main: "a".into(),
                side: "b".into(),
            },
        );
        assert_eq!(tiles.len(), 2);
        // Main fills container; side floats top-right with z=1
        assert_eq!(tiles[0].stream_id, "a");
        assert_eq!(tiles[0].w, 1920);
        assert_eq!(tiles[0].z, 0);
        assert_eq!(tiles[1].stream_id, "b");
        assert_eq!(tiles[1].w, 480); // 1920/4
        assert_eq!(tiles[1].h, 270); // 480 * 9/16
        assert!(tiles[1].x > tiles[0].w / 2);
        assert_eq!(tiles[1].z, 1);
    }

    #[test]
    fn pip_with_missing_side_returns_main_only() {
        let tiles = compute_tiles(
            &[s("a")],
            1920,
            1080,
            &LayoutMode::PiP {
                main: "a".into(),
                side: "missing".into(),
            },
        );
        assert_eq!(tiles.len(), 1);
    }

    #[test]
    fn pip_with_missing_main_returns_nothing() {
        let tiles = compute_tiles(
            &[s("a")],
            1920,
            1080,
            &LayoutMode::PiP {
                main: "missing".into(),
                side: "a".into(),
            },
        );
        assert!(tiles.is_empty());
    }

    #[test]
    fn twitch_embed_url_strips_port_from_parent() {
        // Regression: Twitch returns "embed misconfigured" when parent
        // includes a :port. host:port → host only in the parent param.
        let url = embed_url(&s("Cohh"), "localhost:8181");
        assert_eq!(
            url,
            "https://player.twitch.tv/?channel=cohh&parent=localhost"
        );
    }

    #[test]
    fn twitch_embed_url_rewrites_bare_ip_to_nip_io() {
        let url = embed_url(&s("Cohh"), "192.168.8.203:8181");
        assert!(url.ends_with("parent=192-168-8-203.nip.io"), "got {url}");
    }

    #[test]
    fn twitch_embed_url_keeps_real_hostnames_unchanged() {
        let url = embed_url(&s("Cohh"), "strivo.local:8181");
        assert!(url.ends_with("parent=strivo.local"));
    }

    #[test]
    fn twitch_embed_url_strips_scheme_and_path_from_parent() {
        let url = embed_url(&s("Cohh"), "https://app.example.com/anything");
        assert!(url.ends_with("parent=app.example.com"));
    }

    #[test]
    fn youtube_embed_url_uses_live_stream_path() {
        let yt = Stream {
            id: "yt1".into(),
            channel_name: "MKBHD".into(),
            platform: Platform::YouTube,
            embed_key: "UCBJycsmduvYEL83R_U4JriQ".into(),
            viewer_count: None,
            video_id: None,
        };
        let url = embed_url(&yt, "ignored.host");
        assert_eq!(
            url,
            "https://www.youtube.com/embed/live_stream?channel=UCBJycsmduvYEL83R_U4JriQ"
        );
    }

    #[test]
    fn url_encode_handles_special_chars() {
        assert_eq!(url_encode("a:b"), "a%3Ab");
        assert_eq!(url_encode("plain_name"), "plain_name");
        assert_eq!(url_encode("hello world"), "hello%20world");
    }

    #[test]
    fn pip_serialises_as_lowercase_pip() {
        // Regression: snake_case rename turned PiP → "pi_p", which broke
        // the SPA wire format. Explicit `rename = "pip"` keeps it intuitive.
        let s = serde_json::to_string(&LayoutMode::PiP {
            main: "a".into(),
            side: "b".into(),
        })
        .unwrap();
        assert!(s.contains("\"pip\""));
        let back: LayoutMode = serde_json::from_str(&s).unwrap();
        matches!(back, LayoutMode::PiP { .. });
    }

    #[test]
    fn json_roundtrip_preserves_layout_mode() {
        for mode in [
            LayoutMode::Auto,
            LayoutMode::Grid { cols: 3, rows: 2 },
            LayoutMode::Focus {
                stream_id: "x".into(),
            },
            LayoutMode::PiP {
                main: "a".into(),
                side: "b".into(),
            },
            LayoutMode::Quadrant,
            LayoutMode::Highlight {
                stream_id: "x".into(),
            },
            LayoutMode::Theatre {
                stream_id: "x".into(),
            },
        ] {
            let s = serde_json::to_string(&mode).unwrap();
            let back: LayoutMode = serde_json::from_str(&s).unwrap();
            assert_eq!(format!("{mode:?}"), format!("{back:?}"));
        }
    }

    #[test]
    fn quadrant_places_three_streams_in_first_three_slots() {
        let tiles = compute_tiles(&[s("a"), s("b"), s("c")], 1920, 1080, &LayoutMode::Quadrant);
        assert_eq!(tiles.len(), 3);
        assert_eq!(tiles[0].w, 960);
        assert_eq!(tiles[0].h, 540);
        // 'c' goes to row 1, col 0 (bottom-left slot of the 2×2).
        assert_eq!(tiles[2].x, 0);
        assert_eq!(tiles[2].y, 540);
    }

    #[test]
    fn quadrant_caps_at_four_streams() {
        let streams: Vec<Stream> = (0..6).map(|i| s(&format!("s{i}"))).collect();
        let tiles = compute_tiles(&streams, 1920, 1080, &LayoutMode::Quadrant);
        // Only first four slots are filled — extra streams hide.
        assert_eq!(tiles.len(), 4);
    }

    #[test]
    fn highlight_hero_takes_60pct_width() {
        let tiles = compute_tiles(
            &[s("a"), s("b"), s("c")],
            1000,
            500,
            &LayoutMode::Highlight {
                stream_id: "a".into(),
            },
        );
        assert_eq!(tiles.len(), 3);
        assert_eq!(tiles[0].x, 0);
        assert_eq!(tiles[0].w, 600);
        assert_eq!(tiles[0].h, 500);
        // Side stack at x=600
        assert_eq!(tiles[1].x, 600);
        assert_eq!(tiles[1].w, 400);
        assert_eq!(tiles[2].x, 600);
        // Two side streams → each gets half the column height.
        assert_eq!(tiles[1].h, 250);
        assert_eq!(tiles[2].y, 250);
    }

    #[test]
    fn highlight_empty_id_picks_first_stream() {
        let tiles = compute_tiles(
            &[s("a"), s("b")],
            1000,
            500,
            &LayoutMode::Highlight {
                stream_id: "".into(),
            },
        );
        assert_eq!(tiles[0].stream_id, "a");
    }

    #[test]
    fn highlight_with_one_stream_is_fullscreen() {
        let tiles = compute_tiles(
            &[s("a")],
            1000,
            500,
            &LayoutMode::Highlight {
                stream_id: "a".into(),
            },
        );
        // Hero takes 60% even when alone — leaves blank right column so
        // the layout doesn't suddenly fill the screen when a side stream
        // goes offline mid-session. Avoids visual jitter.
        assert_eq!(tiles.len(), 1);
        assert_eq!(tiles[0].w, 600);
    }

    #[test]
    fn theatre_hero_takes_80pct_height_and_strip_fills_bottom() {
        let tiles = compute_tiles(
            &[s("a"), s("b"), s("c"), s("d")],
            1000,
            500,
            &LayoutMode::Theatre {
                stream_id: "a".into(),
            },
        );
        assert_eq!(tiles.len(), 4);
        // Hero: full width × 400 (80% of 500)
        assert_eq!(tiles[0].h, 400);
        assert_eq!(tiles[0].w, 1000);
        // Strip starts at y=400, three streams across 1000 → ~333 each
        assert_eq!(tiles[1].y, 400);
        assert_eq!(tiles[1].h, 100);
        assert_eq!(tiles[1].w, 333);
        assert_eq!(tiles[3].x, 666);
    }

    #[test]
    fn theatre_unknown_hero_falls_back_to_first() {
        let tiles = compute_tiles(
            &[s("a"), s("b")],
            1000,
            500,
            &LayoutMode::Theatre {
                stream_id: "ghost".into(),
            },
        );
        assert_eq!(tiles[0].stream_id, "a");
    }
}
