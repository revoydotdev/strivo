//! Destination reframing and streamer layouts — report item E06.
//!
//! The gap `docs/CREATOR-EDITION-REPORT-2026-09-07.md:104` records is that a
//! short-form aspect string in a draft does not itself crop or render media.
//!
//! ## FALSIFIER (verbatim, as written before any of this existed)
//!
//! ```text
//! running 1 test
//! test a_nine_by_sixteen_draft_renders_nine_by_sixteen ... FAILED
//! thread '...' panicked at crates/editor/tests/destination_layout.rs:214:5:
//! assertion `left == right` failed: a draft whose aspect is 9:16 must render a 9:16 picture
//!   left: (96, 72)
//!  right: (720, 1280)
//! test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
//! ```
//!
//! That red was **behavioural**, not a missing import: the test compiled against
//! the code as it stood, and the render came back at the source's own 96×72
//! because nothing read the draft's `aspect` string. The two earlier runs
//! before it were not evidence and are not claimed as such — one failed on an
//! ffmpeg filtergraph error (the fixture had no audio stream for the composite
//! to map) and one on a malformed fixture argv. Only the run above tested the
//! claim.
//!
//! ## What is measured where
//!
//! * The **model** claims (typed refusal, override survival, crop arithmetic,
//!   region list) are exact integer assertions on the layout.
//! * The **render** claims are read off a decoded artifact — `ffprobe` for the
//!   frame, raw pixels for what happened to the gameplay and to the facecam.
//!   Todo 43 measured that this box's Chromium cannot decode the corpus at all,
//!   so every dimension here is the ffmpeg/ffprobe path, said plainly.
//! * What happened to non-fitting content is measured, not asserted: the
//!   fixture's gameplay carries four colour bands, the test classifies every
//!   output column against the fixture's own measured band luminas, and the
//!   resulting band widths are compared to the declared crop. A letterbox would
//!   show up as black columns and a scale-to-fit as every band present at full
//!   width; neither is present.
//!
//! Decode, not addressing: the fixtures are written all-intra (`-g 1`), so a
//! sampled frame is the frame at that instant for these files.

use std::path::{Path, PathBuf};
use std::process::Command;

use strivo_editor::destination::{
    AspectRatio, DestinationCapability, DestinationError, DestinationLayout, Rect, RegionRole,
    SourceSize,
};
use strivo_editor::render::{
    compile, export, CaptionMode, CaptionSpec, ClipRequest, Container, Coordinate, LayerKind,
    LayerSpec, OutputProfile, RenderRequest, TrackSpec, Transform, VideoIntent,
};
use strivo_editor::time::{Interval, MediaTime, Rate, SourceRef, SourceSpan, TimeBase};
use strivo_editor::{Cut, CutKind, Edl};

const GAMEPLAY_SECS: i64 = 2_000_000;
const SAMPLES: &str = "1.000000";

fn us(value: i64) -> MediaTime {
    MediaTime::from_micros(value).expect("non-negative media time")
}

fn span(source: &str, start: i64, end: i64) -> SourceSpan {
    SourceSpan::new(
        SourceRef::from_locator(source),
        TimeBase::from_rate(1, 1).expect("identity timebase"),
        Interval::new(us(start), us(end)).expect("ordered"),
        Rate::ONE,
    )
    .expect("non-empty source span")
}

fn tool(name: &str) -> bool {
    Command::new(name)
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn have_media_tools() -> bool {
    if !tool("ffmpeg") || !tool("ffprobe") {
        eprintln!("no media tool; the model assertions in this file still ran");
        return false;
    }
    true
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("strivo-e06-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

/// 4:3 gameplay carrying four vertical colour bands, so a decoded frame says
/// which part of the source survived a crop. A tone, because the composite
/// always maps an audio stream.
fn gameplay(dir: &Path) -> PathBuf {
    let path = dir.join("gameplay.mkv");
    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=96x72:d=2:r=10",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=2",
        ])
        .args([
            "-vf",
            "drawbox=x=0:y=0:w=24:h=72:color=red:t=fill,\
             drawbox=x=24:y=0:w=24:h=72:color=green:t=fill,\
             drawbox=x=48:y=0:w=24:h=72:color=blue:t=fill,\
             drawbox=x=72:y=0:w=24:h=72:color=yellow:t=fill,format=yuv420p",
            "-c:v",
            "libx264",
            "-g",
            "1",
            "-c:a",
            "aac",
            "-shortest",
        ])
        .arg(&path)
        .status()
        .expect("ffmpeg must be on PATH");
    assert!(
        status.success(),
        "failed to synthesize the gameplay fixture"
    );
    path
}

/// A solid magenta camera clip, all frames keyframes so a seek is frame-exact
/// for this file.
///
/// Authored at 30 fps on a 1/1000 timebase, which is the declared destination
/// frame: this render path has no conform stage for overlay inputs (see
/// `DestinationLayout::conform_filter`), so an overlay source has to arrive at
/// the frame it is drawn into.
fn facecam(dir: &Path) -> PathBuf {
    let path = dir.join("facecam.mkv");
    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=magenta:s=1280x720:d=2:r=30",
            "-vf",
            "format=yuv420p,settb=1/1000",
            "-c:v",
            "libx264",
            "-g",
            "1",
            "-an",
        ])
        .arg(&path)
        .status()
        .expect("ffmpeg must be on PATH");
    assert!(status.success(), "failed to synthesize the facecam fixture");
    path
}

