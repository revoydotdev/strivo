use serde_json::json;

use super::{ExtractedSignal, ExtractionContext, ExtractionError, Extractor, run_extractor};
use crate::signal_store::{SignalQuery, SignalStore};

/// Sample extractor with a fixed, deliberately unusual `source_plugin` name
/// so the provenance assertion can't be satisfied by accident (e.g. by a
/// default or an id that happens to match a payload field).
struct SampleExtractor {
    source_plugin: &'static str,
    signals: Vec<ExtractedSignal>,
}

impl Extractor for SampleExtractor {
    fn source_plugin(&self) -> &str {
        self.source_plugin
    }

    fn extract(&self, _ctx: &ExtractionContext) -> Vec<ExtractedSignal> {
        self.signals.clone()
    }
}

fn signal(kind: &str, confidence: f64) -> ExtractedSignal {
    ExtractedSignal {
        t_start: 1.0,
        t_end: 2.5,
        kind: kind.to_string(),
        label: format!("{kind} label"),
        payload: json!({"kind": kind}),
        confidence,
    }
}

#[test]
fn extractor_contract_stamps_provenance_from_the_trait_not_the_payload() {
    let store = SignalStore::open_in_memory().expect("in-memory store should open");
    let ctx = ExtractionContext {
        recording_id: "rec-provenance".into(),
    };
    let extractor = SampleExtractor {
        source_plugin: "sample-extractor-xyz",
        signals: vec![signal("chapter", 0.6), signal("cuepoint", 0.9)],
    };

    let ids = run_extractor(&extractor, &ctx, &store).expect("run_extractor should succeed");
    assert_eq!(ids.len(), 2, "one row id per extracted signal");

    let rows = store
        .query_signals(&SignalQuery::new().recording_id("rec-provenance"))
        .expect("query should succeed");
    assert_eq!(rows.len(), 2);
    for row in &rows {
        assert_eq!(
            row.source_plugin, "sample-extractor-xyz",
            "runner must stamp source_plugin from Extractor::source_plugin, \
             not something the extractor's own output could control"
        );
    }
}

#[test]
fn extractor_contract_round_trips_confidence_and_span() {
    let store = SignalStore::open_in_memory().expect("in-memory store should open");
    let ctx = ExtractionContext {
        recording_id: "rec-roundtrip".into(),
    };
    let extractor = SampleExtractor {
        source_plugin: "sample-extractor-xyz",
        signals: vec![signal("chapter", 0.42)],
    };

    run_extractor(&extractor, &ctx, &store).expect("run_extractor should succeed");

    let rows = store
        .query_signals(&SignalQuery::new().recording_id("rec-roundtrip"))
        .expect("query should succeed");
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.confidence, 0.42, "confidence must round-trip exactly");
    assert_eq!(row.t_start, 1.0);
    assert_eq!(row.t_end, 2.5);
    assert_eq!(row.kind, "chapter");
    assert_eq!(row.recording_id, "rec-roundtrip");
}

#[test]
fn extractor_contract_rejects_out_of_range_confidence_and_writes_nothing() {
    let store = SignalStore::open_in_memory().expect("in-memory store should open");
    let ctx = ExtractionContext {
        recording_id: "rec-invalid".into(),
    };
    // A valid signal followed by an invalid one: the whole batch must be
    // rejected, not partially written.
    let extractor = SampleExtractor {
        source_plugin: "sample-extractor-xyz",
        signals: vec![signal("chapter", 0.5), signal("cuepoint", 1.5)],
    };

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
        .query_signals(&SignalQuery::new().recording_id("rec-invalid"))
        .expect("query should succeed");
    assert!(
        rows.is_empty(),
        "a rejected batch must not persist any of its signals, valid or not"
    );
}
