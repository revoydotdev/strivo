//! Refused inputs. Every one of these would be a silently wrong frame in a
//! less careful model, so each is checked for the specific refusal rather
//! than merely for "an error came back".

use super::super::interval_map::{IntervalMap, SourceRef, SourceSpan};
use super::super::{Interval, MediaTime, Rate, TimeBase, TimeError};
use super::{at, base, span};
use crate::{Cut, CutKind, Edl};

/// Given a negative number of microseconds
/// When a media coordinate is built from it
/// Then it is refused: media time starts at the first frame.
#[test]
fn a_negative_media_time_is_refused() {
    assert_eq!(
        MediaTime::from_micros(-1).expect_err("negative"),
        TimeError::Negative { micros: -1 }
    );
}

/// Given an interval whose end is before its start
/// When it is built
/// Then it is refused rather than swapped, because a caller that passed the
/// arguments the wrong way round has a bug a silent swap would hide.
#[test]
fn an_inverted_interval_is_refused() {
    assert_eq!(
        Interval::new(at(10_000), at(9_999)).expect_err("inverted"),
        TimeError::Inverted {
            start: 10_000,
            end: 9_999,
        }
    );
}

/// Given a source span with no length
/// When it is built
/// Then it is refused: the reverse map divides by that length, and a
/// zero-length span would divide by zero rather than reverse-map.
#[test]
fn an_empty_source_span_is_refused() {
    assert_eq!(
        SourceSpan::new(
            SourceRef::new("a"),
            base(),
            Interval::new(at(5_000), at(5_000)).expect("empty but well-formed"),
            Rate::ONE,
        )
        .expect_err("no length"),
        TimeError::Empty { at: 5_000 }
    );
}

/// Given a speed whose output length is not a whole number of microseconds
/// When the segment is placed
/// Then it is refused: the map would be a rounding, and a rounding is
/// indistinguishable from a wrong frame.
#[test]
fn an_indeterminate_speed_is_refused() {
    let mut map = IntervalMap::new();
    // 1 microsecond of source at 1/2 speed is half a microsecond of output.
    let err = map
        .push_source(
            SourceSpan::new(
                SourceRef::new("a"),
                base(),
                span(0, 1),
                Rate::new(1, 2).expect("a positive rate"),
            )
            .expect("span"),
        )
        .expect_err("half a microsecond of output");
    assert_eq!(
        err,
        TimeError::IndeterminateRate {
            num: 1,
            den: 2,
            source_micros: 1,
        }
    );
    assert!(
        map.entries().is_empty(),
        "and the refused segment left nothing behind"
    );
}

/// Given a segment that does not start where the output timeline is
/// When it is placed explicitly
/// Then the overlap or the hole is refused by name, carrying both the
/// position the timeline is at and the one that was asked for.
#[test]
fn an_overlapping_output_segment_is_refused() {
    let mut map = IntervalMap::new();
    map.push_source(
        SourceSpan::new(SourceRef::new("a"), base(), span(0, 10_000_000), Rate::ONE).expect("span"),
    )
    .expect("placed");

    let overlap = map
        .push_source_at(
            at(9_000_000),
            SourceSpan::new(SourceRef::new("b"), base(), span(0, 1_000_000), Rate::ONE)
                .expect("span"),
        )
        .expect_err("starting inside the previous segment");
    assert_eq!(
        overlap,
        TimeError::OutputDiscontinuity {
            expected: 10_000_000,
            got: 9_000_000,
        }
    );

    let hole = map
        .push_source_at(
            at(11_000_000),
            SourceSpan::new(SourceRef::new("c"), base(), span(0, 1_000_000), Rate::ONE)
                .expect("span"),
        )
        .expect_err("starting after the timeline ends");
    assert_eq!(
        hole,
        TimeError::OutputDiscontinuity {
            expected: 10_000_000,
            got: 11_000_000,
        }
    );
    assert_eq!(map.entries().len(), 1, "neither was recorded");
}

/// Given a rate or a timebase that is not a positive rational
/// When it is built
/// Then it is refused, because a zero, negative or inverted rate is a stalled
/// or backwards timeline rather than a speed.
#[test]
fn a_non_positive_rational_is_refused() {
    for (num, den) in [(0_i64, 1_i64), (1, 0), (-1, 1), (1, -2)] {
        assert_eq!(
            Rate::new(num, den).expect_err("not positive"),
            TimeError::BadRational { num, den }
        );
        assert_eq!(
            TimeBase::from_rate(num, den).expect_err("not positive"),
            TimeError::BadRational { num, den }
        );
    }
    assert_eq!(
        Rate::ONE.num(),
        1,
        "and the identity rate is the documented 1/1"
    );
}

