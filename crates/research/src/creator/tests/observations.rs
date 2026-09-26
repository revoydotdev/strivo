//! Synchronised stream events and viewer samples, with a coverage figure
//! that has a denominator.
//!
//! The claims under test, in the order the plan states them:
//!
//! 1. an event fixture persists with a coverage figure;
//! 2. an unheld permission produces an explicit unavailable state;
//! 3. no live-status field is ever written into the observation table.
//!
//! Claim 1's order matters and is the reason this file leads with the
//! anti-vacuity control. A capture with zero samples satisfies "coverage is
//! reported" for free, so a test that only checks the report passes against
//! nothing at all. Every test here that touches coverage asserts the
//! subject is non-empty **first** and the arithmetic second.

use strivo_editor::time::{ClockMap, MediaTime};

use super::super::ids::{CaptureSessionId, ProjectId};
use super::super::{
    CaptureWindow, CountedTable, Coverage, EventKind, GapReason, MetricAvailability,
    ObservationError, RawStreamEvent, RawViewerSample, Store, StreamEvent, UnavailableReason,
    ViewerSample,
};

// --- fixtures -------------------------------------------------------------
//
// Local rather than in the shared `fixtures` module: three other lanes
// edit that file, and a fixture this lane needs is not a fixture they do.

/// 0-based wall clock. Every timestamp in this file is stated, never read
/// from the host, so no assertion here can be a wall-clock flake.
const T0: i64 = 1_789_000_000_000;

fn window() -> CaptureWindow {
    // 60 s of expected capture.
    CaptureWindow::new(T0, T0 + 60_000).expect("a forward window")
}

/// One unbroken 60 s capture at real time. The clock uncertainty across it
/// is a measured zero, so every non-zero uncertainty in these tests comes
/// from the poll window — which is the point being made.
fn clock_whole() -> ClockMap {
    let mut clock = ClockMap::new();
    clock
        .push(strivo_editor::time::ClockSegment::exact(
            T0,
            MediaTime::ZERO,
            MediaTime::from_millis(60_000).expect("60 s"),
        ))
        .expect("one segment");
    clock
}

/// Two capture parts with a 20 s reconnect hole between them: 0..20 s and
/// 40..60 s are covered, 20..40 s is not.
fn clock_with_a_hole() -> ClockMap {
    let mut clock = ClockMap::new();
    let first = strivo_editor::time::ClockSegment::exact(
        T0,
        MediaTime::ZERO,
        MediaTime::from_millis(20_000).expect("20 s"),
    );
    let second = strivo_editor::time::ClockSegment::exact(
        T0 + 40_000,
        MediaTime::from_millis(40_000).expect("40 s"),
        MediaTime::from_millis(20_000).expect("20 s"),
    );
    clock.push(first).expect("first segment");
    clock.push(second).expect("second segment");
    clock
}

/// A project and a real `creator_capture_sessions` row under it.
///
/// The session row is not decoration: the observation tables carry a foreign
/// key onto it, because a sample that belongs to no capture run is
/// unattributable, and the database saying so is the enforcement.
pub(super) fn with_session(store: &Store, name: &str) -> (ProjectId, CaptureSessionId) {
    let project = super::fixtures::project(name);
    let project_id = project.id;
    store.insert_project(&project).expect("project");
    let session = CaptureSessionId::generate();
    store
        .insert_capture_session(&super::super::entities::CaptureSession {
            id: session,
            project_id,
            source_asset_id: None,
            started_at: "2026-09-07T00:00:00Z".into(),
            ended_at: None,
            finalized: false,
            attributes_json: "{}".into(),
            created_at: "2026-09-07T00:00:00Z".into(),
        })
        .expect("capture session");
    (project_id, session)
}

fn project_and_session(store: &Store) -> (ProjectId, CaptureSessionId) {
    with_session(store, "stream")
}

