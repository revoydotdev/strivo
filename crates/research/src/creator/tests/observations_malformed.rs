//! What the observation model refuses.
//!
//! Each state below is a *typed* refusal, not a coerced value. They are the
//! malformed-input class for this todo: a value that arrived from a provider
//! or a hand-edited row and cannot describe anything real.
//!
//! The shape that matters most is the first group. A sample whose wall clock
//! is outside the capture is not stored at the nearest edge, and it is not
//! stored with a defaulted coordinate — [`ObservationError::OutsideClockMap`]
//! is the answer, which is todo 22's refusal arriving unchanged through this
//! seam.

use strivo_editor::time::{ClockMap, ClockSegment, MediaTime};

use super::super::{
    CaptureWindow, CountedTable, EventKind, MetricAvailability, ObservationError, RawStreamEvent,
    RawViewerSample, Store, StreamEvent, UnavailableReason, ViewerSample, MAX_BATCH,
    MAX_LABEL_BYTES, MAX_METRIC_BYTES,
};

const T0: i64 = 1_789_000_000_000;

fn window() -> CaptureWindow {
    CaptureWindow::new(T0, T0 + 60_000).expect("a forward window")
}

fn clock() -> ClockMap {
    let mut clock = ClockMap::new();
    clock
        .push(ClockSegment::exact(
            T0,
            MediaTime::ZERO,
            MediaTime::from_millis(60_000).expect("60 s"),
        ))
        .expect("one segment");
    clock
}

fn raw(wall_ms: i64, value: i64) -> RawViewerSample {
    RawViewerSample {
        wall_ms,
        value,
        poll_interval_ms: 1_000,
        dropped: false,
    }
}

#[test]
fn an_inverted_capture_window_is_refused() {
    // When a window ends before it starts
    let refusal = CaptureWindow::new(T0 + 1, T0);

    // Then it is refused rather than read as a negative span
    assert!(
        matches!(refusal, Err(ObservationError::InvertedWindow { .. })),
        "got: {refusal:?}"
    );
}

#[test]
fn a_negative_viewer_count_is_a_typed_state_not_a_wrapped_u32() {
    // Given a provider reporting -1 viewers
    let refusal = ViewerSample::observe(&window(), &clock(), raw(T0 + 1_000, -1));

    // Then it is NegativeValue, carrying the number that arrived
    assert!(
        matches!(refusal, Err(ObservationError::NegativeValue { viewers }) if viewers == -1),
        "an `as u32` cast here would report 4 294 967 295 viewers, got: {refusal:?}"
    );
}

#[test]
fn a_zero_viewer_count_is_a_legitimate_measurement() {
    // Given a provider reporting 0 viewers
    let sample = ViewerSample::observe(&window(), &clock(), raw(T0 + 1_000, 0));

    // Then it is a sample, and zero
    assert_eq!(sample.expect("a real measurement").value, 0);
}

#[test]
fn a_non_positive_poll_interval_is_refused() {
    // Given a poll claiming an interval of zero
    let mut zero = raw(T0 + 1_000, 1);
    zero.poll_interval_ms = 0;
    let refusal = ViewerSample::observe(&window(), &clock(), zero.clone());

    // Then the interval is refused
    assert!(
        matches!(
            refusal,
            Err(ObservationError::BadPollInterval { millis: 0 })
        ),
        "a zero-length poll would claim an exact instant it never measured, got: {refusal:?}"
    );

    // And so is a negative one
    zero.poll_interval_ms = -5;
    assert!(matches!(
        ViewerSample::observe(&window(), &clock(), zero),
        Err(ObservationError::BadPollInterval { millis: -5 })
    ));
}

#[test]
fn a_sample_after_the_recording_ended_is_refused() {
    // Given a sample a millisecond after the window closes
    let refusal = ViewerSample::observe(&window(), &clock(), raw(T0 + 60_001, 1));

    // Then the refusal is the specific one
    assert!(
        matches!(refusal, Err(ObservationError::AfterSession { wall_ms, .. }) if wall_ms == T0 + 60_001),
        "got: {refusal:?}"
    );
}

