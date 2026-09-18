//! Twitch/YouTube "auto-download new uploads" detection — the core-edition
//! counterpart to `monitor::patreon`'s per-creator auto-pull, but for the
//! two platforms whose channel-list poll already runs in `ChannelMonitor`.
//!
//! Patreon is deliberately absent here: it has its own dedicated poll loop
//! (`monitor::patreon::PatreonMonitor`) with its own watermark file, and
//! reuses `config.auto_pull_creators` as its opt-in list. Duplicating that
//! machinery for Twitch/YouTube would just be a second copy of the same
//! shape; instead `ChannelMonitor` calls into this module from its existing
//! `poll_all` loop.
//!
//! ## Interval
//!
//! The live channel-poll runs every ~90s across every followed channel
//! (~69 in the reference deployment). Re-fetching each opted-in channel's
//! upload list on that same cadence would multiply API cost for no benefit
//! — new uploads land on the order of hours, not seconds. Upload checks
//! instead run on their own `UPLOAD_CHECK_INTERVAL_SECS` (15 minutes)
//! timer, checked from inside `poll_all`, plus immediately whenever
//! `poll_all` is invoked from a YouTube WebSub `PollNow` (`force` = true) —
//! that call is itself already event-driven and cheap (opted-in channels
//! only), so there is no reason to make a real upload land minutes late.
//!
//! ## YouTube quota cost
//!
//! `Platform::fetch_channel_vods` for YouTube (`YouTubeClient::fetch_uploads_playlist`)
//! calls `playlistItems.list` against the channel's uploads playlist, not
//! `search.list`. `playlistItems.list` costs 1 quota unit per call
//! (vs. 100 for `search.list`), so even a dense follow list checked every
//! 15 minutes stays cheap against the default 10,000/day project quota.
//!
//! ## Twitch/YouTube past-broadcast rule
//!
//! A channel with `auto_record` on already gets its live broadcasts
//! captured directly (and optionally VOD-backfilled — see
//! `RecordingConfig::auto_vod_backfill`). When that channel's past
//! broadcast later shows up in `fetch_channel_vods` as a
//! `VodKind::LiveBroadcast`, it usually isn't a new upload — it's the same
//! stream StriVo already captured under a different code path.
//!
//! `auto_record` being *on* is only ever a hint, not proof: the live
//! recorder can miss a broadcast (daemon restart mid-stream, a capture
//! failure, or the channel going live before monitoring was ever turned
//! on for it). Blanket-skipping every `LiveBroadcast` VOD whenever
//! `auto_record` is on would silently lose those. `select_new_uploads`
//! therefore only skips a `LiveBroadcast` VOD when the journal actually
//! holds a live-capture recording (one with no `source_url` — that's what
//! distinguishes a live capture from a VOD/upload pull, see
//! `RecordingJob::source_url`) for that channel whose `started_at` falls
//! inside the broadcast's own window: `published_at - 30min` through
//! `published_at + duration + 30min`. The ±30 minutes covers scheduling
//! slop between when yt-dlp/ffmpeg actually attached and when the
//! platform stamps the VOD's publish time. `VodKind::Upload` (regular
//! uploads, highlights) is never skipped by this rule.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use crate::platform::{VodEntry, VodKind};

/// Padding on either side of a broadcast's `published_at`..`published_at +
/// duration` window when checking whether a live-capture recording's
/// `started_at` falls inside it. See the module doc's past-broadcast rule.
const BROADCAST_MATCH_PAD_MINS: i64 = 30;

/// True if `captured_starts` (the `started_at` of this channel's live-capture
/// recordings — see the module doc) contains one that falls inside `vod`'s
/// own broadcast window, padded by `BROADCAST_MATCH_PAD_MINS` on each side.
/// A VOD with no `published_at` can't be windowed, so it's never considered
/// captured by this check.
fn broadcast_was_captured(vod: &VodEntry, captured_starts: &[DateTime<Utc>]) -> bool {
    let Some(published) = vod.published_at else {
        return false;
    };
    let pad = chrono::Duration::minutes(BROADCAST_MATCH_PAD_MINS);
    let duration = vod
        .duration
        .and_then(|d| chrono::Duration::from_std(d).ok())
        .unwrap_or_default();
    let window_start = published - pad;
    let window_end = published + duration + pad;
    captured_starts
        .iter()
        .any(|started| *started >= window_start && *started <= window_end)
}

