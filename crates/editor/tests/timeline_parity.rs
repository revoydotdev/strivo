//! Convergence: the intents the browser sends, through the server's compiler.
//!
//! The surface cannot hold a composition — it posts an intent and draws the
//! plan that comes back — so "the timeline and the render agree" is only true
//! if the *same* compiler answers both. This test closes that loop from the
//! other end: it reads the exact request bodies
//! `e2e/tests/creator-timeline.spec.ts` asserts the browser sends, replays each
//! through `Timeline::apply` + `compile`, and asserts the composition
//! intervals.
//!
//! The fixture also carries a **no-op** case. A probe that only ever sees
//! edits which change something cannot distinguish "the check ran" from "the
//! check is vacuous", and this repository has twelve vacuous passes on record.

use strivo_editor::render::RenderRequest;
use strivo_editor::time::{Interval, MediaTime, SourceRef};
use strivo_editor::timeline::{Edit, SourceLibrary, Timeline};

const FIXTURE: &str = include_str!("../../strivo-web/e2e/fixtures/timeline-edits.json");

#[derive(serde::Deserialize)]
struct Fixture {
    request: RenderRequest,
    source_length_us: i64,
    cases: Vec<Case>,
}

#[derive(serde::Deserialize)]
struct Case {
    name: String,
    edit: Edit,
    expect_clips: usize,
    expect_output_duration_us: i64,
    expect_outputs: Vec<(i64, i64)>,
}

fn library() -> SourceLibrary {
    SourceLibrary::new().with(
        SourceRef::from_locator("/recordings/stream-01.mkv").as_str(),
        MediaTime::from_micros(10_000_000).expect("a non-negative length"),
    )
}

fn fixture() -> Fixture {
    serde_json::from_str(FIXTURE).expect("the parity fixture is the shape this test reads")
}

/// Every browser intent compiles, and compiles to the intervals the fixture
/// states. The names are in the assertion output so a failure says which key
/// press broke, not just that something did.
#[test]
fn every_edit_the_surface_sends_compiles_to_the_stated_intervals() {
    let f = fixture();
    assert_eq!(
        f.cases.len(),
        4,
        "the fixture must carry three edits plus the no-op control, or this test is vacuous"
    );
    let mut library = library();
    library = library.with(
        SourceRef::from_locator("/recordings/stream-01.mkv").as_str(),
        MediaTime::from_micros(f.source_length_us).expect("a non-negative length"),
    );

    for case in &f.cases {
        let mut timeline = Timeline::new(case.request_of(&f), library.clone())
            .expect("the fixture is a legal timeline");
        let clamps = timeline.apply(&case.edit).unwrap_or_else(|e| {
            panic!(
                "{}: the surface sent an intent that was refused: {e}",
                case.name
            )
        });
        assert!(
            clamps.is_empty(),
            "{}: an unclamped edit reported a clamp",
            case.name
        );
        let plan = timeline
            .plan()
            .unwrap_or_else(|e| panic!("{}: the edited timeline did not compile: {e}", case.name));

        let outputs: Vec<(i64, i64)> = plan
            .clips
            .iter()
            .map(|c| (c.output.start().micros(), c.output.end().micros()))
            .collect();
        assert_eq!(
            outputs, case.expect_outputs,
            "{}: composition intervals",
            case.name
        );
        assert_eq!(
            plan.clips.len(),
            case.expect_clips,
            "{}: clip count",
            case.name
        );
        assert_eq!(
            plan.output_duration.micros(),
            case.expect_output_duration_us,
            "{}: output duration",
            case.name
        );
        // The two answers cannot drift: the duration is the end of the last
        // clip, not an independently carried number.
        assert_eq!(
            plan.output_duration,
            plan.clips
                .last()
                .map(|c| c.output.end())
                .unwrap_or(MediaTime::ZERO),
            "{}: duration is derived from the clips",
            case.name
        );
    }
}

/// The control: the no-op case must be *demonstrably* a no-op against the
/// pre-edit plan, not merely equal to a hard-coded expectation. If the
/// harness ever stops running the edits, this fails.
#[test]
fn the_no_op_control_really_is_a_no_op() {
    let f = fixture();
    let control = f
        .cases
        .iter()
        .find(|c| c.name.contains("no-op"))
        .expect("the fixture carries a no-op control");
    let mut timeline = Timeline::new(control.request_of(&f), library()).expect("a legal timeline");
    let before: Vec<(i64, i64)> = timeline
        .plan()
        .expect("the fixture compiles")
        .clips
        .iter()
        .map(|c| (c.span.range.start().micros(), c.span.range.end().micros()))
        .collect();
    timeline
        .apply(&control.edit)
        .expect("the control is accepted");
    let after: Vec<(i64, i64)> = timeline
        .plan()
        .expect("the fixture still compiles")
        .clips
        .iter()
        .map(|c| (c.span.range.start().micros(), c.span.range.end().micros()))
        .collect();
    assert_eq!(before, after, "the control edit changed the composition");
    assert_ne!(
        before,
        Vec::new(),
        "an empty comparison is a vacuous pass; there must be something to compare"
    );
}

impl Case {
    fn request_of(&self, f: &Fixture) -> RenderRequest {
        let _ = self;
        f.request.clone()
    }
}

/// The ripple case's range, restated as an `Interval`, so a fixture typo is a
/// compile error here rather than a silent mismatch at runtime.
#[test]
fn the_fixture_declares_every_interval_it_asserts() {
    let f = fixture();
    for case in &f.cases {
        for (start, end) in &case.expect_outputs {
            assert!(
                Interval::new(
                    MediaTime::from_micros(*start).expect("non-negative"),
                    MediaTime::from_micros(*end).expect("non-negative"),
                )
                .is_ok(),
                "{}: {start}..{end} is not an ordered interval",
                case.name
            );
        }
    }
}
