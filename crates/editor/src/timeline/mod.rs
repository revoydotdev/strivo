//! The timeline: a view over the compiled plan, and the six edits that change it.
//!
//! ## The one thing this module insists on
//!
//! A timeline is **not** a second description of a composition. It owns a
//! [`crate::render::RenderRequest`] and hands it to
//! [`crate::render::compile`]; the [`crate::render::CompiledPlan`] that comes
//! back is the only thing anything is allowed to draw. So a surface built on
//! this cannot express an edit the render contract will not compile — and a
//! test proves it, by asking a timeline whose track names a cut that does not
//! exist to produce a plan and watching the compiler refuse.
//!
//! ## Where time comes from
//!
//! Every coordinate is todo 22's [`crate::time::MediaTime`], an integer number
//! of microseconds. There is no `f64` seconds, no millisecond column, and no
//! accessor that would let one back in. The frame layer ([`frames`]) converts a
//! frame index at the boundary and states its own bound, because a frame is not
//! a time — it is a rate, and the rate is a property of a stream.
//!
//! ## What an edit cannot do
//!
//! It cannot touch a file. A `RenderRequest` names source *ranges*; the
//! [`SourceLibrary`] this module requires is a declared length per source, used
//! only to clamp an edit, and there is no path anywhere in the type that could
//! be opened for writing.

mod apply;
mod edit;
mod frames;
mod structure;

use std::collections::BTreeMap;
use std::fmt;

use crate::render::{CompiledPlan, RenderError, RenderRequest};
use crate::time::{Interval, MediaTime, SourceSpan};

pub use edit::{Edge, Edit, TimelineError};
pub use frames::{frame_at, seek_to_frame, FrameIndex};

/// How long each source is, so an edit can be clamped against a real end.
///
/// A declared length rather than a probe: the timeline's job is to answer "is
/// this edit inside the media", and a length the caller states is a length a
/// test can state. A source with no entry here is refused rather than treated
/// as unbounded, because "unbounded" is a clamp to nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceLibrary {
    lengths: BTreeMap<String, MediaTime>,
}

impl SourceLibrary {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Declare one source's length. Re-declaring replaces it, because a source
    /// that changed length is a different source as far as a clamp is
    /// concerned.
    #[must_use]
    pub fn with(mut self, source: &str, length: MediaTime) -> Self {
        self.lengths.insert(source.to_string(), length);
        self
    }

    /// The declared length, or `None` for a source nobody declared. `None` is
    /// the whole of "unknown" — there is no zero default, because a source of
    /// zero length is a different claim.
    #[must_use]
    pub fn length_of(&self, source: &str) -> Option<MediaTime> {
        self.lengths.get(source).copied()
    }
}

/// One edge that moved somewhere other than where it was asked to go.
///
/// Returned rather than logged, because a drag that was clamped and a drag
/// that was applied are two different things to tell a creator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Clamp {
    pub clip: usize,
    /// `"in"` or `"out"` — which edge moved.
    pub edge: &'static str,
    pub requested_micros: i64,
    pub applied_micros: i64,
}

/// Where a drag landed, and what it snapped to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Snap {
    /// The position the edit should use.
    pub at: MediaTime,
    /// The existing edge it snapped to, or `None` for a free drag.
    ///
    /// An `Option<MediaTime>` and not a sentinel, because the first frame is
    /// a real anchor at `0` and a surface that cannot tell "snapped to the
    /// start" from "snapped to nothing" is the `||`-on-a-measured-zero defect
    /// this whole module exists to avoid.
    pub anchor: Option<MediaTime>,
}

/// A composition being edited, and the lanes linked across it.
#[derive(Debug, Clone, PartialEq)]
pub struct Timeline {
    request: RenderRequest,
    links: Vec<(usize, usize)>,
    sources: SourceLibrary,
}

impl Timeline {
    /// Open a timeline over an already-admitted request.
    ///
    /// Refuses a request with no clips: an empty composition renders an empty
    /// file, so a timeline that could hold one would make "the surface and the
    /// plan agree" true for free.
    pub fn new(request: RenderRequest, sources: SourceLibrary) -> Result<Self, TimelineError> {
        if request.clips.is_empty() {
            return Err(TimelineError::NoClips);
        }
        // Every source must have a declared length up front, so the first edit
        // is not the first time a caller learns a source is unbounded.
        for clip in &request.clips {
            let source = clip.span.source.as_str();
            if sources.length_of(source).is_none() {
                return Err(TimelineError::UnknownSource {
                    source: source.to_string(),
                });
            }
        }
        Ok(Self {
            request,
            links: Vec::new(),
            sources,
        })
    }

