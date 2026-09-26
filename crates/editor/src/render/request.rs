//! What a caller hands the compiler: an **already-admitted** description of
//! one revision and the render-affecting settings that belong to it.
//!
//! Deliberately not a stored document. A `RenderRequest` is built by
//! [`crate::render::compile`]'s callers from the revision and the six
//! per-recording sidecars, and it is the *only* way into the compiler — there
//! is no second constructor and no "just give me the strings" entry point,
//! because that is how the two render paths this module replaces came to
//! disagree.
//!
//! Two properties this file enforces by construction:
//!
//! * **no time that is not [`MediaTime`]** — a clip's range arrives as a
//!   [`SourceSpan`], a fade or a transition as a length in whole microseconds;
//! * **no ffmpeg spelling** — a caller hands an [`EffectStep`] whose `filter`
//!   its owning crate produced, and the compiler orders and routes it without
//!   knowing what `rubberband` or `alimiter` mean.

use serde::{Deserialize, Serialize};

use super::MediaTime;
use crate::time::{Coordinate, SourceSpan};

/// One placed clip: which cut of the revision it came from, where it reads
/// from, and its fades.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClipRequest {
    /// Index into the revision's cut list. Recorded so the executor can find
    /// the clip's media locator without the plan having to carry a filesystem
    /// path — a path is a location, not an identity, and a plan that stored
    /// one would be a plan that could not outlive a moved file.
    pub cut: usize,
    /// The output-to-source reading, in integer media time. Must be
    /// non-empty: an empty span has no reverse map.
    pub span: SourceSpan,
    /// Fades applied inside this clip, on the output timeline.
    pub fade_in: MediaTime,
    pub fade_out: MediaTime,
    /// Where this clip is drawn on the output timeline, or `None` for the
    /// programme lane.
    ///
    /// `None` means the clip **consumes** the output timeline: it is appended
    /// to the cut list and the next clip starts where this one ends. `Some`
    /// means the clip is **placed** — it is drawn over that output position
    /// and consumes nothing, so the source keeps running underneath it.
    ///
    /// That distinction is report item E08's whole content. The legacy cut
    /// list has no way to say it: an inserted B-roll is one more ordered entry,
    /// so the output grows by its length and the source is split around it
    /// (`docs/CREATOR-EDITION-REPORT-2026-09-07.md:96`). An `Option` rather
    /// than a separate clip type because the two lanes differ in exactly one
    /// field, and a second struct would be a second timeline.
    #[serde(default)]
    pub overlay_at: Option<MediaTime>,
}

/// What a layer contributes to the picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayerKind {
    /// The cut list itself.
    Video,
    /// A second video source placed over it.
    Overlay,
    /// An image composited over the picture.
    Image,
    /// A colour-only adjustment layer.
    Colour,
}

/// Where a layer sits and how it is drawn. `scale` is the output pixel size,
/// so it is a resolution rather than a ratio — an `f64` scale would need a
/// reference size, and the reference size is the plan's own.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transform {
    /// `None` = keep the source size.
    pub scale: Option<(u32, u32)>,
    /// Top-left offset in output pixels, signed.
    pub offset_px: (i64, i64),
    /// Draw order within the layer's own z; higher paints later.
    pub z: i32,
}

/// Colour handling, declared rather than assumed. Left as its own field
/// because "the plan forgot to say" and "the plan said passthrough" are
/// different answers, and only one of them is a mistake.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Colour {
    /// `None` = carry the source's own transfer through untouched.
    pub transfer: Option<Transfer>,
    pub primaries: Option<Primaries>,
    pub matrix: Option<Matrix>,
    /// Limited or full range.
    pub full_range: Option<bool>,
}

/// ffmpeg `transfer` values the plan can name. A closed set on purpose: an
/// open string here is a place where two callers can spell the same transfer
/// two ways and the plan cannot tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transfer {
    Bt709,
    Bt2020_10,
    Bt2020_12,
    Smpte2084,
    AribStdB67,
}

/// ffmpeg `primaries` values the plan can name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Primaries {
    Bt709,
    Bt2020,
    DisplayP3,
}

