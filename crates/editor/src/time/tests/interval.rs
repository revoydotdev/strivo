//! The bidirectional round trip: trim, repeated clip, speed change, gap and
//! overlap all map correctly in both directions, in integer arithmetic.

use super::super::interval_map::{
    Coordinate, Entry, IntervalMap, Mapped, SourceRef, SourceSpan, Timed,
};
use super::super::{Rate, TimeBase, TimeError};
use super::{at, base, span};

fn segment(source: &str, from: i64, len: i64, rate: Rate) -> SourceSpan {
    SourceSpan::new(SourceRef::new(source), base(), span(from, len), rate)
        .expect("a non-empty source span is constructible")
}

/// Given a composition that trims the head and tail of one source
/// When every output position is mapped forward and every source position back
/// Then both directions agree, and a deleted region maps to nothing at all.
#[test]
fn a_trim_round_trips_in_both_directions() {
    let mut map = IntervalMap::new();
    // Keep 10s..70s of a 120s source: the first 10s and the last 50s are gone.
    map.push_source(segment("a", 10_000_000, 60_000_000, Rate::ONE))
        .expect("segment placed");

    assert_eq!(
        map.output_duration(),
        at(60_000_000),
        "the output is the kept 60s, not the source's 120s"
    );
    for offset in [0_i64, 1, 1_000_000, 59_999_999] {
        let forward = map.source_at(at(offset)).expect("mapped");
        assert_eq!(
            forward.at,
            at(10_000_000 + offset),
            "output {offset} us is source 10s + {offset} us"
        );
        assert_eq!(forward.uncertainty_micros, 0, "the arithmetic is exact");
        let back = map.composition_times_of(forward.at).expect("mapped back");
        assert_eq!(back.len(), 1, "one output position shows it");
        assert_eq!(back[0].at, at(offset), "and the round trip closes");
    }
    let err = map
        .composition_times_of(at(0))
        .expect_err("the deleted head maps to nothing");
    assert_eq!(
        err,
        TimeError::Unmapped {
            coordinate: Coordinate::Source,
            at: 0,
        },
        "a deleted region is unmapped, not clamped to the first kept frame"
    );
}

/// Given one source span used twice in one composition, with a gap between
/// When the shared source position is mapped back
/// Then both output positions come back, because the reverse map is
/// many-to-one and taking only the first would drop the repeat.
#[test]
fn a_repeated_clip_maps_to_every_output_position() {
    let mut map = IntervalMap::new();
    map.push_source(segment("a", 0, 10_000_000, Rate::ONE))
        .expect("first");
    map.push_gap(at(5_000_000)).expect("gap");
    map.push_source(segment("a", 0, 10_000_000, Rate::ONE))
        .expect("repeat");

    let hits = map
        .composition_times_of(at(4_000_000))
        .expect("the repeated source maps back");
    let positions: Vec<i64> = hits.iter().map(|h| h.at.micros()).collect();

    assert_eq!(
        positions,
        vec![4_000_000, 19_000_000],
        "both output positions, the second shifted by the 5s gap"
    );
}

/// Given a clip played at double speed and one at half speed
/// When each is mapped in both directions
/// Then the output length is the scaled source length and the round trip
/// closes on the source position.
#[test]
fn a_speed_change_round_trips() {
    let mut map = IntervalMap::new();
    // `Rate` is output per unit of source, so double speed is 1/2: 20s of
    // source occupies 10s of output.
    map.push_source(segment(
        "a",
        5_000_000,
        20_000_000,
        Rate::new(1, 2).expect("2x"),
    ))
    .expect("fast");
    // Half speed is 2/1: 10s of source occupies 20s of output.
    map.push_source(segment("a", 0, 10_000_000, Rate::new(2, 1).expect("0.5x")))
        .expect("slow");

    assert_eq!(
        map.output_duration(),
        at(30_000_000),
        "10s of doubled output plus 20s of halved output"
    );
    assert_eq!(
        map.source_at(at(0)).expect("mapped").at,
        at(5_000_000),
        "the fast clip starts at the source position the edit states"
    );
    assert_eq!(
        map.source_at(at(9_999_999)).expect("mapped").at,
        at(24_999_998),
        "the last output microsecond is the last source position the 1/2 rate reaches"
    );
    assert_eq!(
        map.source_at(at(10_000_000)).expect("mapped").at,
        at(0),
        "the slow clip starts at its own source position"
    );
    for offset in [0_i64, 1_000_000, 4_000_000] {
        let out = at(10_000_000 + offset * 2);
        let forward = map.source_at(out).expect("mapped");
        let back = map.composition_times_of(forward.at).expect("mapped back");
        assert_eq!(back.len(), 1, "only the slow clip reads this source time");
        assert_eq!(back[0].at, out, "the halved round trip closes exactly");
    }
}

