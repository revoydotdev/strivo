//! The wall-clock contract: a clean reconnect maps exactly, a drifting one
//! reports its uncertainty, and a gap is refused rather than clamped.

use super::super::clock::{ClockMap, ClockSegment};
use super::super::{MediaTime, TimeError};
use super::at;

const T0: i64 = 1_700_000_000_000;
const HOUR_US: i64 = 3_600_000_000;

/// A one-hour capture in two parts with a four-minute reconnect gap between
/// them: part two starts on the wall clock at T0 + 64 minutes while its
/// media time follows part one with no hole, which is the case
/// `src/recording/segments.rs` produces and the one a single rate cannot
/// express.
fn two_part_capture() -> ClockMap {
    let mut map = ClockMap::new();
    map.push(ClockSegment::exact(T0, MediaTime::ZERO, at(HOUR_US)))
        .expect("part one");
    map.push(ClockSegment::exact(
        T0 + 64 * 60_000,
        at(HOUR_US),
        at(HOUR_US),
    ))
    .expect("part two");
    map
}

/// Given a capture whose clock ran in real time across a reconnect
/// When wall-clock instants inside each part are mapped to media time
/// Then every answer is exact, and the media timeline has no hole at the join.
#[test]
fn a_zero_drift_reconnect_maps_exactly() {
    let map = two_part_capture();

    let first = map.media_at(T0).expect("the first instant");
    assert_eq!(first.at, MediaTime::ZERO, "the capture's first frame");
    assert_eq!(
        first.uncertainty_micros, 0,
        "a real-time capture has no uncertainty to report"
    );

    let mid_first = map.media_at(T0 + 1_500).expect("1.5s into part one");
    assert_eq!(mid_first.at, at(1_500_000), "1.5s of media time");
    assert_eq!(mid_first.uncertainty_micros, 0);

    let second_start = T0 + 64 * 60_000;
    let at_join = map
        .media_at(second_start)
        .expect("the first instant of part two");
    assert_eq!(
        at_join.at,
        at(HOUR_US),
        "part two continues the media timeline with no hole"
    );
    assert_eq!(at_join.uncertainty_micros, 0);

    let late = map
        .media_at(second_start + 60_000)
        .expect("a minute into part two");
    assert_eq!(late.at, at(HOUR_US + 60_000_000));
    assert_eq!(late.uncertainty_micros, 0);

    // And the other direction is exact too, to the millisecond.
    let reverse = map.wall_of(at(HOUR_US + 1_000_000)).expect("media to wall");
    assert_eq!(
        reverse.wall_ms,
        second_start + 1_000,
        "media 1s past the join is 1s of wall clock past the join"
    );
    assert_eq!(
        reverse.uncertainty_micros, 0,
        "and a real-time capture is exact in both directions"
    );
}

/// Given a capture whose clock drifted, gaining a millisecond per second
/// When a media coordinate far from the anchor is converted
/// Then the drift is reported as a growing bound rather than being folded
/// silently into the value.
#[test]
fn a_drifting_capture_reports_its_uncertainty() {
    let mut map = ClockMap::new();
    // 1001 wall microseconds per media microsecond: the wall clock gains a
    // millisecond every second of media time.
    map.push(ClockSegment::new(
        T0,
        MediaTime::ZERO,
        at(HOUR_US),
        1_001,
        1_000,
    ))
    .expect("segment");

    let at_anchor = map.media_at(T0).expect("the anchor");
    assert_eq!(
        at_anchor.uncertainty_micros, 0,
        "at the anchor there is nothing to have drifted yet"
    );

    let after_10s = map.media_at(T0 + 10_000).expect("10s of wall clock");
    // 10 000 wall ms is 10 000 000 wall us, which at 1001/1000 is
    // 9 990 009.99 media us.
    assert_eq!(
        after_10s.at,
        at(9_990_009),
        "the media coordinate uses the declared rate, not real time"
    );
    assert!(
        after_10s.uncertainty_micros > 0,
        "a drifting capture must not claim exactness, got {}",
        after_10s.uncertainty_micros
    );
    // The bound is the drift the rate implies: 10ms of wall clock gained in
    // 10s, and it must grow with distance rather than being a constant.
    let after_60s = map.media_at(T0 + 60_000).expect("60s of wall clock");
    assert!(
        after_60s.uncertainty_micros > after_10s.uncertainty_micros,
        "the bound grows with distance from the anchor: {} then {}",
        after_10s.uncertainty_micros,
        after_60s.uncertainty_micros
    );
    assert_eq!(
        after_60s.uncertainty_micros, 59_941,
        "which is the 60 000 000 wall us minus the 59 940 059 media us the rate yields"
    );
    // The reverse direction reports the same divergence to within the one
    // microsecond each direction truncates independently.
    let reverse = map.wall_of(after_10s.at).expect("media to wall");
    assert_eq!(
        reverse.wall_ms,
        T0 + 9_999,
        "media 9 990 009 us is 9 999 ms of wall clock at 1001/1000"
    );
    assert_eq!(
        reverse.uncertainty_micros, 9_990,
        "and the reverse direction reports the same divergence, to the one \
         microsecond each direction truncates independently"
    );
}

