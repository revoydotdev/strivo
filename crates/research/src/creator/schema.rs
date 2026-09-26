//! Numbered forward migrations for the Creator schema.
//!
//! The mechanism is the research kernel's existing one, extended: version
//! bookkeeping lives in the `schema_meta` key/value table the kernel already
//! creates, under the `creator_schema_version` key so the two version lines
//! cannot be confused. There is no second migration framework and no second
//! bookkeeping table — `user_version`, a `migrations` ledger table and a
//! third-party runner are all deliberately absent.
//!
//! Rules this module enforces, from
//! `docs/RESEARCH-PLATFORM-ROADMAP.md:70-72`:
//!
//! * migrations are monotonic and numbered from 1, with no gaps and no
//!   duplicate versions ([`validate`]);
//! * each is applied inside one transaction, so an apply that dies part-way
//!   leaves the previous version intact and the next open re-runs it;
//! * a database newer than this build is **reported**
//!   ([`ResearchError::UnsupportedSchema`]), never opened as if it were
//!   understood;
//! * every migration has an explicit `down`, and destructive `down` is only
//!   ever rehearsed from a backup.

use rusqlite::Connection;

use crate::ResearchError;

/// The `schema_meta` key holding this schema's version line.
pub const VERSION_KEY: &str = "creator_schema_version";

/// Every table migration 1 creates, in dependency order. A PVR install has
/// none of them; this list is what a test asserts against.
pub const TABLES: &[&str] = &[
    "creator_projects",
    "creator_project_research_links",
    "creator_source_assets",
    "creator_capture_sessions",
    "creator_transcript_revisions",
    "creator_composition_revisions",
    "creator_variants",
    "creator_render_plans",
    "creator_artifacts",
    "creator_release_packages",
    "creator_publications",
    "creator_metric_observations",
    "creator_clock_segments",
];

/// One numbered forward step, with the explicit reverse step the rollback
/// rehearsal uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Migration {
    pub version: i64,
    pub name: &'static str,
    pub up: &'static str,
    pub down: &'static str,
}

const CREATE_V1: &str = include_str!("schema_v1.sql");

const DROP_V1: &str = include_str!("schema_v1.down.sql");

const TIME_MODEL_V2: &str = include_str!("schema_v2.sql");

const DROP_TIME_MODEL_V2: &str = include_str!("schema_v2.down.sql");

/// Every migration, in order. Version 1 is the whole Creator identity
/// schema; version 2 is the canonical time model, which replaces version 1's
/// millisecond durations with integer microseconds and adds the wall-clock
/// to media-time segments. Version 1 is frozen — it is committed history,
/// and a database already at version 1 migrates forward from the bytes
/// version 1 wrote.
pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "creator_identity",
        up: CREATE_V1,
        down: DROP_V1,
    },
    Migration {
        version: 2,
        name: "creator_time_model",
        up: TIME_MODEL_V2,
        down: DROP_TIME_MODEL_V2,
    },
];

/// The version this build understands, or the error a newer database gets.
#[must_use]
pub fn latest_version() -> i64 {
    MIGRATIONS.iter().map(|m| m.version).max().unwrap_or(0)
}

/// Reject a migration list that is not monotonic from 1, so a bad append is
/// reported instead of half-applied.
fn validate(migrations: &[Migration]) -> Result<(), ResearchError> {
    for (expected, m) in (1..).zip(migrations.iter()) {
        if m.version != expected {
            return Err(ResearchError::Validation(format!(
                "migration {} ({}) breaks the sequence: expected version {expected}",
                m.version, m.name
            )));
        }
        if m.up.trim().is_empty() || m.down.trim().is_empty() {
            return Err(ResearchError::Validation(format!(
                "migration {} ({}) needs both an up and a down",
                m.version, m.name
            )));
        }
    }
    Ok(())
}

