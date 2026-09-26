//! Tests for the canonical time model.
//!
//! Split by the class of thing under test: the bidirectional round trip in
//! [`interval`], the wall-clock contract in [`clock`], and the refused inputs
//! — including the legacy `f32` boundary, which is a refused input more often
//! than not — in [`malformed`].

mod clock;
mod interval;
mod malformed;

use super::{Interval, MediaTime, TimeBase};

/// A span of `len` microseconds starting at `start`.
pub fn span(start: i64, len: i64) -> Interval {
    Interval::new(
        MediaTime::from_micros(start).expect("start is not negative"),
        MediaTime::from_micros(start + len).expect("end is not negative"),
    )
    .expect("span is not inverted")
}

pub fn at(micros: i64) -> MediaTime {
    MediaTime::from_micros(micros).expect("coordinate is not negative")
}

pub fn base() -> TimeBase {
    TimeBase::from_rate(1, 1).expect("1/1 is a positive rational")
}