#[test]
fn a_wall_clock_no_segment_covers_is_refused_even_inside_the_window() {
    // Given a window of 60 s and a clock covering only its first 10 s
    let mut short = ClockMap::new();
    short
        .push(ClockSegment::exact(
            T0,
            MediaTime::ZERO,
            MediaTime::from_millis(10_000).expect("10 s"),
        ))
        .expect("one segment");
    let raw = raw(T0 + 30_000, 1);

    // Then an instant inside the *window* but outside the *clock* is refused
    let refusal = ViewerSample::observe(&window(), &short, raw);
    assert!(
        matches!(refusal, Err(ObservationError::OutsideClockMap { wall_ms }) if wall_ms == T0 + 30_000),
        "inside the expected window is not inside the capture, got: {refusal:?}"
    );
}

#[test]
fn a_label_over_the_limit_is_refused_rather_than_truncated() {
    // Given a label one byte over the limit
    let oversized = "x".repeat(MAX_LABEL_BYTES + 1);
    let raw = RawStreamEvent {
        wall_ms: T0 + 1_000,
        kind: EventKind::Marker,
        label: oversized.clone(),
        category: None,
        payload_json: "{}".into(),
    };

    // Then it is refused, naming the limit
    let refusal = StreamEvent::observe(&window(), &clock(), raw).expect_err("over the limit");
    assert!(
        matches!(
            refusal,
            ObservationError::LabelTooLong { limit, .. } if limit == MAX_LABEL_BYTES
        ),
        "a truncated label is silent data loss and reads as the whole label, got: {refusal:?}"
    );
}

#[test]
fn a_label_exactly_at_the_limit_is_kept_in_full() {
    // Given a label exactly at the limit
    let exact = "x".repeat(MAX_LABEL_BYTES);
    let raw = RawStreamEvent {
        wall_ms: T0 + 1_000,
        kind: EventKind::Marker,
        label: exact.clone(),
        category: None,
        payload_json: "{}".into(),
    };

    // Then it is accepted whole
    let event = StreamEvent::observe(&window(), &clock(), raw).expect("at the limit");
    assert_eq!(event.label.len(), MAX_LABEL_BYTES);
    assert_eq!(event.label, exact);
}

#[test]
fn an_oversized_category_is_refused_on_the_same_terms_as_the_label() {
    // Given a category over the limit
    let raw = RawStreamEvent {
        wall_ms: T0 + 1_000,
        kind: EventKind::Category,
        label: "fine".into(),
        category: Some("c".repeat(MAX_LABEL_BYTES + 1)),
        payload_json: "{}".into(),
    };

    // Then it is refused
    assert!(matches!(
        StreamEvent::observe(&window(), &clock(), raw),
        Err(ObservationError::LabelTooLong { .. })
    ));
}

#[test]
fn an_event_before_the_recording_is_refused_like_a_sample() {
    // Given an event predating the window
    let raw = RawStreamEvent {
        wall_ms: T0 - 1,
        kind: EventKind::Marker,
        label: "early".into(),
        category: None,
        payload_json: "{}".into(),
    };

    // Then it is refused
    assert!(matches!(
        StreamEvent::observe(&window(), &clock(), raw),
        Err(ObservationError::BeforeSession { .. })
    ));
}

#[test]
fn a_batch_over_the_bound_is_refused_before_anything_is_derived() {
    // Given a batch far past the append bound
    let store = Store::open_in_memory().expect("store");
    let (project_id, session) = super::observations::with_session(&store, "stream");
    let batch: Vec<RawViewerSample> = (0..MAX_BATCH + 1)
        .map(|i| {
            raw(
                T0 + 1_000 + i64::from(i32::try_from(i % 1_000).expect("in range")),
                1,
            )
        })
        .collect();

    // Then the append is refused, and nothing is written
    let refusal = store
        .append_viewer_samples(session, project_id, &batch, &window(), &clock())
        .expect_err("past the bound");
    assert!(
        matches!(refusal, ObservationError::BatchTooLarge { limit, .. } if limit == MAX_BATCH),
        "got: {refusal:?}"
    );
    assert_eq!(
        store.row_count(CountedTable::ViewerSamples).expect("count"),
        0
    );
}
#[test]
fn the_schema_refuses_a_truncated_sample_row_rather_than_leaving_one_to_be_read() {
    // Given an open Creator database
    let store = Store::open_in_memory().expect("store");
    let (project_id, session) = super::observations::with_session(&store, "stream");

    // When a row with no wall clock is forced in, going around every writer
    let refusal = store.raw_execute(
        "INSERT INTO creator_viewer_samples
         (id,capture_session_id,project_id,seq,wall_ms,media_us,uncertainty_us,
          viewer_count,poll_interval_ms,dropped)
         VALUES ('trunc',?1,?2,0,NULL,0,0,1,1000,0)",
        &[session.to_string(), project_id.to_string()],
    );

    // Then the schema refuses it, so a reader never faces one to repair
    assert!(
        refusal.is_err(),
        "a NULL wall clock read as 0 would place a truncated sample on the \
         first frame; NOT NULL means the row cannot exist at all"
    );
    assert_eq!(
        store.row_count(CountedTable::ViewerSamples).expect("count"),
        0,
        "nothing partial was left behind"
    );
}