/// A complete 60 s capture at a 10 s poll interval: six samples, each
/// covering the 10 s before it, tiling the whole window with no gap.
///
/// The first reading is at `T0 + 10_000`, not at `T0`. A poll taken at the
/// window's first instant reaches back before the window began, so it
/// describes nothing inside it — the series would open with a reading that
/// covers no time at all, and coverage would read 50 000 of 60 000 while
/// every sample was "present".
fn full_capture() -> Vec<RawViewerSample> {
    (1..=6)
        .map(|i| RawViewerSample {
            wall_ms: T0 + i * 10_000,
            value: 100 + i,
            poll_interval_ms: 10_000,
            dropped: false,
        })
        .collect()
}

// --- claim 1: coverage is derived, bounded, and needs a denominator -------

#[test]
fn coverage_of_a_complete_capture_is_full_and_its_denominator_is_stated() {
    // Given a session with a complete sample series
    let store = Store::open_in_memory().expect("store");
    let (project, session) = project_and_session(&store);
    let clock = clock_whole();
    let batch = full_capture();
    store
        .append_viewer_samples(session, project, &batch, &window(), &clock)
        .expect("append");

    // When the coverage is computed
    let coverage = store
        .coverage(session, &window(), &clock)
        .expect("coverage");

    // Then the denominator is the stated expected window, not an implied one
    assert_eq!(
        coverage.expected_wall_ms, 60_000,
        "coverage must state the interval it is a fraction of"
    );
    assert_eq!(coverage.observed_wall_ms, 60_000);
    assert_eq!(coverage.per_mille, 1000, "6 samples x 10 s tile 60 s");
    assert!(
        coverage.gaps.is_empty(),
        "a complete capture has no gaps, got {:?}",
        coverage.gaps
    );
}

#[test]
fn coverage_is_computed_and_bounded_before_it_is_complete() {
    // Given nothing at all — this is the anti-vacuity control
    let store = Store::open_in_memory().expect("store");
    let (_project, session) = project_and_session(&store);
    let clock = clock_whole();

    // When the coverage of an empty capture is computed
    let coverage = store
        .coverage(session, &window(), &clock)
        .expect("coverage");

    // Then it is bounded to 0, not vacuously 100%, and says why
    assert_eq!(coverage.per_mille, 0, "no samples means no coverage");
    assert!(
        coverage.per_mille <= 1_000,
        "coverage is a bounded fraction"
    );
    assert_eq!(coverage.gaps.len(), 1, "the whole window is one gap");
    assert_eq!(coverage.gaps[0].reason, GapReason::NoSamples);
    assert_eq!(coverage.gaps[0].start_wall_ms, T0);
    assert_eq!(coverage.gaps[0].end_wall_ms, T0 + 60_000);
}

#[test]
fn a_shortfall_always_names_the_intervals_it_is_short_in() {
    // Given a capture that stops half way: three samples, 30 s of 60 s
    let store = Store::open_in_memory().expect("store");
    let (project, session) = project_and_session(&store);
    let clock = clock_whole();
    let batch: Vec<RawViewerSample> = (1..=3)
        .map(|i| RawViewerSample {
            wall_ms: T0 + i * 10_000,
            value: 10,
            poll_interval_ms: 10_000,
            dropped: false,
        })
        .collect();
    store
        .append_viewer_samples(session, project, &batch, &window(), &clock)
        .expect("append");

    // When the coverage is computed
    let coverage = store
        .coverage(session, &window(), &clock)
        .expect("coverage");

    // Then the figure is 500 per mille and the missing 30 s is named
    assert_eq!(coverage.per_mille, 500);
    assert_eq!(coverage.gaps.len(), 1);
    assert_eq!(coverage.gaps[0].start_wall_ms, T0 + 30_000);
    assert_eq!(coverage.gaps[0].end_wall_ms, T0 + 60_000);
    assert_eq!(
        coverage.gaps[0].reason,
        GapReason::DroppedSample,
        "the interval is inside the clock map, so the polls stopped, not the capture"
    );
}