/// ffmpeg `matrix` values the plan can name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Matrix {
    Bt709,
    Bt2020Ncl,
    Bt2020Cl,
}

/// One compositing layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayerSpec {
    /// Unique within a plan. The compiler sorts by `(z, id)`, so two layers
    /// at the same z have a defined order rather than an arbitrary one.
    pub id: String,
    pub kind: LayerKind,
    pub transform: Transform,
    pub colour: Colour,
    /// The clips this layer draws, as indices into the revision's cut list.
    pub clips: Vec<usize>,
    /// A layer that draws nothing is a mistake, not a no-op: it is either a
    /// half-finished B-roll layer or a layer whose clips were all deleted.
    pub visible: bool,
    /// A layer's transform over time. Empty means the layer's static
    /// [`Self::transform`] for its whole life, which is the ordinary case.
    #[serde(default)]
    pub keyframes: Vec<Keyframe>,
}

/// How a keyframe's value is reached from the one before it.
///
/// A stated choice rather than an emergent one: a zoom that eases and a title
/// that snaps are different artifacts, and "the renderer will work it out" is
/// how a creator's stepped title card comes back smoothly animated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Interpolation {
    /// Interpolate the geometric values between the two keyframes.
    Linear,
    /// Hold the earlier keyframe's value until the next one.
    Stepped,
}

/// One keyframe: the transform held at `at` on the output timeline.
///
/// Times are **output** coordinates, so a keyframe lands where the viewer sees
/// it regardless of what the source underneath is doing. `z` is never
/// interpolated — it is a paint order, and a paint order halfway between two
/// layers is not an order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Keyframe {
    pub at: MediaTime,
    pub transform: Transform,
    /// How this keyframe is reached **from the previous one**. The first
    /// keyframe's own setting is unread, because there is nothing before it.
    #[serde(default = "linear")]
    pub interpolate: Interpolation,
}

fn linear() -> Interpolation {
    Interpolation::Linear
}

/// The declared shape of the output picture: frame size, frame rate, pixel
/// format and timebase.
///
/// A declaration rather than something the renderer infers, because "rendered
/// at *some* resolution" is what report §4.5 calls insufficient
/// (`docs/CREATOR-EDITION-REPORT-2026-09-07.md:90`): with two sources of
/// different sizes and rates in one composition there is no single size the
/// renderer could pick that is not a silent choice. Every field is what a
/// caller asks ffprobe to confirm afterwards.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Frames {
    pub width: u32,
    pub height: u32,
    /// Nominal frame rate as an exact rational, e.g. `24000/1001`.
    pub fps_num: u32,
    pub fps_den: u32,
    pub pixel_format: String,
    /// The output frame timebase, e.g. `1/1000`.
    pub timebase_num: u32,
    pub timebase_den: u32,
}

/// How two overlapping layers meet. Named rather than implied, because a
/// cross-fade and a hard cut are different artifacts and "the renderer will
/// work it out" is how they get confused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionKind {
    /// The outgoing layer is cut away on the frame.
    Cut,
    /// The two layers are mixed over `duration`.
    Dissolve,
    /// The incoming layer pushes the outgoing one off.
    Slide,
    /// The incoming layer wipes over the outgoing one.
    Wipe,
}

/// One transition, placed on the output timeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransitionSpec {
    pub id: String,
    /// The layer the transition happens *on*. Must name a layer in the
    /// request: a transition on a layer that was never declared has no
    /// meaning, and dropping it would render something other than the edit.
    pub on_layer: String,
    pub kind: TransitionKind,
    /// Where the transition starts, on the output timeline.
    pub at: MediaTime,
    pub duration: MediaTime,
}

