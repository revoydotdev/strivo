//! Everything this model refuses, and why each refusal is a state rather
//! than a repaired value.
//!
//! Split out from the model because it is a different responsibility: this
//! module answers *what cannot be represented*, the model answers *what a
//! sample is*. Every variant here is a case where the alternative — a
//! default, a clamp, a cast — would report a value nobody measured.
//!
//! [`ObservationError::OutsideClockMap`] is the load-bearing one: todo 22's
//! own refusal, carried through this seam unchanged rather than softened
//! into the nearest covered edge.

use std::fmt;

use strivo_editor::time::TimeError;

/// Everything this model refuses.
///
/// `Clone` and `Eq` are deliberately absent: `rusqlite::Error` is neither,
/// and no caller here needs them. Flattening the database arm into a string
/// to buy them would be exactly the message-matching this type exists to
/// avoid.
#[derive(Debug, PartialEq)]
pub enum ObservationError {
    InvertedWindow {
        start_wall_ms: i64,
        end_wall_ms: i64,
    },
    /// The instant predates the expected capture window.
    BeforeSession {
        wall_ms: i64,
    },
    /// The instant is past the end of the expected capture window.
    AfterSession {
        wall_ms: i64,
        end_wall_ms: i64,
    },
    /// No clock segment covers this instant. The refusal is the point: this
    /// is todo 22's `OutsideClockMap` and it is not softened into the
    /// nearest edge.
    OutsideClockMap {
        wall_ms: i64,
    },
    NegativeValue {
        viewers: i64,
    },
    BadPollInterval {
        millis: i64,
    },
    LabelTooLong {
        bytes: usize,
        limit: usize,
    },
    MetricNameTooLong {
        bytes: usize,
        limit: usize,
    },
    EmptyMetricName,
    BatchTooLarge {
        len: usize,
        limit: usize,
    },
    /// A batch whose samples are not in capture order. Refused rather than
    /// sorted: a poller submitting its readings backwards is a bug in the
    /// poller, and quietly reordering it would hide that behind a series
    /// that looks right.
    OutOfOrderBatch {
        at_seq: u32,
        previous_wall_ms: i64,
        wall_ms: i64,
    },
    /// A row the model cannot make sense of — a NULL wall clock, or a
    /// negative count written by a hand-edited or corrupt database.
    UnreadableRow {
        table: &'static str,
        detail: String,
    },
    CoverageOutOfRange {
        per_mille: u32,
    },
    UnexplainedShortfall {
        per_mille: u32,
    },
    /// The database itself refused the write. Carried rather than flattened so
    /// a store caller's refusal is still a typed state rather than a message
    /// to pattern-match.
    Database(rusqlite::Error),
    Clock(String),
}

impl fmt::Display for ObservationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvertedWindow {
                start_wall_ms,
                end_wall_ms,
            } => {
                write!(f, "capture window ends at {end_wall_ms} ms, at or before its start {start_wall_ms} ms")
            }
            Self::BeforeSession { wall_ms } => {
                write!(f, "observation at {wall_ms} ms predates the capture window")
            }
            Self::AfterSession {
                wall_ms,
                end_wall_ms,
            } => write!(
                f,
                "observation at {wall_ms} ms is past the capture window's end {end_wall_ms} ms"
            ),
            Self::OutsideClockMap { wall_ms } => write!(
                f,
                "no clock segment covers {wall_ms} ms, so it has no media coordinate"
            ),
            Self::NegativeValue { viewers } => {
                write!(f, "viewer count {viewers} is negative")
            }
            Self::BadPollInterval { millis } => write!(
                f,
                "poll interval {millis} ms is not positive, so the reading describes no window"
            ),
            Self::LabelTooLong { bytes, limit } => {
                write!(f, "label is {bytes} bytes, over the {limit}-byte limit")
            }
            Self::MetricNameTooLong { bytes, limit } => {
                write!(
                    f,
                    "metric name is {bytes} bytes, over the {limit}-byte limit"
                )
            }
            Self::EmptyMetricName => f.write_str("metric name is empty"),
            Self::BatchTooLarge { len, limit } => {
                write!(f, "batch of {len} observations is over the {limit} limit")
            }
            Self::OutOfOrderBatch {
                at_seq,
                previous_wall_ms,
                wall_ms,
            } => write!(
                f,
                "observation {at_seq} is at {wall_ms} ms, before the {previous_wall_ms} ms \
                 that preceded it in the same batch"
            ),
            Self::UnreadableRow { table, detail } => {
                write!(f, "{table} row cannot be read as an observation: {detail}")
            }
            Self::CoverageOutOfRange { per_mille } => write!(
                f,
                "coverage of {per_mille} per mille is above 1000 and is not a fraction"
            ),
            Self::UnexplainedShortfall { per_mille } => write!(
                f,
                "coverage of {per_mille} per mille is below full and names no gap, \
                 so it cannot say what it is short of"
            ),
            Self::Database(e) => write!(f, "database: {e}"),
            Self::Clock(detail) => write!(f, "clock: {detail}"),
        }
    }
}

impl std::error::Error for ObservationError {}

impl From<rusqlite::Error> for ObservationError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Database(e)
    }
}

impl From<TimeError> for ObservationError {
    fn from(e: TimeError) -> Self {
        match e {
            TimeError::OutsideClockMap { wall_ms } => Self::OutsideClockMap { wall_ms },
            other => Self::Clock(other.to_string()),
        }
    }
}

impl From<ObservationError> for crate::ResearchError {
    fn from(e: ObservationError) -> Self {
        Self::Validation(e.to_string())
    }
}