#[test]
fn a_reconnect_hole_is_named_as_a_hole_and_not_as_dropped_polls() {
    // Given samples either side of the 20 s hole and none inside it
    let store = Store::open_in_memory().expect("store");
    let (project, session) = project_and_session(&store);
    let clock = clock_with_a_hole();
    let batch = vec![
        // 0..20 s covered by four 5 s polls
        RawViewerSample {
            wall_ms: T0 + 5_000,
            value: 5,
            poll_interval_ms: 5_000,
            dropped: false,
        },
        RawViewerSample {
            wall_ms: T0 + 10_000,
            value: 5,
            poll_interval_ms: 5_000,
            dropped: false,
        },
        RawViewerSample {
            wall_ms: T0 + 15_000,
            value: 5,
            poll_interval_ms: 5_000,
            dropped: false,
        },
        RawViewerSample {
            wall_ms: T0 + 20_000,
            value: 5,
            poll_interval_ms: 5_000,
            dropped: false,
        },
        // 40..60 s likewise
        RawViewerSample {
            wall_ms: T0 + 45_000,
            value: 5,
            poll_interval_ms: 5_000,
            dropped: false,
        },
        RawViewerSample {
            wall_ms: T0 + 50_000,
            value: 5,
            poll_interval_ms: 5_000,
            dropped: false,
        },
        RawViewerSample {
            wall_ms: T0 + 55_000,
            value: 5,
            poll_interval_ms: 5_000,
            dropped: false,
        },
        RawViewerSample {
            wall_ms: T0 + 60_000,
            value: 5,
            poll_interval_ms: 5_000,
            dropped: false,
        },
    ];
    store
        .append_viewer_samples(session, project, &batch, &window(), &clock)
        .expect("append");

    // When the coverage is computed
    let coverage = store
        .coverage(session, &window(), &clock)
        .expect("coverage");

    // Then 40 s of 60 s, with the 20 s hole named as a hole
    assert_eq!(coverage.observed_wall_ms, 40_000);
    assert_eq!(coverage.per_mille, 666, "40 s of 60 s is 666 per mille");
    assert_eq!(coverage.gaps.len(), 1);
    assert_eq!(coverage.gaps[0].start_wall_ms, T0 + 20_000);
    assert_eq!(coverage.gaps[0].end_wall_ms, T0 + 40_000);
    assert_eq!(
        coverage.gaps[0].reason,
        GapReason::ReconnectGap,
        "no clock segment covers these 20 s, so the capture was not running"
    );
}

#[test]
fn a_coverage_figure_below_full_cannot_be_built_without_its_reasons() {
    // Given a figure of 900 per mille
    let shortfall = Coverage {
        expected_wall_ms: 60_000,
        observed_wall_ms: 54_000,
        per_mille: 900,
        gaps: Vec::new(),
        widest_uncertainty_micros: 0,
    };

    // When it is validated
    let refusal = shortfall
        .validated()
        .expect_err("a bare fraction is not a coverage figure");

    // Then the refusal is the specific one
    assert!(
        refusal.to_string().contains("900"),
        "the refusal must name the figure that has no reasons, got: {refusal}"
    );
}

#[test]
fn coverage_reports_its_worst_uncertainty_because_full_coverage_is_not_full_knowledge() {
    // Given a capture whose polls are a full minute long
    let store = Store::open_in_memory().expect("store");
    let (project, session) = project_and_session(&store);
    let clock = clock_whole();
    let batch = vec![
        RawViewerSample {
            wall_ms: T0 + 30_000,
            value: 7,
            poll_interval_ms: 30_000,
            dropped: false,
        },
        RawViewerSample {
            wall_ms: T0 + 60_000,
            value: 7,
            poll_interval_ms: 30_000,
            dropped: false,
        },
    ];
    store
        .append_viewer_samples(session, project, &batch, &window(), &clock)
        .expect("append");

    // When the coverage is computed
    let coverage = store
        .coverage(session, &window(), &clock)
        .expect("coverage");

    // Then coverage is 100% and the worst uncertainty is a full 30 s
    assert_eq!(coverage.per_mille, 1000, "two 30 s polls tile 60 s");
    assert_eq!(
        coverage.widest_uncertainty_micros, 30_000_000,
        "100% coverage says every instant was inside some poll window; it does \
         not say any instant is known to better than the window"
    );
}