/// One revision: gameplay as the programme lane, the facecam drawn over it.
fn request(gameplay: &Path, facecam: &Path) -> RenderRequest {
    let gameplay = gameplay.display().to_string();
    let facecam = facecam.display().to_string();
    RenderRequest {
        subject: "r1".into(),
        revision: "rev-1".into(),
        clips: vec![
            ClipRequest {
                cut: 0,
                span: span(&gameplay, 0, GAMEPLAY_SECS),
                fade_in: MediaTime::ZERO,
                // A one-millisecond fade puts the render on the transcode path;
                // a stream copy would be compared against nothing.
                fade_out: us(1_000),
                overlay_at: None,
            },
            ClipRequest {
                cut: 1,
                span: span(&facecam, 0, GAMEPLAY_SECS),
                fade_in: MediaTime::ZERO,
                fade_out: MediaTime::ZERO,
                overlay_at: Some(us(0)),
            },
        ],
        layers: vec![
            LayerSpec {
                id: "gameplay".into(),
                kind: LayerKind::Video,
                transform: Transform::default(),
                colour: Default::default(),
                clips: vec![0],
                visible: true,
                keyframes: Vec::new(),
            },
            LayerSpec {
                id: "facecam".into(),
                kind: LayerKind::Overlay,
                transform: Transform::default(),
                colour: Default::default(),
                clips: vec![1],
                visible: true,
                keyframes: Vec::new(),
            },
        ],
        transitions: Vec::new(),
        tracks: vec![TrackSpec {
            label: "programme".into(),
            bus: "master".into(),
            gain_db: 0.0,
            pan: 0.0,
            muted: false,
            solo: false,
            insert: String::new(),
            clips: vec![0],
        }],
        effects: Vec::new(),
        loudness: None,
        captions: CaptionSpec {
            mode: CaptionMode::None,
            style_json: String::new(),
            coordinate: Coordinate::Source,
            safe_area: true,
            cues: Vec::new(),
        },
        video: VideoIntent {
            filter_complex: String::new(),
            colour: None,
            frames: None,
        },
        container: Container {
            extension: "mkv".into(),
            video_codec: "ffv1".into(),
            audio_codec: "flac".into(),
            stream_copy: false,
        },
        profile: OutputProfile {
            name: "test".into(),
            renderer: "ffmpeg".into(),
            renderer_version: "n0".into(),
        },
    }
}

fn locators(gameplay: &Path, facecam: &Path) -> Edl {
    Edl {
        recording_id: "r1".into(),
        cuts: vec![
            Cut {
                start_sec: 0.0,
                end_sec: GAMEPLAY_SECS as f32 / 1e6,
                kind: CutKind::Source {
                    source_path: gameplay.display().to_string(),
                },
                fade_in_sec: 0.0,
                fade_out_sec: 0.0,
            },
            Cut {
                start_sec: 0.0,
                end_sec: GAMEPLAY_SECS as f32 / 1e6,
                kind: CutKind::Broll {
                    broll_path: facecam.display().to_string(),
                },
                fade_in_sec: 0.0,
                fade_out_sec: 0.0,
            },
        ],
    }
}

const GAMEPLAY_SIZE: SourceSize = SourceSize {
    width: 96,
    height: 72,
};

/// The declared destination frame, read back off a rendered file.
fn dimensions(path: &Path) -> (u32, u32) {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "csv=p=0:s=x",
        ])
        .arg(path)
        .output()
        .expect("ffprobe must be on PATH");
    assert!(out.status.success(), "probing {} failed", path.display());
    let text = String::from_utf8_lossy(&out.stdout);
    let (w, h) = text.trim().split_once('x').expect("w x h from ffprobe");
    (
        w.trim().parse().expect("width"),
        h.trim().parse().expect("height"),
    )
}

