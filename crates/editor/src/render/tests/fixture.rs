//! The one revision every test in this module builds from.
//!
//! Its own file because it is the only part of the suite that is not a claim:
//! a test that perturbs one field of this value and asserts one consequence is
//! readable in isolation, and a fixture edit that changed several things at
//! once would be visible here rather than spread across twenty assertions.

use super::*;
use crate::time::{Rate, SourceRef, SourceSpan, TimeBase};
use crate::{Cut, CutKind, Edl};

pub(super) const SOURCE: &str = "/recordings/stream-01.mkv";

pub(super) fn us(value: i64) -> MediaTime {
    MediaTime::from_micros(value).expect("non-negative media time")
}

pub(super) fn span(start: i64, end: i64) -> SourceSpan {
    SourceSpan::new(
        SourceRef::from_locator(SOURCE),
        TimeBase::from_rate(1, 1).expect("identity timebase"),
        Interval::new(us(start), us(end)).expect("ordered"),
        Rate::ONE,
    )
    .expect("non-empty span")
}

pub(super) fn clip(cut: usize, start: i64, end: i64) -> ClipRequest {
    ClipRequest {
        cut,
        span: span(start, end),
        fade_in: MediaTime::ZERO,
        fade_out: MediaTime::ZERO,
    }
}

pub(super) fn effect(id: &str, kind: EffectKind, filter: &str) -> EffectStep {
    EffectStep {
        id: id.into(),
        kind,
        filter: filter.into(),
        arity: 1,
    }
}

/// A revision with two cuts, four audio stages, a branding step, a transition,
/// a loudness target and one caption — the shape the Phase-1 editor produces.
pub(super) fn request() -> RenderRequest {
    RenderRequest {
        subject: "stream-01".into(),
        revision: "rev-7".into(),
        clips: vec![clip(0, 0, 1_000_000), clip(1, 2_000_000, 3_500_000)],
        layers: vec![LayerSpec {
            id: "programme".into(),
            kind: LayerKind::Video,
            transform: Transform::default(),
            colour: Colour::default(),
            clips: vec![0, 1],
            visible: true,
        }],
        transitions: vec![TransitionSpec {
            id: "cut-1".into(),
            on_layer: "programme".into(),
            kind: TransitionKind::Dissolve,
            at: us(900_000),
            duration: us(200_000),
        }],
        tracks: vec![TrackSpec {
            label: "programme".into(),
            bus: "master".into(),
            gain_db: 0.0,
            pan: 0.0,
            muted: false,
            clips: vec![0, 1],
        }],
        effects: vec![
            effect("pitch", EffectKind::PitchTime, "rubberband=tempo=1.10"),
            effect(
                "inserts",
                EffectKind::InsertFx,
                "highpass=f=80,alimiter=limit=0.891",
            ),
            effect("master", EffectKind::SubMix, "volume=-1.50dB"),
            effect("automation", EffectKind::VolumeAutomation, "volume=1.20dB"),
        ],
        loudness: Some(LoudnessSpec {
            platform: "youtube".into(),
            i: -14.0,
            lra: 11.0,
            tp: -1.0,
        }),
        captions: CaptionSpec {
            mode: CaptionMode::Sidecar,
            style_json: "{\"title\":\"t\"}".into(),
            coordinate: Coordinate::Source,
            safe_area: true,
            cues: vec![CueCoord {
                at: us(2_500_000),
                duration: us(400_000),
            }],
        },
        video: VideoIntent {
            filter_complex: "[0:v]eq=brightness=0.0[vout]".into(),
            colour: None,
        },
        container: Container {
            extension: "mkv".into(),
            video_codec: "libx264".into(),
            audio_codec: "aac".into(),
            stream_copy: false,
        },
        profile: OutputProfile {
            name: "long-form".into(),
            renderer: "ffmpeg".into(),
            renderer_version: "n9.0.2".into(),
        },
    }
}

/// The revision in the legacy EDL the executor reads locators from.
pub(super) fn edl() -> Edl {
    Edl {
        recording_id: "stream-01".into(),
        cuts: vec![
            Cut {
                start_sec: 0.0,
                end_sec: 1.0,
                kind: CutKind::Source {
                    source_path: SOURCE.into(),
                },
                fade_in_sec: 0.0,
                fade_out_sec: 0.0,
            },
            Cut {
                start_sec: 2.0,
                end_sec: 3.5,
                kind: CutKind::Source {
                    source_path: SOURCE.into(),
                },
                fade_in_sec: 0.0,
                fade_out_sec: 0.0,
            },
        ],
    }
}

pub(super) fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("strivo-render-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

pub(super) fn ffmpeg_available() -> bool {
    std::process::Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// A real two-second clip with a video and an audio stream, so an export has
/// something to cut.
pub(super) fn source_fixture(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("source.mkv");
    let status = std::process::Command::new("ffmpeg")
        .args(["-y", "-hide_banner", "-loglevel", "error"])
        .args([
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=2:size=64x36:rate=10",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=2",
        ])
        .args([
            "-pix_fmt",
            "yuv420p",
            "-c:v",
            "libx264",
            "-c:a",
            "aac",
            "-shortest",
        ])
        .arg(&path)
        .status()
        .expect("ffmpeg must be on PATH to synthesize the fixture");
    assert!(status.success(), "failed to synthesize the source fixture");
    path
}

pub(super) fn on_fixture(name: &str) -> Option<(RenderRequest, Edl, std::path::PathBuf)> {
    if !ffmpeg_available() {
        return None;
    }
    let dir = scratch(name);
    let fixture = source_fixture(&dir);
    Some((request(), edl_on(&fixture), dir))
}

/// The revision, with every cut pointed at `fixture` instead of the literal
/// `/recordings/stream-01.mkv`.
pub(super) fn edl_on(fixture: &std::path::Path) -> Edl {
    let mut revision = edl();
    for cut in &mut revision.cuts {
        cut.kind = CutKind::Source {
            source_path: fixture.to_string_lossy().into_owned(),
        };
    }
    revision
}

/// A source big enough that a render spends real wall time in it.
///
/// The 64x36 fixture renders in single-digit milliseconds, which is a race
/// window too small to hit on purpose — and a concurrency test that cannot
/// lose is a concurrency test that proves nothing. 320x180 at 25 fps for four
/// seconds puts each per-cut sub-render in the tens of milliseconds, and the
/// request's loudness target adds two more passes, so two overlapping renders
/// have five ffmpeg invocations apiece to interleave inside.
pub(super) fn wide_source_fixture(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("wide.mkv");
    let status = std::process::Command::new("ffmpeg")
        .args(["-y", "-hide_banner", "-loglevel", "error"])
        .args([
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=4:size=320x180:rate=25",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=4",
        ])
        .args([
            "-pix_fmt",
            "yuv420p",
            "-c:v",
            "libx264",
            "-c:a",
            "aac",
            "-shortest",
        ])
        .arg(&path)
        .status()
        .expect("ffmpeg must be on PATH to synthesize the wide fixture");
    assert!(status.success(), "failed to synthesize the wide fixture");
    path
}

pub(super) fn on_wide_fixture(name: &str) -> Option<(RenderRequest, Edl, std::path::PathBuf)> {
    if !ffmpeg_available() {
        return None;
    }
    let dir = scratch(name);
    let fixture = wide_source_fixture(&dir);
    Some((request(), edl_on(&fixture), dir))
}
