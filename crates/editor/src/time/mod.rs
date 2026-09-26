//! The canonical media time model: one unit, one interval map, one clock map.
//!
//! From `docs/CREATOR-EDITION-REPORT-2026-09-07.md:192-196` (§7.2) and F07.
//! Every timestamp in the Creator edition is an integer number of
//! **microseconds** on one axis, counted forward from the first frame. There
//! is deliberately no float accessor on any type here: a `-> f64` on
//! [`MediaTime`] would be the second representation the module exists to
//! remove, and the only conversions out are the two named boundaries below.
//!
//! **The two boundaries, and only these two.**
//!
//! 1. [`MediaTime::from_millis`] — the millisecond-to-microsecond step, the
//!    same arithmetic schema version 2 applies to the rows version 1 wrote.
//! 2. [`crate::time::edl_bridge`] — the legacy `f32`-seconds edit decision
//!    list, read once, explicitly, and rejected rather than coerced when it
//!    carries a value that is not a real media coordinate.
//!
//! **Rounding is a reported property, never a silent one.** [`TimeBase`]
//! records each stream's own rate and converts frame numbers at the
//! boundary; a timebase such as 30000/1001 is not a whole number of
//! microseconds per frame, so the conversion rounds to the nearest
//! microsecond and the residual is stated in its own documentation. Rational
//! timestamps were considered and rejected: no source in this pipeline
//! carries a fractional-frame timebase that a microsecond integer plus a
//! recorded rate cannot represent, and a second numeric representation is
//! the drift this module exists to prevent.

use std::fmt;

use serde::{Deserialize, Serialize};

pub mod clock;
mod edl_bridge;
pub mod interval_map;
#[cfg(test)]
mod tests;

pub use clock::{ClockMap, ClockSegment, WallTime};
pub use interval_map::{
    Coordinate, Entry, IntervalMap, Mapped, MappedTime, SourceRef, SourceSpan, Timed,
};

/// Every failure this model reports, as a distinct value.
///
/// Typed, because a clamped or rounded-in-silence timestamp is a wrong
/// timestamp that nothing downstream can detect. The rule the whole module
/// obeys: **a coordinate that cannot be answered is an error, never an
/// approximation and never a clamp to the nearest edge.**
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimeError {
    /// A negative number of microseconds. Media time starts at the first
    /// frame and counts forward; there is no media time before it.
    Negative { micros: i64 },
    /// An interval whose end is before its start.
    Inverted { start: i64, end: i64 },
    /// A zero-length interval where a positive length is required — a
    /// source span with no frames reads nothing and has no reverse map.
    Empty { at: i64 },
    /// A rate or a timebase that is not a positive rational.
    BadRational { num: i64, den: i64 },
    /// Integer arithmetic that cannot hold the result.
    Overflow,
    /// A speed whose output length is not a whole number of microseconds, so
    /// the mapping would be a rounding rather than a map.
    IndeterminateRate {
        num: i64,
        den: i64,
        source_micros: i64,
    },
    /// A segment that does not start where the previous one ended: the output
    /// timeline is a sequence, and an overlap or a hole here is a mistake in
    /// the composition rather than something to paper over.
    OutputDiscontinuity { expected: i64, got: i64 },
    /// A coordinate on the declared clock that the map cannot answer: an
    /// output position inside a gap has no source, and a source position no
    /// entry reads has no output. `coordinate` says which clock was asked.
    Unmapped { coordinate: Coordinate, at: i64 },
    /// A clock segment starting on media time the previous one already
    /// covered, so both would answer for one frame.
    ClockDiscontinuity { wall_start_ms: i64 },
    /// A clock segment whose wall-clock range overlaps the previous one.
    ClockOverlap { wall_start_ms: i64 },
    /// A wall-clock time between two segments — the reconnect gap, where
    /// nothing was being recorded and media time is simply not advancing.
    OutsideClockMap { wall_ms: i64 },
    /// A NaN or infinity in a float-seconds source. Coercing it to an integer
    /// would invent a timestamp that no frame carries.
    NotFinite { field: &'static str },
    /// A negative value in a float-seconds source.
    NegativeSeconds { field: &'static str },
}

impl fmt::Display for TimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Negative { micros } => {
                write!(f, "media time cannot be negative: {micros} us")
            }
            Self::Inverted { start, end } => {
                write!(f, "interval ends before it starts: {start} -> {end}")
            }
            Self::Empty { at } => write!(f, "interval at {at} has no length"),
            Self::BadRational { num, den } => {
                write!(f, "rate must be a positive rational, got {num}/{den}")
            }
            Self::Overflow => f.write_str("media time arithmetic overflowed"),
            Self::IndeterminateRate {
                num,
                den,
                source_micros,
            } => write!(
                f,
                "speed {num}/{den} maps {source_micros} us to a fractional microsecond count"
            ),
            Self::OutputDiscontinuity { expected, got } => write!(
                f,
                "segment starts at {got} us but the output timeline is at {expected} us"
            ),
            Self::Unmapped { coordinate, at } => {
                write!(f, "no {coordinate:?} coordinate at {at} us")
            }
            Self::ClockDiscontinuity { wall_start_ms } => {
                write!(
                    f,
                    "clock segment at {wall_start_ms} ms starts on media time already covered"
                )
            }
            Self::ClockOverlap { wall_start_ms } => {
                write!(
                    f,
                    "clock segment at {wall_start_ms} ms overlaps the previous one"
                )
            }
            Self::OutsideClockMap { wall_ms } => {
                write!(f, "no clock segment covers {wall_ms} ms")
            }
            Self::NotFinite { field } => write!(f, "{field} is not a finite number"),
            Self::NegativeSeconds { field } => write!(f, "{field} is negative"),
        }
    }
}

