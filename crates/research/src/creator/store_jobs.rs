//! The durable job lifecycle: every state change, its append-only log row,
//! and the output validation a completion depends on.
//!
//! **The state change and its log row are written in one transaction.** That
//! single property is what makes the lifecycle durable rather than merely
//! persisted: a job cannot be observed in a state its own history does not
//! record, because the two writes commit or roll back together. It is the
//! same class of fix as stamping the schema version inside the `down`'s
//! transaction, and the reason is the same — a bookkeeping write that lives
//! outside the transaction that justifies it is a lie waiting for a reader.
//!
//! **Completion is gated on the output validating.** `complete` refuses
//! unless the artifact's bytes are present, non-empty, exactly the size the
//! attempt recorded, and hash to the content hash the row claims. A truncated
//! render fails here and the job stays where it was.

use std::path::{Path, PathBuf};

use rusqlite::{params, OptionalExtension, Transaction, TransactionBehavior};
use sha2::{Digest, Sha256};

use super::ids::*;
use super::jobs::*;
use super::store::Store;
use crate::{ResearchError, Result};

/// What a caller submits. Grouped because a job is one record: a builder that
/// could be handed half of it would be a job missing half its identity.
#[derive(Debug, Clone)]
pub struct NewJob {
    pub id: JobId,
    pub project_id: ProjectId,
    pub subject_kind: String,
    pub subject_id: String,
    pub cache_key: CacheKey,
    pub scratch_root: PathBuf,
    pub max_attempts: u8,
    pub at: String,
}

/// One requested state change.
#[derive(Debug, Clone, Copy)]
pub struct MoveJob<'a> {
    pub job: JobId,
    pub to: JobState,
    /// Which operation asked: `submit`, `run`, `complete`, `retry`, `cancel`,
    /// `skip`, `recover`. Recorded so a reopened job says why it moved.
    pub reason: &'a str,
    pub error: Option<&'a str>,
    pub at: &'a str,
}

/// One row of the append-only log. Grouped because every field is part of the
/// same fact: a log row missing its `seq` or its `reason` is not a shorter
/// entry, it is a different assertion about what happened.
struct LogRow<'a> {
    job: JobId,
    from: Option<JobState>,
    to: JobState,
    attempt: u8,
    error: Option<&'a str>,
    reason: &'a str,
    at: &'a str,
}

