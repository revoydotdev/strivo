//! The one place an edit is stored.
//!
//! **One table, one writer, one definition of "current".** The current edit of
//! a recording *is* its newest `revisions` row — there is no head table, no
//! per-recording cache, and no second copy anywhere a scene can hold. Every
//! edit-affecting read and write goes through [`EdlStore`], so an edit stored
//! in one representation and read from another is not expressible.
//!
//! History is append-only and load-bearing: "current" is `MAX(revision_id)`
//! over the recording's own rows, so restoring an old revision is itself a new
//! revision and is undoable. Nothing here is deleted but the retired second
//! source of truth.
//!
//! Two errors are refusals rather than values, and neither is a default:
//! a row written by a newer build is not guessed at, and a document that names
//! a different recording is not silently retargeted.

use std::fmt;

use anyhow::Result as AnyResult;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::Edl;

/// The document shape this build writes and understands.
///
/// Historical rows default to `1` on migration, which is correct: a
/// pre-unification row holds a version-1 [`Edl`]. A row stamped with anything
/// else was written by a build that understood more than this one, and is
/// refused rather than parsed — [`Edl`] tolerates unknown fields, so parsing
/// one would succeed with the wrong answer.
pub const REVISION_VERSION: i64 = 1;

/// Why a revision operation was refused.
///
/// Every variant names the operator-facing fact. They are distinct variants
/// rather than message strings because a caller has to *branch*: a conflict is
/// retryable, a foreign revision is a data problem, and an unsupported version
/// means the database and the binary do not belong together.
#[derive(Debug)]
pub enum RevisionError {
    /// The save named a base revision that is no longer the newest, so someone
    /// else wrote in between.
    Conflict { base: i64, current: i64 },
    /// No such revision for this recording.
    NotFound {
        recording_id: String,
        revision_id: i64,
    },
    /// The stored document names a different recording than the one it was
    /// loaded for.
    ForeignRecording {
        revision_id: i64,
        expected: String,
        found: String,
    },
    /// The row's document version is not one this build understands.
    UnsupportedVersion { revision_id: i64, found: i64 },
    /// The stored bytes are not a readable document.
    Unreadable {
        revision_id: i64,
        source: serde_json::Error,
    },
    /// The database itself failed.
    Storage(rusqlite::Error),
}

impl From<rusqlite::Error> for RevisionError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Storage(e)
    }
}

impl fmt::Display for RevisionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflict { base, current } => write!(
                f,
                "edit was made against revision {base}, but revision {current} is now current"
            ),
            Self::NotFound {
                recording_id,
                revision_id,
            } => write!(f, "revision {revision_id} not found for {recording_id}"),
            Self::ForeignRecording {
                revision_id,
                expected,
                found,
            } => write!(
                f,
                "revision {revision_id} holds an edit for {found}, not {expected}"
            ),
            Self::UnsupportedVersion { revision_id, found } => write!(
                f,
                "revision {revision_id} is document version {found}; this build understands {REVISION_VERSION}"
            ),
            Self::Unreadable { revision_id, source } => {
                write!(f, "revision {revision_id} is unreadable: {source}")
            }
            Self::Storage(e) => write!(f, "revision store: {e}"),
        }
    }
}

impl std::error::Error for RevisionError {}

pub struct EdlStore {
    conn: Connection,
}

/// Lightweight metadata row for the revisions list.
#[derive(Debug, Clone, Serialize)]
pub struct RevisionMeta {
    pub revision_id: i64,
    pub recording_id: String,
    pub label: String,
    pub created_at: String,
    pub cut_count: usize,
    pub total_duration_sec: f32,
}

/// The current revision of a recording: its identity *and* its content, so a
/// caller can hand the identity straight back as a save's base.
#[derive(Debug, Clone, PartialEq)]
pub struct CurrentRevision {
    pub revision_id: i64,
    pub edl: Edl,
}