impl std::error::Error for TimeError {}

/// A point on a media timeline: a whole number of microseconds from the
/// first frame.
///
/// The unit is the only unit. It is non-negative by construction — the only
/// constructor checks — and it carries no timebase, because a media
/// coordinate *is* the timebase of record: microseconds per microsecond.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MediaTime(i64);

impl MediaTime {
    /// The first frame.
    pub const ZERO: Self = Self(0);

    /// Build from microseconds, refusing a negative coordinate.
    pub fn from_micros(micros: i64) -> Result<Self, TimeError> {
        if micros < 0 {
            return Err(TimeError::Negative { micros });
        }
        Ok(Self(micros))
    }

    /// The one millisecond-to-microsecond step in the codebase. Schema
    /// version 2 applies the same arithmetic to version 1's `duration_ms`
    /// column, so the two cannot disagree about the scale.
    pub fn from_millis(millis: i64) -> Result<Self, TimeError> {
        Self::from_micros(millis.checked_mul(1_000).ok_or(TimeError::Overflow)?)
    }

    /// The whole number of microseconds.
    #[must_use]
    pub const fn micros(self) -> i64 {
        self.0
    }

    /// Build from floating-point seconds, rounding to the nearest whole
    /// microsecond and refusing anything that is not a real position on a
    /// timeline.
    ///
    /// The one of the two named boundaries (the other is the millisecond
    /// step above), shared by every caller that has seconds: the legacy edit
    /// decision list and a chat log's `time_sec` column. A function rather
    /// than a `From` impl so the failure is a value the caller has to
    /// handle, and `f64`-specific on purpose — the point is to consume
    /// these formats correctly, not to bless them.
    pub fn from_secs_f64(secs: f64) -> Result<Self, TimeError> {
        if !secs.is_finite() {
            return Err(TimeError::NotFinite { field: "seconds" });
        }
        if secs < 0.0 {
            return Err(TimeError::NegativeSeconds { field: "seconds" });
        }
        Self::from_micros((secs * f64::from(1_000_000)).round() as i64)
    }

    /// Add a length, refusing an overflow rather than wrapping into a
    /// plausible-looking earlier time.
    pub fn checked_add(self, other: MediaTime) -> Result<Self, TimeError> {
        Self::from_micros(self.0.checked_add(other.0).ok_or(TimeError::Overflow)?)
    }
}

impl fmt::Display for MediaTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}us", self.0)
    }
}

/// A half-open span `[start, end)` of one media timeline.
///
/// Empty spans are legal and meaningful — a cut that trims to nothing is a
/// real edit — but a *source span* must have length, because its reverse map
/// divides by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Interval {
    start: MediaTime,
    end: MediaTime,
}

impl Interval {
    /// Build a span, refusing an end before the start. Never clamps.
    pub fn new(start: MediaTime, end: MediaTime) -> Result<Self, TimeError> {
        if end < start {
            return Err(TimeError::Inverted {
                start: start.micros(),
                end: end.micros(),
            });
        }
        Ok(Self { start, end })
    }

    #[must_use]
    pub const fn start(self) -> MediaTime {
        self.start
    }

    #[must_use]
    pub const fn end(self) -> MediaTime {
        self.end
    }

    /// The length, which is never negative because `new` refused the
    /// inverted span.
    pub fn duration(self) -> Result<MediaTime, TimeError> {
        self.end
            .micros()
            .checked_sub(self.start.micros())
            .map_or(Err(TimeError::Overflow), MediaTime::from_micros)
    }

