//! Timecoded entity/event extractor (the "sports spine").
//!
//! This is a generic, domain-agnostic transform from pre-computed,
//! injected detections — scoreboard events, plays, entity mentions with
//! timing, or anything else that can be described as "something happened
//! between `t_start` and `t_end`" — into [`ExtractedSignal`]s. Per the
//! module contract, acquisition of that detection data (running a vision
//! model, parsing a play-by-play feed, whatever) happens out-of-band; this
//! extractor is constructed with the detections already computed and does
//! no I/O of its own.

use serde_json::Value;

use super::{ExtractedSignal, ExtractionContext, Extractor};

/// A single timecoded entity/event detection, supplied to [`EventExtractor`]
/// by its caller.
#[derive(Debug, Clone, PartialEq)]
pub struct TimecodedEvent {
    /// Span start, in seconds from the start of the recording.
    pub t_start: f64,
    /// Span end, in seconds from the start of the recording.
    pub t_end: f64,
    /// Detection-specific event/entity kind, e.g. `"score"`, `"play"`,
    /// `"entity_mention"`. Carried through into the emitted signal's
    /// `kind` so the original detection category stays traceable.
    pub kind: String,
    /// Short human-readable label for the event, e.g. `"3-pointer"` or
    /// `"Jane Doe mentioned"`.
    pub label: String,
    /// Structured event detail: entities involved, score deltas,
    /// description, or any other detection-specific payload.
    pub payload: Value,
    /// Detection confidence in `[0.0, 1.0]`.
    pub confidence: f64,
}

/// Extracts timecoded entity/event signals from a fixed set of
/// pre-computed [`TimecodedEvent`] detections.
///
/// The detections are injected via the constructor rather than produced by
/// calling out to a detection pipeline here — acquisition is out-of-band,
/// per [`crate::extraction`]'s module contract. `source_plugin` is also
/// injected so a single binary can host several differently-named event
/// extractors (e.g. per detector model) without a new type per name.
pub struct EventExtractor {
    source_plugin: String,
    detections: Vec<TimecodedEvent>,
}

impl EventExtractor {
    /// Build an extractor that will report signals as produced by
    /// `source_plugin` and emit exactly `detections`, converted 1:1 on
    /// [`Extractor::extract`].
    pub fn new(source_plugin: impl Into<String>, detections: Vec<TimecodedEvent>) -> Self {
        Self {
            source_plugin: source_plugin.into(),
            detections,
        }
    }
}

/// Signal category emitted for every [`TimecodedEvent`], with the
/// detection's own `kind` embedded so it stays traceable (see
/// [`EventExtractor::extract`]).
const EVENT_KIND_PREFIX: &str = "event";

impl Extractor for EventExtractor {
    fn source_plugin(&self) -> &str {
        &self.source_plugin
    }

    fn extract(&self, _ctx: &ExtractionContext) -> Vec<ExtractedSignal> {
        self.detections
            .iter()
            .map(|d| ExtractedSignal {
                t_start: d.t_start,
                t_end: d.t_end,
                kind: format!("{EVENT_KIND_PREFIX}:{}", d.kind),
                label: d.label.clone(),
                payload: d.payload.clone(),
                confidence: d.confidence,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{EventExtractor, TimecodedEvent};
    use crate::extraction::{ExtractionContext, ExtractionError, run_extractor};
    use crate::signal_store::{SignalQuery, SignalStore};

    fn detection(kind: &str, label: &str, confidence: f64) -> TimecodedEvent {
        TimecodedEvent {
            t_start: 10.0,
            t_end: 12.5,
            kind: kind.to_string(),
            label: label.to_string(),
            payload: json!({"kind": kind, "entities": ["home", "away"]}),
            confidence,
        }
    }

    #[test]
    fn extractor_events_stamps_provenance_from_the_trait_not_the_payload() {
        let store = SignalStore::open_in_memory().expect("in-memory store should open");
        let ctx = ExtractionContext {
            recording_id: "rec-events-provenance".into(),
        };
        let extractor = EventExtractor::new(
            "event-extractor-xyz",
            vec![
                detection("score", "3-pointer", 0.7),
                detection("play", "fast break", 0.8),
            ],
        );

        let ids = run_extractor(&extractor, &ctx, &store).expect("run_extractor should succeed");
        assert_eq!(ids.len(), 2, "one row id per detection");
        assert!(!ids.is_empty(), "nonzero signals must have been written");

        let rows = store
            .query_signals(&SignalQuery::new().recording_id("rec-events-provenance"))
            .expect("query should succeed");
        assert_eq!(rows.len(), 2);
        for row in &rows {
            assert_eq!(
                row.source_plugin, "event-extractor-xyz",
                "runner must stamp source_plugin from Extractor::source_plugin, \
                 not something the detection payload could control"
            );
        }
    }

    #[test]
    fn extractor_events_round_trips_span_label_payload_and_confidence() {
        let store = SignalStore::open_in_memory().expect("in-memory store should open");
        let ctx = ExtractionContext {
            recording_id: "rec-events-roundtrip".into(),
        };
        let extractor = EventExtractor::new(
            "event-extractor-xyz",
            vec![detection("score", "3-pointer", 0.42)],
        );

        run_extractor(&extractor, &ctx, &store).expect("run_extractor should succeed");

        let rows = store
            .query_signals(&SignalQuery::new().recording_id("rec-events-roundtrip"))
            .expect("query should succeed");
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.confidence, 0.42, "confidence must round-trip exactly");
        assert_eq!(row.t_start, 10.0);
        assert_eq!(row.t_end, 12.5);
        assert_eq!(
            row.kind, "event:score",
            "emitted kind must embed the detection's own kind for traceability"
        );
        assert_eq!(row.label, "3-pointer");
        assert_eq!(
            row.payload,
            json!({"kind": "score", "entities": ["home", "away"]}),
            "structured event payload must round-trip unchanged"
        );
        assert_eq!(row.recording_id, "rec-events-roundtrip");
    }

    #[test]
    fn extractor_events_rejects_out_of_range_confidence_and_writes_nothing() {
        let store = SignalStore::open_in_memory().expect("in-memory store should open");
        let ctx = ExtractionContext {
            recording_id: "rec-events-invalid".into(),
        };
        // A valid detection followed by an invalid one: the whole batch
        // must be rejected, not partially written.
        let extractor = EventExtractor::new(
            "event-extractor-xyz",
            vec![
                detection("score", "3-pointer", 0.5),
                detection("play", "fast break", 1.5),
            ],
        );

        let err = run_extractor(&extractor, &ctx, &store)
            .expect_err("confidence 1.5 is out of range and must be rejected");
        match err {
            ExtractionError::InvalidConfidence { index, confidence } => {
                assert_eq!(index, 1);
                assert_eq!(confidence, 1.5);
            }
            other => panic!("expected InvalidConfidence, got {other:?}"),
        }

        let rows = store
            .query_signals(&SignalQuery::new().recording_id("rec-events-invalid"))
            .expect("query should succeed");
        assert!(
            rows.is_empty(),
            "a rejected batch must not persist any of its detections, valid or not"
        );
    }
}