/// Given a legacy cut carrying a NaN, an infinity, a negative or an
/// inverted span
/// When it is read into the time model
/// Then each is refused by name, so a corrupt EDL is reported at the
/// boundary instead of arriving downstream as a plausible wrong frame.
#[test]
fn a_corrupt_legacy_cut_is_refused_not_coerced() {
    let cut = |start: f32, end: f32| Cut {
        start_sec: start,
        end_sec: end,
        kind: CutKind::Source {
            source_path: "/archive/a.mkv".into(),
        },
        fade_in_sec: 0.0,
        fade_out_sec: 0.0,
    };

    assert_eq!(
        cut(f32::NAN, 1.0).span().expect_err("NaN start"),
        TimeError::NotFinite { field: "start_sec" }
    );
    assert_eq!(
        cut(0.0, f32::INFINITY).span().expect_err("infinite end"),
        TimeError::NotFinite { field: "end_sec" }
    );
    assert_eq!(
        cut(-0.5, 1.0).span().expect_err("negative start"),
        TimeError::NegativeSeconds { field: "start_sec" }
    );
    assert_eq!(
        cut(2.0, 1.0).span().expect_err("inverted"),
        TimeError::Inverted {
            start: 2_000_000,
            end: 1_000_000,
        }
    );
    assert_eq!(
        cut(1.0, 1.0).span().expect_err("no length"),
        TimeError::Empty { at: 1_000_000 }
    );
}

/// Given a legacy cut
/// When its span is read
/// Then a `f32` that cannot represent microseconds exactly still converts to
/// the nearest whole microsecond rather than accumulating error, and the
/// source is a locator stand-in rather than an identity claim.
#[test]
fn a_legacy_cut_converts_to_exact_microseconds() {
    let span = Cut {
        start_sec: 1.1,
        end_sec: 2.2,
        kind: CutKind::Broll {
            broll_path: "/broll/intro.mov".into(),
        },
        fade_in_sec: 0.0,
        fade_out_sec: 0.0,
    }
    .span()
    .expect("a well-formed cut");

    assert_eq!(span.range.start(), at(1_100_000), "1.1s is 1 100 000 us");
    assert_eq!(span.range.end(), at(2_200_000), "2.2s is 2 200 000 us");
    assert_eq!(span.rate, Rate::ONE, "a cut plays at its own rate");
    assert_eq!(
        span.timebase.rate(),
        (1, 1),
        "a cut declares the identity timebase"
    );
    assert!(
        span.source.as_str().starts_with("locator:"),
        "and the source is marked as a locator stand-in, not an asset id: {}",
        span.source.as_str()
    );
}

/// Given an edit decision list whose cuts do not tile the output
/// When the whole list is mapped
/// Then the output timeline is exactly the concatenation of the cuts' own
/// durations, which is the one place the integer map and the legacy float
/// total can disagree.
#[test]
fn an_edl_maps_to_the_concatenation_of_its_cuts() {
    let cut = |start: f32, end: f32| Cut {
        start_sec: start,
        end_sec: end,
        kind: CutKind::Source {
            source_path: "/archive/a.mkv".into(),
        },
        fade_in_sec: 0.0,
        fade_out_sec: 0.0,
    };
    let edl = Edl {
        recording_id: "r1".into(),
        cuts: vec![cut(0.0, 10.0), cut(5.0, 9.0), cut(30.0, 30.5)],
    };

    let map = edl.interval_map().expect("a well-formed edl");
    assert_eq!(
        map.output_duration(),
        at(14_500_000),
        "10s + 4s + 0.5s, in integer microseconds"
    );
    assert_eq!(map.entries().len(), 3);
    // The third cut's source is 30s, and it starts on the output at 14s.
    assert_eq!(
        map.source_at(at(14_000_000)).expect("mapped").at,
        at(30_000_000)
    );
}

/// Given an edit decision list with a corrupt cut
/// When the whole list is mapped
/// Then the refusal is reported rather than the corrupt cut being skipped,
/// because a composition that silently drops a cut is a different edit.
#[test]
fn an_edl_with_one_corrupt_cut_reports_it() {
    let edl = Edl {
        recording_id: "r1".into(),
        cuts: vec![
            Cut {
                start_sec: 0.0,
                end_sec: 10.0,
                kind: CutKind::Source {
                    source_path: "/archive/a.mkv".into(),
                },
                fade_in_sec: 0.0,
                fade_out_sec: 0.0,
            },
            Cut {
                start_sec: f32::NAN,
                end_sec: 20.0,
                kind: CutKind::Source {
                    source_path: "/archive/a.mkv".into(),
                },
                fade_in_sec: 0.0,
                fade_out_sec: 0.0,
            },
        ],
    };

    assert_eq!(
        edl.interval_map().expect_err("one corrupt cut"),
        TimeError::NotFinite { field: "start_sec" }
    );
}