/// The version recorded in the database, or 0 for a database that has never
/// had this schema. A missing `schema_meta` row is version 0, not an error:
/// that is a PVR-era file, or a research-only file.
pub fn applied_version(conn: &Connection) -> Result<i64, ResearchError> {
    let has_meta: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='schema_meta'",
        [],
        |row| row.get(0),
    )?;
    if has_meta == 0 {
        return Ok(0);
    }
    let raw: Option<String> = conn
        .query_row(
            "SELECT value FROM schema_meta WHERE key=?1",
            [VERSION_KEY],
            |row| row.get(0),
        )
        .ok();
    match raw {
        None => Ok(0),
        Some(text) => text
            .parse()
            .map_err(|_| ResearchError::Validation(format!("unreadable {VERSION_KEY}: {text}"))),
    }
}

/// Apply every migration newer than the recorded version, in order.
///
/// Returns the version the database is at afterwards. A database already
/// ahead of this build is an error, not a no-op: silently opening a schema
/// this code does not know is how state gets dropped.
pub fn migrate(conn: &mut Connection, migrations: &[Migration]) -> Result<i64, ResearchError> {
    validate(migrations)?;
    let start = applied_version(conn)?;
    let latest = migrations.iter().map(|m| m.version).max().unwrap_or(0);
    if start > latest {
        return Err(ResearchError::UnsupportedSchema {
            found: start,
            supported: latest,
        });
    }
    for m in migrations.iter().filter(|m| m.version > start) {
        apply_one(conn, m)?;
    }
    applied_version(conn)
}

/// Apply exactly one migration, atomically. On any error the transaction is
/// rolled back, so a migration that dies half-way leaves the recorded
/// version untouched and the next open re-runs it from the start.
fn apply_one(conn: &mut Connection, m: &Migration) -> Result<(), ResearchError> {
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let outcome = (|| -> Result<(), ResearchError> {
        conn.execute_batch(m.up)?;
        conn.execute(
            "INSERT INTO schema_meta(key,value) VALUES(?1,?2)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            rusqlite::params![VERSION_KEY, m.version.to_string()],
        )?;
        Ok(())
    })();
    match outcome {
        Ok(()) => {
            conn.execute_batch("COMMIT")?;
            Ok(())
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

/// Record `version` on this schema's version line, or remove the line
/// entirely at version 0 — a database that has never had this schema has no
/// line, and leaving a `0` behind would make `applied_version` and the
/// tables disagree.
fn stamp(conn: &Connection, version: i64) -> Result<(), ResearchError> {
    if version <= 0 {
        conn.execute("DELETE FROM schema_meta WHERE key=?1", [VERSION_KEY])?;
    } else {
        conn.execute(
            "INSERT INTO schema_meta(key,value) VALUES(?1,?2)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            rusqlite::params![VERSION_KEY, version.to_string()],
        )?;
    }
    Ok(())
}

/// Roll the schema back to `target`, one explicit `down` per step. Destructive
/// by construction: only ever run against a backup copy, which is what the
/// rehearsal does.
///
/// The version line moves down with the schema, in the same transaction as
/// the `down` that justifies it. A rollback that left the line where it was
/// would make the next open re-apply a migration whose statements had already
/// been undone — the database would claim version 2 while holding version 1's
/// tables, and `duration_us` would be "migrated" onto a schema that no longer
/// has the column.
pub fn rollback(
    conn: &mut Connection,
    target: i64,
    migrations: &[Migration],
) -> Result<i64, ResearchError> {
    validate(migrations)?;
    let start = applied_version(conn)?;
    if start <= target {
        return Ok(start);
    }
    for m in migrations.iter().rev().filter(|m| m.version > target) {
        conn.execute_batch("BEGIN IMMEDIATE")?;
        let outcome = (|| -> Result<(), ResearchError> {
            conn.execute_batch(m.down)?;
            stamp(conn, m.version - 1)
        })();
        match outcome {
            Ok(()) => conn.execute_batch("COMMIT")?,
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK");
                return Err(e);
            }
        }
    }
    applied_version(conn)
}