/// One decoded RGB frame and its luma per column.
fn frame(path: &Path) -> (u32, u32, Vec<u8>, Vec<u8>) {
    let probe = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "csv=p=0:s=x",
        ])
        .arg(path)
        .output()
        .expect("ffprobe must be on PATH");
    let text = String::from_utf8_lossy(&probe.stdout);
    let (w, h) = text.trim().split_once('x').expect("w x h");
    let (width, height): (u32, u32) = (w.trim().parse().expect("w"), h.trim().parse().expect("h"));

    let out = Command::new("ffmpeg")
        .args(["-v", "error", "-ss", SAMPLES, "-i"])
        .arg(path)
        .args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
        .output()
        .expect("ffmpeg decode");
    assert!(
        out.status.success(),
        "decoding {} at {SAMPLES} failed",
        path.display()
    );
    assert_eq!(
        out.stdout.len() as u32,
        width * height * 3,
        "a partial frame was decoded from {}",
        path.display()
    );
    // The luma of the middle row per column. Four vertical bands means one row
    // identifies the band; the middle row avoids any edge ffmpeg might soften.
    let row = (height / 2) as usize;
    let luma: Vec<u8> = (0..width as usize)
        .map(|x| {
            let i = (row * width as usize + x) * 3;
            let (r, g, b) = (
                f32::from(out.stdout[i]),
                f32::from(out.stdout[i + 1]),
                f32::from(out.stdout[i + 2]),
            );
            (0.299 * r + 0.587 * g + 0.114 * b).round() as u8
        })
        .collect();
    (width, height, out.stdout, luma)
}

/// The four band colours of the *source* fixture, read from its own pixels so
/// the classification below is calibrated rather than hard-coded.
///
/// Classification is nearest-colour in RGB, not on luma: red and green as
/// yuv420p carries them here differ by one luma step, so a luma classifier
/// alternates between the two across a whole band.
fn band_refs(gameplay: &Path) -> [[u8; 3]; 4] {
    let (width, _, rgb, _) = frame(gameplay);
    assert_eq!(width, 96, "the gameplay fixture is 96 px wide");
    let row = 36 * width as usize;
    [12usize, 36, 60, 84].map(|x| {
        let i = (row + x) * 3;
        [rgb[i], rgb[i + 1], rgb[i + 2]]
    })
}

/// The RGB triple of every column in one output row.
fn row_pixels(path: &Path, y: usize) -> Vec<[u8; 3]> {
    let (width, _, rgb, _) = frame(path);
    (0..width as usize)
        .map(|x| {
            let i = (y * width as usize + x) * 3;
            [rgb[i], rgb[i + 1], rgb[i + 2]]
        })
        .collect()
}

/// Runs of equal-class columns, as `(band, width_in_px)`.
///
/// A run narrower than 8 px is dropped: those are the chroma-scaled seams where
/// two bands meet, not a band.
fn band_runs(row: &[[u8; 3]], reference: [[u8; 3]; 4]) -> Vec<(usize, u32)> {
    let class = |pixel: [u8; 3]| -> usize {
        let mut best = 0usize;
        let mut best_gap = u64::MAX;
        for (index, want) in reference.iter().enumerate() {
            let gap: u64 = pixel
                .iter()
                .zip(want.iter())
                .map(|(a, b)| {
                    let d = u64::from(a.abs_diff(*b));
                    d * d
                })
                .sum();
            if gap < best_gap {
                best_gap = gap;
                best = index;
            }
        }
        best
    };
    let mut runs: Vec<(usize, u32)> = Vec::new();
    for pixel in row {
        let this = class(*pixel);
        match runs.last_mut() {
            Some(last) if last.0 == this => last.1 += 1,
            _ => runs.push((this, 1)),
        }
    }
    runs.retain(|(_, width_px)| *width_px >= 8);
    runs
}

/// The bounding box of the magenta camera in a decoded frame, in pixels.
fn magenta_box(path: &Path) -> Option<(u32, u32, u32, u32)> {
    let (width, height, rgb, _) = frame(path);
    let mut bounds: Option<(u32, u32, u32, u32)> = None;
    for y in 0..height as usize {
        for x in 0..width as usize {
            let i = (y * width as usize + x) * 3;
            let (r, g, b) = (rgb[i], rgb[i + 1], rgb[i + 2]);
            if r > 170 && b > 170 && g < 110 {
                bounds = Some(match bounds {
                    None => (x as u32, y as u32, x as u32, y as u32),
                    Some((x0, y0, x1, y1)) => (
                        x0.min(x as u32),
                        y0.min(y as u32),
                        x1.max(x as u32),
                        y1.max(y as u32),
                    ),
                });
            }
        }
    }
    bounds.map(|(x0, y0, x1, y1)| (x0, y0, x1 - x0 + 1, y1 - y0 + 1))
}

