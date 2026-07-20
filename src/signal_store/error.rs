//! Typed error surface for the signal store.

/// Errors returned by [`crate::signal_store::SignalStore`].
///
/// Invalid provenance (out-of-range confidence, empty `source_plugin`) is
/// rejected explicitly rather than silently clamped or defaulted — callers
/// must fix the extractor rather than trust a mangled row.
#[derive(Debug)]
pub enum SignalStoreError {
    /// `confidence` was outside the inclusive `[0.0, 1.0]` range (or NaN).
    InvalidConfidence(f64),
    /// `source_plugin` was empty or whitespace-only.
    EmptySourcePlugin,
    /// Underlying sqlite failure (schema, statement, or constraint).
    Sqlite(rusqlite::Error),
    /// A signal payload failed to (de)serialize as JSON.
    Json(serde_json::Error),
    /// Filesystem failure while preparing the database path.
    Io(std::io::Error),
}

impl std::fmt::Display for SignalStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidConfidence(c) => {
                write!(f, "confidence {c} is out of range — must be in [0.0, 1.0]")
            }
            Self::EmptySourcePlugin => write!(f, "source_plugin must be non-empty"),
            Self::Sqlite(e) => write!(f, "signal store sqlite error: {e}"),
            Self::Json(e) => write!(f, "signal store payload JSON error: {e}"),
            Self::Io(e) => write!(f, "signal store io error: {e}"),
        }
    }
}

impl std::error::Error for SignalStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidConfidence(_) | Self::EmptySourcePlugin => None,
            Self::Sqlite(e) => Some(e),
            Self::Json(e) => Some(e),
            Self::Io(e) => Some(e),
        }
    }
}

impl From<rusqlite::Error> for SignalStoreError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sqlite(e)
    }
}

impl From<serde_json::Error> for SignalStoreError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

impl From<std::io::Error> for SignalStoreError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