/// Given a composition with a gap in the middle
/// When a composition coordinate inside the gap is mapped forward
/// Then it is refused: a gap has no source, and clamping to the gap edge
/// would put a caption on a frame that does not exist.
#[test]
fn a_gap_has_no_source_and_is_not_clamped() {
    let mut map = IntervalMap::new();
    map.push_source(segment("a", 0, 10_000_000, Rate::ONE))
        .expect("head");
    map.push_gap(at(4_000_000)).expect("gap");
    map.push_source(segment("a", 20_000_000, 10_000_000, Rate::ONE))
        .expect("tail");

    let inside = at(12_000_000);
    assert!(
        map.entries()
            .iter()
            .any(|e| matches!(e, Entry::Gap { output } if output.contains(inside))),
        "the position really is inside the gap"
    );
    assert_eq!(
        map.source_at(inside).expect_err("a gap has no source"),
        TimeError::Unmapped {
            coordinate: Coordinate::Composition,
            at: 12_000_000,
        },
        "the refusal names the composition clock the caller asked about"
    );
    assert_eq!(
        map.source_at(at(9_999_999)).expect("before the gap").at,
        at(9_999_999)
    );
    assert_eq!(
        map.source_at(at(14_000_000)).expect("after the gap").at,
        at(20_000_000)
    );
}

/// Given one caption and one detection that share a numeric coordinate but
/// are declared on different clocks
/// When both are resolved
/// Then each is answered on the other clock, and a source-time coordinate no
/// entry reads is refused rather than served on the output.
#[test]
fn a_declared_coordinate_selects_the_direction() {
    let mut map = IntervalMap::new();
    // The source span starts at 100s, so composition 1s and source 1s are
    // genuinely different instants.
    map.push_source(segment("a", 100_000_000, 10_000_000, Rate::ONE))
        .expect("segment");

    let caption = map
        .resolve(Timed::composition(at(1_000_000)))
        .expect("a composition-time caption maps to a source time");
    assert!(
        matches!(caption, Mapped::Source(m) if m.at == at(101_000_000)),
        "composition 1s is source 101s, got {caption:?}"
    );

    let detection = map
        .resolve(Timed::source(at(101_000_000)))
        .expect("a source-time detection maps to composition times");
    assert!(
        matches!(detection, Mapped::Composition(ref v) if v.len() == 1 && v[0].at == at(1_000_000)),
        "source 101s is composition 1s, got {detection:?}"
    );

    assert_eq!(
        map.resolve(Timed::source(at(1_000_000)))
            .expect_err("composition 1s is not a source coordinate"),
        TimeError::Unmapped {
            coordinate: Coordinate::Source,
            at: 1_000_000,
        },
        "the refusal names the source clock the caller actually asked for"
    );
}

/// Given a fractional-frame timebase
/// When a frame index is converted at the boundary
/// Then the microsecond coordinate is the nearest whole microsecond, which
/// for 30000/1001 is not a whole number, so the rounding is a stated rule
/// rather than a silent truncation.
#[test]
fn a_fractional_timebase_rounds_at_the_boundary() {
    let tb = TimeBase::from_rate(30_000, 1_001).expect("30000/1001");
    assert_eq!(
        tb.rate(),
        (30_000, 1_001),
        "the timebase is recorded, not assumed"
    );

    // One tick is 1001/30000 s = 33 366.67 us, which is not a whole
    // microsecond, so frame 1 is the rounding this timebase forces.
    assert_eq!(tb.frame_to_media(0).expect("zero").micros(), 0);
    assert_eq!(tb.frame_to_media(1).expect("one").micros(), 33_367);
    // 30 ticks land on a whole microsecond: 30 * 1001 * 1e6 / 30000.
    assert_eq!(tb.frame_to_media(30).expect("thirty").micros(), 1_001_000);
    // And 30 000 ticks is exactly 1001 s, because 30000/1001 is the rate.
    assert_eq!(
        tb.frame_to_media(30_000).expect("thirty thousand").micros(),
        1_001_000_000,
        "where the rational is exact, the conversion is exact"
    );
}

/// Given a map with a fractional timebase, a fractional rate and a gap
/// When it is serialised and read back
/// Then every segment, rate, timebase and coordinate declaration survives,
/// because this is the document a composition revision stores.
#[test]
fn the_map_survives_serialisation() {
    let mut map = IntervalMap::new();
    map.push_source(
        SourceSpan::new(
            SourceRef::new("asset-1"),
            TimeBase::from_rate(30_000, 1_001).expect("timebase"),
            span(1_000_000, 2_000_000),
            Rate::new(1, 2).expect("rate"),
        )
        .expect("span"),
    )
    .expect("placed");
    map.push_gap(at(500_000)).expect("gap");

    let json = serde_json::to_string(&map).expect("serialises");
    let back: IntervalMap = serde_json::from_str(&json).expect("deserialises");

    assert_eq!(back, map, "the map is the same map");
    assert_eq!(back.output_duration(), at(1_500_000));
    let Entry::Source { span, output } = &back.entries()[0] else {
        panic!("the first entry is the source segment");
    };
    assert_eq!(span.timebase.rate(), (30_000, 1_001));
    assert_eq!((span.rate.num(), span.rate.den()), (1, 2));
    assert_eq!(output.start(), at(0));
    assert_eq!(output.duration().expect("length"), at(1_000_000));
}