/// Render one revision through one layout. Each ratio is its own attempt and
/// its own file — todo 30's rule, and the reason three renders cannot destroy
/// each other.
fn render(layout: &DestinationLayout, gameplay: &Path, facecam: &Path, out: &Path) -> bool {
    let mut request = request(gameplay, facecam);
    layout
        .apply(&mut request, GAMEPLAY_SIZE)
        .expect("a generated layout applies");
    export(&request, &locators(gameplay, facecam), out, None).is_ok()
}

// ── the typed refusal ──────────────────────────────────────────────────

/// A draft's aspect string is resolved by exactly one function, and anything
/// that is not one of the three renderable shapes is an error rather than a
/// field that is quietly dropped.
#[test]
fn an_unrenderable_draft_aspect_is_refused_by_name() {
    // `strivo_reuse::Format::aspect` returns exactly these two for the podcast
    // and blog formats, and "4:3" is what a creator types by hand.
    for found in ["audio", "text", "4:3", "", "9:17", "916", "vertical"] {
        assert!(
            matches!(
                DestinationLayout::from_draft_aspect(found, us(1_000_000), None),
                Err(DestinationError::UnknownAspect { .. })
            ),
            "{found:?} must be refused, not defaulted"
        );
    }
    // Surrounding whitespace is trimmed rather than refused: a draft row read
    // out of a spreadsheet carries it, and a trailing space is not a different
    // destination.
    assert!(
        DestinationLayout::from_draft_aspect(" 1:1 ", us(1_000_000), None).is_ok(),
        "surrounding whitespace is trimmed, not refused"
    );
    assert_eq!(
        AspectRatio::parse("9:16").expect("a short-form aspect resolves"),
        AspectRatio::Portrait
    );
}

// ── no universal duration cap ──────────────────────────────────────────

/// A layout holds no duration limit. The cap is injected, absent until todo 60
/// supplies one, and a three-hour render is not refused without it.
#[test]
fn no_destination_capability_means_no_duration_limit() {
    let three_hours = us(3 * 60 * 60 * 1_000_000);
    let layout = DestinationLayout::from_draft_aspect("9:16", three_hours, None)
        .expect("with no capability injected, a three-hour layout is legal");
    assert_eq!(layout.aspect, AspectRatio::Portrait);

    // And the same duration is refused the moment a capability states a limit,
    // which is what makes the assertion above mean "no cap", not "no check".
    let short_form = DestinationCapability {
        max_duration: Some(us(60_000_000)),
    };
    assert!(matches!(
        DestinationLayout::from_draft_aspect("9:16", three_hours, Some(&short_form)),
        Err(DestinationError::DurationExceedsCapability { .. })
    ));
    assert!(
        DestinationLayout::from_draft_aspect("9:16", us(30_000_000), Some(&short_form)).is_ok(),
        "a capability that allows the duration must not refuse it"
    );
    assert!(
        DestinationLayout::from_draft_aspect(
            "9:16",
            three_hours,
            Some(&DestinationCapability { max_duration: None })
        )
        .is_ok(),
        "a capability that states no limit states no limit"
    );
}

// ── regions are a list ─────────────────────────────────────────────────

/// Two facecams and a demo is expressible because a role is attached to an
/// entry in a list, not held as a flag on the layout.
#[test]
fn two_facecams_and_a_demo_are_expressible() {
    let mut layout = DestinationLayout::for_aspect(AspectRatio::Landscape);
    let (width, height) = AspectRatio::Landscape.frame();
    layout.regions.extend([
        strivo_editor::destination::Region {
            id: "facecam-2".into(),
            role: RegionRole::Facecam,
            rect: Rect {
                x: 32,
                y: 32,
                w: 200,
                h: 150,
            },
            z: 11,
        },
        strivo_editor::destination::Region {
            id: "demo".into(),
            role: RegionRole::Demo,
            rect: Rect {
                x: 300,
                y: 300,
                w: 400,
                h: 225,
            },
            z: 12,
        },
    ]);
    let transforms = layout.region_transforms();
    let ids: Vec<&str> = transforms.iter().map(|(id, _)| *id).collect();
    assert_eq!(
        ids,
        vec!["facecam", "facecam-2", "demo"],
        "three overlays over the gameplay, in paint order"
    );
    assert_eq!(
        transforms[2].1.offset_px,
        (300, 300),
        "the demo region is placed where the layout says"
    );
    assert!(layout
        .regions
        .iter()
        .any(|r| r.role == RegionRole::Gameplay));
    assert!(!layout
        .region_transforms()
        .iter()
        .any(|(id, _)| *id == "gameplay"));
    assert_eq!(width * height, 1280 * 720);
}

