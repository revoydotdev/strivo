//! The compiled plan: the one description of what a revision renders as.
//!
//! It is a **value**, not a document and not a string. Two things follow, and
//! they are the point of the whole module:
//!
//! * it compares with `==`, so "preview and export agree" is a single
//!   assertion rather than a diff of two argv strings;
//! * it serialises deterministically — every collection in it is ordered by
//!   an explicit key, never by insertion order or a hash map's iteration —
//!   so a content hash over it is a statement about the plan rather than about
//!   the order a caller happened to build it in.
//!
//! **What is deliberately absent:** anything that differs between a preview and
//! a full render. A "preview quality" flag, a resolution cap, a field saying
//! "this stage is skipped in preview" — every one of those is a second answer
//! to "what does this render as", stored in the same struct. Preview
//! approximations are *derived* from this struct by
//! [`crate::render::approximations`], so a caller cannot set one.
//!
//! `f64` appears on gains and dB, never on a coordinate or a length.

use serde::{Deserialize, Serialize};

use super::request::{Colour, EffectKind, EffectStep, Transform};
use crate::time::{Interval, IntervalMap, MediaTime, SourceSpan};

/// One placed clip, as the plan carries it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    /// Index into the revision's cut list, carried through from the request
    /// so the executor can find the locator without the plan storing a path.
    pub cut: usize,
    /// The output interval this clip occupies.
    pub output: Interval,
    pub span: SourceSpan,
    pub fade_in: MediaTime,
    pub fade_out: MediaTime,
}

/// One layer, already sorted into paint order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanLayer {
    pub id: String,
    pub kind: super::request::LayerKind,
    pub transform: Transform,
    pub colour: Colour,
    /// Indices into [`CompiledPlan::clips`], ascending.
    pub clips: Vec<usize>,
    pub visible: bool,
}

/// One transition, already ordered by output position.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transition {
    pub id: String,
    pub on_layer: String,
    pub kind: super::request::TransitionKind,
    pub at: MediaTime,
    pub duration: MediaTime,
}

/// One track's resolved routing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub label: String,
    pub bus: String,
    pub gain_db: f64,
    pub pan: f64,
    /// What the request asked for, carried through so the mixer surface can
    /// show which button is down. **Not** what the render reads — see
    /// [`Self::audible`].
    pub muted: bool,
    /// What the request asked for, for the same reason.
    pub solo: bool,
    /// Whether this track reaches the programme. **The** answer, computed once
    /// by the compiler from `muted`, `solo` and whether *any* track is soloed,
    /// and read by both callers.
    ///
    /// The rules, in full, because "mute and solo are independent" is
    /// conventional rather than universal and a convention is what two render
    /// paths disagree about:
    ///
    /// * one or more tracks soloed ⇒ every non-soloed track is silent;
    /// * a muted track is silent **even when soloed** — solo is a listening
    ///   aid, not an unmute;
    /// * nothing soloed ⇒ exactly the muted tracks are silent.
    ///
    /// [`super::invoke::track_filter`] reads this field and nothing else, so a
    /// track cannot be audible in the preview and silent in the render.
    pub audible: bool,
    /// This track's pre-master tap, verbatim from the request. Applied to the
    /// track's own clips, not to the sum — see
    /// [`super::request::TrackSpec::insert`].
    pub insert: String,
    /// Indices into [`CompiledPlan::clips`], ascending.
    pub clips: Vec<usize>,
}

/// The loudness stage, resolved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Loudness {
    pub platform: String,
    pub i: f64,
    pub lra: f64,
    pub tp: f64,
    /// The target filter. `ffmpeg`'s `loudnorm` is two-pass and the second
    /// pass needs the first pass's readings, which do not exist until a signal
    /// has been analysed — so the plan records the **target** and the executor
    /// runs the measurement. That is what lets a plan be complete and
    /// comparable before any media exists, and what makes a target change
    /// visible in the plan rather than only in the output.
    pub filter: String,
}

impl Loudness {
    /// Resolve a declared target into the stage.
    #[must_use]
    pub fn from_spec(spec: &super::request::LoudnessSpec) -> Self {
        Self {
            platform: spec.platform.clone(),
            i: spec.i,
            lra: spec.lra,
            tp: spec.tp,
            filter: format!("loudnorm=I={}:LRA={}:TP={}", spec.i, spec.lra, spec.tp),
        }
    }
}

/// A caption cue with its coordinate **resolved** into composition time.
///
/// A source-time cue that the composition deleted has no composition time, so
/// it is refused at compile time rather than clamped to a cut edge and shown
/// on the wrong frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cue {
    /// Where the cue lands on the output timeline.
    pub at: MediaTime,
    pub duration: MediaTime,
    /// Every composition position the cue's source coordinate reaches. More
    /// than one when the same span is used twice — the repeat is real and
    /// dropping it would lose a caption.
    pub source_at: Vec<MediaTime>,
}

/// Everything one revision renders as, resolved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompiledPlan {
    /// What this plan is about.
    pub subject: String,
    /// The revision's cut index the plan was compiled from. Part of the plan's
    /// identity: the same settings over a different revision is a different
    /// plan, and a plan that could not tell is a stale plan.
    pub revision: String,
    /// The output-to-source map, in integer media time. The single authority
    /// on the output length: `clips` and `output_duration` are derived from
    /// it, so they cannot disagree.
    pub timeline: IntervalMap,
    pub output_duration: MediaTime,
    /// In output order.
    pub clips: Vec<Clip>,
    /// In paint order, `(z, id)`.
    pub layers: Vec<PlanLayer>,
    /// In output position order, then by id.
    pub transitions: Vec<Transition>,
    /// By label.
    pub tracks: Vec<Track>,
    /// Audio effects, in signal-flow order. Empty-fragment steps are already
    /// dropped, so every entry here is work the render will do.
    pub audio_chain: Vec<EffectStep>,
    /// Video effects, in signal-flow order.
    pub video_chain: Vec<EffectStep>,
    pub loudness: Option<Loudness>,
    /// The caller's video `filter_complex`, or empty for passthrough.
    pub video_filter_complex: String,
    /// The output's declared colour handling.
    pub colour: Option<Colour>,
    /// Resolved cues, in output order.
    pub cues: Vec<Cue>,
    pub caption_mode: super::request::CaptionMode,
    pub caption_style_json: String,
    pub caption_coordinate: crate::time::Coordinate,
    pub caption_safe_area: bool,
    pub container: super::request::Container,
    pub profile: super::request::OutputProfile,
}

impl CompiledPlan {
    /// The number of things this plan actually asks for. A plan that compiles
    /// to nothing is legal — an empty composition renders an empty file — so
    /// "the plan is non-empty" is a property a caller asserts about the
    /// *revision*, not about the compiler.
    #[must_use]
    pub fn work_items(&self) -> usize {
        self.clips.len()
            + self.layers.len()
            + self.transitions.len()
            + self.tracks.len()
            + self.audio_chain.len()
            + self.video_chain.len()
            + self.cues.len()
    }

    /// The audio effect kinds present, in order. A test-facing view of the
    /// signal flow; the compiler's own contract test reads it rather than
    /// re-deriving the order.
    #[must_use]
    pub fn audio_stages(&self) -> Vec<EffectKind> {
        self.audio_chain.iter().map(|e| e.kind).collect()
    }
}
