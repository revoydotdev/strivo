//! The six edits, named — and every way one of them can be refused.
//!
//! Every operation here transforms the **clip list**, never a file. A
//! [`crate::render::RenderRequest`] carries source *ranges* and no path it could
//! write, so "the original media is never touched" is a property of the type
//! rather than a promise a caller has to keep.
//!
//! Composition coordinates (`Split`'s `at`, `Ripple`'s `range`) resolve through
//! todo 22's [`crate::time::IntervalMap`] — built here from the current clip
//! list and dropped afterwards — so the output-to-source mapping is the
//! compiler's own code rather than a second implementation of it.

use std::fmt;

use crate::time::{Interval, MediaTime, TimeError};

/// Which end of a clip an edit moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Edge {
    /// The clip's first source microsecond.
    In,
    /// One past the clip's last source microsecond.
    Out,
}

impl Edge {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::In => "in",
            Self::Out => "out",
        }
    }
}

/// One user edit, in the vocabulary the surface speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Edit {
    /// Shorten or lengthen one end of one clip. Clamped to the source, and the
    /// clamp is reported rather than performed silently.
    Trim {
        clip: usize,
        edge: Edge,
        to: MediaTime,
    },
    /// Cut one clip in two at a **composition** position. Adds time.
    Split { clip: usize, at: MediaTime },
    /// Remove a **composition** range and close the hole. Takes time.
    Ripple { range: Interval },
    /// Move a clip's source window without moving its output. `by` is a signed
    /// count of microseconds — a *delta*, not a coordinate, which is why it is
    /// an `i64` and not a `MediaTime` that could not say "earlier".
    Slip { clip: usize, by: i64 },
    /// Make an edit on one lane also happen on another.
    Link {
        clip: usize,
        with: usize,
        linked: bool,
    },
}

/// Every way an edit can be refused, as a distinct value.
///
/// Typed because "your drag was clamped" and "your drag was applied" are two
/// different outcomes, and a caller that cannot tell them apart will report a
/// success it did not get.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimelineError {
    /// A timeline with no clips renders to nothing, and "the plan and the
    /// surface agree" would then hold for free.
    NoClips,
    NoClip {
        index: usize,
        clips: usize,
    },
    /// A clip trimmed to no length has no reverse map, so it is refused rather
    /// than placed.
    ZeroLength {
        clip: usize,
        at: i64,
    },
    OutOfSource {
        clip: usize,
        requested: i64,
        available: i64,
    },
    /// A source with no declared length cannot be clamped against, and
    /// clamping to nothing is what a silent over-read looks like.
    UnknownSource {
        source: String,
    },
    SplitOutsideClip {
        clip: usize,
        at: i64,
        output: (i64, i64),
    },
    RippleOutsideComposition {
        range: (i64, i64),
        duration: i64,
    },
    SelfLink {
        clip: usize,
    },
    /// The clip list cannot be placed on an output timeline at all — an
    /// overlap, a hole, an empty span, or a speed whose output is not a whole
    /// microsecond.
    Time(TimeError),
}

impl fmt::Display for TimelineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoClips => f.write_str("a timeline needs at least one clip; this one has none"),
            Self::NoClip { index, clips } => {
                write!(f, "clip {index} does not exist; the timeline has {clips}")
            }
            Self::ZeroLength { clip, at } => {
                write!(f, "clip {clip} would trim to no length at {at} us")
            }
            Self::OutOfSource {
                clip,
                requested,
                available,
            } => write!(
                f,
                "clip {clip} would read {requested} us of a source that is {available} us long"
            ),
            Self::UnknownSource { source } => write!(
                f,
                "source {source:?} has no declared length, so an edit cannot be clamped against it"
            ),
            Self::SplitOutsideClip { clip, at, output } => write!(
                f,
                "cannot split clip {clip} at {at} us: it occupies output {}..{}",
                output.0, output.1
            ),
            Self::RippleOutsideComposition { range, duration } => write!(
                f,
                "cannot ripple {}..{} of a composition that is {duration} us long",
                range.0, range.1
            ),
            Self::SelfLink { clip } => {
                write!(
                    f,
                    "clip {clip} cannot be linked to itself; every edit would apply twice"
                )
            }
            Self::Time(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for TimelineError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Time(e) => Some(e),
            _ => None,
        }
    }
}

impl From<TimeError> for TimelineError {
    fn from(e: TimeError) -> Self {
        Self::Time(e)
    }
}

impl TimelineError {
    /// The stable token a route reports, so a client can branch on the class
    /// without matching on prose.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NoClips | Self::ZeroLength { .. } | Self::Time(_) => "timeline/malformed",
            Self::NoClip { .. }
            | Self::SplitOutsideClip { .. }
            | Self::RippleOutsideComposition { .. }
            | Self::SelfLink { .. } => "timeline/out-of-range",
            Self::OutOfSource { .. } | Self::UnknownSource { .. } => "timeline/source",
        }
    }
}