    /// Apply one edit, returning every edge that was clamped on the way.
    ///
    /// The clamps are the whole report: an empty return means every edge landed
    /// exactly where it was asked to.
    pub fn apply(&mut self, edit: &Edit) -> Result<Vec<Clamp>, TimelineError> {
        match *edit {
            Edit::Trim { clip, edge, to } => self.trim(clip, edge, to),
            Edit::Split { clip, at } => self.split(clip, at).map(|()| Vec::new()),
            Edit::Ripple { range } => self.ripple(range).map(|()| Vec::new()),
            Edit::Slip { clip, by } => self.slip(clip, by).map(|()| Vec::new()),
            Edit::Link { clip, with, linked } => self.link(clip, with, linked).map(|()| Vec::new()),
        }
    }

    /// Compile the edited request — the **only** way to a plan.
    ///
    /// Delegates to todo 23's compiler rather than describing a composition
    /// here, which is what makes "preview and export agree" one assertion.
    pub fn plan(&self) -> Result<CompiledPlan, RenderError> {
        crate::render::compile(&self.request)
    }

    /// The request as it stands, for a caller that needs the clips themselves
    /// rather than the plan.
    #[must_use]
    pub const fn request(&self) -> &RenderRequest {
        &self.request
    }

    pub(crate) fn require(&self, clip: usize) -> Result<(), TimelineError> {
        if clip >= self.request.clips.len() {
            return Err(TimelineError::NoClip {
                index: clip,
                clips: self.request.clips.len(),
            });
        }
        Ok(())
    }

    pub(crate) fn span(&self, clip: usize) -> Result<&SourceSpan, TimelineError> {
        self.require(clip)?;
        Ok(&self.request.clips[clip].span)
    }

    /// The clip's source range. Callers have already proved `clip` exists.
    pub(crate) fn range(&self, clip: usize) -> (MediaTime, MediaTime) {
        let r = self.request.clips[clip].span.range;
        (r.start(), r.end())
    }

    pub(crate) fn set_range(&mut self, clip: usize, range: Interval) -> Result<(), TimelineError> {
        let span = self.request.clips[clip].span.clone();
        self.request.clips[clip].span =
            crate::time::SourceSpan::new(span.source, span.timebase, range, span.rate)?;
        Ok(())
    }

    /// The declared length of the source this clip reads. Without it there is
    /// nothing honest to clamp an edit against.
    pub(crate) fn length(&self, clip: usize) -> Result<MediaTime, TimelineError> {
        let source = self.request.clips[clip].span.source.as_str().to_string();
        self.sources
            .length_of(&source)
            .ok_or(TimelineError::UnknownSource { source })
    }

    /// The next unused cut index. Cut indices are what a layer or a track
    /// names, so a new piece needs one nothing else holds.
    pub(crate) fn next_cut(&self) -> usize {
        self.request.clips.iter().map(|c| c.cut).max().unwrap_or(0) + 1
    }

    /// Whether two lanes are linked.
    #[must_use]
    pub fn is_linked(&self, clip: usize, with: usize) -> bool {
        let (a, b) = (clip.min(with), clip.max(with));
        self.links.contains(&(a, b))
    }

    /// Every position a drag could snap to: the first frame, each clip's
    /// output boundaries, and each clip's source in-point.
    ///
    /// The output boundaries come from todo 22's `IntervalMap`, so a clip list
    /// it cannot place is an error here rather than a set of wrong edges.
    pub fn snap_points(&self) -> Result<Vec<MediaTime>, TimelineError> {
        let outputs = structure::placed(&self.request.clips)?;
        let mut points = vec![MediaTime::ZERO];
        for (index, output) in outputs.iter().enumerate() {
            points.push(output.start());
            points.push(output.end());
            points.push(self.request.clips[index].span.range.start());
        }
        points.sort_unstable();
        points.dedup();
        Ok(points)
    }

    /// Pull `at` onto the nearest snap point within `threshold`, or leave it
    /// where it is.
    ///
    /// Ties go to the **earlier** point: a drag that lands exactly between two
    /// edges snaps backwards, which is the direction that never hides a frame
    /// the creator can no longer reach.
    #[must_use]
    pub fn snap(&self, at: MediaTime, threshold: MediaTime) -> Snap {
        let Ok(points) = self.snap_points() else {
            return Snap { at, anchor: None };
        };
        let window = threshold.micros();
        let mut best: Option<(i64, MediaTime)> = None;
        for point in points {
            let distance = (point.micros() - at.micros()).abs();
            if distance > window {
                continue;
            }
            let better = best.is_none_or(|(d, p)| distance < d || (distance == d && point < p));
            if better {
                best = Some((distance, point));
            }
        }
        match best {
            Some((_, anchor)) => Snap {
                at: anchor,
                anchor: Some(anchor),
            },
            None => Snap { at, anchor: None },
        }
    }
}

impl fmt::Display for Timeline {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} clip(s), {} link(s), revision {}",
            self.request.clips.len(),
            self.links.len(),
            self.request.revision
        )
    }
}
