//! The stages of the compile, one function each.
//!
//! Split out of `compile` so the top-level function reads as the order things
//! happen in rather than as the bodies of eight unrelated checks. Every
//! function here is total: it either places what it was given or returns a
//! [`RenderError`](super::RenderError) naming what it could not.

use std::collections::{BTreeMap, BTreeSet};

use super::error::RenderError;
use super::plan::{Cue, PlanLayer, Track, Transition};
use super::request::{CaptionMode, EffectKind, EffectStep, RenderRequest, TransitionSpec};
use crate::time::{IntervalMap, Mapped, Timed};

/// Refuse a repeated name. Every plan collection is keyed by something a human
/// chose — a layer id, a track label, an effect id — and two entries under one
/// key are two answers to "which limiter", so the compiler refuses rather than
/// picking one.
fn distinct<'a>(kind: &'static str, ids: impl Iterator<Item = &'a str>) -> Result<(), RenderError> {
    let mut seen: BTreeSet<&'a str> = BTreeSet::new();
    for id in ids {
        if !seen.insert(id) {
            return Err(RenderError::DuplicateId {
                kind,
                id: id.to_string(),
            });
        }
    }
    Ok(())
}

/// Sort the layers into paint order and map their cut indices onto clips.
pub(super) fn compile_layers(
    req: &RenderRequest,
    placed: &BTreeMap<usize, usize>,
    overlay: &[bool],
) -> Result<Vec<PlanLayer>, RenderError> {
    distinct("layers", req.layers.iter().map(|l| l.id.as_str()))?;
    let mut layers: Vec<PlanLayer> = Vec::with_capacity(req.layers.len());
    for layer in &req.layers {
        let mut clips = Vec::with_capacity(layer.clips.len());
        for cut in &layer.clips {
            let clip = *placed.get(cut).ok_or_else(|| RenderError::UnknownCut {
                owner: format!("layer {:?}", layer.id),
                cut: *cut,
                placed: req.clips.len(),
            })?;
            // A layer drawn over the programme must be *placed*. Without the
            // placement it falls back to the programme lane, where it would
            // lengthen the output and split the bed — the exact defect this
            // lane exists to refuse, reached by forgetting one field.
            if layer.kind != super::request::LayerKind::Video && !overlay[clip] {
                return Err(RenderError::UnplacedLayerClip {
                    layer: layer.id.clone(),
                    cut: *cut,
                });
            }
            clips.push(clip);
        }
        clips.sort_unstable();
        let mut keyframes = layer.keyframes.clone();
        // Ordered here, not at the point of use, so the plan carries one order.
        keyframes.sort_by_key(|k| k.at);
        if let Some(duplicate) = keyframes
            .windows(2)
            .find(|w| w[0].at == w[1].at)
            .map(|w| w[0].at)
        {
            return Err(RenderError::DuplicateKeyframe {
                layer: layer.id.clone(),
                at: duplicate.micros(),
            });
        }
        layers.push(PlanLayer {
            id: layer.id.clone(),
            kind: layer.kind,
            transform: layer.transform,
            colour: layer.colour,
            clips,
            visible: layer.visible,
            keyframes,
        });
    }
    // Paint order is `(z, id)`. The id tiebreak is what makes two layers at
    // the same z deterministic instead of dependent on the caller's ordering.
    layers.sort_by(|a, b| (a.transform.z, &a.id).cmp(&(b.transform.z, &b.id)));
    Ok(layers)
}

fn to_transition(spec: &TransitionSpec) -> Transition {
    Transition {
        id: spec.id.clone(),
        on_layer: spec.on_layer.clone(),
        kind: spec.kind,
        at: spec.at,
        duration: spec.duration,
    }
}

/// Sort the transitions and refuse the ones the output cannot hold.
pub(super) fn compile_transitions(
    req: &RenderRequest,
    layers: &[PlanLayer],
    output: i64,
) -> Result<Vec<Transition>, RenderError> {
    distinct("transitions", req.transitions.iter().map(|t| t.id.as_str()))?;
    for spec in &req.transitions {
        if !layers.iter().any(|l| l.id == spec.on_layer) {
            return Err(RenderError::UnknownLayer {
                transition: spec.id.clone(),
                layer: spec.on_layer.clone(),
            });
        }
        let end = spec.at.micros().saturating_add(spec.duration.micros());
        if end > output {
            return Err(RenderError::TransitionOutOfRange {
                id: spec.id.clone(),
                at: spec.at.micros(),
                end,
                output,
            });
        }
    }
    let mut out: Vec<Transition> = req.transitions.iter().map(to_transition).collect();
    out.sort_by(|a, b| (a.at, &a.id).cmp(&(b.at, &b.id)));
    Ok(out)
}