/// Which chain an effect belongs to, and therefore where in the signal flow it
/// is placed. The variant order **is** the canonical audio order: pitch/time
/// first because it changes the length everything downstream is measured
/// against, then the inserts, then the bus mix, then automation, then
/// loudness last because it is the only stage that reads the finished signal.
///
/// **Loudness is a rank, not a chain stage.** [`Self::Loudness`] exists so the
/// order has a name for the last position, and a request that uses it as a step
/// is refused ([`super::RenderError::LoudnessAsChainStep`]) — normalisation is
/// measure-then-apply and the executor runs it from [`super::plan::Loudness`],
/// twice. A loudness step in the chain would be a third, single-pass
/// normalisation of an already-normalised programme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectKind {
    /// Tempo and pitch. Changes duration.
    PitchTime,
    /// The ordered per-bus insert chain.
    InsertFx,
    /// Per-track gains, pans and the master bus.
    SubMix,
    /// The volume automation curve.
    VolumeAutomation,
    /// EBU R128 normalisation. Always last.
    Loudness,
    /// The watermark/banner overlay. Video, not audio.
    Branding,
    /// Caption burn-in. Video, not audio.
    CaptionBurn,
}

impl EffectKind {
    /// Whether this stage runs on the audio signal. Branding and caption burn
    /// run on the picture and are ordered separately — an audio stage and a
    /// video stage in one chain is not an ffmpeg expression.
    #[must_use]
    pub const fn is_audio(self) -> bool {
        matches!(
            self,
            Self::PitchTime
                | Self::InsertFx
                | Self::SubMix
                | Self::VolumeAutomation
                | Self::Loudness
        )
    }
}

/// One resolved effect: a name, the stage it belongs to, and the ffmpeg
/// fragment its owning crate produced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectStep {
    /// Unique within a plan. Two effects with one name are two answers to
    /// "which limiter", so the compiler refuses rather than picking one.
    pub id: String,
    pub kind: EffectKind,
    /// The caller's crate's own ffmpeg fragment, e.g. `InsertChain::to_filter`.
    /// Empty means "this stage emitted nothing" and the step is dropped —
    /// which is how an empty saved chain stops being a render cost.
    pub filter: String,
    /// How many separate signals this stage mixes. `1` for everything that
    /// operates on the one stream a concat produces; a sub-mix bus with two
    /// tracks is `2` and needs two ffmpeg inputs. Recorded because the
    /// compiler can then refuse a bus the plan has no tracks for, instead of
    /// emitting a graph that silently drops the tracks it cannot reach.
    pub arity: usize,
    /// Declares that this stage is a **pre-master tap** on one bus rather than
    /// a master-bus effect.
    ///
    /// `None` is correct and is what every real stage uses: the
    /// whole-programme chain runs once, on the summed programme, after every
    /// bus has been mixed. A tap cannot be expressed there — the chain has one
    /// input and that input is the sum — so a caller that asks for one is
    /// refused rather than quietly given a filter on the sum. See
    /// [`super::RenderError::TapInProgrammeChain`]; the field exists so the
    /// refusal has something to name, and so the correct spelling
    /// ([`TrackSpec::insert`]) is a declared alternative rather than folklore.
    #[serde(default)]
    pub bus: Option<String>,
}

/// A bus a track is routed into. Phase 1 has one; the type exists so a second
/// is a data change rather than a signature change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrackSpec {
    /// User-visible name. Unique within the plan.
    pub label: String,
    /// The bus this track feeds. Tracks sharing a bus are summed into it.
    pub bus: String,
    /// Linear trim in dB. Not time, so a float is the right type here.
    pub gain_db: f64,
    /// -1.0 hard left … +1.0 hard right.
    pub pan: f64,
    pub muted: bool,
    /// Listen to this track alone.
    ///
    /// Independent of `muted` rather than exclusive with it, and the
    /// combination is resolved once by the compiler into
    /// [`super::plan::Track::audible`]. Declared here and *decided* there is
    /// the point: two places each answering "is this track audible" is how a
    /// preview and a render come to disagree about a solo button.
    #[serde(default)]
    pub solo: bool,
    /// This track's own insert chain, as the ffmpeg fragment its crate
    /// produced — a **pre-master tap**: it runs on this track's signal, inside
    /// the clip's own sub-render, before the tracks are summed.
    ///
    /// It lives here rather than in the request's effect list because the
    /// effect list is the *programme* chain and runs on the sum. Putting a tap
    /// there is the silent-substitution defect: a user sets a voice high-pass
    /// and hears it applied to the game audio as well. Empty means "no chain",
    /// and an empty chain is not a render cost.
    #[serde(default)]
    pub insert: String,
    /// A track that declares a clip index which no cut carries.
    pub clips: Vec<usize>,
}