// --- per-sample uncertainty ----------------------------------------------

#[test]
fn a_samples_uncertainty_is_its_poll_window_plus_the_clocks_own_drift() {
    // Given an exactly-running capture and a 5 s poll
    let clock = clock_whole();

    // When a sample's position is derived
    let sample = ViewerSample::observe(
        &window(),
        &clock,
        RawViewerSample {
            wall_ms: T0 + 10_000,
            value: 42,
            poll_interval_ms: 5_000,
            dropped: false,
        },
    )
    .expect("a covered instant");

    // Then the bound is the 5 s window, the drift being a measured zero
    assert_eq!(
        sample.position.media,
        MediaTime::from_millis(10_000).expect("10 s")
    );
    assert_eq!(
        sample.position.uncertainty_micros, 5_000_000,
        "the reading is a statement about the preceding 5 s, so its position \
         is uncertain by 5 s even on an exactly-running clock"
    );
}

#[test]
fn a_drifting_clock_raises_the_samples_own_bound() {
    // Given a capture that gains a millisecond per second
    let mut clock = ClockMap::new();
    let segment = strivo_editor::time::ClockSegment::new(
        T0,
        MediaTime::ZERO,
        MediaTime::from_millis(60_000).expect("60 s"),
        1001,
        1000,
    );
    clock.push(segment).expect("one segment");

    // When a sample ten seconds in is placed
    let sample = ViewerSample::observe(
        &CaptureWindow::new(T0, T0 + 60_000).expect("window"),
        &clock,
        RawViewerSample {
            wall_ms: T0 + 10_000,
            value: 1,
            poll_interval_ms: 1_000,
            dropped: false,
        },
    )
    .expect("a covered instant");

    // Then the bound is the poll window plus the drift, never less than either
    let drift = sample.position.uncertainty_micros - 1_000_000;
    assert!(
        drift > 0,
        "a 1001/1000 clock accumulates real drift and the bound must carry it, \
         got {} us",
        sample.position.uncertainty_micros
    );
}

#[test]
fn an_event_on_an_exact_segment_has_a_measured_zero_bound() {
    // Given an exactly-running capture
    let clock = clock_whole();

    // When an event is placed on it
    let event = StreamEvent::observe(
        &window(),
        &clock,
        RawStreamEvent {
            wall_ms: T0 + 3_000,
            kind: EventKind::Marker,
            label: "intro".into(),
            category: None,
            payload_json: "{}".into(),
        },
    )
    .expect("a covered instant");

    // Then its bound is zero, and zero is a measurement rather than a default
    assert_eq!(
        event.position.media,
        MediaTime::from_millis(3_000).expect("3 s")
    );
    assert_eq!(
        event.position.uncertainty_micros, 0,
        "an instantaneous event on a 1:1 segment is exact"
    );
}

// --- the gap is refused, not clamped -------------------------------------

#[test]
fn a_sample_inside_a_reconnect_hole_is_refused_rather_than_clamped_to_the_nearest_edge() {
    // Given a sample taken during the 20 s hole
    let clock = clock_with_a_hole();
    let raw = RawViewerSample {
        wall_ms: T0 + 30_000,
        value: 1,
        poll_interval_ms: 1_000,
        dropped: false,
    };

    // When it is placed
    let refusal = ViewerSample::observe(&window(), &clock, raw)
        .expect_err("nothing was recorded at T0+30s, so there is no coordinate");

    // Then the refusal is OutsideClockMap, naming the instant
    assert!(
        matches!(refusal, ObservationError::OutsideClockMap { wall_ms } if wall_ms == T0 + 30_000),
        "a hole is a refusal, not the nearest edge, got: {refusal:?}"
    );
}