/// Resolve the tracks. Every cut must land on exactly one bus: a clip on no
/// bus renders silent, and a clip on two buses is a routing one output stream
/// cannot express.
pub(super) fn compile_tracks(
    req: &RenderRequest,
    placed: &BTreeMap<usize, usize>,
    overlay: &[bool],
) -> Result<Vec<Track>, RenderError> {
    distinct("tracks", req.tracks.iter().map(|t| t.label.as_str()))?;
    let mut routed: BTreeSet<usize> = BTreeSet::new();
    // Resolved once, before the loop, so the answer cannot depend on the order
    // the tracks happen to be declared in.
    let any_solo = req.tracks.iter().any(|t| t.solo);
    let mut tracks: Vec<Track> = Vec::with_capacity(req.tracks.len());
    for spec in &req.tracks {
        let mut clips = Vec::with_capacity(spec.clips.len());
        for cut in &spec.clips {
            let clip = *placed.get(cut).ok_or_else(|| RenderError::UnknownCut {
                owner: format!("track {:?}", spec.label),
                cut: *cut,
                placed: req.clips.len(),
            })?;
            // An overlay is composited as **video only** — the executor renders
            // it with no audio stream at all, so the source underneath keeps
            // playing without interruption. Routing one onto a bus would
            // promise its audio in the mix and deliver a silent track, which is
            // the false-success class this compiler exists to refuse. (E08's
            // `Must NOT`: the source audio is never re-run under an insert.)
            if overlay[clip] {
                return Err(RenderError::OverlayOnAudioTrack { cut: *cut });
            }
            if !routed.insert(*cut) {
                return Err(RenderError::ClipRoutedTwice { cut: *cut });
            }
            clips.push(clip);
        }
        clips.sort_unstable();
        tracks.push(Track {
            audible: !spec.muted && (!any_solo || spec.solo),
            label: spec.label.clone(),
            bus: spec.bus.clone(),
            gain_db: spec.gain_db,
            pan: spec.pan,
            muted: spec.muted,
            solo: spec.solo,
            insert: spec.insert.clone(),
            clips,
        });
    }
    for (index, requested) in req.clips.iter().enumerate() {
        if !overlay[index] && !routed.contains(&requested.cut) {
            return Err(RenderError::UnroutedClip { cut: requested.cut });
        }
    }
    tracks.sort_by(|a, b| a.label.cmp(&b.label));
    Ok(tracks)
}

/// Order the effects into signal flow and drop the ones that emit nothing.
///
/// An empty `filter` is a saved chain that produces no work — an insert chain
/// with no effects, a default automation curve. Keeping it would make two
/// plans differ for no reason, and would move the plan's content hash when a
/// user opens a panel and saves the defaults.
pub(super) fn compile_effects(
    req: &RenderRequest,
    tracks: &[Track],
) -> Result<(Vec<EffectStep>, Vec<EffectStep>), RenderError> {
    distinct("effects", req.effects.iter().map(|e| e.id.as_str()))?;
    let buses: BTreeSet<&str> = tracks.iter().map(|t| t.bus.as_str()).collect();
    for effect in &req.effects {
        if effect.arity > buses.len() {
            return Err(RenderError::BusArity {
                stage: effect.id.clone(),
                mixes: effect.arity,
                tracks: buses.len(),
            });
        }
        // A pre-master tap cannot be a stage in the programme chain: that
        // chain has one input, and the input is the sum of every bus. Accepting
        // one would apply a user's voice high-pass to the game audio too and
        // report success. Refused, naming the field that does express it.
        if let Some(bus) = &effect.bus {
            return Err(RenderError::TapInProgrammeChain {
                id: effect.id.clone(),
                bus: bus.clone(),
            });
        }
    }
    let mut kept: Vec<EffectStep> = req
        .effects
        .iter()
        .filter(|e| !e.filter.trim().is_empty())
        .cloned()
        .collect();
    // Normalisation is the one stage that cannot be a chain step. It is
    // measure-then-apply — `loudnorm` pass 1 reads the finished signal and pass
    // 2 re-runs with those readings — and the executor already runs it from
    // `plan.loudness`. Accepting it here as well would normalise the programme
    // twice, and the second pass would undo the first's ceiling by a margin no
    // test reading a single dB would catch. Refused, so the choice is a type's
    // rather than a comment's.
    for effect in &req.effects {
        if effect.kind == EffectKind::Loudness {
            return Err(RenderError::LoudnessAsChainStep {
                id: effect.id.clone(),
            });
        }
    }
    // `EffectKind`'s declaration order *is* the canonical order, so sorting by
    // it cannot drift from the comment that explains it. The id tiebreak keeps
    // two effects of one stage (a track chain and a master chain) stable.
    kept.sort_by(|a, b| (a.kind, &a.id).cmp(&(b.kind, &b.id)));
    let video = kept
        .iter()
        .filter(|e| !e.kind.is_audio())
        .cloned()
        .collect();
    let audio = kept.into_iter().filter(|e| e.kind.is_audio()).collect();
    Ok((audio, video))
}

/// Resolve every cue into composition time through the interval map.
///
/// A source-time cue the composition cut away has no answer, and
/// [`IntervalMap::resolve`] reports that rather than inventing one. A cue
/// whose source span is used twice resolves to every position it reaches,
/// because a repeat is footage the user asked for twice.
pub(super) fn compile_cues(
    req: &RenderRequest,
    map: &IntervalMap,
) -> Result<Vec<Cue>, RenderError> {
    if req.captions.mode == CaptionMode::BurnedIn && req.captions.style_json.trim().is_empty() {
        return Err(RenderError::BurnWithoutStyle);
    }
    let mut cues: Vec<Cue> = Vec::with_capacity(req.captions.cues.len());
    for (index, cue) in req.captions.cues.iter().enumerate() {
        let resolved = map
            .resolve(Timed {
                at: cue.at,
                coordinate: req.captions.coordinate,
            })
            .map_err(|error| RenderError::CueUnmapped {
                index,
                at: cue.at.micros(),
                error,
            })?;
        match resolved {
            Mapped::Source(one) => cues.push(Cue {
                at: one.at,
                duration: cue.duration,
                source_at: vec![cue.at],
            }),
            Mapped::Composition(many) => {
                let mut positions = many;
                positions.sort_by_key(|m| m.at);
                for position in positions {
                    cues.push(Cue {
                        at: position.at,
                        duration: cue.duration,
                        source_at: vec![cue.at],
                    });
                }
            }
        }
    }
    cues.sort_by_key(|c| c.at);
    Ok(cues)
}