/// Focus names a region, so a layout can follow a speaker's camera.
#[test]
fn a_focus_on_an_unknown_region_is_refused() {
    let mut layout = DestinationLayout::for_aspect(AspectRatio::Square);
    assert!(matches!(
        layout.set_focus(Some("nobody")),
        Err(DestinationError::UnknownRegion { .. })
    ));
    layout
        .set_focus(Some("facecam"))
        .expect("a real region focuses");
    assert_eq!(layout.focus.as_deref(), Some("facecam"));
}

// ── the crop decision, on the model ────────────────────────────────────

/// The stated policy, as arithmetic: every ratio crops to fill, and each one
/// crops the axis a 4:3 source does not fit on. Nothing is letterboxed.
#[test]
fn a_non_fitting_source_is_cropped_on_the_axis_it_does_not_fit() {
    let cases = [
        (
            AspectRatio::Landscape,
            96,
            54,
            "rows go: 4:3 is too tall for 16:9",
        ),
        (
            AspectRatio::Portrait,
            40,
            72,
            "columns go: 4:3 is too wide for 9:16",
        ),
        (
            AspectRatio::Square,
            72,
            72,
            "columns go: 4:3 is too wide for 1:1",
        ),
    ];
    for (aspect, w, h, why) in cases {
        let layout = DestinationLayout::for_aspect(aspect);
        let crop = layout.crop(GAMEPLAY_SIZE);
        assert_eq!((crop.w, crop.h), (w, h), "{why}");
        assert!(
            crop.w.is_multiple_of(2) && crop.h.is_multiple_of(2),
            "yuv420p needs even edges"
        );
        assert!(crop.x % 2 == 0 && crop.y % 2 == 0, "even chroma origin");
        assert!(
            crop.x + i64::from(crop.w) <= 96 && crop.y + i64::from(crop.h) <= 72,
            "the crop must be inside the source"
        );
    }
}

/// Focus moves the crop; without it the crop is centred.
#[test]
fn focusing_a_region_moves_the_crop() {
    let mut layout = DestinationLayout::for_aspect(AspectRatio::Portrait);
    let centred = layout.crop(GAMEPLAY_SIZE);
    assert_eq!(centred.x, 28, "a 40 px window in a 96 px source, centred");

    // A camera parked at the far left pulls the window left with it.
    layout
        .override_region(
            "facecam",
            Rect {
                x: 0,
                y: 32,
                w: 200,
                h: 200,
            },
        )
        .expect("a facecam in the frame");
    layout
        .set_focus(Some("facecam"))
        .expect("focus the facecam");
    let focused = layout.crop(GAMEPLAY_SIZE);
    assert_eq!(
        focused.x, 0,
        "focus on a left-hand region pulls the crop to the source's left edge"
    );
}

// ── a manual override survives regeneration ────────────────────────────

/// The override is recorded, and a regeneration *after* it still has it in
/// effect. Regenerating before setting it, or never regenerating, would pass
/// this test for free — so the order here is the assertion.
#[test]
fn a_manual_override_survives_a_later_regeneration() {
    let mut layout = DestinationLayout::for_aspect(AspectRatio::Landscape);
    let moved = Rect {
        x: 40,
        y: 300,
        w: 300,
        h: 200,
    };
    layout.override_region("facecam", moved).expect("override");

    for aspect in [
        AspectRatio::Landscape,
        AspectRatio::Portrait,
        AspectRatio::Square,
        AspectRatio::Landscape,
    ] {
        let regenerated = layout.regenerate(aspect);
        let facecam = regenerated
            .regions
            .iter()
            .find(|r| r.id == "facecam")
            .expect("the facecam region survives a regeneration");
        assert_eq!(
            facecam.rect, moved,
            "the override must still be in effect after regenerating for {aspect:?}"
        );
        assert_eq!(regenerated.overrides.get("facecam"), Some(&moved));
    }

    // And the regenerated layout is still that ratio, not the one it came from.
    let portrait = layout.regenerate(AspectRatio::Portrait);
    assert_eq!(portrait.aspect, AspectRatio::Portrait);
    assert_eq!(portrait.aspect.frame(), (720, 1280));
    assert_ne!(
        portrait
            .regions
            .iter()
            .find(|r| r.id == "facecam")
            .expect("facecam"),
        &DestinationLayout::for_aspect(AspectRatio::Portrait)
            .regions
            .into_iter()
            .find(|r| r.id == "facecam")
            .expect("facecam"),
        "a regeneration that merely kept the old layout would pass the assertions above"
    );
}