impl EdlStore {
    pub fn open(path: &std::path::Path) -> AnyResult<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS revisions (
                 revision_id  INTEGER PRIMARY KEY AUTOINCREMENT,
                 recording_id TEXT NOT NULL,
                 doc_version  INTEGER NOT NULL,
                 edl_json     TEXT NOT NULL,
                 label        TEXT NOT NULL,
                 created_at   TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_revisions_recording ON revisions(recording_id, created_at);",
        )?;
        // A database written before the unification has no `doc_version`.
        // `NOT NULL DEFAULT 1` backfills every existing row, and 1 is the
        // truth: those rows hold a version-1 `Edl`. History is never rewritten.
        if !has_column(&conn, "revisions", "doc_version") {
            conn.execute_batch(
                "ALTER TABLE revisions ADD COLUMN doc_version INTEGER NOT NULL DEFAULT 1",
            )?;
        }
        // The retired head table held the same bytes as the newest `revisions`
        // row — the old writer put both in one transaction, from one serialised
        // value — so dropping it discards no edit. Its only other writer was
        // `clear`, which had no production caller and whose meaning ("forget
        // the current edit but keep the history") is now what "current is the
        // newest row" already means.
        conn.execute_batch("DROP TABLE IF EXISTS edls")?;
        Ok(Self { conn })
    }

    /// The one writer. `base` is the revision the edit was made against; when
    /// it is not the newest revision for the recording, someone else wrote in
    /// between and the save is refused as a conflict rather than overwriting
    /// their work. `None` means "I did not read a revision first", which is
    /// what a first save and every existing caller sends.
    pub fn save(
        &self,
        base: Option<i64>,
        edl: &Edl,
        label: &str,
    ) -> Result<CurrentRevision, RevisionError> {
        let recording_id = edl.recording_id.clone();
        let tx = self.conn.unchecked_transaction()?;
        let current: Option<i64> = tx
            .query_row(
                "SELECT MAX(revision_id) FROM revisions WHERE recording_id = ?1",
                params![recording_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        if let (Some(base), Some(current)) = (base, current) {
            if base != current {
                return Err(RevisionError::Conflict { base, current });
            }
        }
        let json = encode(edl, 0)?;
        let revision_id = append(
            &tx,
            &recording_id,
            &json,
            label,
            REVISION_VERSION,
            &Utc::now().to_rfc3339(),
        )?;
        tx.commit()?;
        Ok(CurrentRevision {
            revision_id,
            edl: edl.clone(),
        })
    }

    /// The one reader of "what is the current edit". The newest revision *is*
    /// the current edit; there is no second place it could also live.
    pub fn current(&self, recording_id: &str) -> Result<Option<CurrentRevision>, RevisionError> {
        let row: Option<(i64, i64, String)> = self
            .conn
            .query_row(
                "SELECT revision_id, doc_version, edl_json FROM revisions
                 WHERE recording_id = ?1 ORDER BY revision_id DESC LIMIT 1",
                params![recording_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((revision_id, doc_version, json)) = row else {
            return Ok(None);
        };
        let edl = decode(doc_version, &json, revision_id, recording_id)?;
        Ok(Some(CurrentRevision { revision_id, edl }))
    }

    /// Make `revision_id` the current edit, atomically.
    ///
    /// The target is read *and* validated *and* appended inside one
    /// transaction, so an unusable target leaves the current edit exactly as
    /// it was. The recording identity comes from the caller, never from the
    /// stored document: a historical row that names a different recording is a
    /// data problem, and silently writing it into the recording named by its
    /// own bytes is how an edit ends up in the wrong library.
    pub fn activate(
        &self,
        recording_id: &str,
        revision_id: i64,
        label: &str,
    ) -> Result<CurrentRevision, RevisionError> {
        let tx = self.conn.unchecked_transaction()?;
        let row: Option<(i64, String)> = tx
            .query_row(
                "SELECT doc_version, edl_json FROM revisions
                 WHERE recording_id = ?1 AND revision_id = ?2",
                params![recording_id, revision_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((doc_version, json)) = row else {
            return Err(RevisionError::NotFound {
                recording_id: recording_id.to_string(),
                revision_id,
            });
        };
        let edl = decode(doc_version, &json, revision_id, recording_id)?;
        let reserialised = encode(&edl, revision_id)?;
        let new_id = append(
            &tx,
            recording_id,
            &reserialised,
            label,
            REVISION_VERSION,
            &Utc::now().to_rfc3339(),
        )?;
        tx.commit()?;
        Ok(CurrentRevision {
            revision_id: new_id,
            edl,
        })
    }

    /// Newest-first list of revision metadata for `recording_id`.
    pub fn list_revisions(
        &self,
        recording_id: &str,
        limit: usize,
    ) -> Result<Vec<RevisionMeta>, RevisionError> {
        let mut stmt = self.conn.prepare(
            "SELECT revision_id, doc_version, edl_json, label, created_at
             FROM revisions WHERE recording_id = ?1 ORDER BY revision_id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![recording_id, limit as i64], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (revision_id, doc_version, json, label, created_at) = row?;
            let edl = decode(doc_version, &json, revision_id, recording_id)?;
            out.push(RevisionMeta {
                revision_id,
                recording_id: recording_id.to_string(),
                label,
                created_at,
                cut_count: edl.cuts.len(),
                total_duration_sec: edl.total_duration(),
            });
        }
        Ok(out)
    }
}

/// The one place a stored document becomes an [`Edl`], so no caller can read a
/// row by a laxer route than another.
///
/// Two things are refused rather than interpreted. A version this build does
/// not understand, because `Edl` tolerates unknown fields and would parse one
/// into the wrong answer. And a document that names a different recording than
/// the row it is stored under — the identity of the edit comes from the row,
/// never from its own bytes, so a row cannot move an edit into another
/// recording's library just by carrying that recording's id.
fn decode(
    doc_version: i64,
    json: &str,
    revision_id: i64,
    expected: &str,
) -> Result<Edl, RevisionError> {
    if doc_version != REVISION_VERSION {
        return Err(RevisionError::UnsupportedVersion {
            revision_id,
            found: doc_version,
        });
    }
    let edl: Edl = serde_json::from_str(json).map_err(|source| RevisionError::Unreadable {
        revision_id,
        source,
    })?;
    if edl.recording_id != expected {
        return Err(RevisionError::ForeignRecording {
            revision_id,
            expected: expected.to_string(),
            found: edl.recording_id,
        });
    }
    Ok(edl)
}

fn encode(edl: &Edl, revision_id: i64) -> Result<String, RevisionError> {
    serde_json::to_string(edl).map_err(|source| RevisionError::Unreadable {
        revision_id,
        source,
    })
}

/// The single append. Both public writers go through it, so there is one place
/// a row is created and one place the version is stamped.
fn append(
    tx: &rusqlite::Transaction<'_>,
    recording_id: &str,
    json: &str,
    label: &str,
    doc_version: i64,
    created_at: &str,
) -> Result<i64, RevisionError> {
    tx.execute(
        "INSERT INTO revisions (recording_id, doc_version, edl_json, label, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![recording_id, doc_version, json, label, created_at],
    )?;
    Ok(tx.last_insert_rowid())
}

fn has_column(conn: &Connection, table: &str, column: &str) -> bool {
    let Ok(mut stmt) = conn.prepare(&format!("PRAGMA table_info({table})")) else {
        return true;
    };
    let Ok(mut rows) = stmt.query([]) else {
        return true;
    };
    while let Ok(Some(row)) = rows.next() {
        if row.get::<_, String>(1).as_deref() == Ok(column) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cut, CutKind};

    fn temp_store() -> (EdlStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = EdlStore::open(&dir.path().join("editor.db")).unwrap();
        (store, dir)
    }

    fn sample_edl(rec: &str, n: usize) -> Edl {
        Edl {
            recording_id: rec.into(),
            cuts: (0..n)
                .map(|i| Cut {
                    kind: CutKind::Source {
                        source_path: "/tmp/x.mkv".into(),
                    },
                    start_sec: i as f32 * 10.0,
                    end_sec: i as f32 * 10.0 + 5.0,
                    fade_in_sec: 0.0,
                    fade_out_sec: 0.0,
                })
                .collect(),
        }
    }

    #[test]
    fn save_returns_the_revision_it_wrote() {
        let (store, _d) = temp_store();
        let written = store
            .save(None, &sample_edl("rec1", 2), "manual edit")
            .unwrap();
        assert_eq!(written.edl.cuts.len(), 2);
        let listed = store.list_revisions("rec1", 50).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].revision_id, written.revision_id);
        assert_eq!(listed[0].label, "manual edit");
    }

    #[test]
    fn current_is_the_newest_revision() {
        let (store, _d) = temp_store();
        store.save(None, &sample_edl("r", 1), "a").unwrap();
        let second = store.save(None, &sample_edl("r", 3), "b").unwrap();
        let current = store.current("r").unwrap().unwrap();
        assert_eq!(current.revision_id, second.revision_id);
        assert_eq!(current.edl.cuts.len(), 3);
    }

    #[test]
    fn current_is_none_for_a_recording_with_no_revisions() {
        let (store, _d) = temp_store();
        assert!(store.current("never-edited").unwrap().is_none());
    }

    #[test]
    fn multiple_saves_accumulate_revisions_newest_first() {
        let (store, _d) = temp_store();
        for (n, label) in [(1, "a"), (2, "b"), (3, "c")] {
            store.save(None, &sample_edl("r", n), label).unwrap();
        }
        let revs = store.list_revisions("r", 50).unwrap();
        let labels: Vec<&str> = revs.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, vec!["c", "b", "a"]);
        assert!(revs[0].revision_id > revs[1].revision_id);
    }

    #[test]
    fn revisions_are_scoped_to_recording_id() {
        let (store, _d) = temp_store();
        store.save(None, &sample_edl("alpha", 1), "x").unwrap();
        store.save(None, &sample_edl("beta", 2), "y").unwrap();
        assert_eq!(store.list_revisions("alpha", 50).unwrap().len(), 1);
        assert_eq!(store.list_revisions("beta", 50).unwrap().len(), 1);
        assert_eq!(store.current("alpha").unwrap().unwrap().edl.cuts.len(), 1);
        assert_eq!(store.current("beta").unwrap().unwrap().edl.cuts.len(), 2);
    }

    #[test]
    fn limit_clamps_returned_rows() {
        let (store, _d) = temp_store();
        for i in 0..7 {
            store
                .save(None, &sample_edl("r", i + 1), &format!("v{i}"))
                .unwrap();
        }
        assert_eq!(store.list_revisions("r", 3).unwrap().len(), 3);
    }

    #[test]
    fn metadata_reflects_stored_cut_count_and_duration() {
        let (store, _d) = temp_store();
        store.save(None, &sample_edl("r", 3), "z").unwrap();
        let revs = store.list_revisions("r", 50).unwrap();
        assert_eq!(revs[0].cut_count, 3);
        assert!((revs[0].total_duration_sec - 15.0).abs() < 1e-3);
    }

    /// A save that names the current revision as its base is accepted.
    #[test]
    fn a_save_against_the_current_base_is_accepted() {
        let (store, _d) = temp_store();
        let first = store.save(None, &sample_edl("r", 1), "first").unwrap();
        let second = store
            .save(Some(first.revision_id), &sample_edl("r", 2), "second")
            .unwrap();
        assert!(second.revision_id > first.revision_id);
    }

    /// A save that names a superseded base is refused, and nothing is written.
    #[test]
    fn a_save_against_a_superseded_base_is_refused() {
        let (store, _d) = temp_store();
        let first = store.save(None, &sample_edl("r", 1), "first").unwrap();
        let second = store.save(None, &sample_edl("r", 9), "other").unwrap();
        let outcome = store.save(Some(first.revision_id), &sample_edl("r", 4), "stale");
        match outcome {
            Err(RevisionError::Conflict { base, current }) => {
                assert_eq!(base, first.revision_id);
                assert_eq!(current, second.revision_id);
            }
            other => panic!("expected a conflict, got {other:?}"),
        }
        // The refused save left no trace and did not become current.
        assert_eq!(store.list_revisions("r", 50).unwrap().len(), 2);
        assert_eq!(store.current("r").unwrap().unwrap().edl.cuts.len(), 9);
    }

    /// Activation is a save: it appends, it does not rewind.
    #[test]
    fn activate_appends_and_makes_the_target_current() {
        let (store, _d) = temp_store();
        let five = store.save(None, &sample_edl("r", 5), "five").unwrap();
        store.save(None, &sample_edl("r", 1), "one").unwrap();
        let restored = store.activate("r", five.revision_id, "revert").unwrap();
        assert!(restored.revision_id > five.revision_id);
        assert_eq!(restored.edl.cuts.len(), 5);
        assert_eq!(store.current("r").unwrap().unwrap().edl.cuts.len(), 5);
        // The target is still readable afterwards; nothing was consumed.
        assert_eq!(store.list_revisions("r", 50).unwrap().len(), 3);
    }

    /// The real defect: the restore path used to take the recording identity
    /// from the stored document, so restoring a row whose bytes named another
    /// recording wrote that edit into the other recording.
    #[test]
    fn activate_refuses_a_revision_naming_another_recording() {
        let (store, _d) = temp_store();
        store.save(None, &sample_edl("mine", 3), "mine").unwrap();
        // A row under `mine` whose document says it belongs to `theirs`.
        {
            let foreign = sample_edl("theirs", 8);
            store
                .conn
                .execute(
                    "INSERT INTO revisions (recording_id, doc_version, edl_json, label, created_at)
                     VALUES ('mine', ?1, ?2, 'foreign', '2026-01-01T00:00:00Z')",
                    params![REVISION_VERSION, serde_json::to_string(&foreign).unwrap()],
                )
                .unwrap();
        }
        let foreign_id: i64 = store
            .conn
            .query_row(
                "SELECT revision_id FROM revisions WHERE label='foreign'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        match store.activate("mine", foreign_id, "revert") {
            Err(RevisionError::ForeignRecording {
                expected, found, ..
            }) => {
                assert_eq!(expected, "mine");
                assert_eq!(found, "theirs");
            }
            other => panic!("expected a foreign-recording refusal, got {other:?}"),
        }
        // Nothing moved, in either direction — and the foreign row is refused
        // by every reader, not only by `activate`. A reader that served it
        // would make the corrupt row the current edit of `mine`.
        assert!(matches!(
            store.current("mine"),
            Err(RevisionError::ForeignRecording { .. })
        ));
        assert!(matches!(
            store.list_revisions("mine", 50),
            Err(RevisionError::ForeignRecording { .. })
        ));
        assert!(store.current("theirs").unwrap().is_none());
        let rows: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM revisions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 2, "the refused row is reported, never deleted");
    }

    #[test]
    fn activate_reports_a_missing_revision() {
        let (store, _d) = temp_store();
        store.save(None, &sample_edl("r", 1), "x").unwrap();
        match store.activate("r", 99_999, "revert") {
            Err(RevisionError::NotFound { revision_id, .. }) => assert_eq!(revision_id, 99_999),
            other => panic!("expected NotFound, got {other:?}"),
        }
        assert!(store.activate("other-recording", 1, "revert").is_err());
    }

    /// A row from a newer build is refused, not parsed. `Edl` tolerates
    /// unknown fields, so a successful parse here would be a *wrong* answer.
    #[test]
    fn a_row_from_a_newer_build_is_refused_rather_than_parsed() {
        let (store, _d) = temp_store();
        store.save(None, &sample_edl("r", 2), "v1").unwrap();
        let future_id = {
            let json = serde_json::json!({
                "recording_id": "r",
                "cuts": [],
                "tracks": [{"id": "t1"}],
            })
            .to_string();
            store
                .conn
                .execute(
                    "INSERT INTO revisions (recording_id, doc_version, edl_json, label, created_at)
                     VALUES ('r', 2, ?1, 'from the future', '2026-01-01T00:00:00Z')",
                    params![json],
                )
                .unwrap();
            store.conn.last_insert_rowid()
        };
        match store.activate("r", future_id, "revert") {
            Err(RevisionError::UnsupportedVersion { found, .. }) => assert_eq!(found, 2),
            other => panic!("expected UnsupportedVersion, got {other:?}"),
        }
        // Every reader refuses it, rather than serving the row before it as
        // though the newer one did not exist.
        assert!(matches!(
            store.list_revisions("r", 50),
            Err(RevisionError::UnsupportedVersion { found: 2, .. })
        ));
        assert!(matches!(
            store.current("r"),
            Err(RevisionError::UnsupportedVersion { found: 2, .. })
        ));
        // And the row is still in the table — refused, not deleted.
        let rows: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM revisions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 2, "a refused row must be reported, never removed");
    }

    #[test]
    fn unreadable_bytes_are_reported_not_guessed_at() {
        let (store, _d) = temp_store();
        store.save(None, &sample_edl("r", 2), "v1").unwrap();
        store
            .conn
            .execute(
                "INSERT INTO revisions (recording_id, doc_version, edl_json, label, created_at)
                 VALUES ('r', ?1, '{\"not\":\"an edl\"}', 'corrupt', '2026-01-01T00:00:00Z')",
                params![REVISION_VERSION],
            )
            .unwrap();
        assert!(matches!(
            store.list_revisions("r", 50),
            Err(RevisionError::Unreadable { .. })
        ));
        // The corrupt row is the newest, so `current` refuses rather than
        // quietly serving the row before it as though nothing were wrong.
        assert!(matches!(
            store.current("r"),
            Err(RevisionError::Unreadable { .. })
        ));
        let rows: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM revisions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 2, "a corrupt row is reported, never deleted");
    }
}