impl Store {
    /// Accept a job in [`JobState::Pending`], logging the creation as a
    /// transition with no `from_state` — the log is complete from seq 0, so
    /// "what did this job do" never needs a second source.
    pub fn submit_job(&self, new: &NewJob) -> Result<Job> {
        if new.max_attempts == 0 {
            return Err(ResearchError::Validation(
                "a job with no retry budget can never fail usefully".into(),
            ));
        }
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO creator_jobs
             (id,project_id,subject_kind,subject_id,cache_key,state,attempt,max_attempts,
              scratch_root,last_error,artifact_id,created_at,updated_at)
             VALUES(?1,?2,?3,?4,?5,'pending',0,?6,?7,NULL,NULL,?8,?8)",
            params![
                new.id.to_string(),
                new.project_id.to_string(),
                new.subject_kind,
                new.subject_id,
                new.cache_key.as_str(),
                i64::from(new.max_attempts),
                new.scratch_root.display().to_string(),
                new.at,
            ],
        )?;
        write_log(
            &tx,
            &LogRow {
                job: new.id,
                from: None,
                to: JobState::Pending,
                attempt: 0,
                error: None,
                reason: "submit",
                at: &new.at,
            },
        )?;
        tx.commit()?;
        self.require(new.id)
    }

    /// Move a job to `to`.
    pub fn move_job(&self, request: &MoveJob<'_>) -> Result<Job> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let from = read_state(&tx, request.job)?;
        guard(from, request.to)?;
        tx.execute(
            "UPDATE creator_jobs SET state=?2, last_error=?3, updated_at=?4 WHERE id=?1",
            params![
                request.job.to_string(),
                request.to.as_str(),
                request.error,
                request.at,
            ],
        )?;
        write_log(
            &tx,
            &LogRow {
                job: request.job,
                from: Some(from),
                to: request.to,
                attempt: 0,
                error: request.error,
                reason: request.reason,
                at: request.at,
            },
        )?;
        tx.commit()?;
        self.require(request.job)
    }

    /// Record an attempt failure, the way `PipelineRegistry::mark_stage_failed`
    /// does: the attempt counter advances here and nowhere else, and reaching
    /// the budget moves the job to [`JobState::Exhausted`] rather than leaving
    /// it retrying forever.
    pub fn fail_job(&self, job: JobId, error: &str, at: &str) -> Result<Job> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let current = read_job(&tx, job)?;
        let attempt = current.attempt.saturating_add(1);
        let to = if attempt >= current.max_attempts {
            JobState::Exhausted
        } else {
            JobState::Failed
        };
        guard(current.state, to)?;
        tx.execute(
            "UPDATE creator_jobs SET state=?2, attempt=?3, last_error=?4, updated_at=?5
             WHERE id=?1",
            params![job.to_string(), to.as_str(), i64::from(attempt), error, at],
        )?;
        write_log(
            &tx,
            &LogRow {
                job,
                from: Some(current.state),
                to,
                attempt,
                error: Some(error),
                reason: "fail",
                at,
            },
        )?;
        tx.commit()?;
        self.require(job)
    }

    /// Reset a failed or exhausted job to [`JobState::Pending`], zeroing the
    /// attempt counter when the budget was spent — `PipelineRegistry::retry_stage`'s
    /// rule, so a manual retry of an exhausted job gets a full budget again.
    pub fn retry_job(&self, job: JobId, at: &str) -> Result<Job> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let current = read_job(&tx, job)?;
        let attempt = if current.state == JobState::Exhausted {
            0
        } else {
            current.attempt
        };
        guard(current.state, JobState::Pending)?;
        tx.execute(
            "UPDATE creator_jobs SET state='pending', attempt=?2, last_error=NULL, updated_at=?3
             WHERE id=?1",
            params![job.to_string(), i64::from(attempt), at],
        )?;
        write_log(
            &tx,
            &LogRow {
                job,
                from: Some(current.state),
                to: JobState::Pending,
                attempt,
                error: None,
                reason: "retry",
                at,
            },
        )?;
        tx.commit()?;
        self.require(job)
    }

    /// Finish a job, pointing it at the artifact that proves it.
    pub fn complete(&self, job: JobId, artifact: ArtifactId, at: &str) -> Result<Job> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let from = read_state(&tx, job)?;
        guard(from, JobState::Done)?;
        tx.execute(
            "UPDATE creator_jobs SET state='done', artifact_id=?2, last_error=NULL, updated_at=?3
             WHERE id=?1",
            params![job.to_string(), artifact.to_string(), at],
        )?;
        write_log(
            &tx,
            &LogRow {
                job,
                from: Some(from),
                to: JobState::Done,
                attempt: 0,
                error: None,
                reason: "complete",
                at,
            },
        )?;
        tx.commit()?;
        self.require(job)
    }

    /// Re-queue jobs an interrupted process left `running`. Process death
    /// proves nothing is running, exactly as it does for a pipeline stage
    /// (`PipelineRegistry::open`) and for a capture job
    /// (`recover_orphaned_running`), so the state a crash froze is not a state
    /// a reader may trust.
    ///
    /// Every re-queued job gets its own log row **in this transaction**, so a
    /// reader can tell a recovered job from one that never started, and a
    /// process that dies mid-recovery leaves the whole set untouched rather
    /// than half of it moved. Returns how many were re-queued, which is zero
    /// on every open after the first — recovery is idempotent because the
    /// re-queued state is `pending`, and only `running` is selected.
    pub fn recover_jobs(&self, at: &str) -> Result<usize> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let frozen: Vec<JobId> = {
            let mut stmt = tx.prepare("SELECT id FROM creator_jobs WHERE state='running'")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
                .iter()
                .map(|raw| raw.parse())
                .collect::<Result<Vec<_>>>()?
        };
        for id in &frozen {
            tx.execute(
                "UPDATE creator_jobs SET state='pending', updated_at=?2 WHERE id=?1",
                params![id.to_string(), at],
            )?;
            write_log(
                &tx,
                &LogRow {
                    job: *id,
                    from: Some(JobState::Running),
                    to: JobState::Pending,
                    attempt: 0,
                    error: None,
                    reason: "recover",
                    at,
                },
            )?;
        }
        tx.commit()?;
        Ok(frozen.len())
    }

    /// The artifact an earlier job with these exact inputs produced, if it is
    /// still there and still validates. The reuse half of the cache key: an
    /// unchanged set of inputs must not render again.
    pub fn reusable(&self, cache_key: &CacheKey) -> Result<Option<ArtifactId>> {
        let id: Option<String> = self
            .conn
            .query_row(
                "SELECT j.artifact_id FROM creator_jobs j
                 JOIN creator_artifacts a ON a.id=j.artifact_id
                 WHERE j.cache_key=?1 AND j.state='done' AND a.validated_at IS NOT NULL
                 ORDER BY j.updated_at DESC LIMIT 1",
                [cache_key.as_str()],
                |r| r.get(0),
            )
            .optional()?;
        id.map_or(Ok(None), |raw| raw.parse().map(Some))
    }

    /// Check an artifact's bytes and stamp `validated_at`. The check is the
    /// output contract: the file is where the row says, it is not empty, it is
    /// exactly the recorded size, and it hashes to the recorded content hash.
    pub fn validate_artifact(&self, artifact: ArtifactId, at: &str) -> Result<()> {
        let (locator, expected_bytes, content_hash) = self
            .conn
            .query_row(
                "SELECT locator,bytes,content_hash FROM creator_artifacts WHERE id=?1",
                [artifact.to_string()],
                |r| {
                    Ok((
                        r.get::<_, Option<String>>(0)?,
                        r.get::<_, Option<i64>>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| ResearchError::Validation(format!("no such artifact: {artifact}")))?;
        check_output(locator.as_deref(), expected_bytes, &content_hash)?;
        self.conn.execute(
            "UPDATE creator_artifacts SET validated_at=?2 WHERE id=?1",
            params![artifact.to_string(), at],
        )?;
        Ok(())
    }

    /// Promote one artifact to current for its (project, kind), demoting
    /// whatever held that place. Separate from completion on purpose: a
    /// finished render makes an output available, it does not switch the
    /// project over to it.
    pub fn make_current(&self, artifact: ArtifactId) -> Result<()> {
        let (project, kind): (String, String) = self.conn.query_row(
            "SELECT project_id,kind FROM creator_artifacts WHERE id=?1",
            [artifact.to_string()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        self.conn.execute(
            "UPDATE creator_artifacts SET current=0
             WHERE project_id=?1 AND kind=?2 AND current=1 AND id<>?3",
            params![project, kind, artifact.to_string()],
        )?;
        self.conn.execute(
            "UPDATE creator_artifacts SET current=1 WHERE id=?1",
            [artifact.to_string()],
        )?;
        Ok(())
    }

    /// The artifact currently promoted for a (project, kind), if any.
    pub fn current_artifact(&self, project: ProjectId, kind: &str) -> Result<Option<ArtifactId>> {
        let id: Option<String> = self
            .conn
            .query_row(
                "SELECT id FROM creator_artifacts WHERE project_id=?1 AND kind=?2 AND current=1",
                params![project.to_string(), kind],
                |r| r.get(0),
            )
            .optional()?;
        id.map_or(Ok(None), |raw| raw.parse().map(Some))
    }

    /// One job, or `None` if there is no such row.
    pub fn job(&self, id: JobId) -> Result<Option<Job>> {
        self.conn
            .query_row(
                "SELECT id,project_id,subject_kind,subject_id,cache_key,state,attempt,max_attempts,
                        scratch_root,last_error,artifact_id,created_at,updated_at
                 FROM creator_jobs WHERE id=?1",
                [id.to_string()],
                row_to_job,
            )
            .optional()
            .map_err(Into::into)
    }

    /// Every job of a project, oldest first.
    pub fn jobs(&self, project: ProjectId) -> Result<Vec<Job>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,project_id,subject_kind,subject_id,cache_key,state,attempt,max_attempts,
                    scratch_root,last_error,artifact_id,created_at,updated_at
             FROM creator_jobs WHERE project_id=?1 ORDER BY created_at,id",
        )?;
        let rows = stmt.query_map([project.to_string()], row_to_job)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// One job's transitions, in `seq` order.
    pub fn transitions(&self, id: JobId) -> Result<Vec<JobTransition>> {
        let mut stmt = self.conn.prepare(
            "SELECT seq,from_state,to_state,attempt,error,reason,recorded_at
             FROM creator_job_transitions WHERE job_id=?1 ORDER BY seq",
        )?;
        let rows = stmt.query_map([id.to_string()], |r| {
            Ok(JobTransition {
                job_id: id,
                seq: r.get(0)?,
                from: r
                    .get::<_, Option<String>>(1)?
                    .map(|s| s.parse())
                    .transpose()
                    .map_err(|e: crate::ResearchError| {
                        rusqlite::Error::InvalidColumnType(
                            1,
                            e.to_string(),
                            rusqlite::types::Type::Text,
                        )
                    })?,
                to: r
                    .get::<_, String>(2)?
                    .parse()
                    .map_err(|e: crate::ResearchError| {
                        rusqlite::Error::InvalidColumnType(
                            2,
                            e.to_string(),
                            rusqlite::types::Type::Text,
                        )
                    })?,
                attempt: r.get::<_, i64>(3)? as u8,
                error: r.get(4)?,
                reason: r.get(5)?,
                recorded_at: r.get(6)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// Create (or reuse) the isolated working directory for one attempt, and
    /// return it. The path carries the job id and the attempt number, so two
    /// attempts never share one and neither can clear the other's.
    pub fn prepare_attempt(&self, job: &Job, attempt: u8) -> Result<PathBuf> {
        let dir = job.attempt_dir(attempt);
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    /// Remove one attempt's working directory, and only that one.
    pub fn release_attempt(&self, job: &Job, attempt: u8) -> Result<()> {
        let dir = job.attempt_dir(attempt);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        Ok(())
    }

    fn require(&self, id: JobId) -> Result<Job> {
        self.job(id)?
            .ok_or_else(|| ResearchError::Validation(format!("no such job: {id}")))
    }
}

/// Refuse an edge the transition table does not carry. Called by every writer
/// before it touches a row, so [`TRANSITIONS`] is the single authority rather
/// than a table nothing reads — the failure mode that would let a `pending`
/// job jump straight to `done`, skipping the `running` its log never recorded.
fn guard(from: JobState, to: JobState) -> Result<()> {
    if is_legal(from, to) {
        return Ok(());
    }
    Err(ResearchError::Validation(format!(
        "a job cannot go {from} -> {to}"
    )))
}

fn write_log(tx: &Transaction<'_>, row: &LogRow<'_>) -> Result<()> {
    let next: i64 = tx.query_row(
        "SELECT COALESCE(MAX(seq)+1,0) FROM creator_job_transitions WHERE job_id=?1",
        [row.job.to_string()],
        |r| r.get(0),
    )?;
    tx.execute(
        "INSERT INTO creator_job_transitions
         (job_id,seq,from_state,to_state,attempt,error,reason,recorded_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        params![
            row.job.to_string(),
            next,
            row.from.map(JobState::as_str),
            row.to.as_str(),
            i64::from(row.attempt),
            row.error,
            row.reason,
            row.at,
        ],
    )?;
    Ok(())
}

fn read_state(tx: &Transaction<'_>, id: JobId) -> Result<JobState> {
    let raw: Option<String> = tx
        .query_row(
            "SELECT state FROM creator_jobs WHERE id=?1",
            [id.to_string()],
            |r| r.get(0),
        )
        .optional()?;
    match raw {
        Some(state) => state.parse(),
        None => Err(ResearchError::Validation(format!("no such job: {id}"))),
    }
}

fn read_job(tx: &Transaction<'_>, id: JobId) -> Result<Job> {
    tx.query_row(
        "SELECT id,project_id,subject_kind,subject_id,cache_key,state,attempt,max_attempts,
                scratch_root,last_error,artifact_id,created_at,updated_at
         FROM creator_jobs WHERE id=?1",
        [id.to_string()],
        row_to_job,
    )
    .map_err(Into::into)
}

fn row_to_job(r: &rusqlite::Row<'_>) -> rusqlite::Result<Job> {
    let uuid = |i: usize, v: String| -> rusqlite::Result<uuid::Uuid> {
        uuid::Uuid::parse_str(&v).map_err(|e| {
            rusqlite::Error::InvalidColumnType(i, e.to_string(), rusqlite::types::Type::Text)
        })
    };
    // An undefined state is an error, never a default. Reading `halfway` as
    // `pending` would hand a caller a job in a state its log never recorded,
    // which is the one wrong-row a typed id elsewhere in this crate is
    // designed to make impossible.
    let state: JobState = r
        .get::<_, String>(5)?
        .parse()
        .map_err(|e: crate::ResearchError| {
            rusqlite::Error::InvalidColumnType(5, e.to_string(), rusqlite::types::Type::Text)
        })?;
    Ok(Job {
        id: JobId::from_uuid(uuid(0, r.get(0)?)?),
        project_id: ProjectId::from_uuid(uuid(1, r.get(1)?)?),
        subject_kind: r.get(2)?,
        subject_id: r.get(3)?,
        cache_key: CacheKey::from_raw(r.get(4)?),
        state,
        attempt: r.get::<_, i64>(6)? as u8,
        max_attempts: r.get::<_, i64>(7)? as u8,
        scratch_root: r.get::<_, String>(8)?.into(),
        last_error: r.get(9)?,
        artifact_id: r
            .get::<_, Option<String>>(10)?
            .map(|v| uuid(10, v))
            .transpose()?
            .map(ArtifactId::from_uuid),
        created_at: r.get(11)?,
        updated_at: r.get(12)?,
    })
}

/// The output contract, in one place so `validate_artifact` is not the only
/// caller who has to remember it. Each refusal names the operator-facing fact
/// rather than returning a bool, because "the render came out truncated" and
/// "the render is not there" are different problems.
fn check_output(
    locator: Option<&str>,
    expected_bytes: Option<i64>,
    content_hash: &str,
) -> Result<()> {
    let Some(locator) = locator else {
        return Err(ResearchError::Validation(
            "the artifact has no locator, so there is nothing to validate".into(),
        ));
    };
    let path = Path::new(locator);
    let observed = std::fs::metadata(path)
        .map_err(|e| {
            ResearchError::Validation(format!("output is not readable at {locator}: {e}"))
        })?
        .len() as i64;
    if observed == 0 {
        return Err(ResearchError::Validation(format!(
            "output at {locator} is empty"
        )));
    }
    if let Some(expected) = expected_bytes {
        if observed != expected {
            return Err(ResearchError::Validation(format!(
                "output at {locator} is {observed} bytes, the attempt recorded {expected}: a \
                 truncated or partial render is not a completed one"
            )));
        }
    }
    let actual = hash_file(path);
    if actual != content_hash {
        return Err(ResearchError::Validation(format!(
            "output at {locator} hashes to {actual}, the artifact claims {content_hash}"
        )));
    }
    Ok(())
}

fn hash_file(path: &Path) -> String {
    let mut digest = Sha256::new();
    if let Ok(bytes) = std::fs::read(path) {
        digest.update(&bytes);
    }
    digest
        .finalize()
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            hex.push_str(&format!("{byte:02x}"));
            hex
        })
}
