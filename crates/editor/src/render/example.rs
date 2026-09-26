//! The canonical example revision, as a builder.
//!
//! One fixture, shared. `strivo-editor`'s own contract tests, `strivo-research`'s
//! plan-row tests and `strivo-web`'s route tests all build from this, so a
//! change to the plan's shape surfaces in one place instead of three copies
//! that drift. It is a **public** module rather than a `#[cfg(test)]` one
//! because two of those three crates consume it as an ordinary dependency.
//!
//! What it is: two cuts of one recording, four audio stages, a branding
//! overlay, a transition, a loudness target and one source-time caption. That
//! is the shape the Phase-1 editor can actually produce — not a maximal
//! request — so a test that passes on it is a claim about the real thing.

use super::request::{
    CaptionMode, CaptionSpec, ClipRequest, Colour, Container, CueCoord, EffectKind, EffectStep,
    LayerKind, LayerSpec, LoudnessSpec, OutputProfile, RenderRequest, TrackSpec, Transform,
    TransitionKind, TransitionSpec, VideoIntent,
};
use super::{CompiledPlan, MediaTime};
use crate::time::{Coordinate, Interval, Rate, SourceRef, SourceSpan, TimeBase};
use crate::CutKind;

/// The recording the example revision cuts.
pub const SOURCE: &str = "/recordings/stream-01.mkv";

fn us(value: i64) -> MediaTime {
    MediaTime::from_micros(value).expect("a non-negative media coordinate")
}

fn span(start: i64, end: i64) -> SourceSpan {
    SourceSpan::new(
        SourceRef::from_locator(SOURCE),
        TimeBase::from_rate(1, 1).expect("the identity timebase is a positive rational"),
        Interval::new(us(start), us(end)).expect("an ordered interval"),
        Rate::ONE,
    )
    .expect("a non-empty source span")
}

fn clip(cut: usize, start: i64, end: i64) -> ClipRequest {
    ClipRequest {
        cut,
        span: span(start, end),
        fade_in: MediaTime::ZERO,
        fade_out: MediaTime::ZERO,
        overlay_at: None,
    }
}

fn effect(id: &str, kind: EffectKind, filter: &str) -> EffectStep {
    EffectStep {
        id: id.into(),
        kind,
        filter: filter.into(),
        arity: 1,
        bus: None,
    }
}

/// The example request. Edit it freely: it is a value, and every field is
/// public precisely so a test can perturb one thing and assert one thing.
#[must_use]
pub fn request() -> RenderRequest {
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
            keyframes: Vec::new(),
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
            solo: false,
            insert: String::new(),
            clips: vec![0, 1],
        }],
        effects: vec![
            effect("pitch", EffectKind::PitchTime, "rubberband=tempo=1.10"),
            effect(
                "inserts",
                EffectKind::InsertFx,
                "highpass=f=80,alimiter=limit=0.891",
            ),
            effect("master-bus", EffectKind::SubMix, "volume=-1.50dB"),
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
            style_json: "{\"title\":\"StriVo styled captions\"}".into(),
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
            frames: None,
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

/// The example revision in the legacy EDL the executor reads locators from.
#[must_use]
pub fn edl() -> crate::Edl {
    crate::Edl {
        recording_id: "stream-01".into(),
        cuts: vec![
            crate::Cut {
                start_sec: 0.0,
                end_sec: 1.0,
                kind: CutKind::Source {
                    source_path: SOURCE.into(),
                },
                fade_in_sec: 0.0,
                fade_out_sec: 0.0,
            },
            crate::Cut {
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

/// The example request pointed at a real file, for a caller that is about to
/// execute rather than inspect.
#[must_use]
pub fn request_for(path: &str) -> RenderRequest {
    let mut request = request();
    for clip in &mut request.clips {
        clip.span = SourceSpan::new(
            SourceRef::from_locator(path),
            TimeBase::from_rate(1, 1).expect("the identity timebase is a positive rational"),
            clip.span.range,
            Rate::ONE,
        )
        .expect("a non-empty source span");
    }
    request
}

/// The example revision pointed at a real file.
#[must_use]
pub fn edl_for(path: &str) -> crate::Edl {
    let mut edl = edl();
    for cut in &mut edl.cuts {
        cut.kind = CutKind::Source {
            source_path: path.to_string(),
        };
    }
    edl
}

/// Compile the example, panicking on a refusal.
///
/// For a caller that wants the plan and where a refusal would be a broken
/// fixture rather than a finding. A test that wants the refusal calls
/// [`super::compile`] directly and matches on the error.
#[must_use]
pub fn compiled(request: &RenderRequest) -> CompiledPlan {
    super::compile(request).unwrap_or_else(|e| panic!("the example revision must compile: {e}"))
}
