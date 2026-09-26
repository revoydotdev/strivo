//! The timeline's own contract, from outside the crate.
//!
//! Two of the four names the plan fixes for this todo are here: **interval**
//! (an edit lands on the interval it was given) and **timing** (a frame-accurate
//! seek resolves to the right microsecond). Both are written against the
//! public surface, so they cannot pass by reaching into a private helper.

use strivo_editor::render::{example, CompiledPlan, RenderError};
use strivo_editor::time::{Interval, MediaTime, TimeBase};
use strivo_editor::timeline::{
    frame_at, seek_to_frame, Edge, Edit, SourceLibrary, Timeline, TimelineError,
};

fn fps_30() -> TimeBase {
    TimeBase::from_rate(30, 1).expect("30/1 is a positive rational")
}

fn fps_30000_1001() -> TimeBase {
    TimeBase::from_rate(30000, 1001).expect("30000/1001 is a positive rational")
}

/// One source, 10 seconds long, on the 30000/1001 timebase.
///
/// A non-integer timebase on purpose: 1001/30000 s is not a whole number of
/// microseconds per frame, so every frame-to-microsecond claim in this file is
/// a rounding claim and the test has to state the bound it accepts.
fn library() -> SourceLibrary {
    SourceLibrary::new().with(
        strivo_editor::time::SourceRef::from_locator(example::SOURCE).as_str(),
        MediaTime::from_micros(10_000_000).unwrap(),
    )
}

fn timeline() -> Timeline {
    Timeline::new(example::request(), library()).expect("the example request is a legal timeline")
}

fn output_of(plan: &CompiledPlan, cut: usize) -> (i64, i64) {
    let clip = plan
        .clips
        .iter()
        .find(|c| c.cut == cut)
        .unwrap_or_else(|| panic!("plan carries no clip for cut {cut}"));
    (clip.output.start().micros(), clip.output.end().micros())
}

fn source_of(plan: &CompiledPlan, cut: usize) -> (i64, i64) {
    let clip = plan
        .clips
        .iter()
        .find(|c| c.cut == cut)
        .unwrap_or_else(|| panic!("plan carries no clip for cut {cut}"));
    (
        clip.span.range.start().micros(),
        clip.span.range.end().micros(),
    )
}

// ── INTERVAL ───────────────────────────────────────────────────────────────
// The example revision places cut 0 at output [0, 1_000_000) reading source
// [0, 1_000_000), and cut 1 at output [1_000_000, 2_500_000) reading source
// [2_000_000, 3_500_000). Every expectation below is derived from those two
// stated facts and the rate (1:1), not from a previous run's output.

#[test]
fn trim_shortens_the_named_edge_and_leaves_the_other_alone() {
    let mut t = timeline();
    t.apply(&Edit::Trim {
        clip: 0,
        edge: Edge::Out,
        to: MediaTime::from_micros(600_000).unwrap(),
    })
    .expect("a trim inside the source is legal");

    let plan = t.plan().expect("the trimmed timeline still compiles");
    assert_eq!(
        output_of(&plan, 0),
        (0, 600_000),
        "output shrank to the trim"
    );
    assert_eq!(
        source_of(&plan, 0),
        (0, 600_000),
        "source read shrank with it"
    );
    // A trim shortens; it does not close the hole it leaves. The next clip
    // slides left by exactly the 400_000 us that went away, which is what
    // makes `Ripple` a separate operation rather than trim's side effect.
    assert_eq!(output_of(&plan, 1), (600_000, 2_100_000));
    assert_eq!(plan.output_duration.micros(), 2_100_000);
}

#[test]
fn trim_past_the_source_end_is_clamped_and_reported() {
    let mut t = timeline();
    let notes = t
        .apply(&Edit::Trim {
            clip: 0,
            edge: Edge::Out,
            // Ten seconds is the last microsecond a ten-second source has, so
            // ask for one past it.
            to: MediaTime::from_micros(10_000_001).unwrap(),
        })
        .expect("an over-long trim is clamped, not refused");

    let plan = t.plan().expect("the clamped timeline still compiles");
    assert_eq!(
        source_of(&plan, 0),
        (0, 10_000_000),
        "the source range is the source's own length, not what was asked for"
    );
    assert_eq!(output_of(&plan, 0), (0, 10_000_000));
    assert_eq!(notes.len(), 1, "a clamp is reported, never silent");
    assert_eq!(notes[0].requested_micros, 10_000_001);
    assert_eq!(notes[0].applied_micros, 10_000_000);
}

#[test]
fn trim_to_nothing_is_refused_rather_than_producing_an_empty_clip() {
    let mut t = timeline();
    let err = t
        .apply(&Edit::Trim {
            clip: 0,
            edge: Edge::Out,
            to: MediaTime::ZERO,
        })
        .expect_err("a zero-length clip has no reverse map and is refused");
    assert_eq!(err, TimelineError::ZeroLength { clip: 0, at: 0 });
}

