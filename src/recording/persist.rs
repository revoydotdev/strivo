//! Sqlite-backed persistence for recording jobs and catalog dedupe.
//!
//! Single file at `{data_dir}/jobs.db`, accessed through a small pool of
//! persistent connections (see `ConnPool` below) rather than one shared
//! `Mutex<Connection>` (V07 remediation). A single global mutex removes the
//! per-request `Connection::open` + PRAGMA + `CREATE TABLE IF NOT EXISTS`
//! cost that per-request opens paid (a real win, measured), but it also
//! serialises every caller onto one handle — throwing away the concurrent
//! reads SQLite's WAL mode would otherwise allow. A small fixed pool keeps
//! the "open once" win (every pooled connection has PRAGMAs + schema applied
//! exactly once, at pool-open time) while letting independent callers run
//! concurrently.
//!
//! Each borrow runs its query inline on the calling async task rather than
//! via `tokio::task::spawn_blocking`. That was the first cut, on the theory
//! that synchronous rusqlite work should never occupy a Tokio worker — but
//! measured (`scripts/bench_jobs_db.sh`, see V07 closure evidence)
//! `spawn_blocking`'s fixed dispatch cost (queueing onto the blocking pool
//! and back) exceeded the query time it was meant to protect against: these
//! are indexed, WAL-mode, page-cache-resident reads/writes with no network
//! or disk-seek latency, typically sub-millisecond. Paying a cross-thread
//! hop on every call regressed the sequential path back to worse than the
//! single-mutex design it was replacing, which fails V07's own acceptance
//! bar ("at least matches the shared-handle design sequentially"). Inline
//! execution plus the `sem` semaphore (bounding concurrent synchronous holders
//! to `POOL_SIZE`) matched the shared-mutex design sequentially and still won
//! at c=8/c=16. If a genuinely slow query ever lands on this path, revisit —
//! `spawn_blocking` is the correct tool for that case, just not this one.
//!
//! Hand-rolled rather than pulled in via `r2d2`/`r2d2_sqlite`: those crates
//! pin a newer `rusqlite` (0.38+) than the `0.33` this workspace shares
//! across `strivo-core`, `strivo-web`, and the Creator-edition
//! `strivo-plugins` crate (9 source files). Bumping the major version to fit
//! the pool crate would ripple across all three, well outside what a
//! database-handle-shape fix should touch. A `Vec<Mutex<Connection>>`
//! guarded by a `Semaphore` gets the same property (bounded, reusable,
//! pre-warmed connections) with a self-contained diff.

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex, TryLockError as StdTryLockError};
use tokio::sync::Semaphore;

use crate::platform::{PlatformKind, VodEntry};

/// Number of persistent connections kept open per `PersistDb`. Small and
/// fixed: this is a self-hosted daemon serving a handful of browser tabs,
/// not a multi-tenant service — a handful of readers is enough to stop
/// requests from queuing behind one another while staying cheap to hold
/// open for the process lifetime.
const POOL_SIZE: usize = 4;

fn open_pooled_connection(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)
        .with_context(|| format!("failed to open jobs.db at {}", path.display()))?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         PRAGMA foreign_keys=ON;
         PRAGMA busy_timeout=30000;",
    )?;
    Ok(conn)
}

/// A small fixed-size pool of persistent, pre-warmed connections. `sem`
/// bounds concurrent borrows to `conns.len()`; `with_conn` scans for a free
/// slot (cheap — the pool is tiny) rather than assigning connections
/// round-robin, so load naturally balances across whichever connections are
/// free.
struct ConnPool {
    conns: Vec<StdMutex<Connection>>,
    sem: Arc<Semaphore>,
}

impl ConnPool {
    fn open(path: &Path, size: usize) -> Result<Self> {
        let mut conns = Vec::with_capacity(size);
        for _ in 0..size {
            conns.push(StdMutex::new(open_pooled_connection(path)?));
        }
        // Schema DDL is idempotent (`IF NOT EXISTS`); applying it once on
        // the first connection is enough since every connection targets the
        // same file.
        conns[0]
            .lock()
            .expect("pool mutex poisoned during schema init")
            .execute_batch(SCHEMA)?;
        Ok(Self {
            conns,
            sem: Arc::new(Semaphore::new(size)),
        })
    }
}

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
    pool: Arc<ConnPool>,
    #[allow(dead_code)]
    path: PathBuf,
}