#[test]
fn a_refused_sample_leaves_no_trace_so_the_hole_stays_visible() {
    // Given a session with a complete series either side of the hole
    let store = Store::open_in_memory().expect("store");
    let (project, session) = project_and_session(&store);
    let clock = clock_with_a_hole();
    store
        .append_viewer_samples(
            session,
            project,
            &[
                RawViewerSample {
                    wall_ms: T0 + 20_000,
                    value: 1,
                    poll_interval_ms: 20_000,
                    dropped: false,
                },
                RawViewerSample {
                    wall_ms: T0 + 60_000,
                    value: 1,
                    poll_interval_ms: 20_000,
                    dropped: false,
                },
            ],
            &window(),
            &clock,
        )
        .expect("append");
    let before = store.viewer_samples(session).expect("read").len();

    // When a batch containing a sample inside the hole is appended
    let refused = store.append_viewer_samples(
        session,
        project,
        &[
            RawViewerSample {
                wall_ms: T0 + 60_000,
                value: 1,
                poll_interval_ms: 1_000,
                dropped: false,
            },
            RawViewerSample {
                wall_ms: T0 + 30_000,
                value: 1,
                poll_interval_ms: 1_000,
                dropped: false,
            },
        ],
        &window(),
        &clock,
    );

    // Then the whole batch is refused and nothing was written
    assert!(refused.is_err(), "a batch is atomic");
    let after = store.viewer_samples(session).expect("read");
    assert_eq!(after.len(), before, "the refused sample was not clamped in");
    assert_eq!(
        after.last().expect("a sample").value,
        1,
        "the pre-existing rows are untouched"
    );
}

#[test]
fn a_sample_predating_the_recording_is_refused() {
    // Given a sample a second before the session starts
    let clock = clock_whole();
    let raw = RawViewerSample {
        wall_ms: T0 - 1_000,
        value: 3,
        poll_interval_ms: 1_000,
        dropped: false,
    };

    // When it is placed
    let refusal = ViewerSample::observe(&window(), &clock, raw).expect_err("before the session");

    // Then the refusal is the specific one
    assert!(
        matches!(refusal, ObservationError::BeforeSession { wall_ms, .. } if wall_ms == T0 - 1_000),
        "got: {refusal:?}"
    );
}

// --- claim 2: an unheld permission is a state, not a zero ----------------

#[test]
fn an_unheld_permission_persists_as_an_explicit_unavailable_state() {
    // Given a session where the analytics permission is not held
    let store = Store::open_in_memory().expect("store");
    let (project, session) = project_and_session(&store);

    // When the metric is declared
    store
        .declare_metric(
            session,
            project,
            "concurrent_viewers",
            MetricAvailability::Unavailable {
                reason: UnavailableReason::PermissionNotHeld,
                detail: "youtube.analytics.readonly is not granted".into(),
            },
        )
        .expect("declare");

    // When it is read back
    let state = store
        .metric_state(session, "concurrent_viewers")
        .expect("read")
        .expect("a declared metric exists");

    // Then it is Unavailable with its reason, and not a zero
    assert_eq!(
        state.observed(),
        None,
        "an unheld permission yields no value"
    );
    assert_eq!(
        state.unavailable_reason(),
        Some(UnavailableReason::PermissionNotHeld),
        "the reason survives the round trip"
    );
    match state {
        MetricAvailability::Unavailable { ref detail, .. } => assert!(
            detail.contains("readonly"),
            "the reason's detail is carried, got: {detail}"
        ),
        other => panic!("expected Unavailable, got {other:?}"),
    }
}