#[test]
fn split_produces_two_clips_whose_output_intervals_tile_the_original() {
    let mut t = timeline();
    t.apply(&Edit::Split {
        clip: 0,
        at: MediaTime::from_micros(400_000).unwrap(),
    })
    .expect("a split inside the clip is legal");

    let plan = t.plan().expect("the split timeline still compiles");
    // Cut 0 is now [0,400k) reading [0,400k); the new cut reads [400k,1M)
    // and lands immediately after it. A hole or an overlap here would be a
    // composition the compiler refuses — `plan()` is that check.
    let half = plan
        .clips
        .iter()
        .find(|c| c.cut == 2)
        .expect("the split added a cut")
        .clone();
    assert_eq!(
        (half.output.start().micros(), half.output.end().micros()),
        (400_000, 1_000_000)
    );
    assert_eq!(
        (
            half.span.range.start().micros(),
            half.span.range.end().micros()
        ),
        (400_000, 1_000_000)
    );
    assert_eq!(output_of(&plan, 0), (0, 400_000));
    // A split takes no time and adds none: the two halves are exactly as long
    // as the clip they replaced, so nothing downstream moves. (The timeline's
    // output duration is the assertion that would catch a split that inserted
    // or dropped a frame.)
    assert_eq!(output_of(&plan, 1), (1_000_000, 2_500_000));
    assert_eq!(plan.output_duration.micros(), 2_500_000);
}

#[test]
fn split_outside_the_named_clip_is_refused_with_the_clip_s_output() {
    let mut t = timeline();
    let err = t
        .apply(&Edit::Split {
            clip: 0,
            at: MediaTime::from_micros(1_200_000).unwrap(),
        })
        .expect_err("1.2s is inside cut 1, not cut 0");
    assert_eq!(
        err,
        TimelineError::SplitOutsideClip {
            clip: 0,
            at: 1_200_000,
            output: (0, 1_000_000),
        }
    );
}

#[test]
fn ripple_delete_closes_the_hole_it_makes() {
    let mut t = timeline();
    t.apply(&Edit::Ripple {
        range: Interval::new(
            MediaTime::from_micros(200_000).unwrap(),
            MediaTime::from_micros(600_000).unwrap(),
        )
        .unwrap(),
    })
    .expect("a ripple over a range that exists is legal");

    let plan = t.plan().expect("the rippled timeline still compiles");
    // Cut 0 lost [200k,600k) of its source, so it reads [0,200k) then
    // [600k,1M) as two clips totalling 600k of output, and cut 1 starts
    // 400k earlier. Three clips, not two: the deleted range split the first.
    assert_eq!(plan.clips.len(), 3, "the deleted range split clip 0 in two");
    let spans: Vec<(i64, i64)> = plan
        .clips
        .iter()
        .map(|c| (c.output.start().micros(), c.output.end().micros()))
        .collect();
    assert_eq!(
        spans,
        vec![(0, 200_000), (200_000, 600_000), (600_000, 2_100_000)]
    );
    assert_eq!(
        plan.output_duration.micros(),
        2_100_000,
        "400k removed from 2.5s"
    );
    assert_eq!(
        (
            plan.clips[0].span.range.start().micros(),
            plan.clips[0].span.range.end().micros()
        ),
        (0, 200_000),
        "the head keeps the source it had"
    );
    assert_eq!(
        (
            plan.clips[1].span.range.start().micros(),
            plan.clips[1].span.range.end().micros()
        ),
        (600_000, 1_000_000),
        "the tail resumes at the source position the delete ended on"
    );
}

#[test]
fn ripple_over_a_range_that_is_not_there_is_refused() {
    let mut t = timeline();
    let err = t
        .apply(&Edit::Ripple {
            range: Interval::new(
                MediaTime::from_micros(5_000_000).unwrap(),
                MediaTime::from_micros(6_000_000).unwrap(),
            )
            .unwrap(),
        })
        .expect_err("past the end of the composition is not a ripple");
    assert_eq!(
        err,
        TimelineError::RippleOutsideComposition {
            range: (5_000_000, 6_000_000),
            duration: 2_500_000,
        }
    );
}

#[test]
fn slip_moves_the_source_window_and_leaves_the_output_untouched() {
    let mut t = timeline();
    let before = output_of(&t.plan().unwrap(), 1);
    t.apply(&Edit::Slip {
        clip: 1,
        by: 500_000,
    })
    .expect("a slip inside the source is legal");

    let plan = t.plan().unwrap();
    assert_eq!(
        output_of(&plan, 1),
        before,
        "slip is a source-side edit only"
    );
    assert_eq!(source_of(&plan, 1), (2_500_000, 4_000_000));
}