impl PersistDb {
    /// Remove journal rows whose output has disappeared. This is deliberately
    /// separate from recovery so the cleanup is visible and auditable.
    pub async fn prune_missing_recordings(&self) -> Result<Vec<(uuid::Uuid, PathBuf)>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare("SELECT id, episode_dir, payload FROM jobs WHERE kind='Recording'")?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?, row.get::<_, String>(2)?))
            })?;
            let mut removed = Vec::new();
            for row in rows {
                let (id, dir, payload) = row?;
                let path = serde_json::from_str::<crate::recording::job::RecordingJob>(&payload)
                    .ok().map(|j| j.output_path).or_else(|| dir.map(PathBuf::from));
                let Some(path) = path else { continue };
                if path.exists() { continue; }
                if let Ok(uuid) = uuid::Uuid::parse_str(&id) {
                    conn.execute("DELETE FROM jobs WHERE kind='Recording' AND id=?1", params![id])?;
                    removed.push((uuid, path));
                }
            }
            Ok(removed)
        }).await
    }
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create data dir {}", parent.display()))?;
        }
        let pool = ConnPool::open(path, POOL_SIZE)?;
        Ok(Self {
            pool: Arc::new(pool),
            path: path.to_path_buf(),
        })
    }

    /// Borrow a pooled connection and run `f` against it inline (see the
    /// module doc for why not `spawn_blocking`). Bounds concurrent borrows
    /// to the pool size via `sem`, then scans the small connection list for
    /// a free slot — cheap given `POOL_SIZE` is a handful, and it naturally
    /// spreads load across whichever connections are idle rather than
    /// pinning callers to one.
    async fn with_conn<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Connection) -> rusqlite::Result<T>,
    {
        let _permit = self
            .pool
            .sem
            .acquire()
            .await
            .context("connection pool semaphore closed")?;
        loop {
            for slot in self.pool.conns.iter() {
                match slot.try_lock() {
                    Ok(guard) => return f(&guard).context("sqlite operation failed"),
                    // A panic inside a previous borrow poisons the slot.
                    // `StdMutex` reports that as an error indistinguishable
                    // from contention, so skipping it would retire the
                    // connection permanently and — once every slot is
                    // poisoned — spin this loop forever. The connection
                    // itself is still sound: rusqlite holds no partial
                    // state across a panicking closure, and each call here
                    // is a single self-contained statement. Recover the
                    // guard and carry on.
                    Err(StdTryLockError::Poisoned(poisoned)) => {
                        return f(&poisoned.into_inner()).context("sqlite operation failed");
                    }
                    Err(StdTryLockError::WouldBlock) => continue,
                }
            }
            tokio::task::yield_now().await;
        }
    }

    /// Returns `true` if this VOD is already recorded (present in `catalog`
    /// with a non-null `recorded_at`). Used by the catalog runner to skip work.
    pub async fn is_vod_recorded(
        &self,
        platform: PlatformKind,
        channel_id: &str,
        vod_id: &str,
    ) -> Result<bool> {
        let platform = platform.to_string();
        let channel_id = channel_id.to_string();
        let vod_id = vod_id.to_string();
        self.with_conn(move |conn| {
            let recorded: Option<Option<String>> = conn
                .query_row(
                    "SELECT recorded_at FROM catalog WHERE platform=?1 AND channel_id=?2 AND vod_id=?3",
                    params![platform, channel_id, vod_id],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()?;
            Ok(matches!(recorded, Some(Some(s)) if !s.is_empty()))
        })
        .await
    }

    /// Insert a discovered VOD (idempotent) — typically before queueing the
    /// recording job. `recorded_at` stays null until the job finishes.
    pub async fn upsert_catalog_entry(&self, vod: &VodEntry) -> Result<()> {
        let platform = vod.platform.to_string();
        let channel_id = vod.channel_id.clone();
        let id = vod.id.clone();
        let title = vod.title.clone();
        let published_at = vod.published_at.map(|d| d.to_rfc3339());
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT OR IGNORE INTO catalog (platform, channel_id, vod_id, title, published_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![platform, channel_id, id, title, published_at],
            )?;
            Ok(())
        })
        .await
    }

    /// Mark a VOD as recorded with the resolved episode directory.
    pub async fn mark_vod_recorded(
        &self,
        platform: PlatformKind,
        channel_id: &str,
        vod_id: &str,
        episode_dir: &Path,
    ) -> Result<()> {
        let platform = platform.to_string();
        let channel_id = channel_id.to_string();
        let vod_id = vod_id.to_string();
        let episode_dir = episode_dir.to_string_lossy().into_owned();
        self.with_conn(move |conn| {
            conn.execute(
                "UPDATE catalog SET recorded_at = ?4, episode_dir = ?5
                 WHERE platform=?1 AND channel_id=?2 AND vod_id=?3",
                params![
                    platform,
                    channel_id,
                    vod_id,
                    chrono::Utc::now().to_rfc3339(),
                    episode_dir,
                ],
            )?;
            Ok(())
        })
        .await
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
        let platform = platform.to_string();
        let channel_id = channel_id.to_string();
        let vod_id = vod_id.unwrap_or("").to_string();
        let reason = reason.map(str::to_string);
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT OR REPLACE INTO blocklist (platform, channel_id, vod_id, reason, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    platform,
                    channel_id,
                    vod_id,
                    reason,
                    chrono::Utc::now().to_rfc3339(),
                ],
            )?;
            Ok(())
        })
        .await
    }

    /// Remove a blocklist entry (channel-level when `vod_id = None`).
    pub async fn remove_blocklist(
        &self,
        platform: PlatformKind,
        channel_id: &str,
        vod_id: Option<&str>,
    ) -> Result<()> {
        let platform = platform.to_string();
        let channel_id = channel_id.to_string();
        let vod_id = vod_id.unwrap_or("").to_string();
        self.with_conn(move |conn| {
            conn.execute(
                "DELETE FROM blocklist WHERE platform=?1 AND channel_id=?2 AND vod_id=?3",
                params![platform, channel_id, vod_id],
            )?;
            Ok(())
        })
        .await
    }

    /// True if this VOD is blocked directly OR its whole channel is blocked.
    pub async fn is_blocked(
        &self,
        platform: PlatformKind,
        channel_id: &str,
        vod_id: &str,
    ) -> Result<bool> {
        let platform = platform.to_string();
        let channel_id = channel_id.to_string();
        let vod_id = vod_id.to_string();
        self.with_conn(move |conn| {
            let n: i64 = conn.query_row(
                "SELECT COUNT(*) FROM blocklist
                 WHERE platform=?1 AND channel_id=?2 AND (vod_id=?3 OR vod_id='')",
                params![platform, channel_id, vod_id],
                |row| row.get(0),
            )?;
            Ok(n > 0)
        })
        .await
    }

    /// All blocklist entries as `(platform, channel_id, vod_id, reason, created_at)`.
    pub async fn list_blocklist(&self) -> Result<Vec<BlockEntry>> {
        self.with_conn(|conn| {
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
        })
        .await
    }

    /// Mark a VOD as transcribed (after a transcription plugin finishes its pipeline).
    pub async fn mark_vod_transcribed(
        &self,
        platform: PlatformKind,
        channel_id: &str,
        vod_id: &str,
    ) -> Result<()> {
        let platform = platform.to_string();
        let channel_id = channel_id.to_string();
        let vod_id = vod_id.to_string();
        self.with_conn(move |conn| {
            conn.execute(
                "UPDATE catalog SET transcribed_at = ?4
                 WHERE platform=?1 AND channel_id=?2 AND vod_id=?3",
                params![
                    platform,
                    channel_id,
                    vod_id,
                    chrono::Utc::now().to_rfc3339()
                ],
            )?;
            Ok(())
        })
        .await
    }

    /// Persist a job in any state. Uses `INSERT OR REPLACE` so callers don't
    /// have to track which transitions are inserts vs updates.
    pub async fn upsert_job(&self, job: &PersistedJob) -> Result<()> {
        let job = job.clone();
        self.with_conn(move |conn| {
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
        })
        .await
    }

    /// Load all jobs whose state is in the given list. Used by `recover()` on
    /// daemon startup to re-queue interrupted work.
    pub async fn load_jobs_in_states(&self, states: &[&str]) -> Result<Vec<PersistedJob>> {
        let states: Vec<String> = states.iter().map(|s| s.to_string()).collect();
        self.with_conn(move |conn| {
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
        })
        .await
    }

    /// Reconstruct `RecordingJob`s persisted by `persist_event`. Called once
    /// at daemon startup so the TUI sees its history (including
    /// interrupted-but-not-finished rows) even after a crash.
    pub async fn load_recording_jobs(&self) -> Result<Vec<crate::recording::job::RecordingJob>> {
        self.with_conn(|conn| {
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
                let Ok(mut job) =
                    serde_json::from_str::<crate::recording::job::RecordingJob>(&payload)
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
        })
        .await
    }

    /// Resolve one recording from the durable journal.  This deliberately has
    /// no recovery cap: an archived item must remain addressable after the
    /// daemon has evicted it from its bounded in-memory snapshot.
    pub async fn load_recording_job(
        &self,
        id: uuid::Uuid,
    ) -> Result<Option<crate::recording::job::RecordingJob>> {
        self.with_conn(move |conn| {
            let row = conn
                .query_row(
                    "SELECT payload, state, last_error FROM jobs WHERE kind = 'Recording' AND id = ?1",
                    params![id.to_string()],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Option<String>>(2)?,
                        ))
                    },
                )
                .optional()?;
            Ok(row.and_then(|(payload, state, err)| {
                let mut job = serde_json::from_str::<crate::recording::job::RecordingJob>(&payload).ok()?;
                if let Some(mapped) = map_journal_state(&state) {
                    job.state = mapped;
                }
                if job.error.is_none() {
                    job.error = err;
                }
                Some(job)
            }))
        })
        .await
    }

    /// Load a bounded durable-history page directly in SQLite. The legacy
    /// `load_recording_jobs` remains capped at 500 for daemon recovery, while
    /// web clients can walk the complete journal without allocating it all.
    pub async fn load_recording_jobs_page(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<(Vec<crate::recording::job::RecordingJob>, usize)> {
        self.with_conn(move |conn| {
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
                let Ok(mut job) =
                    serde_json::from_str::<crate::recording::job::RecordingJob>(&payload)
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
        })
        .await
    }

    /// Count finished recordings for a channel (roadmap item 21 cutoff). Used
    /// by the monitor to stop auto-recording once a profile's cutoff is met.
    ///
    /// Uses a single SQL `COUNT(*)` with a `json_extract` predicate instead of
    /// loading and filtering up to 500 rows in Rust (the previous O(N) approach
    /// silently undercounted channels with more than 500 records).
    pub async fn count_finished_recordings(&self, channel_id: &str) -> Result<usize> {
        let channel_id = channel_id.to_string();
        self.with_conn(move |conn| {
            let n: i64 = conn.query_row(
                "SELECT COUNT(*) FROM jobs
                 WHERE kind = 'Recording'
                   AND state = 'finished'
                   AND json_extract(payload, '$.channel_id') = ?1",
                params![channel_id],
                |row| row.get(0),
            )?;
            Ok(n as usize)
        })
        .await
    }

    /// Hard-delete a Recording row from the journal. The recording manager
    /// handles trashing the file separately; this just drops the audit row
    /// after that succeeds (or after the file is already gone).
    pub async fn delete_recording_job(&self, job_id: uuid::Uuid) -> Result<u64> {
        self.with_conn(move |conn| {
            let n = conn.execute(
                "DELETE FROM jobs WHERE kind='Recording' AND id=?1",
                params![job_id.to_string()],
            )?;
            Ok(n as u64)
        })
        .await
    }

    /// Every Recording journal row whose state is errored (`failed` or
    /// `interrupted`). Used by the webui's "Clear errored" toolbar action.
    pub async fn load_errored_recording_jobs(
        &self,
    ) -> Result<Vec<crate::recording::job::RecordingJob>> {
        self.with_conn(|conn| {
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
                let Ok(mut job) =
                    serde_json::from_str::<crate::recording::job::RecordingJob>(&payload)
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
        })
        .await
    }

    /// Mark any job in "running" or "queued" state as "interrupted". Called once
    /// at daemon startup so a crashed run leaves a clear audit trail and doesn't
    /// look like work is still in flight. Returns how many rows were updated.
    pub async fn recover_orphaned_running(&self) -> Result<u64> {
        self.with_conn(|conn| {
            let now = chrono::Utc::now().to_rfc3339();
            let rows = conn.execute(
                "UPDATE jobs SET state='interrupted', updated_at=?1
                 WHERE state IN ('running', 'queued')",
                params![now],
            )?;
            Ok(rows as u64)
        })
        .await
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

    /// A panic inside a pooled closure poisons that connection's `StdMutex`.
    /// `try_lock` reports poisoning as `Err`, indistinguishable from
    /// contention, so a poisoned slot is skipped forever. Once every slot is
    /// poisoned the borrow loop spins on `yield_now` and never returns —
    /// a livelock, not an error. The previous `tokio::sync::Mutex` had no
    /// poisoning, so this is a regression in failure behaviour.
    #[tokio::test]
    async fn pool_survives_a_panic_inside_a_borrowed_connection() {
        let dir = tempfile::tempdir().unwrap();
        let db = PersistDb::open(&dir.path().join("jobs.db")).unwrap();

        for _ in 0..POOL_SIZE {
            let db2 = db.clone();
            let _ = tokio::spawn(async move {
                db2.with_conn(|_conn| -> rusqlite::Result<()> {
                    panic!("simulated failure while holding a pooled connection")
                })
                .await
            })
            .await;
        }

        // Every slot is now poisoned. The pool must still serve requests.
        let res = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            db.count_finished_recordings("chanA"),
        )
        .await;
        assert!(
            res.is_ok(),
            "pool livelocked after panics poisoned every connection"
        );
        assert_eq!(res.unwrap().unwrap(), 0);
    }
}