/// An override the new frame cannot hold is kept but not applied, so nothing is
/// silently clamped onto the frame.
#[test]
fn an_override_that_no_longer_fits_is_kept_but_not_applied() {
    let mut layout = DestinationLayout::for_aspect(AspectRatio::Landscape);
    let wide = Rect {
        x: 900,
        y: 400,
        w: 320,
        h: 180,
    };
    layout.override_region("facecam", wide).expect("override");
    let square = layout.regenerate(AspectRatio::Square);
    let facecam = square
        .regions
        .iter()
        .find(|r| r.id == "facecam")
        .expect("facecam");
    assert_ne!(facecam.rect, wide, "x=900 is off a 720 px frame");
    assert!(
        facecam.rect.inside(AspectRatio::Square.frame()),
        "and the applied rect is the regenerated default, which fits"
    );
    assert_eq!(
        square.overrides.get("facecam"),
        Some(&wide),
        "the user's placement is still recorded, so going back to 16:9 restores it"
    );
    let back = square.regenerate(AspectRatio::Landscape);
    assert_eq!(
        back.regions
            .iter()
            .find(|r| r.id == "facecam")
            .expect("facecam")
            .rect,
        wide
    );
}

/// An override off the frame, or naming a region that does not exist, is
/// refused rather than clamped.
#[test]
fn an_impossible_override_is_refused() {
    let mut layout = DestinationLayout::for_aspect(AspectRatio::Square);
    assert!(matches!(
        layout.override_region(
            "facecam",
            Rect {
                x: 700,
                y: 0,
                w: 100,
                h: 100
            }
        ),
        Err(DestinationError::RegionOutOfFrame { .. })
    ));
    assert!(matches!(
        layout.override_region(
            "ghost",
            Rect {
                x: 0,
                y: 0,
                w: 10,
                h: 10
            }
        ),
        Err(DestinationError::UnknownRegion { .. })
    ));
    assert!(
        layout.overrides.is_empty(),
        "a refused override records nothing"
    );
}

/// A layer that names no region keeps the transform it already had, so a B-roll
/// layer composed in the same plan is not moved by a destination layout.
#[test]
fn a_layer_that_names_no_region_is_left_alone() {
    let mut request = request(Path::new("/gameplay.mkv"), Path::new("/facecam.mkv"));
    request.layers.push(LayerSpec {
        id: "broll".into(),
        kind: LayerKind::Overlay,
        transform: Transform {
            scale: Some((64, 64)),
            offset_px: (7, 9),
            z: 20,
        },
        colour: Default::default(),
        clips: Vec::new(),
        visible: true,
        keyframes: Vec::new(),
    });
    DestinationLayout::for_aspect(AspectRatio::Square)
        .apply(&mut request, GAMEPLAY_SIZE)
        .expect("applies");
    let broll = request
        .layers
        .iter()
        .find(|l| l.id == "broll")
        .expect("broll");
    assert_eq!(broll.transform.offset_px, (7, 9));
    assert_eq!(broll.transform.scale, Some((64, 64)));
}

// ── the plan carries the destination, and no aspect string ─────────────

/// The compiled plan carries the destination frame as numbers, and the plan's
/// own serialisation contains no aspect string — the structural half of "no
/// draft-only aspect string survives into a render".
#[test]
fn the_plan_carries_the_frame_as_numbers_and_no_aspect_string() {
    for aspect in AspectRatio::ALL {
        let mut request = request(Path::new("/gameplay.mkv"), Path::new("/facecam.mkv"));
        DestinationLayout::for_aspect(aspect)
            .apply(&mut request, GAMEPLAY_SIZE)
            .expect("applies");
        let plan = compile(&request).expect("compiles");
        let (width, height) = aspect.frame();
        let chain = &plan.video_filter_complex;
        assert!(
            chain.contains(&format!("scale={width}:{height}")),
            "{aspect:?}: the plan must conform to the declared frame, got {chain}"
        );
        assert!(
            plan.frames.is_none(),
            "{aspect:?}: the chain carries its own conform, so a second one would \
             be a second consumer of the same ffmpeg label"
        );
        let json = serde_json::to_string(&plan).expect("the plan serialises");
        for label in ["16:9", "9:16", "1:1"] {
            assert!(
                !json.contains(label),
                "the aspect string {label:?} reached the render plan"
            );
        }
    }
}