#[test]
fn a_row_the_schema_permits_but_the_reader_cannot_represent_is_still_refused() {
    // Given a count inside the column's CHECK but past what a viewer count
    // can hold: 2^32, which `viewer_count >= 0` allows
    let store = Store::open_in_memory().expect("store");
    let (project_id, session) = super::observations::with_session(&store, "stream");
    store
        .raw_execute(
            "INSERT INTO creator_viewer_samples
             (id,capture_session_id,project_id,seq,wall_ms,media_us,uncertainty_us,
              viewer_count,poll_interval_ms,dropped)
             VALUES ('big',?1,?2,0,?3,0,0,?4,1000,0)",
            &[
                session.to_string(),
                project_id.to_string(),
                (T0 + 1_000).to_string(),
                "4294967296".to_string(),
            ],
        )
        .expect("the column permits it");
    assert_eq!(
        store.row_count(CountedTable::ViewerSamples).expect("count"),
        1,
        "the row is physically there"
    );

    // Then reading it is refused rather than truncated into a smaller count
    assert!(
        store.viewer_samples(session).is_err(),
        "a value no viewer count can hold must not come back as a different one"
    );
}

#[test]
fn the_schema_refuses_a_negative_viewer_count_outright() {
    // Given a count of -5
    let store = Store::open_in_memory().expect("store");
    let (project_id, session) = super::observations::with_session(&store, "stream");

    // When the row is forced in
    let refusal = store.raw_execute(
        "INSERT INTO creator_viewer_samples
         (id,capture_session_id,project_id,seq,wall_ms,media_us,uncertainty_us,
          viewer_count,poll_interval_ms,dropped)
         VALUES ('neg',?1,?2,0,?3,0,0,-5,1000,0)",
        &[
            session.to_string(),
            project_id.to_string(),
            (T0 + 1_000).to_string(),
        ],
    );

    // Then the CHECK refuses it, so -5 can never become 4 294 967 291
    assert!(
        refusal.is_err(),
        "a hand-edited row must not become four billion viewers on read"
    );
}

#[test]
fn a_metric_declared_for_an_unknown_metric_name_is_kept_but_bounded() {
    // Given a metric name past the limit
    let store = Store::open_in_memory().expect("store");
    let (project_id, session) = super::observations::with_session(&store, "stream");
    let huge = "m".repeat(MAX_METRIC_BYTES + 1);

    // Then it is refused
    let refusal = store
        .declare_metric(session, project_id, &huge, MetricAvailability::Observed(1))
        .expect_err("past the limit");
    assert!(
        matches!(refusal, ObservationError::MetricNameTooLong { limit, .. } if limit == MAX_METRIC_BYTES),
        "got: {refusal:?}"
    );
}

#[test]
fn every_unavailable_reason_round_trips_through_the_store() {
    // Given each reason the model can name
    let reasons = [
        UnavailableReason::PermissionNotHeld,
        UnavailableReason::NotConfigured,
        UnavailableReason::ProviderUnsupported,
        UnavailableReason::RateLimited {
            retry_after_ms: 30_000,
        },
    ];
    let store = Store::open_in_memory().expect("store");
    let (project_id, session) = super::observations::with_session(&store, "stream");

    for (i, reason) in reasons.iter().enumerate() {
        let name = format!("metric_{i}");
        store
            .declare_metric(
                session,
                project_id,
                &name,
                MetricAvailability::Unavailable {
                    reason: *reason,
                    detail: "because".into(),
                },
            )
            .expect("declare");
        let read = store
            .metric_state(session, &name)
            .expect("read")
            .expect("present");
        assert_eq!(
            read.unavailable_reason(),
            Some(*reason),
            "reason {i} did not survive the store"
        );
    }
}
