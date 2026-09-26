//! Structural edits: the two that change the composition's *shape* — split and
//! ripple — plus the bookkeeping a new or removed cut forces.

use super::edit::TimelineError;
use super::Timeline;
use crate::render::ClipRequest;
use crate::time::{Entry, Interval, IntervalMap, MediaTime, SourceSpan, TimeError};

/// The authoritative output interval of every clip, in clip order.
///
/// Built with [`IntervalMap::push_source`] so an overlap, a hole, an empty span
/// and a non-integral speed are refused by todo 22's own code rather than
/// re-checked here.
pub(crate) fn placed(clips: &[ClipRequest]) -> Result<Vec<Interval>, TimeError> {
    let mut map = IntervalMap::new();
    for clip in clips {
        map.push_source(clip.span.clone())?;
    }
    Ok(map
        .entries()
        .iter()
        .filter_map(|entry| match entry {
            Entry::Source { output, .. } => Some(*output),
            Entry::Gap { .. } => None,
        })
        .collect())
}

/// Rebuild `span` over a new source range, keeping its source, timebase, rate.
fn respan(span: &SourceSpan, range: Interval) -> Result<SourceSpan, TimeError> {
    SourceSpan::new(span.source.clone(), span.timebase, range, span.rate)
}

/// The source coordinate that `offset` of output reads from this span's start.
fn source_at_offset(span: &SourceSpan, offset: MediaTime) -> Result<MediaTime, TimeError> {
    span.rate.source_of(offset)
}

impl Timeline {
    /// Cut one clip in two at a composition position. The right half inherits
    /// the out-fade, because the fade belonged to the end of the picture and
    /// the end has not moved.
    pub(crate) fn split(&mut self, clip: usize, at: MediaTime) -> Result<(), TimelineError> {
        self.require(clip)?;
        let outputs = placed(&self.request.clips)?;
        let output = outputs[clip];
        if !output.contains(at) {
            return Err(TimelineError::SplitOutsideClip {
                clip,
                at: at.micros(),
                output: (output.start().micros(), output.end().micros()),
            });
        }
        let span = self.span(clip)?.clone();
        let cut_at = source_at_offset(
            &span,
            MediaTime::from_micros(at.micros() - output.start().micros())?,
        )?
        .checked_add(span.range.start())?;
        let left = Interval::new(span.range.start(), cut_at)?;
        let right = Interval::new(cut_at, span.range.end())?;
        if left.duration()?.micros() == 0 || right.duration()?.micros() == 0 {
            return Err(TimelineError::ZeroLength {
                clip,
                at: cut_at.micros(),
            });
        }
        let tail = ClipRequest {
            cut: self.next_cut(),
            span: respan(&span, right)?,
            fade_in: MediaTime::ZERO,
            fade_out: self.request.clips[clip].fade_out,
            overlay_at: self.request.clips[clip].overlay_at,
        };
        self.request.clips[clip].span = respan(&span, left)?;
        self.request.clips[clip].fade_out = MediaTime::ZERO;
        self.request.clips.insert(clip + 1, tail);
        self.redeclare();
        Ok(())
    }

    /// Delete a composition range and close the hole. A clip the range only
    /// partly covers is trimmed; one it straddles becomes two; one it covers
    /// wholly is gone.
    pub(crate) fn ripple(&mut self, range: Interval) -> Result<(), TimelineError> {
        let outputs = placed(&self.request.clips)?;
        let duration = outputs.last().map_or(MediaTime::ZERO, |i| i.end());
        if range.end() > duration {
            return Err(TimelineError::RippleOutsideComposition {
                range: (range.start().micros(), range.end().micros()),
                duration: duration.micros(),
            });
        }
        let mut kept: Vec<ClipRequest> = Vec::with_capacity(self.request.clips.len());
        for (index, clip) in self.request.clips.iter().enumerate() {
            let output = outputs[index];
            let from = output.start().max(range.start());
            let to = output.end().min(range.end());
            if from >= to {
                kept.push(clip.clone());
                continue;
            }
            let span = &clip.span;
            let next_cut = self.next_cut();
            if from > output.start() {
                let head = source_at_offset(
                    span,
                    MediaTime::from_micros(from.micros() - output.start().micros())?,
                )?
                .checked_add(span.range.start())?;
                let mut piece = clip.clone();
                piece.span = respan(span, Interval::new(span.range.start(), head)?)?;
                kept.push(piece);
            }
            if to < output.end() {
                // The offset of the delete's *end* within this clip, not the
                // length that survives after it: the tail resumes at the source
                // the delete stopped on, which is a different number.
                let tail = source_at_offset(
                    span,
                    MediaTime::from_micros(to.micros() - output.start().micros())?,
                )?
                .checked_add(span.range.start())?;
                let mut piece = clip.clone();
                piece.cut = next_cut;
                piece.span = respan(span, Interval::new(tail, span.range.end())?)?;
                piece.fade_in = MediaTime::ZERO;
                kept.push(piece);
            }
        }
        self.request.clips = kept;
        self.redeclare();
        Ok(())
    }

    /// A structural edit adds and removes cuts, and no declared layer or track
    /// named the new one. Rather than leave the request naming cuts that do not
    /// exist — which the compiler refuses, correctly — every lane takes the
    /// whole ordered list. Per-lane clip membership is a later todo's business;
    /// a timeline here is one lane plus its linked partners.
    fn redeclare(&mut self) {
        let cuts: Vec<usize> = self.request.clips.iter().map(|c| c.cut).collect();
        for layer in &mut self.request.layers {
            layer.clips = cuts.clone();
        }
        for track in &mut self.request.tracks {
            track.clips = cuts.clone();
        }
    }
}
