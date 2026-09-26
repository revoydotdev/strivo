//! Point edits: the ones that move a clip's source window and leave the
//! composition's *shape* alone — trim, slip and linking.
//!
//! None of them adds or removes a clip, so none of them has to re-declare which
//! clips a lane claims. The two that do live in [`super::structure`].

use super::edit::{Edge, TimelineError};
use super::{Clamp, Timeline};
use crate::time::{Interval, MediaTime};

impl Timeline {
    /// Trim one edge of one clip, then the same edge of every linked lane by
    /// the *applied* delta — an edit that had to be clamped on one lane must
    /// not silently become a different edit on the other.
    pub(crate) fn trim(
        &mut self,
        clip: usize,
        edge: Edge,
        to: MediaTime,
    ) -> Result<Vec<Clamp>, TimelineError> {
        self.require(clip)?;
        // Captured before the trim, because the delta a linked lane inherits is
        // the change *this* edge made — not where this edge ended up measured
        // against a different clip's range.
        let (was_start, was_end) = self.range(clip);
        let applied = self.trim_one(clip, edge, to)?;
        let mut notes = vec![Clamp {
            clip,
            edge: edge.as_str(),
            requested_micros: to.micros(),
            applied_micros: applied.micros(),
        }];
        let delta = match edge {
            Edge::In => applied.micros() - was_start.micros(),
            Edge::Out => applied.micros() - was_end.micros(),
        };
        for partner in self.partners(clip) {
            self.require(partner)?;
            if delta != 0 {
                notes.push(self.shift(partner, edge, delta)?);
            }
        }
        Ok(notes)
    }

    /// Apply one trim, returning the coordinate the edge actually landed on.
    fn trim_one(
        &mut self,
        clip: usize,
        edge: Edge,
        to: MediaTime,
    ) -> Result<MediaTime, TimelineError> {
        let (start, end) = self.range(clip);
        let available = self.length(clip)?;
        // A trim can never invert its own range, so both ends are clamped
        // against each other as well as against the source.
        let applied = match edge {
            Edge::In => to.min(available).min(end),
            Edge::Out => to.max(start).min(available),
        };
        let range = match edge {
            Edge::In => Interval::new(applied, end)?,
            Edge::Out => Interval::new(start, applied)?,
        };
        if range.duration()?.micros() == 0 {
            return Err(TimelineError::ZeroLength {
                clip,
                at: range.start().micros(),
            });
        }
        self.set_range(clip, range)?;
        Ok(applied)
    }

    /// Move one edge of a lane by a signed delta, for a linked edit.
    fn shift(&mut self, clip: usize, edge: Edge, delta: i64) -> Result<Clamp, TimelineError> {
        let (start, end) = self.range(clip);
        let available = self.length(clip)?.micros();
        let target = match edge {
            Edge::In => start.micros().saturating_add(delta),
            Edge::Out => end.micros().saturating_add(delta),
        }
        .clamp(0, available);
        let range = match edge {
            Edge::In => Interval::new(MediaTime::from_micros(target)?, end)?,
            Edge::Out => Interval::new(start, MediaTime::from_micros(target)?)?,
        };
        let landed = match edge {
            Edge::In => range.start().micros(),
            Edge::Out => range.end().micros(),
        };
        // A lane whose linked edit would leave it with no length is reported
        // and left alone: silently emptying a linked lane is how a linked edit
        // loses a take.
        if range.duration()?.micros() > 0 {
            self.set_range(clip, range)?;
        }
        Ok(Clamp {
            clip,
            edge: edge.as_str(),
            requested_micros: target,
            applied_micros: landed,
        })
    }

    /// Move a clip's source window, leaving its output where it is.
    ///
    /// Refuses rather than clamps: a slip that cannot fit has no honest
    /// landing, and sliding the window to the edge would show frames the
    /// creator did not ask for while the surface reported success.
    pub(crate) fn slip(&mut self, clip: usize, by: i64) -> Result<(), TimelineError> {
        self.require(clip)?;
        let available = self.length(clip)?.micros();
        let (start, end) = self.range(clip);
        let shifted = |value: i64| value.checked_add(by);
        let (Some(new_start), Some(new_end)) = (shifted(start.micros()), shifted(end.micros()))
        else {
            return Err(TimelineError::OutOfSource {
                clip,
                requested: i64::MAX,
                available,
            });
        };
        for coordinate in [new_start, new_end] {
            if !(0..=available).contains(&coordinate) {
                return Err(TimelineError::OutOfSource {
                    clip,
                    requested: coordinate,
                    available,
                });
            }
        }
        self.set_range(
            clip,
            Interval::new(
                MediaTime::from_micros(new_start)?,
                MediaTime::from_micros(new_end)?,
            )?,
        )?;
        Ok(())
    }

    /// Link or unlink two lanes.
    pub(crate) fn link(
        &mut self,
        clip: usize,
        with: usize,
        linked: bool,
    ) -> Result<(), TimelineError> {
        if clip == with {
            return Err(TimelineError::SelfLink { clip });
        }
        self.require(clip)?;
        self.require(with)?;
        self.links
            .retain(|&(a, b)| !((a == clip && b == with) || (a == with && b == clip)));
        if linked {
            self.links.push((clip.min(with), clip.max(with)));
            self.links.sort_unstable();
            self.links.dedup();
        }
        Ok(())
    }

    /// Every lane linked to `clip`, in ascending order.
    pub(crate) fn partners(&self, clip: usize) -> Vec<usize> {
        self.links
            .iter()
            .filter_map(|&(a, b)| match (a == clip, b == clip) {
                (true, false) => Some(b),
                (false, true) => Some(a),
                _ => None,
            })
            .collect()
    }
}
