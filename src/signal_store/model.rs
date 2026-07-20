//! Typed rows and query filters for the signal store.

use serde_json::Value;

/// A signal appended by an extractor plugin. Not yet persisted — construct
/// this and pass it to [`crate::signal_store::SignalStore::write_signals`].
#[derive(Debug, Clone, PartialEq)]
pub struct NewSignal {
    /// Stable identifier of the recording this signal belongs to.
    pub recording_id: String,
    /// Span start, in seconds from the start of the recording.
    pub t_start: f64,
    /// Span end, in seconds from the start of the recording.
    pub t_end: f64,
    /// Extractor-defined category, e.g. `"cuepoint"`, `"chapter"`, `"vad"`.
    pub kind: String,
    /// Short human-readable label for the signal.
    pub label: String,
    /// Extractor-specific structured detail, stored as JSON.
    pub payload: Value,
    /// Extractor confidence in `[0.0, 1.0]`.
    pub confidence: f64,
    /// Name of the plugin that produced this signal (provenance).
    pub source_plugin: String,
}

/// A signal as persisted and returned by the store.
#[derive(Debug, Clone, PartialEq)]
pub struct Signal {
    /// Autoincrement row id assigned on insert.
    pub id: i64,
    pub recording_id: String,
    pub t_start: f64,
    pub t_end: f64,
    pub kind: String,
    pub label: String,
    pub payload: Value,
    pub confidence: f64,
    pub source_plugin: String,
    /// Insertion timestamp assigned by the store (`signals.created_at`),
    /// as the raw SQLite timestamp string.
    pub created_at: String,
}

/// Filter for [`crate::signal_store::SignalStore::query_signals`].
///
/// All set fields are AND-ed together; an unset field imposes no
/// restriction. `time_range` matches on overlap, not containment: a signal
/// spanning `[t_start, t_end]` matches a query range `[from, to]` whenever
/// `t_start <= to && t_end >= from`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SignalQuery {
    pub recording_id: Option<String>,
    pub kind: Option<String>,
    pub time_range: Option<(f64, f64)>,
}

impl SignalQuery {
    /// An unfiltered query — matches every signal in the store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Restrict to signals belonging to `recording_id`.
    pub fn recording_id(mut self, recording_id: impl Into<String>) -> Self {
        self.recording_id = Some(recording_id.into());
        self
    }

    /// Restrict to signals of the given `kind`.
    pub fn kind(mut self, kind: impl Into<String>) -> Self {
        self.kind = Some(kind.into());
        self
    }

    /// Restrict to signals whose `[t_start, t_end]` span overlaps
    /// `[from, to]`.
    pub fn time_range(mut self, from: f64, to: f64) -> Self {
        self.time_range = Some((from, to));
        self
    }
}
