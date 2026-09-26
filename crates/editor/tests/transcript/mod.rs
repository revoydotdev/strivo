//! One transcript, one composition, one source — the fixture every test in
//! this todo builds from.
//!
//! Its own module because it is the only part of the suite that is not a
//! claim: a test that perturbs one field of this value and asserts one
//! consequence is readable in isolation, and a fixture edit that changed
//! several things at once would be visible here rather than spread across
//! every assertion.
//!
//! **The numbers are chosen so the arithmetic is checkable by hand.** The
//! whole composition is a straight copy of the source, so a composition
//! coordinate equals a source coordinate until an edit removes something, and
//! every expectation in this suite is written as plain integer microseconds
//! rather than recomputed by the code under test.

#![allow(dead_code)]

use strivo_editor::time::{
    Interval, IntervalMap, MediaTime, Rate, SourceRef, SourceSpan, TimeBase,
};
use strivo_editor::transcript::{Segment, Transcript, Word, WordId};

/// A stable source identity, never a path. The link is to this, so moving the
/// file on disk cannot move a transcript word.
pub const SOURCE_ID: &str = "6f1c1c9e-0000-4000-8000-000000000001";

/// The revision this fixture's transcript came from.
pub const REVISION: &str = "9a2b7d10-0000-4000-8000-000000000002";

/// Ten seconds of media. Every timing in the fixture fits inside it, and the
/// fixture asserts it is non-empty before anything is asserted about the
/// timings — a transcript with no word timings satisfies "a word seeks to
/// where it says" for free.
pub const MEDIA_US: i64 = 10_000_000;

pub fn us(value: i64) -> MediaTime {
    MediaTime::from_micros(value).expect("a non-negative media coordinate")
}

pub fn span(start: i64, end: i64) -> Interval {
    Interval::new(us(start), us(end)).expect("an ordered interval")
}

pub fn id(raw: &str) -> WordId {
    raw.parse().expect("a well-formed word id")
}

pub fn source() -> SourceRef {
    SourceRef::new(SOURCE_ID)
}

/// A span of the fixture source, at the identity timebase and a straight-copy
/// rate, so its output interval is its source interval.
pub fn source_span(start: i64, end: i64) -> SourceSpan {
    SourceSpan::new(
        source(),
        TimeBase::from_rate(1, 1).expect("the identity timebase is a positive rational"),
        span(start, end),
        Rate::ONE,
    )
    .expect("a non-empty source span")
}

/// The unedited composition: the whole source, played straight through.
/// Output position `n` is source position `n`.
pub fn composition() -> IntervalMap {
    let mut map = IntervalMap::new();
    map.push_source(source_span(0, MEDIA_US))
        .expect("a straight copy appends at the end");
    map
}

pub fn word(raw_id: &str, start: i64, end: i64) -> Word {
    Word {
        id: id(raw_id),
        at: Some(span(start, end)),
        text: format!("word-{raw_id}"),
        speaker: Some("host".to_string()),
    }
}

pub fn segment(start: i64, end: i64, words: Vec<Word>) -> Segment {
    Segment {
        at: Some(span(start, end)),
        words,
    }
}

/// The reference transcript: five words, each with a real word timing, laid
/// out contiguously over the first five seconds of the source.
///
/// | word  | source (µs)        |
/// |-------|--------------------|
/// | `w1`  | 0 .. 1 000 000     |
/// | `w2`  | 1 000 000 .. 2 000 000 |
/// | `w3`  | 2 000 000 .. 3 500 000 |
/// | `w4`  | 3 500 000 .. 4 000 000 |
/// | `w5`  | 4 000 000 .. 5 000 000 |
pub fn transcript() -> Transcript {
    Transcript {
        segments: vec![
            segment(
                0,
                3_500_000,
                vec![
                    word("w1", 0, 1_000_000),
                    word("w2", 1_000_000, 2_000_000),
                    word("w3", 2_000_000, 3_500_000),
                ],
            ),
            segment(
                3_500_000,
                5_000_000,
                vec![
                    word("w4", 3_500_000, 4_000_000),
                    word("w5", 4_000_000, 5_000_000),
                ],
            ),
        ],
    }
}

/// The same transcript as [`transcript`], but as a provider that reports only
/// segment-level timing: every word's `at` is `None` and the segment carries
/// the span. This is the degraded fixture, and the word timings are absent
/// rather than interpolated.
pub fn segment_timed_transcript() -> Transcript {
    Transcript {
        segments: vec![
            segment(
                0,
                3_500_000,
                vec![
                    Word {
                        at: None,
                        ..word("w1", 0, 0)
                    },
                    Word {
                        at: None,
                        ..word("w2", 0, 0)
                    },
                    Word {
                        at: None,
                        ..word("w3", 0, 0)
                    },
                ],
            ),
            segment(
                3_500_000,
                5_000_000,
                vec![
                    Word {
                        at: None,
                        ..word("w4", 0, 0)
                    },
                    Word {
                        at: None,
                        ..word("w5", 0, 0)
                    },
                ],
            ),
        ],
    }
}

/// A transcript whose provider reported no timings at all: neither words nor
/// segments. Not an error — a named state.
pub fn untimed_transcript() -> Transcript {
    Transcript {
        segments: vec![Segment {
            at: None,
            words: vec![Word {
                at: None,
                ..word("w1", 0, 0)
            }],
        }],
    }
}