#[test]
fn an_unavailable_state_is_not_a_measured_zero() {
    // Given the two states side by side
    let unavailable = MetricAvailability::Unavailable {
        reason: UnavailableReason::PermissionNotHeld,
        detail: "not granted".into(),
    };
    let observed_zero = MetricAvailability::Observed(0);

    // Then they are different states, and a caller can tell them apart
    assert_ne!(unavailable, observed_zero);
    assert_eq!(unavailable.observed(), None);
    assert_eq!(
        observed_zero.observed(),
        Some(0),
        "zero viewers is a measurement; the absence of one is not"
    );
    assert!(unavailable.is_unavailable());
    assert!(!observed_zero.is_unavailable());
}

#[test]
fn a_declared_metric_is_per_session_and_never_leaks_into_another() {
    // Given two sessions, only one of which has the permission
    let store = Store::open_in_memory().expect("store");
    let (project, granted) = project_and_session(&store);
    let refused = CaptureSessionId::generate();
    store
        .declare_metric(
            granted,
            project,
            "concurrent_viewers",
            MetricAvailability::Observed(12),
        )
        .expect("declare");

    // Then the other session has no state at all, which is different again
    assert_eq!(
        store
            .metric_state(refused, "concurrent_viewers")
            .expect("read"),
        None,
        "a session nobody declared a metric for has no state to confuse with zero"
    );
}

// --- claim 3: live status is never an outcome ----------------------------

#[test]
fn a_viewer_sample_is_never_written_into_the_observation_table() {
    // Given a session with a full sample series and a set of events
    let store = Store::open_in_memory().expect("store");
    let (project, session) = project_and_session(&store);
    let clock = clock_whole();
    store
        .append_viewer_samples(session, project, &full_capture(), &window(), &clock)
        .expect("append samples");
    store
        .append_stream_events(
            session,
            project,
            &[RawStreamEvent {
                wall_ms: T0 + 1_000,
                kind: EventKind::Marker,
                label: "welcome".into(),
                category: Some("chapter".into()),
                payload_json: "{}".into(),
            }],
            &window(),
            &clock,
        )
        .expect("append events");

    // Then the samples and events are somewhere and the observations are empty
    assert_eq!(
        store.row_count(CountedTable::ViewerSamples).expect("count"),
        6
    );
    assert_eq!(
        store.row_count(CountedTable::StreamEvents).expect("count"),
        1
    );
    assert_eq!(
        store
            .row_count(CountedTable::MetricObservations)
            .expect("count"),
        0,
        "a live-status badge value is not a measured outcome and must not \
         appear in the observation table"
    );
}

#[test]
fn the_database_itself_refuses_a_live_status_metric_in_the_observation_table() {
    // Given an open Creator database
    let store = Store::open_in_memory().expect("store");
    let (project_id, _session) = project_and_session(&store);
    let source = super::fixtures::source(project_id, None);
    store.insert_source_asset(&source).expect("source");

    // When a live-status field is written into the observation table
    // directly, going around every Rust writer in the crate
    let refusal = store
        .raw_execute(
            "INSERT INTO creator_metric_observations
             (id,project_id,provider,metric,definition_version,scope,unit,
              interval_start,interval_end,timezone,value,retrieved_at,
              source_query_json,transform_json,chart_json)
             VALUES ('x',?1,'youtube','concurrent_viewers','v1','live','viewers',
                     '2026-09-07T00:00:00Z','2026-09-07T01:00:00Z','UTC',7,
                     '2026-09-07T01:00:00Z','{}','{}','{}')",
            &[project_id.to_string()],
        )
        .expect_err("the schema, not a Rust `if`, is the enforcing mechanism");

    // Then the refusal names the reason
    let text = refusal.to_string();
    assert!(
        text.contains("live-status") || text.contains("concurrent_viewers"),
        "the refusal must name what was refused, got: {text}"
    );
}

