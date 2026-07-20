//! Canonical append-only signal store (VISION AX-6).
//!
//! Every extractor plugin (chapters, cuepoints, clipper, deadair, vad, ...)
//! historically kept its own fragmented per-plugin SQLite database. This
//! module is the single canonical store all of them write into and every
//! analytic reads through: one `signals` table, one schema, one typed API.
//!
//! A [`Signal`] is an append-only fact about a span of a recording:
//! `(recording_id, t_start, t_end, kind, label, payload, confidence,
//! source_plugin)`. Rows are never mutated or deleted through this API —
//! extractors only ever append, and analytics only ever query.
//!
//! ```no_run
//! use strivo_core::signal_store::{NewSignal, SignalQuery, SignalStore};
//! use serde_json::json;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let store = SignalStore::open("signals.db")?;
//! store.write_signals(&[NewSignal {
//!     recording_id: "rec-1".into(),
//!     t_start: 12.0,
//!     t_end: 14.5,
//!     kind: "cuepoint".into(),
//!     label: "highlight".into(),
//!     payload: json!({"reason": "hype"}),
//!     confidence: 0.92,
//!     source_plugin: "cuepoints".into(),
//! }])?;
//!
//! let hits = store.query_signals(&SignalQuery::new().recording_id("rec-1"))?;
//! assert_eq!(hits.len(), 1);
//! # Ok(())
//! # }
//! ```

mod error;
mod model;
mod store;

pub use error::SignalStoreError;
pub use model::{NewSignal, Signal, SignalQuery};
pub use store::SignalStore;

#[cfg(test)]
mod tests;
