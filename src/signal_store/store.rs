//! Schema, migration, and the typed `SignalStore` handle.

use std::path::Path;

use rusqlite::{Connection, ToSql};

use super::error::SignalStoreError;
use super::model::{NewSignal, Signal, SignalQuery};

/// Current schema version. Bump when adding a migration, and append the
/// migration step in [`migrate`] guarded by `if current < N`.
const SCHEMA_VERSION: i64 = 1;

const SCHEMA_V1: &str = r#"
CREATE TABLE IF NOT EXISTS signals (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    recording_id    TEXT NOT NULL,
    t_start         REAL NOT NULL,
    t_end           REAL NOT NULL,
    kind            TEXT NOT NULL,
    label           TEXT NOT NULL,
    payload         TEXT NOT NULL,
    confidence      REAL NOT NULL,
    source_plugin   TEXT NOT NULL,
    created_at      TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_signals_recording ON signals(recording_id);
CREATE INDEX IF NOT EXISTS idx_signals_kind ON signals(kind);
CREATE INDEX IF NOT EXISTS idx_signals_recording_range ON signals(recording_id, t_start, t_end);
"#;

/// Idempotently bring `conn` up to [`SCHEMA_VERSION`], tracked via
/// `PRAGMA user_version`. Safe to call on every open: a database already at
/// the current version runs no DDL.
fn migrate(conn: &Connection) -> Result<(), SignalStoreError> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if current < 1 {
        conn.execute_batch(SCHEMA_V1)?;
    }
    if current < SCHEMA_VERSION {
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    }
    Ok(())
}

/// The canonical append-only signal store.
///
/// Open with [`SignalStore::open`], append with
/// [`SignalStore::write_signals`], read with [`SignalStore::query_signals`].
/// There is deliberately no update or delete API: signals are facts, not
/// mutable state.
pub struct SignalStore {
    conn: Connection,
}

impl SignalStore {
    /// Open (creating if necessary) the signal store database at `path`,
    /// running any pending migration. Re-opening an already-migrated
    /// database is a no-op beyond the connection setup.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, SignalStoreError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=30000;",
        )?;
        migrate(&conn)?;
        Ok(Self { conn })
    }

    /// Open an in-memory signal store. Useful for tests; the database does
    /// not persist and is not shared across connections.
    pub fn open_in_memory() -> Result<Self, SignalStoreError> {
        let conn = Connection::open_in_memory()?;
        migrate(&conn)?;
        Ok(Self { conn })
    }

    /// Append `signals` to the store in a single transaction.
    ///
    /// Every signal is validated before any row is written: `confidence`
    /// must lie in `[0.0, 1.0]` and `source_plugin` must be non-empty. On a
    /// validation failure, none of the batch is persisted — the whole call
    /// returns the typed error instead of silently clamping or dropping the
    /// offending rows.
    ///
    /// Returns the assigned row ids, in the same order as `signals`.
    pub fn write_signals(&self, signals: &[NewSignal]) -> Result<Vec<i64>, SignalStoreError> {
        for signal in signals {
            if !(0.0..=1.0).contains(&signal.confidence) {
                return Err(SignalStoreError::InvalidConfidence(signal.confidence));
            }
            if signal.source_plugin.trim().is_empty() {
                return Err(SignalStoreError::EmptySourcePlugin);
            }
        }

        let tx = self.conn.unchecked_transaction()?;
        let mut ids = Vec::with_capacity(signals.len());
        {
            let mut stmt = tx.prepare(
                "INSERT INTO signals
                    (recording_id, t_start, t_end, kind, label, payload, confidence, source_plugin)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?;
            for signal in signals {
                let payload_text = serde_json::to_string(&signal.payload)?;
                stmt.execute(rusqlite::params![
                    signal.recording_id,
                    signal.t_start,
                    signal.t_end,
                    signal.kind,
                    signal.label,
                    payload_text,
                    signal.confidence,
                    signal.source_plugin,
                ])?;
                ids.push(tx.last_insert_rowid());
            }
        }
        tx.commit()?;
        Ok(ids)
    }

    /// Query signals matching `query`, ordered by insertion order (`id`).
    ///
    /// `recording_id` and `kind` are exact matches; `time_range` matches on
    /// span overlap (see [`SignalQuery`]). Fields left unset in `query`
    /// impose no restriction.
    pub fn query_signals(&self, query: &SignalQuery) -> Result<Vec<Signal>, SignalStoreError> {
        let mut sql = String::from(
            "SELECT id, recording_id, t_start, t_end, kind, label, payload, confidence, source_plugin, created_at
             FROM signals WHERE 1=1",
        );
        let mut params: Vec<Box<dyn ToSql>> = Vec::new();

        if let Some(recording_id) = &query.recording_id {
            sql.push_str(" AND recording_id = ?");
            params.push(Box::new(recording_id.clone()));
        }
        if let Some(kind) = &query.kind {
            sql.push_str(" AND kind = ?");
            params.push(Box::new(kind.clone()));
        }
        if let Some((from, to)) = query.time_range {
            sql.push_str(" AND t_start <= ? AND t_end >= ?");
            params.push(Box::new(to));
            params.push(Box::new(from));
        }
        sql.push_str(" ORDER BY id ASC");

        let mut stmt = self.conn.prepare(&sql)?;
        let param_refs: Vec<&dyn ToSql> = params.iter().map(std::convert::AsRef::as_ref).collect();

        let rows = stmt.query_map(param_refs.as_slice(), |row| {
            Ok(RawSignal {
                id: row.get(0)?,
                recording_id: row.get(1)?,
                t_start: row.get(2)?,
                t_end: row.get(3)?,
                kind: row.get(4)?,
                label: row.get(5)?,
                payload: row.get(6)?,
                confidence: row.get(7)?,
                source_plugin: row.get(8)?,
                created_at: row.get(9)?,
            })
        })?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row?.into_signal()?);
        }
        Ok(out)
    }
}

/// Intermediate row shape before the JSON payload is parsed — `rusqlite`'s
/// `query_map` closure can only return `rusqlite::Error`, so payload
/// parsing happens one layer up where a [`SignalStoreError::Json`] can be
/// returned instead.
struct RawSignal {
    id: i64,
    recording_id: String,
    t_start: f64,
    t_end: f64,
    kind: String,
    label: String,
    payload: String,
    confidence: f64,
    source_plugin: String,
    created_at: String,
}

impl RawSignal {
    fn into_signal(self) -> Result<Signal, SignalStoreError> {
        Ok(Signal {
            id: self.id,
            recording_id: self.recording_id,
            t_start: self.t_start,
            t_end: self.t_end,
            kind: self.kind,
            label: self.label,
            payload: serde_json::from_str(&self.payload)?,
            confidence: self.confidence,
            source_plugin: self.source_plugin,
            created_at: self.created_at,
        })
    }
}