#[test]
fn a_measured_outcome_is_still_allowed_in_the_observation_table() {
    // Given the same direct write with a real outcome metric
    let store = Store::open_in_memory().expect("store");
    let (project_id, _session) = project_and_session(&store);

    // When it is written
    store
        .raw_execute(
            "INSERT INTO creator_metric_observations
             (id,project_id,provider,metric,definition_version,scope,unit,
              interval_start,interval_end,timezone,value,retrieved_at,
              source_query_json,transform_json,chart_json)
             VALUES ('y',?1,'youtube','average_view_duration','v1','post-publish','seconds',
                     '2026-09-07T00:00:00Z','2026-09-07T01:00:00Z','UTC',412.5,
                     '2026-09-07T01:00:00Z','{}','{}','{}')",
            &[project_id.to_string()],
        )
        .expect("a measured outcome is the observation table's own subject");

    // Then it is there
    assert_eq!(
        store
            .row_count(CountedTable::MetricObservations)
            .expect("count"),
        1
    );
}

// --- durability, sequence and identity -----------------------------------

#[test]
fn a_resumed_capture_cannot_reuse_a_sequence_number() {
    // Given three samples, then a cancelled batch, then a resume
    let store = Store::open_in_memory().expect("store");
    let (project, session) = project_and_session(&store);
    let clock = clock_whole();
    let at = |wall_ms: i64| RawViewerSample {
        wall_ms,
        value: 1,
        poll_interval_ms: 1_000,
        dropped: false,
    };
    let first = store
        .append_viewer_samples(
            session,
            project,
            &[at(T0 + 1_000), at(T0 + 2_000)],
            &window(),
            &clock,
        )
        .expect("append");
    assert_eq!(first.iter().map(|s| s.seq).collect::<Vec<_>>(), vec![0, 1]);

    // When the capture is interrupted: a batch that cannot be written
    let interrupted = store.append_viewer_samples(
        session,
        project,
        &[at(T0 + 3_000), at(T0 + 999_999_999)],
        &window(),
        &clock,
    );
    assert!(
        interrupted.is_err(),
        "the second sample is out of the window"
    );
    assert_eq!(
        store.viewer_samples(session).expect("read").len(),
        2,
        "an interrupted batch writes nothing"
    );

    // Then the resume continues the sequence rather than restarting it
    let resumed = store
        .append_viewer_samples(session, project, &[at(T0 + 3_000)], &window(), &clock)
        .expect("resume");
    assert_eq!(
        resumed[0].seq, 2,
        "the store assigns the sequence, so a resume cannot collide with it"
    );
    let all: Vec<u32> = store
        .viewer_samples(session)
        .expect("read")
        .iter()
        .map(|s| s.seq)
        .collect();
    assert_eq!(all, vec![0, 1, 2], "every sequence number is used once");
}

#[test]
fn a_samples_identity_is_its_session_and_sequence_never_a_locator() {
    // Given a sample on a source whose locator is later rewritten
    let store = Store::open_in_memory().expect("store");
    let (project, session) = project_and_session(&store);
    let clock = clock_whole();
    let mut source = super::fixtures::source(project, Some("/old/path.mkv".into()));
    let source_id = source.id;
    store.insert_source_asset(&source).expect("source");
    store
        .append_viewer_samples(
            session,
            project,
            &[RawViewerSample {
                wall_ms: T0 + 1_000,
                value: 9,
                poll_interval_ms: 1_000,
                dropped: false,
            }],
            &window(),
            &clock,
        )
        .expect("append");

    // When the source moves
    source.locator = Some("/new/path.mkv".into());
    store
        .set_source_locator(source_id, source.locator.as_deref())
        .expect("relink");

    // Then the sample is still there, unchanged, and still readable
    let samples = store.viewer_samples(session).expect("read");
    assert_eq!(samples.len(), 1, "moving a file is not an event");
    assert_eq!(samples[0].value, 9);
    assert_eq!(samples[0].capture_session_id, session);
}

