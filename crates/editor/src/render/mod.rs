//! One render contract: a revision compiled to a plan, and one plan executed.
//!
//! From `docs/CREATOR-EDITION-REPORT-2026-09-07.md` F02/R01. Before this
//! module, "what will this edit render as?" had as many answers as there were
//! callers: the EDL renderer took two filter strings, a second argv builder in
//! `strivo-plugins` synthesised its own, and each plugin's panel showed a
//! *preview* string built by its own crate. The UI promised five effects were
//! baked; two were.
//!
//! **The seam, and why it is here rather than in the plugin crates.**
//!
//! [`compile`] owns *composition and policy* — layer order, routing, the
//! canonical order of the audio chain, and every refusal. It does **not** own
//! filter spelling: each effect crate already emits its own ffmpeg fragment
//! from its own model ([`request::EffectStep::filter`]), and re-deriving those
//! strings here would be a second implementation of six of them. The caller —
//! the only place that already links every plugin crate — resolves the saved
//! chains into steps, and the compiler orders, routes and refuses.
//!
//! That is also why this lives in `strivo-editor` and not in
//! `strivo-research`: adding six plugin crates as dependencies of the research
//! kernel to hold a struct would invert the dependency graph for nothing.
//!
//! **One compiler, two callers.** [`preview`] and [`export`] are the only two,
//! they differ in what they *do* with the plan, and [`CompiledPlan`] has no
//! public constructor, so there is no third way to reach a plan. Preview's
//! approximations are **derived from** the plan ([`approximations`]) and never
//! stored in it — an approximation field *on* the plan would be a second
//! answer to "what does this render as" for preview and export to disagree
//! about.
//!
//! ## The mixer is inside that, not beside it
//!
//! Mute, solo, gain, pan, source and master routing, ducking, cleanup, fades and
//! normalisation reach a plan as fields of a [`request::RenderRequest`] and are
//! **resolved here, once**. Audibility in particular is decided by
//! [`compose::compile_tracks`] into [`plan::Track::audible`] — mute beats solo,
//! a solo silences the rest, and a muted-and-soloed track stays silent — and
//! [`invoke::track_filter`] reads that one field. Two rules that would
//! otherwise be conventions, and conventions are what two render paths disagree
//! about:
//!
//! * **a master-bus effect is not a pre-master tap.** The whole-programme chain
//!   runs on the sum, so a tap declared there is refused
//!   ([`RenderError::TapInProgrammeChain`]) rather than quietly applied to every
//!   bus. A tap belongs on [`request::TrackSpec::insert`], which the per-clip
//!   sub-render runs before the sum.
//! * **normalisation is measure-then-apply.** It lives on [`plan::Loudness`],
//!   where the executor runs `loudnorm` twice, and a loudness stage in the
//!   chain is refused ([`RenderError::LoudnessAsChainStep`]) because it would be
//!   a second, single-pass normalisation of an already-normalised programme.
//!
//! A test that reads *the field the control writes* and compares it to the
//! decoded artifact is the teeth-check for this: `strivo-web`'s `mixer.rs` and
//! `mixer_falsifier.rs` do that, and the falsifier binary records the
//! mutations that turn them red.
//!
//! ## Time
//!
//! Every coordinate and length here is a [`MediaTime`] — todo 22's single
//! integer representation — and every mapping between source and composition
//! time goes through the [`IntervalMap`] it defines. This module adds no `f64`
//! time and no millisecond column. `f64` appears on **gains, ratios and dB**
//! only, which are not time.
//!
//! ## One attempt, one owner
//!
//! Every render attempt gets an [`AttemptId`](attempt::AttemptId), a scratch
//! directory derived from it and an output path derived from it — see
//! [`attempt`]. Naming them after the subject instead is what let two renders
//! of one recording destroy each other's files, so the subject appears in both
//! paths for legibility and decides nothing.
//!
//! ## Validation is not here
//!
//! Admission of a *revision's inputs* — is this path inside the media root,
//! does this cut fit inside its source — belongs to
//! `strivo_web::routes::media_input`, and runs before a
//! [`request::RenderRequest`] is built. This module starts from an
//! already-admitted request and refuses only what a request can be internally
//! inconsistent about.

mod approximate;
pub(crate) mod attempt;
mod compile;
mod compose;
mod error;
pub mod example;
mod execute;
mod invoke;
mod plan;
mod request;

#[cfg(test)]
mod tests;

pub use approximate::{approximations, Approximation};
pub use attempt::{output_path, validate_output, AttemptId};
pub use compile::compile;
pub use error::RenderError;
pub use execute::{export, preview, Export, Preview};
pub use invoke::{
    audio_chain, clip_args, composite_args, concat_args, fade_filter, frames_filter,
    is_passthrough, layer_offset_exprs, layer_scale_filter, measure_filter, normalize_args,
    normalize_filter, overlay_clip_args, overlay_layers, parse_measurements, track_filter,
    video_chain, video_is_passthrough,
};
pub use plan::{Clip, CompiledPlan, Cue, Loudness, PlanLayer, Track, Transition};
pub use request::{
    CaptionMode, CaptionSpec, ClipRequest, Colour, Container, CueCoord, EffectKind, EffectStep,
    Frames, Interpolation, Keyframe, LayerKind, LayerSpec, LoudnessSpec, OutputProfile,
    RenderRequest, TrackSpec, Transform, TransitionKind, TransitionSpec, VideoIntent,
};

pub use crate::time::{Coordinate, IntervalMap, MediaTime, SourceSpan, TimeError};
