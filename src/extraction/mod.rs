//! Common `Extractor` contract for writing into the canonical signal store
//! (VISION AX-6).
//!
//! Every domain-agnostic extraction flow (chapters, cuepoints, and future
//! extractors such as events or OCR) is expected to implement [`Extractor`]
//! and run through [`run_extractor`] rather than talking to
//! [`crate::signal_store::SignalStore`] directly. That gives every signal in
//! the store two load-bearing guarantees, enforced in one place:
//!
//! - **Provenance**: an extractor cannot lie about who produced a signal.
//!   [`ExtractedSignal`] — the value an [`Extractor`] returns — deliberately
//!   has no `source_plugin` field; [`run_extractor`] stamps it from
//!   [`Extractor::source_plugin`], so the identity on every row is the
//!   runner's, not the extractor's own claim.
//! - **Confidence**: every extracted signal's `confidence` is validated to
//!   lie in `[0.0, 1.0]` before anything is written. An out-of-range value
//!   is rejected with a typed [`ExtractionError`] rather than silently
//!   clamped — the same explicit-rejection contract
//!   [`crate::signal_store::SignalStore::write_signals`] already applies at
//!   the storage layer.
//!
//! ```
//! use strivo_core::extraction::{ExtractedSignal, Extractor, ExtractionContext, run_extractor};
//! use strivo_core::signal_store::SignalStore;
//! use serde_json::json;
//!
//! struct DemoExtractor;
//!
//! impl Extractor for DemoExtractor {
//!     fn source_plugin(&self) -> &str {
//!         "demo"
//!     }
//!
//!     fn extract(&self, _ctx: &ExtractionContext) -> Vec<ExtractedSignal> {
//!         vec![ExtractedSignal {
//!             t_start: 0.0,
//!             t_end: 1.0,
//!             kind: "demo".into(),
//!             label: "hello".into(),
//!             payload: json!({}),
//!             confidence: 0.5,
//!         }]
//!     }
//! }
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let store = SignalStore::open_in_memory()?;
//! let ctx = ExtractionContext { recording_id: "rec-1".into() };
//! let ids = run_extractor(&DemoExtractor, &ctx, &store)?;
//! assert_eq!(ids.len(), 1);
//! # Ok(())
//! # }
//! ```

use serde_json::Value;

use crate::signal_store::{NewSignal, SignalStore, SignalStoreError};

pub mod events;

/// Minimal input an [`Extractor`] needs to run.
///
/// Extractors are domain-agnostic: this carries only the identity of the
/// recording being processed. A concrete extractor (events, OCR, ...) reads
/// whatever media/transcript/frame data it needs out-of-band using
/// `recording_id`; the contract here is deliberately silent on how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractionContext {
    /// Stable identifier of the recording being processed.
    pub recording_id: String,
}

/// A signal produced by an [`Extractor`], not yet stamped with provenance.
///
/// Note there is no `source_plugin` field here by design: an extractor
/// cannot assert its own identity. [`run_extractor`] stamps
/// `source_plugin` from [`Extractor::source_plugin`] when converting this
/// into a [`NewSignal`], making a misattributed signal unrepresentable.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtractedSignal {
    /// Span start, in seconds from the start of the recording.
    pub t_start: f64,
    /// Span end, in seconds from the start of the recording.
    pub t_end: f64,
    /// Extractor-defined category, e.g. `"cuepoint"`, `"chapter"`, `"event"`.
    pub kind: String,
    /// Short human-readable label for the signal.
    pub label: String,
    /// Extractor-specific structured detail, stored as JSON.
    pub payload: Value,
    /// Extractor confidence. Must lie in `[0.0, 1.0]` — [`run_extractor`]
    /// rejects the whole batch with [`ExtractionError::InvalidConfidence`]
    /// otherwise.
    pub confidence: f64,
}

/// Errors returned by [`run_extractor`].
#[derive(Debug)]
pub enum ExtractionError {
    /// One of the extractor's signals had a `confidence` outside the
    /// inclusive `[0.0, 1.0]` range (or NaN). Rejected explicitly, before
    /// any signal in the batch is written, rather than clamped — an
    /// out-of-range confidence indicates a bug in the extractor, not a
    /// value worth silently coercing.
    InvalidConfidence {
        /// Index into the `Vec<ExtractedSignal>` returned by `extract`.
        index: usize,
        confidence: f64,
    },
    /// The underlying signal store rejected or failed the write.
    Store(SignalStoreError),
}

impl std::fmt::Display for ExtractionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidConfidence { index, confidence } => write!(
                f,
                "extracted signal at index {index} has confidence {confidence} \
                 out of range — must be in [0.0, 1.0]"
            ),
            Self::Store(e) => write!(f, "signal store error: {e}"),
        }
    }
}

impl std::error::Error for ExtractionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidConfidence { .. } => None,
            Self::Store(e) => Some(e),
        }
    }
}

impl From<SignalStoreError> for ExtractionError {
    fn from(e: SignalStoreError) -> Self {
        Self::Store(e)
    }
}

/// The common contract every domain-agnostic extractor implements to write
/// into the canonical signal store.
///
/// Implementors produce [`ExtractedSignal`]s from an [`ExtractionContext`];
/// they never construct a [`NewSignal`] or talk to
/// [`crate::signal_store::SignalStore`] directly. Running an extractor via
/// [`run_extractor`] is what stamps provenance and persists the result.
pub trait Extractor {
    /// Stable name identifying this extractor, e.g. `"chapters"` or
    /// `"events"`. Stamped as `source_plugin` on every signal this
    /// extractor produces — provenance the extractor itself cannot alter.
    fn source_plugin(&self) -> &str;

    /// Produce signals for `ctx`. Pure with respect to the signal store:
    /// no writes happen here, only in [`run_extractor`].
    fn extract(&self, ctx: &ExtractionContext) -> Vec<ExtractedSignal>;
}

/// Run `extractor` against `ctx`, stamp provenance, and persist the result
/// into `store`.
///
/// Every [`ExtractedSignal`] returned by `extractor.extract(ctx)` is
/// validated (`confidence` in `[0.0, 1.0]`) and converted into a
/// [`NewSignal`] with `recording_id` taken from `ctx` and `source_plugin`
/// taken from `extractor.source_plugin()` — never from the extractor's own
/// output. On success, returns the assigned row ids in extraction order.
///
/// Validation happens before any row is written: an out-of-range confidence
/// anywhere in the batch fails the whole call, matching
/// [`crate::signal_store::SignalStore::write_signals`]'s all-or-nothing
/// behavior at the storage layer.
pub fn run_extractor<E: Extractor + ?Sized>(
    extractor: &E,
    ctx: &ExtractionContext,
    store: &SignalStore,
) -> Result<Vec<i64>, ExtractionError> {
    let extracted = extractor.extract(ctx);
    let source_plugin = extractor.source_plugin();

    let mut new_signals = Vec::with_capacity(extracted.len());
    for (index, signal) in extracted.into_iter().enumerate() {
        if !(0.0..=1.0).contains(&signal.confidence) {
            return Err(ExtractionError::InvalidConfidence {
                index,
                confidence: signal.confidence,
            });
        }
        new_signals.push(NewSignal {
            recording_id: ctx.recording_id.clone(),
            t_start: signal.t_start,
            t_end: signal.t_end,
            kind: signal.kind,
            label: signal.label,
            payload: signal.payload,
            confidence: signal.confidence,
            source_plugin: source_plugin.to_string(),
        });
    }

    let ids = store.write_signals(&new_signals)?;
    Ok(ids)
}

pub mod ocr;

#[cfg(test)]
mod tests;