#[test]
fn a_dropped_poll_is_recorded_and_does_not_claim_to_cover_its_window() {
    // Given a series where the third poll reported itself dropped
    let store = Store::open_in_memory().expect("store");
    let (project, session) = project_and_session(&store);
    let clock = clock_whole();
    let mut batch = full_capture();
    batch[2].dropped = true;
    store
        .append_viewer_samples(session, project, &batch, &window(), &clock)
        .expect("append");

    // Then the sample is kept, with its value, but contributes no coverage
    let samples = store.viewer_samples(session).expect("read");
    assert_eq!(samples.len(), 6, "a dropped poll is data too");
    assert!(samples[2].dropped);
    let coverage = store
        .coverage(session, &window(), &clock)
        .expect("coverage");
    assert_eq!(
        coverage.observed_wall_ms, 50_000,
        "five 10 s polls, not six"
    );
    assert_eq!(coverage.gaps.len(), 1);
    assert_eq!(
        coverage.gaps[0].start_wall_ms,
        T0 + 20_000,
        "the dropped reading is the third, at T0+30s, and it is the 10 s \
         *before* it that goes uncovered"
    );
    assert_eq!(coverage.gaps[0].end_wall_ms, T0 + 30_000);
}

// --- untrusted text is data ---------------------------------------------

#[test]
fn a_hostile_event_label_is_stored_as_data_and_never_interpreted() {
    // Given a label carrying markup, a direction override and a NUL
    let store = Store::open_in_memory().expect("store");
    let (project, session) = project_and_session(&store);
    let clock = clock_whole();
    let hostile = "</script><img src=x onerror=alert(1)>\u{202e}gnp.exe\u{0}";
    store
        .append_stream_events(
            session,
            project,
            &[RawStreamEvent {
                wall_ms: T0 + 1_000,
                kind: EventKind::ProviderEvent,
                label: hostile.into(),
                category: Some("<b>bold</b>".into()),
                payload_json: "{\"raw\":\"<iframe>\"}".into(),
            }],
            &window(),
            &clock,
        )
        .expect("append");

    // Then it round-trips byte for byte, with no field reinterpreting it
    let events = store.stream_events(session).expect("read");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].label, hostile, "stored as data, byte for byte");
    assert_eq!(events[0].category.as_deref(), Some("<b>bold</b>"));
    assert_eq!(events[0].payload_json, "{\"raw\":\"<iframe>\"}");
}

// --- ordering ------------------------------------------------------------

#[test]
fn an_out_of_order_batch_is_a_typed_refusal_rather_than_a_silent_sort() {
    // Given a batch whose readings are reversed
    let store = Store::open_in_memory().expect("store");
    let (project, session) = project_and_session(&store);
    let clock = clock_whole();
    let mut batch = full_capture();
    batch.reverse();

    // When it is appended
    let refusal = store
        .append_viewer_samples(session, project, &batch, &window(), &clock)
        .expect_err("a poller submitting backwards is a bug in the poller");

    // Then it is the specific refusal, naming the offending position
    assert!(
        matches!(refusal, ObservationError::OutOfOrderBatch { at_seq: 1, .. }),
        "got: {refusal:?}"
    );
    assert_eq!(
        store.row_count(CountedTable::ViewerSamples).expect("count"),
        0,
        "a refused batch writes nothing, so it cannot be half-stored"
    );
}

#[test]
fn an_in_order_batch_reads_back_in_capture_order() {
    // Given a batch written in capture order
    let store = Store::open_in_memory().expect("store");
    let (project, session) = project_and_session(&store);
    let clock = clock_whole();
    store
        .append_viewer_samples(session, project, &full_capture(), &window(), &clock)
        .expect("append");

    // Then it reads back in that order, with the sequence matching the clock
    let samples = store.viewer_samples(session).expect("read");
    let seqs: Vec<u32> = samples.iter().map(|s| s.seq).collect();
    assert_eq!(
        seqs,
        (0..6).collect::<Vec<_>>(),
        "sequence is capture order"
    );
    assert!(
        samples.windows(2).all(|w| w[0].wall_ms < w[1].wall_ms),
        "and so is the wall clock"
    );
}
