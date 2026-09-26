//! The Creator-edition schema bootstrap.
//!
//! This module is `#[cfg(feature = "creator")]`-gated in `lib.rs`, so a PVR
//! build does not compile it, does not link `strivo-research`, and has no
//! statement from `strivo_research::creator::schema` anywhere in the binary.
//! That is the boundary F04 asks for: a PVR install never creates these
//! tables, because a PVR install has no code that could.
//!
//! Declared here rather than in `strivo-research` because the feature that
//! gates it has to be the one that actually removes code from the PVR binary,
//! and `strivo-research` has no such feature — it is an optional dependency
//! of this crate that only `creator` activates.

use std::path::Path;

/// The database the Creator schema shares with the research kernel:
/// `{data_dir}/research.db`, beside `jobs.db`.
pub fn creator_db_path() -> std::path::PathBuf {
    strivo_core::config::AppConfig::data_dir().join("research.db")
}

/// Apply any pending Creator migrations. Called once from `serve`, before
/// the first request. Idempotent, and a failure is logged rather than fatal:
/// a broken Creator schema must not stop the PVR-grade parts of the web UI
/// from serving.
pub fn ensure_creator_schema() {
    if let Err(e) = strivo_research::creator::ensure_schema(&creator_db_path()) {
        tracing::warn!("creator schema not applied: {e}");
    }
}

/// The Creator tables this build's store would create. Named so the edition
/// tests and the PVR proof can assert on the list rather than on prose.
pub fn creator_tables() -> &'static [&'static str] {
    strivo_research::creator::TABLES
}

/// Path of the file a Creator install writes. Split out for the edition
/// tests, which must not touch a real data directory.
pub fn ensure_schema_at(path: &Path) -> Result<i64, String> {
    strivo_research::creator::ensure_schema(path).map_err(|e| e.to_string())
}
