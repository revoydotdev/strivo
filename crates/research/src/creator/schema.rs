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

/// Every Creator table the current version has, in dependency order. A PVR
/// install has none of them; this list is what a test asserts against, and
/// what a late migration's `ALTER`s are appended to.
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
    "creator_jobs",
    "creator_job_transitions",
    "creator_workspace",
    "creator_viewer_samples",
    "creator_stream_events",
    "creator_metric_availability",
    "creator_source_notes",
    "creator_history",
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

const JOB_LIFECYCLE_V3: &str = include_str!("schema_v3.sql");

const DROP_JOB_LIFECYCLE_V3: &str = include_str!("schema_v3.down.sql");

const PROJECT_WORKSPACE_V4: &str = include_str!("schema_v4.sql");

const DROP_PROJECT_WORKSPACE_V4: &str = include_str!("schema_v4.down.sql");

const OBSERVATION_SERIES_V5: &str = include_str!("schema_v5.sql");

const DROP_OBSERVATION_SERIES_V5: &str = include_str!("schema_v5.down.sql");

const SOURCE_RIGHTS_V6: &str = include_str!("schema_v6.sql");

const DROP_SOURCE_RIGHTS_V6: &str = include_str!("schema_v6.down.sql");

const EDIT_HISTORY_V7: &str = include_str!("schema_v7.sql");

const DROP_EDIT_HISTORY_V7: &str = include_str!("schema_v7.down.sql");

/// Every migration, in order. Version 1 is the whole Creator identity
/// schema; version 2 is the canonical time model, which replaces version 1's
/// millisecond durations with integer microseconds and adds the wall-clock
/// to media-time segments; version 3 is the durable job and artifact
/// lifecycle; version 4 is the persistent project workspace — one row per
/// project carrying the one selected revision every view reads, the playhead,
/// the selection range, the active view and unsaved work; version 5 is the
/// observation series; version 6 is the source rights notes, and it adds a
/// table with **no eligibility column** because publication eligibility is
/// derived from the note rather than stored beside it (see
/// [`super::rights`]). Versions 1 to 3 are frozen — committed history with
/// rehearsed rollbacks, and a database already at those versions migrates
/// forward from the bytes they wrote.
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
    Migration {
        version: 3,
        name: "creator_job_lifecycle",
        up: JOB_LIFECYCLE_V3,
        down: DROP_JOB_LIFECYCLE_V3,
    },
    Migration {
        version: 4,
        name: "creator_project_workspace",
        up: PROJECT_WORKSPACE_V4,
        down: DROP_PROJECT_WORKSPACE_V4,
    },
    Migration {
        version: 5,
        name: "creator_observation_series",
        up: OBSERVATION_SERIES_V5,
        down: DROP_OBSERVATION_SERIES_V5,
    },
    Migration {
        version: 6,
        name: "creator_source_rights",
        up: SOURCE_RIGHTS_V6,
        down: DROP_SOURCE_RIGHTS_V6,
    },
    Migration {
        version: 7,
        name: "creator_edit_history",
        up: EDIT_HISTORY_V7,
        down: DROP_EDIT_HISTORY_V7,
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
    let latest = migrations.iter().map(|m| m.version).max().unwrap_or(0);
    loop {
        let start = applied_version(conn)?;
        if start > latest {
            return Err(ResearchError::UnsupportedSchema {
                found: start,
                supported: latest,
            });
        }
        let Some(next) = migrations.iter().find(|m| m.version > start) else {
            return Ok(start);
        };
        apply_one(conn, next)?;
    }
}

/// Apply exactly one migration, atomically, and report whether this caller was
/// the one that applied it.
///
/// On any error the transaction is rolled back, so a migration that dies
/// half-way leaves the recorded version untouched and the next open re-runs it
/// from the start.
///
/// **The version is re-read inside this transaction, under the write lock
/// `BEGIN IMMEDIATE` takes.** Reading it outside is what let two connections
/// opening the same Creator database at once both decide to apply the same
/// step, and the loser died on the first statement naming something the winner
/// had already added — `duplicate column name: duration_us`, which is schema
/// version 2's `ALTER TABLE`. That is not a theoretical race: two views of one
/// project is the case this whole schema exists to serve, and the workspace
/// routes open a connection per request. `BEGIN IMMEDIATE` takes the write lock
/// before the re-read, so the second applier blocks, then sees the committed
/// version and skips.
fn apply_one(conn: &mut Connection, m: &Migration) -> Result<bool, ResearchError> {
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let outcome = (|| -> Result<bool, ResearchError> {
        if applied_version(conn)? >= m.version {
            return Ok(false);
        }
        conn.execute_batch(m.up)?;
        conn.execute(
            "INSERT INTO schema_meta(key,value) VALUES(?1,?2)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            rusqlite::params![VERSION_KEY, m.version.to_string()],
        )?;
        Ok(true)
    })();
    match outcome {
        Ok(applied) => {
            conn.execute_batch("COMMIT")?;
            Ok(applied)
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
///
/// **Only a version the database actually reached has its `down` run.** The
/// list is this build's, which may be longer than what a given file went
/// through — a database recorded at version 2, opened by a build that also
/// carries version 3. Rolling that one to 1 used to run version 3's `down`
/// first, on a schema version 3 never touched, and died on the first statement
/// naming a column or table that was never added. The filter is
/// `version > target && version <= start`, so the steps walked are exactly the
/// ones that were applied, in reverse.
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
    for m in migrations
        .iter()
        .rev()
        .filter(|m| m.version > target && m.version <= start)
    {
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
