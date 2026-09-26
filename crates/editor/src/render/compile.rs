//! The compiler: one function, no callers outside this module's two entry
//! points.
//!
//! ## The refusals, and the lazy wrong answer each replaces
//!
//! | Refusal | The wrong answer it replaces |
//! |---|---|
//! | [`RenderError::NoCuts`] | compiling to an empty plan that "agrees" with another empty plan |
//! | [`RenderError::Time`] | rounding a speed whose output is not a whole microsecond, or clamping a cut to its source's edge |
//! | [`RenderError::UnknownCut`] | dropping a layer or track that names a cut the revision does not have |
//! | [`RenderError::DuplicateId`] | picking one of two effects called the same thing |
//! | [`RenderError::UnroutedClip`] | rendering a clip into silence because no track claimed it |
//! | [`RenderError::TransitionOutOfRange`] | shortening a transition to fit an output it does not fit |
//! | [`RenderError::CueUnmapped`] | clamping a caption to the nearest cut edge, putting it on the wrong frame |
//!
//! ## Determinism
//!
//! Every collection the plan carries is sorted by an explicit key — clips by
//! output position, layers by `(z, id)`, transitions by `(at, id)`, tracks by
//! label, effects by stage rank then id. A caller that builds the same effects
//! in a different order gets the same plan, which is what makes a content hash
//! over the plan a statement about the edit rather than about the call site.

use std::collections::BTreeMap;

use super::compose::{
    compile_cues, compile_effects, compile_layers, compile_tracks, compile_transitions,
};
use super::error::RenderError;
use super::plan::{Clip, CompiledPlan, Loudness};
use super::request::{LayerKind, RenderRequest};
use crate::time::{Entry, Interval, IntervalMap};

/// Place every clip on the output timeline, in order.
///
/// Built through [`IntervalMap::push_source`] rather than by computing output
/// positions here, so an overlap, a hole, an empty span and a non-integral
/// speed are all refused by todo 22's own code instead of being re-checked.
fn build_timeline(req: &RenderRequest) -> Result<IntervalMap, RenderError> {
    let mut map = IntervalMap::new();
    for (clip, requested) in req.clips.iter().enumerate() {
        map.push_source(requested.span.clone())
            .map_err(|error| RenderError::Time { clip, error })?;
    }
    Ok(map)
}

/// Where each clip landed, read off the map rather than recomputed — the map
/// placed them, so it is what says where they are.
fn clip_outputs(map: &IntervalMap) -> Vec<Interval> {
    map.entries()
        .iter()
        .filter_map(|entry| match entry {
            Entry::Source { output, .. } => Some(*output),
            Entry::Gap { .. } => None,
        })
        .collect()
}

/// Compile one already-admitted request into the one plan both callers consume.
pub fn compile(req: &RenderRequest) -> Result<CompiledPlan, RenderError> {
    if req.clips.is_empty() {
        return Err(RenderError::NoCuts);
    }
    let timeline = build_timeline(req)?;
    let output_duration = timeline.output_duration();
    let outputs = clip_outputs(&timeline);

    let mut placed: BTreeMap<usize, usize> = BTreeMap::new();
    let mut clips = Vec::with_capacity(req.clips.len());
    for (index, requested) in req.clips.iter().enumerate() {
        placed.insert(requested.cut, index);
        clips.push(Clip {
            cut: requested.cut,
            output: outputs[index],
            span: requested.span.clone(),
            fade_in: requested.fade_in,
            fade_out: requested.fade_out,
        });
    }

    let layers = compile_layers(req, &placed)?;
    if !layers.iter().any(|l| l.kind == LayerKind::Video) {
        return Err(RenderError::NoVideoLayer);
    }
    let transitions = compile_transitions(req, &layers, output_duration.micros())?;
    let tracks = compile_tracks(req, &placed)?;
    let (audio_chain, video_chain) = compile_effects(req, &tracks)?;
    let cues = compile_cues(req, &timeline)?;
    let loudness = req.loudness.as_ref().map(Loudness::from_spec);

    Ok(CompiledPlan {
        subject: req.subject.clone(),
        revision: req.revision.clone(),
        timeline,
        output_duration,
        clips,
        layers,
        transitions,
        tracks,
        audio_chain,
        video_chain,
        loudness,
        video_filter_complex: req.video.filter_complex.clone(),
        colour: req.video.colour,
        cues,
        caption_mode: req.captions.mode,
        caption_style_json: req.captions.style_json.clone(),
        caption_coordinate: req.captions.coordinate,
        caption_safe_area: req.captions.safe_area,
        container: req.container.clone(),
        profile: req.profile.clone(),
    })
}