/// Given a capture with a reconnect gap on the wall clock
/// When a wall-clock instant inside the gap is converted
/// Then it is refused: nothing was being recorded, so there is no media
/// coordinate, and the nearest segment edge is not one.
#[test]
fn a_reconnect_gap_has_no_media_coordinate() {
    let map = two_part_capture();
    let in_gap = T0 + HOUR_US / 1_000 + 60_000;

    let err = map.media_at(in_gap).expect_err("a gap has no media time");
    assert_eq!(
        err,
        TimeError::OutsideClockMap { wall_ms: in_gap },
        "and the refusal is typed, carrying the instant that could not be placed"
    );
    // An instant before the capture started is refused for the same reason.
    assert_eq!(
        map.media_at(T0 - 1).expect_err("before the capture"),
        TimeError::OutsideClockMap { wall_ms: T0 - 1 }
    );
    // An empty map has no answer either, rather than a default of zero.
    assert_eq!(
        ClockMap::new().media_at(T0).expect_err("no segments"),
        TimeError::OutsideClockMap { wall_ms: T0 }
    );
}

/// Given a segment list in which a second segment claims media time the first
/// already covers
/// When it is added
/// Then it is refused, because two segments answering for one frame have no
/// single answer and taking the first would hide the conflict.
#[test]
fn an_overlapping_clock_segment_is_refused() {
    let mut map = ClockMap::new();
    map.push(ClockSegment::exact(T0, MediaTime::ZERO, at(HOUR_US)))
        .expect("part one");

    let media_overlap = map
        .push(ClockSegment::exact(
            T0 + 2 * 3_600_000,
            at(HOUR_US - 1),
            at(1_000),
        ))
        .expect_err("media time already covered");
    assert_eq!(
        media_overlap,
        TimeError::ClockDiscontinuity {
            wall_start_ms: T0 + 2 * 3_600_000,
        }
    );

    // A segment that starts inside the first one's wall range, while its
    // media time follows on, is a wall-clock overlap.
    let wall_overlap = map
        .push(ClockSegment::exact(T0 + 1_000, at(HOUR_US), at(1_000)))
        .expect_err("wall time already covered");
    assert_eq!(
        wall_overlap,
        TimeError::ClockOverlap {
            wall_start_ms: T0 + 1_000,
        }
    );
    assert_eq!(
        map.segments().len(),
        1,
        "and the refused segments were not recorded"
    );
}

/// Given a clock map
/// When it is serialised and read back
/// Then every segment's rate, anchor and covered length survive, because this
/// is the document a capture session stores.
#[test]
fn the_clock_map_survives_serialisation() {
    let map = two_part_capture();
    let json = serde_json::to_string(&map).expect("serialises");
    let back: ClockMap = serde_json::from_str(&json).expect("deserialises");

    assert_eq!(back, map);
    assert_eq!(back.segments().len(), 2);
    assert_eq!(
        back.media_at(T0 + 1_500)
            .expect("still exact")
            .uncertainty_micros,
        0
    );
}
