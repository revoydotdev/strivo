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
use crate::time::{Interval, IntervalMap, MediaTime};

/// Where every clip landed.
///
/// The two lanes are separated here and nowhere else. The programme lane goes
/// into the [`IntervalMap`], which can only append and is therefore the
/// authority on the output length; an overlay is **placed** on that timeline and
/// never enters it, so a B-roll cannot become a segment of the source and
/// split the bed around itself.
struct Placement {
    timeline: IntervalMap,
    /// Per clip index, in request order.
    outputs: Vec<Interval>,
    /// Per clip index, in request order.
    overlay: Vec<bool>,
    /// Cut index to clip index.
    placed: BTreeMap<usize, usize>,
}

fn span_length(request: &RenderRequest, index: usize) -> Result<MediaTime, RenderError> {
    request.clips[index]
        .span
        .rate
        .output_of(
            request.clips[index]
                .span
                .range
                .duration()
                .map_err(|error| RenderError::Time { clip: index, error })?,
        )
        .map_err(|error| RenderError::Time { clip: index, error })
}

/// Place the programme lane, then the overlays, then check the overlays fit.
///
/// The two orderings are forced. An overlay's output span is known from its
/// placement alone, but whether that span exists at all is only knowable once
/// the programme lane is fully placed — so the range check is necessarily a
/// second pass, and it **refuses** rather than clamping. Clamping would pull a
/// layer back inside the output and render something other than the edit, with
/// no error anywhere.
fn place(req: &RenderRequest) -> Result<Placement, RenderError> {
    let mut timeline = IntervalMap::new();
    let mut outputs = Vec::with_capacity(req.clips.len());
    let mut overlay = Vec::with_capacity(req.clips.len());
    for (index, requested) in req.clips.iter().enumerate() {
        match requested.overlay_at {
            None => {
                outputs.push(
                    timeline
                        .push_source(requested.span.clone())
                        .map_err(|error| RenderError::Time { clip: index, error })?,
                );
                overlay.push(false);
            }
            Some(at) => {
                let length = span_length(req, index)?;
                outputs.push(
                    Interval::new(
                        at,
                        at.checked_add(length)
                            .map_err(|error| RenderError::Time { clip: index, error })?,
                    )
                    .map_err(|error| RenderError::Time { clip: index, error })?,
                );
                overlay.push(true);
            }
        }
    }
    let output = timeline.output_duration();
    for (index, requested) in req.clips.iter().enumerate() {
        if overlay[index] && outputs[index].end() > output {
            return Err(RenderError::OverlayOutOfRange {
                cut: requested.cut,
                at: outputs[index].start().micros(),
                end: outputs[index].end().micros(),
                output: output.micros(),
            });
        }
    }
    let placed = req
        .clips
        .iter()
        .enumerate()
        .map(|(index, requested)| (requested.cut, index))
        .collect();
    Ok(Placement {
        timeline,
        outputs,
        overlay,
        placed,
    })
}

/// Compile one already-admitted request into the one plan both callers consume.
pub fn compile(req: &RenderRequest) -> Result<CompiledPlan, RenderError> {
    if req.clips.is_empty() {
        return Err(RenderError::NoCuts);
    }
    let placement = place(req)?;
    let timeline = placement.timeline;
    let output_duration = timeline.output_duration();

    let mut clips = Vec::with_capacity(req.clips.len());
    for (index, requested) in req.clips.iter().enumerate() {
        clips.push(Clip {
            cut: requested.cut,
            output: placement.outputs[index],
            span: requested.span.clone(),
            fade_in: requested.fade_in,
            fade_out: requested.fade_out,
            overlay: placement.overlay[index],
        });
    }

    let layers = compile_layers(req, &placement.placed, &placement.overlay)?;
    if !layers.iter().any(|l| l.kind == LayerKind::Video) {
        return Err(RenderError::NoVideoLayer);
    }
    let transitions = compile_transitions(req, &layers, output_duration.micros())?;
    let tracks = compile_tracks(req, &placement.placed, &placement.overlay)?;
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
        frames: req.video.frames.clone(),
        cues,
        caption_mode: req.captions.mode,
        caption_style_json: req.captions.style_json.clone(),
        caption_coordinate: req.captions.coordinate,
        caption_safe_area: req.captions.safe_area,
        container: req.container.clone(),
        profile: req.profile.clone(),
    })
}