/// How often (seconds) opted-in channels are checked for new uploads.
/// See the module doc for the cost rationale.
pub const UPLOAD_CHECK_INTERVAL_SECS: u64 = 900;

/// Build the watermark map key. Shared by the monitor and the state file.
pub fn watermark_key(platform: crate::platform::PlatformKind, channel_id: &str) -> String {
    format!("{platform}:{channel_id}")
}

/// Decide which of a channel's recently-fetched VODs are genuinely new
/// uploads to auto-download.
///
/// Pure — no IO, no network — so it's unit-testable without a platform
/// client. `watermark` is `None` on a channel's first-ever check: in that
/// case nothing is "new" (a fresh opt-in must not trigger a back-catalog
/// stampede — the caller establishes the watermark from this call and
/// downloads begin on the next check). `captured_broadcast_starts` (the
/// channel's live-capture recordings' `started_at`s) drives the past-
/// broadcast rule described in the module doc — a `LiveBroadcast` VOD is
/// only skipped when one of them actually falls inside its window.
pub fn select_new_uploads(
    vods: &[VodEntry],
    watermark: Option<DateTime<Utc>>,
    captured_broadcast_starts: &[DateTime<Utc>],
) -> Vec<VodEntry> {
    let Some(watermark) = watermark else {
        return Vec::new();
    };
    vods.iter()
        .filter(|v| v.published_at.is_some_and(|p| p > watermark))
        .filter(|v| {
            !(v.kind == VodKind::LiveBroadcast
                && broadcast_was_captured(v, captured_broadcast_starts))
        })
        .cloned()
        .collect()
}

/// The watermark to store after a check: the newest `published_at` among
/// `vods`, never moving backwards. On a first check with no dated VODs the
/// watermark starts at now, so the channel stops being "first check" and
/// uploads published afterwards are picked up. Returns `None` when there is
/// nothing to change.
pub fn next_watermark(vods: &[VodEntry], current: Option<DateTime<Utc>>) -> Option<DateTime<Utc>> {
    let newest = vods.iter().filter_map(|v| v.published_at).max();
    match (current, newest) {
        (Some(cur), Some(n)) if n > cur => Some(n),
        (Some(_), _) => None,
        (None, Some(n)) => Some(n),
        (None, None) => Some(Utc::now()),
    }
}

pub type WatermarkMap = HashMap<String, DateTime<Utc>>;

