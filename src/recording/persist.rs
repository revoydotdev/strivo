//! Sqlite-backed persistence for recording jobs, catalog dedupe, and Crunchr queue.
//!
//! Single file at `{data_dir}/jobs.db`. All writes go through a single
//! `tokio::sync::Mutex<rusqlite::Connection>` to keep the schema migration story
//! trivial — the daemon is single-instance per machine, so contention is bounded
//! and the lock is short-lived.

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::platform::{PlatformKind, VodEntry};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS jobs (
    id           TEXT PRIMARY KEY,
    kind         TEXT NOT NULL,
    payload      TEXT NOT NULL,
    state        TEXT NOT NULL,
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL,
    attempts     INTEGER NOT NULL DEFAULT 0,
    last_error   TEXT,
    episode_dir  TEXT
);
CREATE INDEX IF NOT EXISTS idx_jobs_state ON jobs(state);
CREATE INDEX IF NOT EXISTS idx_jobs_kind_updated ON jobs(kind, updated_at DESC);

CREATE TABLE IF NOT EXISTS catalog (
    platform        TEXT NOT NULL,
    channel_id      TEXT NOT NULL,
    vod_id          TEXT NOT NULL,
    title           TEXT NOT NULL,
    published_at    TEXT,
    episode_dir     TEXT,
    recorded_at     TEXT,
    transcribed_at  TEXT,
    PRIMARY KEY (platform, channel_id, vod_id)
);
CREATE INDEX IF NOT EXISTS idx_catalog_recorded ON catalog(recorded_at);

CREATE TABLE IF NOT EXISTS crunchr_queue (
    job_id       TEXT PRIMARY KEY,
    episode_dir  TEXT NOT NULL,
    backend      TEXT NOT NULL,
    diarize      INTEGER NOT NULL DEFAULT 0,
    state        TEXT NOT NULL,
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL,
    attempts     INTEGER NOT NULL DEFAULT 0,
    last_error   TEXT
);
CREATE INDEX IF NOT EXISTS idx_crunchr_state ON crunchr_queue(state);

CREATE TABLE IF NOT EXISTS blocklist (
    platform    TEXT NOT NULL,
    channel_id  TEXT NOT NULL,
    vod_id      TEXT NOT NULL DEFAULT '',  -- '' = whole channel blocked
    reason      TEXT,
    created_at  TEXT NOT NULL,
    PRIMARY KEY (platform, channel_id, vod_id)
);
"#;

/// One blocklist row. `vod_id` is empty for a whole-channel block.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BlockEntry {
    pub platform: String,
    pub channel_id: String,
    pub vod_id: String,
    pub reason: Option<String>,
    pub created_at: String,
}

#[derive(Clone)]
pub struct PersistDb {
    inner: Arc<Mutex<Connection>>,
    #[allow(dead_code)]
    path: PathBuf,
}