#[test]
fn slip_past_the_source_end_is_refused_not_wrapped() {
    let mut t = timeline();
    let err = t
        .apply(&Edit::Slip {
            clip: 1,
            by: 9_000_000,
        })
        .expect_err("slipping a 1.5s clip 9s forward on a 10s source leaves it empty");
    assert_eq!(
        err,
        TimelineError::OutOfSource {
            clip: 1,
            requested: 11_000_000,
            available: 10_000_000,
        }
    );
}

#[test]
fn linking_makes_an_edit_on_one_lane_land_on_the_other() {
    let mut t = timeline();
    t.apply(&Edit::Link {
        clip: 0,
        with: 1,
        linked: true,
    })
    .expect("linking two distinct clips is legal");
    t.apply(&Edit::Trim {
        clip: 0,
        edge: Edge::Out,
        to: MediaTime::from_micros(500_000).unwrap(),
    })
    .expect("the linked trim is legal");

    let plan = t.plan().expect("the linked timeline still compiles");
    assert_eq!(source_of(&plan, 0), (0, 500_000), "the edited lane moved");
    assert_eq!(
        source_of(&plan, 1),
        (2_000_000, 3_000_000),
        "the linked lane lost the same 500k of length at its own end"
    );
    assert!(t.is_linked(0, 1), "the link survives the edit it produced");
}

#[test]
fn linking_two_clips_of_the_same_index_is_refused() {
    let mut t = timeline();
    let err = t
        .apply(&Edit::Link {
            clip: 0,
            with: 0,
            linked: true,
        })
        .expect_err("a clip linked to itself would apply every edit twice");
    assert_eq!(err, TimelineError::SelfLink { clip: 0 });
}

#[test]
fn a_timeline_with_no_clips_is_refused_rather_than_rendering_nothing() {
    let mut request = example::request();
    request.clips.clear();
    let err = Timeline::new(request, library()).expect_err("an empty timeline is not a timeline");
    assert_eq!(err, TimelineError::NoClips);
}

#[test]
fn a_clip_reading_a_source_the_library_does_not_know_is_refused() {
    let mut request = example::request();
    request.clips[0].span.source = strivo_editor::time::SourceRef::new("locator:/elsewhere.mkv");
    let err = Timeline::new(request, library())
        .expect_err("a source with no declared length cannot be clamped against");
    assert_eq!(
        err,
        TimelineError::UnknownSource {
            source: "locator:/elsewhere.mkv".into()
        }
    );
}

#[test]
fn a_timeline_whose_tracks_claim_no_clip_is_refused_by_the_compiler() {
    // The plan-versus-surface invariant, in the smallest form: the timeline
    // cannot produce a composition the render contract will not compile.
    let mut request = example::request();
    request.tracks[0].clips = vec![9];
    let t = Timeline::new(request, library()).expect("the timeline itself is well-formed");
    assert_eq!(
        t.plan()
            .expect_err("a track naming a cut that does not exist must not compile"),
        RenderError::UnknownCut {
            owner: "track \"programme\"".into(),
            cut: 9,
            placed: 2,
        }
    );
}

#[test]
fn no_edit_ever_changes_the_source_media() {
    // "with no source mutation" is the load-bearing half of the acceptance
    // bar, and it is a property of the *type*: a Timeline owns a
    // `RenderRequest`, which carries source ranges and no paths it could
    // write. Asserting it as a value comparison is what makes it true.
    let original = example::request();
    let mut t = timeline();
    t.apply(&Edit::Trim {
        clip: 0,
        edge: Edge::In,
        to: MediaTime::from_micros(100_000).unwrap(),
    })
    .unwrap();
    t.apply(&Edit::Split {
        clip: 1,
        at: MediaTime::from_micros(1_500_000).unwrap(),
    })
    .unwrap();
    t.apply(&Edit::Slip {
        clip: 0,
        by: -50_000,
    })
    .unwrap();
    let after = t.request().clone();
    let mut named: Vec<String> = after
        .clips
        .iter()
        .map(|c| c.span.source.to_string())
        .collect();
    named.sort();
    named.dedup();
    let mut before: Vec<String> = original
        .clips
        .iter()
        .map(|c| c.span.source.to_string())
        .collect();
    before.sort();
    before.dedup();
    assert_eq!(
        named, before,
        "every edit names the same sources; none of them writes to one"
    );
    assert_eq!(
        after.clips.len(),
        3,
        "the split added a clip, so a count comparison would be measuring the \
         edit rather than the invariant"
    );
}

// ── TIMING ─────────────────────────────────────────────────────────────────