pub fn load_watermarks(path: &Path) -> WatermarkMap {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_watermarks(path: &PathBuf, state: &WatermarkMap) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(state)?;
    std::fs::write(path, content)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::PlatformKind;

    fn vod(id: &str, published_secs_ago: i64, kind: VodKind) -> VodEntry {
        vod_with_duration(id, published_secs_ago, kind, None)
    }

    fn vod_with_duration(
        id: &str,
        published_secs_ago: i64,
        kind: VodKind,
        duration: Option<std::time::Duration>,
    ) -> VodEntry {
        VodEntry {
            id: id.into(),
            platform: PlatformKind::Twitch,
            channel_id: "ch1".into(),
            title: id.into(),
            published_at: Some(Utc::now() - chrono::Duration::seconds(published_secs_ago)),
            duration,
            url: format!("https://example/{id}"),
            thumbnail_url: None,
            kind,
        }
    }

    #[test]
    fn watermark_tracks_newest_published_not_wall_clock() {
        let cur = Utc::now() - chrono::Duration::seconds(3600);
        let vods = vec![
            vod("a", 1800, VodKind::Upload),
            vod("b", 7200, VodKind::Upload),
        ];
        let next = next_watermark(&vods, Some(cur)).unwrap();
        assert_eq!(Some(next), vods[0].published_at);
        // Nothing newer than the current watermark: leave it alone.
        assert_eq!(
            next_watermark(&[vod("old", 7200, VodKind::Upload)], Some(cur)),
            None
        );
        // First check on an empty channel starts the watermark.
        assert!(next_watermark(&[], None).is_some());
    }

    #[test]
    fn first_check_downloads_nothing() {
        let vods = vec![vod("a", 10, VodKind::Upload)];
        let picked = select_new_uploads(&vods, None, &[]);
        assert!(
            picked.is_empty(),
            "first poll must only establish a watermark"
        );
    }

    #[test]
    fn newer_vod_is_selected_once() {
        let watermark = Utc::now() - chrono::Duration::seconds(3600);
        let vods = vec![
            vod("new", 10, VodKind::Upload),
            vod("old", 7200, VodKind::Upload),
        ];
        let picked = select_new_uploads(&vods, Some(watermark), &[]);
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].id, "new");
    }

    #[test]
    fn live_broadcast_skipped_when_a_capture_falls_in_its_window() {
        let watermark = Utc::now() - chrono::Duration::seconds(3600);
        let v = vod("stream-vod", 10, VodKind::LiveBroadcast);
        // A live capture that started 5 minutes after publish is well
        // inside the padded window.
        let captured = [v.published_at.unwrap() + chrono::Duration::minutes(5)];
        let picked = select_new_uploads(&[v], Some(watermark), &captured);
        assert!(picked.is_empty());
    }

    #[test]
    fn live_broadcast_kept_when_no_capture_exists() {
        let watermark = Utc::now() - chrono::Duration::seconds(3600);
        let vods = vec![vod("stream-vod", 10, VodKind::LiveBroadcast)];
        // No captured starts at all — e.g. auto_record was off, or the
        // live recorder missed this broadcast despite being on.
        let picked = select_new_uploads(&vods, Some(watermark), &[]);
        assert_eq!(picked.len(), 1);
    }

    #[test]
    fn live_broadcast_kept_when_capture_falls_outside_the_window() {
        let watermark = Utc::now() - chrono::Duration::seconds(3600);
        let v = vod("stream-vod", 10, VodKind::LiveBroadcast);
        // A capture 2 hours away from publish is nowhere near this
        // broadcast — some other recording, not this one.
        let captured = [v.published_at.unwrap() - chrono::Duration::hours(2)];
        let picked = select_new_uploads(&[v], Some(watermark), &captured);
        assert_eq!(
            picked.len(),
            1,
            "an unrelated capture must not suppress a real miss"
        );
    }

    #[test]
    fn upload_kind_never_skipped_by_the_broadcast_rule() {
        let watermark = Utc::now() - chrono::Duration::seconds(3600);
        let v = vod("clip", 10, VodKind::Upload);
        let captured = [v.published_at.unwrap()];
        let picked = select_new_uploads(&[v], Some(watermark), &captured);
        assert_eq!(picked.len(), 1);
    }

    #[test]
    fn broadcast_capture_window_padding() {
        let published = Utc::now();
        let v = VodEntry {
            duration: Some(std::time::Duration::from_secs(3600)),
            ..vod_with_duration("live", 0, VodKind::LiveBroadcast, None)
        };
        let v = VodEntry {
            published_at: Some(published),
            ..v
        };
        // Just inside the padded start (published - 29min).
        assert!(broadcast_was_captured(
            &v,
            &[published - chrono::Duration::minutes(29)]
        ));
        // Just inside the padded end (published + 1h duration + 29min).
        assert!(broadcast_was_captured(
            &v,
            &[published + chrono::Duration::minutes(60 + 29)]
        ));
        // Outside on both sides.
        assert!(!broadcast_was_captured(
            &v,
            &[published - chrono::Duration::minutes(31)]
        ));
        assert!(!broadcast_was_captured(
            &v,
            &[published + chrono::Duration::minutes(60 + 31)]
        ));
        // No published_at at all — can't be windowed, never "captured".
        let undated = VodEntry {
            published_at: None,
            ..v
        };
        assert!(!broadcast_was_captured(&undated, &[published]));
    }

    #[test]
    fn watermark_round_trips_through_state_file() {
        let dir =
            std::env::temp_dir().join(format!("strivo-upload-wm-test-{}", uuid::Uuid::new_v4()));
        let path = dir.join("upload_watermarks.json");
        let mut state = WatermarkMap::new();
        state.insert("Twitch:ch1".to_string(), Utc::now());
        save_watermarks(&path, &state).unwrap();
        let loaded = load_watermarks(&path);
        assert_eq!(loaded.len(), 1);
        assert!(loaded.contains_key("Twitch:ch1"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