/// A request that already carries a video graph is refused rather than
/// overwritten — dropping a branding chain silently is the failure this guards.
#[test]
fn a_layout_refuses_to_overwrite_an_existing_video_graph() {
    let mut request = request(Path::new("/gameplay.mkv"), Path::new("/facecam.mkv"));
    request.video.filter_complex = "[0:v]drawtext=text=BRAND[vout]".into();
    assert!(matches!(
        DestinationLayout::for_aspect(AspectRatio::Square).apply(&mut request, GAMEPLAY_SIZE),
        Err(DestinationError::CallerVideoGraph { .. })
    ));
    assert_eq!(
        request.video.filter_complex, "[0:v]drawtext=text=BRAND[vout]",
        "the refusal must leave the caller's graph exactly as it was"
    );
}

// ── the decoded artifact ───────────────────────────────────────────────

/// The acceptance criterion: each of the three ratios renders and decodes to
/// the dimensions it declared, with the facecam painted where the layout put it.
#[test]
fn each_ratio_renders_at_its_declared_frame_with_a_facecam_over_gameplay() {
    if !have_media_tools() {
        return;
    }
    let dir = scratch("three");
    let gameplay = gameplay(&dir);
    let facecam = facecam(&dir);

    for aspect in AspectRatio::ALL {
        let layout = DestinationLayout::for_aspect(aspect);
        let declared = layout.gameplay().expect("gameplay").rect;
        assert_eq!(
            (declared.w, declared.h),
            aspect.frame(),
            "the gameplay region fills the declared frame"
        );
        let out = dir.join(format!("{aspect:?}.mkv").to_lowercase());
        assert!(
            render(&layout, &gameplay, &facecam, &out),
            "{aspect:?} must render"
        );
        assert_eq!(
            dimensions(&out),
            aspect.frame(),
            "{aspect:?} must decode to the dimensions it declared"
        );

        let facecam_rect = layout
            .regions
            .iter()
            .find(|r| r.id == "facecam")
            .expect("facecam region")
            .rect;
        let painted = magenta_box(&out).expect("the facecam is visible");
        assert_eq!(
            painted,
            (
                facecam_rect.x as u32,
                facecam_rect.y as u32,
                facecam_rect.w,
                facecam_rect.h
            ),
            "{aspect:?}: the facecam must be painted exactly where the layout put it"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// What happens to gameplay that does not fit, measured on decoded pixels: the
/// four source bands survive in the widths the declared crop predicts, and no
/// column is black — a letterbox would be exactly that.
#[test]
fn a_non_fitting_source_is_cropped_not_lettered() {
    if !have_media_tools() {
        return;
    }
    let dir = scratch("crop");
    let gameplay = gameplay(&dir);
    let facecam = facecam(&dir);
    let reference = band_refs(&gameplay);

    for aspect in AspectRatio::ALL {
        let layout = DestinationLayout::for_aspect(aspect);
        let crop = layout.crop(GAMEPLAY_SIZE);
        // The expectation is derived from the layout's own crop rather than
        // written down, so a change to the crop policy fails here instead of
        // silently agreeing with itself.
        let out = dir.join(format!("crop-{aspect:?}.mkv").to_lowercase());
        assert!(
            render(&layout, &gameplay, &facecam, &out),
            "{aspect:?} renders"
        );
        let (width, height, _, _) = frame(&out);
        assert_eq!((width, height), aspect.frame());

        let want = expected_runs(&crop, width);
        let scale_x = f64::from(width) / f64::from(crop.w);
        let row = row_pixels(&out, row_below_facecam(&layout));
        let runs = band_runs(&row, reference);
        assert_eq!(
            runs.iter().map(|(b, _)| *b).collect::<Vec<_>>(),
            want.iter().map(|(b, _)| *b).collect::<Vec<_>>(),
            "{aspect:?}: which bands survived the crop"
        );
        for (index, (band, width_px)) in runs.iter().enumerate() {
            let want_width = want[index].1;
            let tolerance = (want_width / 20).max(4);
            assert!(
                width_px.abs_diff(want_width) <= tolerance,
                "{aspect:?}: band {band} is {width_px} px, the declared crop predicts \
                 about {want_width} px (source window x={}..{}, scaled by {scale_x:.2})",
                crop.x,
                crop.x + i64::from(crop.w)
            );
        }
        assert!(
            row.iter()
                .all(|pixel| pixel.iter().any(|channel| *channel > 40)),
            "{aspect:?}: a black column would be a letterbox, and the policy is crop-to-fill"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// What the crop predicts for the four source bands, in output pixels: each
/// band's overlap with the crop window, scaled to the output width, with
/// zero-overlap bands dropped.
fn expected_runs(crop: &Rect, out_width: u32) -> Vec<(usize, u32)> {
    let edges = [0i64, 24, 48, 72, 96];
    edges
        .windows(2)
        .enumerate()
        .filter_map(|(band, pair)| {
            let lo = pair[0].max(crop.x);
            let hi = pair[1].min(crop.x + i64::from(crop.w));
            let overlap = hi - lo;
            (overlap > 0).then(|| {
                let width_px = (overlap * i64::from(out_width) / i64::from(crop.w)) as u32;
                (band, width_px)
            })
        })
        .filter(|(_, width_px)| *width_px >= 8)
        .collect()
}

#[test]
fn the_predicted_bands_are_the_ones_the_crop_selects() {
    // The helper above is the expectation, so it needs its own check: if it
    // returned nothing, every "which bands survived" assertion would pass for
    // free.
    for (aspect, want) in [
        (
            AspectRatio::Landscape,
            vec![(0, 320), (1, 320), (2, 320), (3, 320)],
        ),
        (AspectRatio::Portrait, vec![(1, 360), (2, 360)]),
        (
            AspectRatio::Square,
            vec![(0, 120), (1, 240), (2, 240), (3, 120)],
        ),
    ] {
        let crop = DestinationLayout::for_aspect(aspect).crop(GAMEPLAY_SIZE);
        assert_eq!(
            expected_runs(&crop, aspect.frame().0),
            want,
            "{aspect:?}: the prediction helper must not be vacuous"
        );
    }
}

/// An output row inside the gameplay and clear of the camera, so a band
/// measurement is never the camera's colour.
fn row_below_facecam(layout: &DestinationLayout) -> usize {
    let (_, height) = layout.aspect.frame();
    let bottom = layout
        .regions
        .iter()
        .find(|r| r.id == "facecam")
        .map(|r| r.rect.y + i64::from(r.rect.h))
        .unwrap_or(0);
    ((bottom + 8).max(0) as u32).min(height.saturating_sub(2)) as usize
}

/// A focus is a crop decision, so it changes the rendered picture: the same
/// source and the same 9:16 frame show different bands when the crop follows a
/// left-hand camera.
#[test]
fn focusing_a_region_changes_what_the_render_crops() {
    if !have_media_tools() {
        return;
    }
    let dir = scratch("focus");
    let gameplay = gameplay(&dir);
    let facecam = facecam(&dir);
    let reference = band_refs(&gameplay);

    let centred = DestinationLayout::for_aspect(AspectRatio::Portrait);
    let mut focused = DestinationLayout::for_aspect(AspectRatio::Portrait);
    focused
        .override_region(
            "facecam",
            Rect {
                x: 0,
                y: 32,
                w: 200,
                h: 200,
            },
        )
        .expect("override");
    focused.set_focus(Some("facecam")).expect("focus");
    assert_ne!(
        centred.crop(GAMEPLAY_SIZE).x,
        focused.crop(GAMEPLAY_SIZE).x,
        "the two layouts must crop differently, or this renders the same file twice"
    );

    let mut observed: Vec<Vec<usize>> = Vec::new();
    for (name, layout) in [("centred", centred), ("focused", focused)] {
        let out = dir.join(format!("focus-{name}.mkv"));
        assert!(render(&layout, &gameplay, &facecam, &out), "{name} renders");
        assert_eq!(dimensions(&out), AspectRatio::Portrait.frame());
        let row = row_pixels(&out, row_below_facecam(&layout));
        observed.push(
            band_runs(&row, reference)
                .into_iter()
                .map(|(b, _)| b)
                .collect(),
        );
    }
    // Centred: a window in the middle of the source, so the two middle bands.
    assert_eq!(
        observed[0],
        vec![1, 2],
        "an unfocused 9:16 crop keeps the middle of the gameplay"
    );
    // Focused left: the window reaches the source's left edge, so the first
    // band appears — which is the claim that focus moves the crop.
    assert_eq!(
        observed[1],
        vec![0, 1],
        "a left-hand focus must pull the crop onto the first band"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