    #[must_use]
    pub fn contains(self, at: MediaTime) -> bool {
        at >= self.start && at < self.end
    }
}

/// How much output one unit of source produces, as an exact rational.
///
/// `Rate::ONE` is a straight copy. A rational is stored rather than a
/// decimal so that `2/1`, `1/2` and `30000/1001` are all representable
/// without a rounding step, and a speed whose output length is not a whole
/// number of microseconds is **refused** rather than rounded — a map that
/// rounds is not a map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rate {
    num: i64,
    den: i64,
}

impl Rate {
    /// Straight copy: one second of source per second of output.
    pub const ONE: Self = Self { num: 1, den: 1 };

    /// Build a positive rational. Zero and negatives are refused: a
    /// backwards or stalled timeline is an error, not a rate.
    pub fn new(num: i64, den: i64) -> Result<Self, TimeError> {
        if num <= 0 || den <= 0 {
            return Err(TimeError::BadRational { num, den });
        }
        Ok(Self { num, den })
    }

    #[must_use]
    pub const fn num(self) -> i64 {
        self.num
    }

    #[must_use]
    pub const fn den(self) -> i64 {
        self.den
    }

    /// Forward: a source length becomes an output length. Exact or refused.
    pub fn output_of(self, source: MediaTime) -> Result<MediaTime, TimeError> {
        let scaled = i128::from(source.micros()) * i128::from(self.num);
        if scaled % i128::from(self.den) != 0 {
            return Err(TimeError::IndeterminateRate {
                num: self.num,
                den: self.den,
                source_micros: source.micros(),
            });
        }
        MediaTime::from_micros(
            i64::try_from(scaled / i128::from(self.den)).map_err(|_| TimeError::Overflow)?,
        )
    }

    /// Reverse: an output length becomes a source length. Exact or refused,
    /// with the same rule — a repeated clip at 2/1 over an odd number of
    /// microseconds is refused, not truncated.
    pub fn source_of(self, output: MediaTime) -> Result<MediaTime, TimeError> {
        let scaled = i128::from(output.micros()) * i128::from(self.den);
        if scaled % i128::from(self.num) != 0 {
            return Err(TimeError::IndeterminateRate {
                num: self.num,
                den: self.den,
                source_micros: output.micros(),
            });
        }
        MediaTime::from_micros(
            i64::try_from(scaled / i128::from(self.num)).map_err(|_| TimeError::Overflow)?,
        )
    }
}

/// A stream's own timebase: whole ticks per second, as an exact rational.
///
/// Recorded separately from the media coordinate, and converted explicitly
/// at the boundary where frames become microseconds. The typical fractional
/// case is 30000/1001, which is **not** a whole number of microseconds per
/// frame, so [`TimeBase::frame_to_media`] rounds to the nearest microsecond
/// and the residual — `1001/30000` of a microsecond, which the next frame
/// absorbs — is stated here rather than being discovered later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeBase {
    num: i64,
    den: i64,
}

impl TimeBase {
    /// Build from a positive rational tick rate.
    pub fn from_rate(num: i64, den: i64) -> Result<Self, TimeError> {
        if num <= 0 || den <= 0 {
            return Err(TimeError::BadRational { num, den });
        }
        Ok(Self { num, den })
    }

    /// Whole ticks per second, e.g. `30000/1001`.
    #[must_use]
    pub const fn rate(self) -> (i64, i64) {
        (self.num, self.den)
    }

    /// The media coordinate of a frame index, rounded to the nearest whole
    /// microsecond (halves away from zero). The rounding error is bounded by
    /// half a microsecond per frame, so a frame index large enough to
    /// accumulate it is a caller that should be working in frames rather
    /// than in microseconds.
    pub fn frame_to_media(self, frame: i64) -> Result<MediaTime, TimeError> {
        // micros = frame * den * 1_000_000 / num. Every step is checked: an
        // overflowing multiplication is a refusal, not a wrapped number.
        let scaled = i128::from(frame)
            .checked_mul(i128::from(self.den))
            .and_then(|v| v.checked_mul(1_000_000))
            .ok_or(TimeError::Overflow)?;
        let num = i128::from(self.num);
        let (quotient, remainder) = (scaled / num, scaled % num);
        // `remainder * 2` cannot overflow: it is below 2 * i64::MAX.
        let rounded = if remainder * 2 >= num {
            quotient + 1
        } else {
            quotient
        };
        MediaTime::from_micros(i64::try_from(rounded).map_err(|_| TimeError::Overflow)?)
    }
}
