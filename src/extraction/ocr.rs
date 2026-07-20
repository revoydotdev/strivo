//! Visual/OCR extractor for on-screen text regions (scoreboards,
//! lower-thirds/chyrons, etc.).
//!
//! Per the [`crate::extraction`] contract, acquisition is out-of-band: this
//! extractor does not run any OCR engine itself. The caller injects
//! pre-computed [`OcrDetection`]s (produced elsewhere, e.g. by a
//! frame-sampling and OCR pipeline) into [`OcrExtractor::new`] alongside the
//! `source_plugin` name; [`OcrExtractor::extract`] is a pure mapping from
//! those detections to [`ExtractedSignal`]s.

use serde_json::json;

use super::{ExtractedSignal, ExtractionContext, Extractor};

/// A single recognized on-screen text region, supplied by an out-of-band OCR
/// pipeline.
#[derive(Debug, Clone, PartialEq)]
pub struct OcrDetection {
    /// The kind of on-screen region the text was recognized in, e.g.
    /// `"scoreboard"` or `"lower_third"`.
    pub region: String,
    /// The text recognized by the OCR engine.
    pub text: String,
    /// Span start, in seconds from the start of the recording.
    pub t_start: f64,
    /// Span end, in seconds from the start of the recording.
    pub t_end: f64,
    /// OCR engine confidence for this detection, expected in `[0.0, 1.0]`.
    pub confidence: f64,
}

/// Extractor that turns pre-computed OCR text detections into
/// [`ExtractedSignal`]s.
///
/// Constructed via dependency injection: the caller supplies both the
/// `source_plugin` identity and the `detections` this extractor will emit.
/// No vision/OCR engine is invoked here.
pub struct OcrExtractor {
    source_plugin: String,
    detections: Vec<OcrDetection>,
}

impl OcrExtractor {
    /// Build an extractor over a fixed set of already-recognized
    /// `detections`, identifying itself as `source_plugin` when run through
    /// [`super::run_extractor`].
    pub fn new(source_plugin: impl Into<String>, detections: Vec<OcrDetection>) -> Self {
        Self {
            source_plugin: source_plugin.into(),
            detections,
        }
    }
}

impl Extractor for OcrExtractor {
    fn source_plugin(&self) -> &str {
        &self.source_plugin
    }

    fn extract(&self, _ctx: &ExtractionContext) -> Vec<ExtractedSignal> {
        self.detections
            .iter()
            .map(|detection| ExtractedSignal {
                t_start: detection.t_start,
                t_end: detection.t_end,
                kind: format!("ocr:{}", detection.region),
                label: detection.text.clone(),
                payload: json!({
                    "region": detection.region,
                    "text": detection.text,
                }),
                confidence: detection.confidence,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{OcrDetection, OcrExtractor};
    use crate::extraction::{ExtractionContext, ExtractionError, run_extractor};
    use crate::signal_store::{SignalQuery, SignalStore};

    fn detection(region: &str, text: &str, confidence: f64) -> OcrDetection {
        OcrDetection {
            region: region.to_string(),
            text: text.to_string(),
            t_start: 12.0,
            t_end: 14.5,
            confidence,
        }
    }

    #[test]
    fn extractor_ocr_stamps_provenance_from_the_trait_not_the_payload() {
        let store = SignalStore::open_in_memory().expect("in-memory store should open");
        let ctx = ExtractionContext {
            recording_id: "rec-ocr-provenance".into(),
        };
        let extractor = OcrExtractor::new(
            "ocr-extractor-xyz",
            vec![
                detection("scoreboard", "HOME 3 - 2 AWAY", 0.88),
                detection("lower_third", "BREAKING: OT THRILLER", 0.77),
            ],
        );

        let ids = run_extractor(&extractor, &ctx, &store).expect("run_extractor should succeed");
        assert_eq!(ids.len(), 2, "one row id per detection");

        let rows = store
            .query_signals(&SignalQuery::new().recording_id("rec-ocr-provenance"))
            .expect("query should succeed");
        assert_eq!(rows.len(), 2);
        for row in &rows {
            assert_eq!(
                row.source_plugin, "ocr-extractor-xyz",
                "runner must stamp source_plugin from Extractor::source_plugin, \
                 not something the extractor's own output could control"
            );
        }
    }

    #[test]
    fn extractor_ocr_round_trips_span_label_payload_and_confidence() {
        let store = SignalStore::open_in_memory().expect("in-memory store should open");
        let ctx = ExtractionContext {
            recording_id: "rec-ocr-roundtrip".into(),
        };
        let extractor = OcrExtractor::new(
            "ocr-extractor-xyz",
            vec![detection("scoreboard", "HOME 3 - 2 AWAY", 0.91)],
        );

        run_extractor(&extractor, &ctx, &store).expect("run_extractor should succeed");

        let rows = store
            .query_signals(&SignalQuery::new().recording_id("rec-ocr-roundtrip"))
            .expect("query should succeed");
        assert_eq!(rows.len(), 1, "expected a nonzero, exact signal count");
        let row = &rows[0];

        assert_eq!(row.kind, "ocr:scoreboard");
        assert_eq!(row.label, "HOME 3 - 2 AWAY");
        assert_eq!(row.t_start, 12.0);
        assert_eq!(row.t_end, 14.5);
        assert_eq!(row.confidence, 0.91, "confidence must round-trip exactly");
        assert_eq!(
            row.payload,
            json!({
                "region": "scoreboard",
                "text": "HOME 3 - 2 AWAY",
            }),
            "payload must carry recognized text and region"
        );
    }

    #[test]
    fn extractor_ocr_rejects_out_of_range_confidence_and_writes_nothing() {
        let store = SignalStore::open_in_memory().expect("in-memory store should open");
        let ctx = ExtractionContext {
            recording_id: "rec-ocr-invalid".into(),
        };
        // A valid detection followed by an invalid one: the whole batch must
        // be rejected, not partially written.
        let extractor = OcrExtractor::new(
            "ocr-extractor-xyz",
            vec![
                detection("scoreboard", "HOME 3 - 2 AWAY", 0.5),
                detection("lower_third", "GARBLED TEXT", 1.4),
            ],
        );

        let err = run_extractor(&extractor, &ctx, &store)
            .expect_err("confidence 1.4 is out of range and must be rejected");
        match err {
            ExtractionError::InvalidConfidence { index, confidence } => {
                assert_eq!(index, 1);
                assert_eq!(confidence, 1.4);
            }
            other => panic!("expected InvalidConfidence, got {other:?}"),
        }

        let rows = store
            .query_signals(&SignalQuery::new().recording_id("rec-ocr-invalid"))
            .expect("query should succeed");
        assert!(
            rows.is_empty(),
            "a rejected batch must not persist any of its detections, valid or not"
        );
    }
}