/// The loudness stage. `platform` is the preset's name and the numbers are
/// what the user overrode it with, so the plan records the **resolved** target
/// rather than the preset it came from — two platforms' presets are data, and
/// a plan that recorded the name would not be reproducible without the preset
/// table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoudnessSpec {
    pub platform: String,
    /// Integrated loudness target, LUFS.
    pub i: f64,
    /// Loudness range target, LU.
    pub lra: f64,
    /// True peak ceiling, dBTP.
    pub tp: f64,
}

/// How captions reach the output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptionMode {
    /// Written beside the artifact, not drawn into it.
    Sidecar,
    /// Drawn into the picture.
    BurnedIn,
    /// Neither: the plan has no captions.
    None,
}

/// The caption stage, including the safe-area contract the preview must honour.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptionSpec {
    pub mode: CaptionMode,
    /// The ASS style document, verbatim, exactly as the captions crate
    /// serialises it. The plan does not re-type twenty style fields it would
    /// then have to keep in step with that crate's `AssStyle`.
    pub style_json: String,
    /// Which clock the cue coordinates are on. Declared, never assumed:
    /// captions are written in source time by the transcript and must be
    /// mapped through the interval map to survive a cut.
    pub coordinate: Coordinate,
    /// Keep every cue inside the frame the preview claims is safe.
    pub safe_area: bool,
    /// How many cues this plan carries. The timings themselves are resolved
    /// through the interval map, not stored twice.
    pub cues: Vec<CueCoord>,
}

/// One caption cue's position, before resolution. `Source` coordinates must
/// resolve through the interval map; a cue inside a cut the composition
/// deleted is refused rather than clamped to the cut's edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CueCoord {
    pub at: MediaTime,
    pub duration: MediaTime,
}

/// The container the output is written into.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Container {
    /// File extension without the dot, e.g. `mkv`.
    pub extension: String,
    pub video_codec: String,
    pub audio_codec: String,
    /// Copy the elementary streams instead of re-encoding. Only ever true for
    /// a plan whose audio chain is empty *and* whose video chain is a
    /// passthrough — the compiler decides, so a caller cannot ask for a copy
    /// and get a filter it silently dropped.
    pub stream_copy: bool,
}

/// The named destination profile, as the creator row stored it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputProfile {
    pub name: String,
    /// Render tool identity, e.g. `ffmpeg`. Part of the plan's identity: the
    /// same revision rendered by two tools is two plans.
    pub renderer: String,
    pub renderer_version: String,
}

/// Video-side intent that is not a layer: the declared colour handling of the
/// output as a whole, and whether a picture chain was supplied at all.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct VideoIntent {
    /// The caller's video `filter_complex`, ending at `[vout]`. Empty means
    /// passthrough.
    pub filter_complex: String,
    /// Declared output colour handling. `None` = carry through.
    pub colour: Option<Colour>,
    /// The declared output picture shape. `None` = no conform, so the output
    /// keeps the base layer's own size and rate.
    #[serde(default)]
    pub frames: Option<Frames>,
}

/// Everything the compiler is given. One value, built by one function, handed
/// to [`crate::render::compile`] by both callers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RenderRequest {
    /// What this plan is for — a recording id, a project id. Recorded so a
    /// plan says what it is about without being looked up.
    pub subject: String,
    /// The revision this was compiled from. Part of the plan's identity: the
    /// same settings over a different revision is a different plan, and a plan
    /// that could not tell is a plan that goes stale silently.
    pub revision: String,
    pub clips: Vec<ClipRequest>,
    pub layers: Vec<LayerSpec>,
    pub transitions: Vec<TransitionSpec>,
    pub tracks: Vec<TrackSpec>,
    /// Resolved effects. The compiler **orders** these; the order they arrive
    /// in carries no meaning, and two callers that build them in different
    /// orders get the same plan.
    pub effects: Vec<EffectStep>,
    pub loudness: Option<LoudnessSpec>,
    pub captions: CaptionSpec,
    pub video: VideoIntent,
    pub container: Container,
    pub profile: OutputProfile,
}