impl PersistDb {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create data dir {}", parent.display()))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("failed to open jobs.db at {}", path.display()))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA foreign_keys=ON;
             PRAGMA busy_timeout=30000;",
        )?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            inner: Arc::new(Mutex::new(conn)),
            path: path.to_path_buf(),
        })
    }

    /// Returns `true` if this VOD is already recorded (present in `catalog`
    /// with a non-null `recorded_at`). Used by the catalog runner to skip work.
    pub async fn is_vod_recorded(
        &self,
        platform: PlatformKind,
        channel_id: &str,
        vod_id: &str,
    ) -> Result<bool> {
        let conn = self.inner.lock().await;
        let recorded: Option<Option<String>> = conn
            .query_row(
                "SELECT recorded_at FROM catalog WHERE platform=?1 AND channel_id=?2 AND vod_id=?3",
                params![platform.to_string(), channel_id, vod_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?;
        Ok(matches!(recorded, Some(Some(s)) if !s.is_empty()))
    }

    /// Insert a discovered VOD (idempotent) — typically before queueing the
    /// recording job. `recorded_at` stays null until the job finishes.
    pub async fn upsert_catalog_entry(&self, vod: &VodEntry) -> Result<()> {
        let conn = self.inner.lock().await;
        conn.execute(
            "INSERT OR IGNORE INTO catalog (platform, channel_id, vod_id, title, published_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                vod.platform.to_string(),
                vod.channel_id,
                vod.id,
                vod.title,
                vod.published_at.map(|d| d.to_rfc3339()),
            ],
        )?;
        Ok(())
    }

    /// Mark a VOD as recorded with the resolved episode directory.
    pub async fn mark_vod_recorded(
        &self,
        platform: PlatformKind,
        channel_id: &str,
        vod_id: &str,
        episode_dir: &Path,
    ) -> Result<()> {
        let conn = self.inner.lock().await;
        conn.execute(
            "UPDATE catalog SET recorded_at = ?4, episode_dir = ?5
             WHERE platform=?1 AND channel_id=?2 AND vod_id=?3",
            params![
                platform.to_string(),
                channel_id,
                vod_id,
                chrono::Utc::now().to_rfc3339(),
                episode_dir.to_string_lossy(),
            ],
        )?;
        Ok(())
    }

    // ── Blocklist (roadmap item 17) — skip-this-VOD / skip-this-channel. ──

    /// Block a VOD (`vod_id = Some`) or a whole channel (`vod_id = None`) so
    /// the catalog/auto-record path stops grabbing it. Idempotent.
    pub async fn add_blocklist(
        &self,
        platform: PlatformKind,
        channel_id: &str,
        vod_id: Option<&str>,
        reason: Option<&str>,
    ) -> Result<()> {
        // An empty vod_id is the channel-level sentinel; callers must pass
        // None for that, never Some(""), or it would be indistinguishable
        // from a whole-channel block.
        if matches!(vod_id, Some("")) {
            anyhow::bail!("vod_id must be non-empty; pass None to block the whole channel");
        }
        let conn = self.inner.lock().await;
        conn.execute(
            "INSERT OR REPLACE INTO blocklist (platform, channel_id, vod_id, reason, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                platform.to_string(),
                channel_id,
                vod_id.unwrap_or(""),
                reason,
                chrono::Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    /// Remove a blocklist entry (channel-level when `vod_id = None`).
    pub async fn remove_blocklist(
        &self,
        platform: PlatformKind,
        channel_id: &str,
        vod_id: Option<&str>,
    ) -> Result<()> {
        let conn = self.inner.lock().await;
        conn.execute(
            "DELETE FROM blocklist WHERE platform=?1 AND channel_id=?2 AND vod_id=?3",
            params![platform.to_string(), channel_id, vod_id.unwrap_or("")],
        )?;
        Ok(())
    }

    /// True if this VOD is blocked directly OR its whole channel is blocked.
    pub async fn is_blocked(
        &self,
        platform: PlatformKind,
        channel_id: &str,
        vod_id: &str,
    ) -> Result<bool> {
        let conn = self.inner.lock().await;
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM blocklist
             WHERE platform=?1 AND channel_id=?2 AND (vod_id=?3 OR vod_id='')",
            params![platform.to_string(), channel_id, vod_id],
            |row| row.get(0),
        )?;
        Ok(n > 0)
    }

    /// All blocklist entries as `(platform, channel_id, vod_id, reason, created_at)`.
    pub async fn list_blocklist(&self) -> Result<Vec<BlockEntry>> {
        let conn = self.inner.lock().await;
        let mut stmt = conn.prepare(
            "SELECT platform, channel_id, vod_id, reason, created_at
             FROM blocklist ORDER BY created_at DESC",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(BlockEntry {
                    platform: r.get(0)?,
                    channel_id: r.get(1)?,
                    vod_id: r.get(2)?,
                    reason: r.get(3)?,
                    created_at: r.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Mark a VOD as transcribed (after Crunchr finishes its pipeline).
    pub async fn mark_vod_transcribed(
        &self,
        platform: PlatformKind,
        channel_id: &str,
        vod_id: &str,
    ) -> Result<()> {
        let conn = self.inner.lock().await;
        conn.execute(
            "UPDATE catalog SET transcribed_at = ?4
             WHERE platform=?1 AND channel_id=?2 AND vod_id=?3",
            params![
                platform.to_string(),
                channel_id,
                vod_id,
                chrono::Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    /// Persist a job in any state. Uses `INSERT OR REPLACE` so callers don't
    /// have to track which transitions are inserts vs updates.
    pub async fn upsert_job(&self, job: &PersistedJob) -> Result<()> {
        let conn = self.inner.lock().await;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO jobs (id, kind, payload, state, created_at, updated_at, attempts, last_error, episode_dir)
             VALUES (?1, ?2, ?3, ?4, COALESCE((SELECT created_at FROM jobs WHERE id=?1), ?5), ?5, ?6, ?7, ?8)
             ON CONFLICT(id) DO UPDATE SET
                kind=excluded.kind,
                payload=excluded.payload,
                state=excluded.state,
                updated_at=excluded.updated_at,
                attempts=excluded.attempts,
                last_error=excluded.last_error,
                episode_dir=excluded.episode_dir",
            params![
                job.id,
                job.kind,
                job.payload,
                job.state,
                now,
                job.attempts,
                job.last_error,
                job.episode_dir.as_ref().map(|p| p.to_string_lossy().into_owned()),
            ],
        )?;
        Ok(())
    }

    /// Load all jobs whose state is in the given list. Used by `recover()` on
    /// daemon startup to re-queue interrupted work.
    pub async fn load_jobs_in_states(&self, states: &[&str]) -> Result<Vec<PersistedJob>> {
        let conn = self.inner.lock().await;
        let placeholders = vec!["?"; states.len()].join(",");
        let sql = format!(
            "SELECT id, kind, payload, state, attempts, last_error, episode_dir FROM jobs WHERE state IN ({placeholders})"
        );
        let mut stmt = conn.prepare(&sql)?;
        let params_iter: Vec<&dyn rusqlite::ToSql> =
            states.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(&params_iter[..], |row| {
            Ok(PersistedJob {
                id: row.get(0)?,
                kind: row.get(1)?,
                payload: row.get(2)?,
                state: row.get(3)?,
                attempts: row.get(4)?,
                last_error: row.get(5)?,
                episode_dir: row.get::<_, Option<String>>(6)?.map(PathBuf::from),
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Reconstruct `RecordingJob`s persisted by `persist_event`. Called once
    /// at daemon startup so the TUI sees its history (including
    /// interrupted-but-not-finished rows) even after a crash.
    pub async fn load_recording_jobs(&self) -> Result<Vec<crate::recording::job::RecordingJob>> {
        let conn = self.inner.lock().await;
        let mut stmt = conn.prepare(
            "SELECT payload, state, last_error FROM jobs
             WHERE kind = 'Recording'
             ORDER BY updated_at DESC
             LIMIT 500",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?;

        let mut out = Vec::new();
        for r in rows {
            let (payload, state, err) = r?;
            let Ok(mut job) = serde_json::from_str::<crate::recording::job::RecordingJob>(&payload)
            else {
                continue;
            };
            // Force the state field to match the journal — `payload` was
            // serialized at job-creation time and may say 'queued'.
            if let Some(mapped) = map_journal_state(&state) {
                job.state = mapped;
            }
            if job.error.is_none() {
                job.error = err;
            }
            out.push(job);
        }
        Ok(out)
    }

    /// Load a bounded durable-history page directly in SQLite. The legacy
    /// `load_recording_jobs` remains capped at 500 for daemon recovery, while
    /// web clients can walk the complete journal without allocating it all.
    pub async fn load_recording_jobs_page(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<(Vec<crate::recording::job::RecordingJob>, usize)> {
        let conn = self.inner.lock().await;
        let total: i64 = conn.query_row(
            "SELECT COUNT(*) FROM jobs WHERE kind = 'Recording'",
            [],
            |row| row.get(0),
        )?;
        let mut stmt = conn.prepare(
            "SELECT payload, state, last_error FROM jobs
             WHERE kind = 'Recording'
             ORDER BY updated_at DESC
             LIMIT ?1 OFFSET ?2",
        )?;
        let rows = stmt.query_map(params![limit.min(500) as i64, offset as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (payload, state, err) = row?;
            let Ok(mut job) = serde_json::from_str::<crate::recording::job::RecordingJob>(&payload)
            else {
                continue;
            };
            if let Some(mapped) = map_journal_state(&state) {
                job.state = mapped;
            }
            if job.error.is_none() {
                job.error = err;
            }
            out.push(job);
        }
        Ok((out, total.max(0) as usize))
    }

    /// Count finished recordings for a channel (roadmap item 21 cutoff). Used
    /// by the monitor to stop auto-recording once a profile's cutoff is met.
    ///
    /// Uses a single SQL `COUNT(*)` with a `json_extract` predicate instead of
    /// loading and filtering up to 500 rows in Rust (the previous O(N) approach
    /// silently undercounted channels with more than 500 records).
    pub async fn count_finished_recordings(&self, channel_id: &str) -> Result<usize> {
        let conn = self.inner.lock().await;
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM jobs
             WHERE kind = 'Recording'
               AND state = 'finished'
               AND json_extract(payload, '$.channel_id') = ?1",
            params![channel_id],
            |row| row.get(0),
        )?;
        Ok(n as usize)
    }

    pub async fn upsert_crunchr_queue(&self, entry: &CrunchrQueueEntry) -> Result<()> {
        let conn = self.inner.lock().await;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO crunchr_queue (job_id, episode_dir, backend, diarize, state, created_at, updated_at, attempts, last_error)
             VALUES (?1, ?2, ?3, ?4, ?5, COALESCE((SELECT created_at FROM crunchr_queue WHERE job_id=?1), ?6), ?6, ?7, ?8)
             ON CONFLICT(job_id) DO UPDATE SET
                episode_dir=excluded.episode_dir,
                backend=excluded.backend,
                diarize=excluded.diarize,
                state=excluded.state,
                updated_at=excluded.updated_at,
                attempts=excluded.attempts,
                last_error=excluded.last_error",
            params![
                entry.job_id,
                entry.episode_dir.to_string_lossy(),
                entry.backend,
                entry.diarize as i64,
                entry.state,
                now,
                entry.attempts,
                entry.last_error,
            ],
        )?;
        Ok(())
    }

    pub async fn load_crunchr_queue_pending(&self) -> Result<Vec<CrunchrQueueEntry>> {
        let conn = self.inner.lock().await;
        let mut stmt = conn.prepare(
            "SELECT job_id, episode_dir, backend, diarize, state, attempts, last_error
             FROM crunchr_queue WHERE state IN ('queued', 'running', 'interrupted')",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(CrunchrQueueEntry {
                job_id: row.get(0)?,
                episode_dir: PathBuf::from(row.get::<_, String>(1)?),
                backend: row.get(2)?,
                diarize: row.get::<_, i64>(3)? != 0,
                state: row.get(4)?,
                attempts: row.get(5)?,
                last_error: row.get(6)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub async fn delete_crunchr_queue_entry(&self, job_id: &str) -> Result<()> {
        let conn = self.inner.lock().await;
        conn.execute("DELETE FROM crunchr_queue WHERE job_id=?1", params![job_id])?;
        Ok(())
    }

    /// Hard-delete a Recording row from the journal. The recording manager
    /// handles trashing the file separately; this just drops the audit row
    /// after that succeeds (or after the file is already gone).
    pub async fn delete_recording_job(&self, job_id: uuid::Uuid) -> Result<u64> {
        let conn = self.inner.lock().await;
        let n = conn.execute(
            "DELETE FROM jobs WHERE kind='Recording' AND id=?1",
            params![job_id.to_string()],
        )?;
        Ok(n as u64)
    }

    /// Every Recording journal row whose state is errored (`failed` or
    /// `interrupted`). Used by the webui's "Clear errored" toolbar action.
    pub async fn load_errored_recording_jobs(
        &self,
    ) -> Result<Vec<crate::recording::job::RecordingJob>> {
        let conn = self.inner.lock().await;
        let mut stmt = conn.prepare(
            "SELECT payload, state, last_error FROM jobs
             WHERE kind = 'Recording' AND state IN ('failed', 'interrupted')
             ORDER BY updated_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?;
        let mut out = Vec::new();
        for r in rows {
            let (payload, state, err) = r?;
            let Ok(mut job) = serde_json::from_str::<crate::recording::job::RecordingJob>(&payload)
            else {
                continue;
            };
            if let Some(mapped) = map_journal_state(&state) {
                job.state = mapped;
            }
            if job.error.is_none() {
                job.error = err;
            }
            out.push(job);
        }
        Ok(out)
    }

    /// Mark any job in "running" or "queued" state as "interrupted". Called once
    /// at daemon startup so a crashed run leaves a clear audit trail and doesn't
    /// look like work is still in flight. Returns how many rows were updated.
    pub async fn recover_orphaned_running(&self) -> Result<u64> {
        let conn = self.inner.lock().await;
        let now = chrono::Utc::now().to_rfc3339();
        let rows = conn.execute(
            "UPDATE jobs SET state='interrupted', updated_at=?1
             WHERE state IN ('running', 'queued')",
            params![now],
        )?;
        // Same for crunchr queue.
        conn.execute(
            "UPDATE crunchr_queue SET state='interrupted', updated_at=?1
             WHERE state IN ('running')",
            params![now],
        )?;
        Ok(rows as u64)
    }
}

#[derive(Debug, Clone)]
pub struct PersistedJob {
    pub id: String,
    pub kind: String,
    pub payload: String,
    pub state: String,
    pub attempts: i64,
    pub last_error: Option<String>,
    pub episode_dir: Option<PathBuf>,
}

fn map_journal_state(s: &str) -> Option<crate::recording::job::RecordingState> {
    use crate::recording::job::RecordingState as S;
    match s {
        "resolvingurl" | "resolving" => Some(S::ResolvingUrl),
        "recording" | "running" => Some(S::Recording),
        "stopping" => Some(S::Stopping),
        "finished" => Some(S::Finished),
        // 'interrupted' isn't a RecordingState variant — surface it as
        // Failed so the TUI shows the row in the failure styling.
        "failed" | "interrupted" => Some(S::Failed),
        _ => None,
    }
}

#[derive(Debug, Clone)]
pub struct CrunchrQueueEntry {
    pub job_id: String,
    pub episode_dir: PathBuf,
    pub backend: String,
    pub diarize: bool,
    pub state: String,
    pub attempts: i64,
    pub last_error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn catalog_dedupe_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let db = PersistDb::open(&dir.path().join("jobs.db")).unwrap();
        let vod = VodEntry {
            id: "abc".into(),
            platform: PlatformKind::YouTube,
            channel_id: "UC123".into(),
            title: "Episode 1".into(),
            published_at: Some(chrono::Utc::now()),
            duration: None,
            url: "https://example.com/abc".into(),
            thumbnail_url: None,
            kind: crate::platform::VodKind::Upload,
        };
        assert!(!db
            .is_vod_recorded(PlatformKind::YouTube, "UC123", "abc")
            .await
            .unwrap());
        db.upsert_catalog_entry(&vod).await.unwrap();
        assert!(!db
            .is_vod_recorded(PlatformKind::YouTube, "UC123", "abc")
            .await
            .unwrap());
        db.mark_vod_recorded(
            PlatformKind::YouTube,
            "UC123",
            "abc",
            std::path::Path::new("/tmp/ep"),
        )
        .await
        .unwrap();
        assert!(db
            .is_vod_recorded(PlatformKind::YouTube, "UC123", "abc")
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn blocklist_vod_and_channel() {
        let dir = tempfile::tempdir().unwrap();
        let db = PersistDb::open(&dir.path().join("jobs.db")).unwrap();
        let p = PlatformKind::Twitch;

        assert!(!db.is_blocked(p, "ch1", "v1").await.unwrap());

        // Block one VOD.
        db.add_blocklist(p, "ch1", Some("v1"), Some("dupe"))
            .await
            .unwrap();
        assert!(db.is_blocked(p, "ch1", "v1").await.unwrap());
        assert!(!db.is_blocked(p, "ch1", "v2").await.unwrap());

        // Block the whole channel → any VOD on it is blocked.
        db.add_blocklist(p, "ch2", None, None).await.unwrap();
        assert!(db.is_blocked(p, "ch2", "anything").await.unwrap());

        // List + remove.
        assert_eq!(db.list_blocklist().await.unwrap().len(), 2);
        db.remove_blocklist(p, "ch1", Some("v1")).await.unwrap();
        assert!(!db.is_blocked(p, "ch1", "v1").await.unwrap());
        assert_eq!(db.list_blocklist().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn count_finished_recordings_sql_count() {
        use crate::recording::job::RecordingJob;

        let dir = tempfile::tempdir().unwrap();
        let db = PersistDb::open(&dir.path().join("jobs.db")).unwrap();

        // Zero for an unknown channel.
        assert_eq!(db.count_finished_recordings("chanA").await.unwrap(), 0);

        // Helper: insert a finished recording for a given channel_id.
        let insert = |channel_id: &'static str| {
            let db = db.clone();
            async move {
                let job = RecordingJob::new(
                    channel_id.into(),
                    "test".into(),
                    PlatformKind::Twitch,
                    std::path::PathBuf::from(format!("/tmp/{channel_id}.mkv")),
                    false,
                    None,
                );
                let payload = serde_json::to_string(&job).unwrap();
                db.upsert_job(&PersistedJob {
                    id: job.id.to_string(),
                    kind: "Recording".into(),
                    payload,
                    state: "finished".into(),
                    attempts: 0,
                    last_error: None,
                    episode_dir: None,
                })
                .await
                .unwrap();
            }
        };

        // 3 finished for chanA, 1 for chanB, 1 "running" for chanA (not counted).
        insert("chanA").await;
        insert("chanA").await;
        insert("chanA").await;
        insert("chanB").await;

        // Also insert one running recording for chanA — must not be counted.
        let running_job = RecordingJob::new(
            "chanA".into(),
            "test".into(),
            PlatformKind::Twitch,
            std::path::PathBuf::from("/tmp/running.mkv"),
            false,
            None,
        );
        let running_payload = serde_json::to_string(&running_job).unwrap();
        db.upsert_job(&PersistedJob {
            id: running_job.id.to_string(),
            kind: "Recording".into(),
            payload: running_payload,
            state: "running".into(),
            attempts: 0,
            last_error: None,
            episode_dir: None,
        })
        .await
        .unwrap();

        assert_eq!(db.count_finished_recordings("chanA").await.unwrap(), 3);
        assert_eq!(db.count_finished_recordings("chanB").await.unwrap(), 1);
        assert_eq!(db.count_finished_recordings("chanC").await.unwrap(), 0);
    }

    #[tokio::test]
    async fn job_upsert_overwrites_state() {
        let dir = tempfile::tempdir().unwrap();
        let db = PersistDb::open(&dir.path().join("jobs.db")).unwrap();
        let job = PersistedJob {
            id: "j1".into(),
            kind: "Recording".into(),
            payload: "{}".into(),
            state: "queued".into(),
            attempts: 0,
            last_error: None,
            episode_dir: None,
        };
        db.upsert_job(&job).await.unwrap();
        let mut updated = job.clone();
        updated.state = "running".into();
        updated.attempts = 1;
        db.upsert_job(&updated).await.unwrap();
        let loaded = db.load_jobs_in_states(&["running"]).await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].attempts, 1);
    }
}