#[test]
fn a_frame_accurate_seek_resolves_to_the_right_microsecond() {
    // 30fps exactly: frame 900 is 30 seconds, and 30 * 1/30 s is a whole
    // number of microseconds, so the answer is exact and not a rounding.
    let at = seek_to_frame(fps_30(), 900).unwrap();
    assert_eq!(at.micros(), 30_000_000);
    // And it is the inverse of the index it came from.
    assert_eq!(frame_at(fps_30(), at).unwrap().frame, 900);
}

#[test]
fn a_fractional_timebase_rounds_to_within_half_a_microsecond_and_says_so() {
    // 30000/1001: one frame is 1001/30000 s = 33366.666... us. Frame 1000 is
    // 1000 * 1001/30000 s = 33.3666... s = 33_366_666.67 us, which is not a
    // whole microsecond. The claim under test is therefore a *bound*, and
    // the bound is the rounding residual — which must never exceed half a
    // microsecond, because that is the worst case of rounding to nearest.
    let at = seek_to_frame(fps_30000_1001(), 1000).unwrap();
    assert_eq!(
        at.micros(),
        33_366_667,
        "rounded to the nearest microsecond"
    );

    let idx = frame_at(fps_30000_1001(), at).unwrap();
    assert_eq!(idx.frame, 1000, "the index reads the frame it resolved to");
    assert_eq!(idx.error_micros, 0, "the boundary itself is exact");
    assert!(
        idx.frame_micros <= 33_367,
        "one frame at 30000/1001 is {} us, which rounds up from 33366.67",
        idx.frame_micros
    );
}

#[test]
fn a_playhead_between_two_frames_reports_how_far_it_is_from_the_nearest() {
    // 100ms is frame 3.0 at 30fps, so ask for a position deliberately
    // between frames and assert the *measured* residual rather than a
    // tolerance nobody chose.
    let between = MediaTime::from_micros(100_010_000).unwrap();
    let idx = frame_at(fps_30(), between).unwrap();
    assert_eq!(idx.frame, 3000);
    assert_eq!(idx.at.micros(), 100_000_000, "snapped down to the frame");
    assert_eq!(
        idx.error_micros, 10_000,
        "the position is 10_000 us past the frame it belongs to, measured"
    );
}

#[test]
fn a_seek_onto_an_exact_frame_reports_zero_error_which_is_not_the_same_as_rounded() {
    // The `||` trap this module exists to avoid: a measured zero is a real
    // measurement, so `error_micros` is a plain i64 and zero survives.
    let exact = seek_to_frame(fps_30(), 12).unwrap();
    assert_eq!(frame_at(fps_30(), exact).unwrap().error_micros, 0);
    assert_eq!(exact.micros(), 400_000);
}

#[test]
fn a_frame_index_past_the_end_of_a_long_recording_still_resolves_exactly() {
    // "including on a long recording" — six hours at 30fps is 648_000 frames,
    // which is 21_600 s = 21_600_000_000 us: past i32 microseconds and past
    // the millisecond column's comfortable range. A second representation
    // would have lost this.
    let at = seek_to_frame(fps_30(), 648_000).unwrap();
    assert_eq!(at.micros(), 21_600_000_000);
    let back = frame_at(fps_30(), at).unwrap();
    assert_eq!(back.frame, 648_000);
    assert_eq!(back.error_micros, 0);
}

// ── SNAPPING ───────────────────────────────────────────────────────────────

#[test]
fn snapping_pulls_a_drag_onto_a_clip_edge_within_the_threshold() {
    let t = timeline();
    // Cut 0's output end is 1_000_000. A drag 8ms past it is inside a 10ms
    // threshold and snaps; the same drag 50ms past it does not.
    let near = MediaTime::from_micros(1_008_000).unwrap();
    let snapped = t.snap(near, MediaTime::from_micros(10_000).unwrap());
    assert_eq!(snapped.anchor.map(MediaTime::micros), Some(1_000_000));
    assert_eq!(snapped.at.micros(), 1_000_000, "snapped *to* the anchor");

    let far = MediaTime::from_micros(1_050_000).unwrap();
    let free = t.snap(far, MediaTime::from_micros(10_000).unwrap());
    assert_eq!(free.anchor, None, "a drag outside the threshold is free");
    assert_eq!(free.at.micros(), 1_050_000, "and keeps its own position");
}

#[test]
fn snapping_to_the_zeroth_anchor_is_a_snap_and_not_a_missing_value() {
    let t = timeline();
    // The first output frame is an anchor at 0. A drag 2ms in snaps to it,
    // and `anchor: Some(0)` must be distinguishable from "no anchor" — which
    // is why the field is an Option of a MediaTime and not a MediaTime with a
    // sentinel, and why nothing here defaults with `||`.
    let snapped = t.snap(
        MediaTime::from_micros(2_000).unwrap(),
        MediaTime::from_micros(10_000).unwrap(),
    );
    assert_eq!(snapped.anchor, Some(MediaTime::ZERO));
    assert_eq!(snapped.at.micros(), 0);
}
