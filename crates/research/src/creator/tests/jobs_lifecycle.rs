//! The durable job and artifact lifecycle, and the two claims that carry it.
//!
//! **Claim 1 — restart-safe.** A job interrupted mid-attempt must come back in
//! a state a reader may trust. The test that proves it kills the *store*, not
//! a variable: [`restart_resumes_an_interrupted_job`] writes a real database
//! file, drops the connection as a process death would, reopens it, and
//! asserts the job's state **changed** from `running` to `pending`. Asserting
//! merely that the job still exists would pass against a lifecycle that
//! recovers nothing, so the assertion is on the transition, and the log row
//! naming `recover` is what proves the change was recorded and not faked.
//!
//! **Claim 2 — one lifecycle, not several.** [`every_state_appears_in_the_migration_check`]
//! pins [`JobState::ALL`] to the `CHECK` list the migration wrote, and
//! [`only_done_has_no_way_out`] pins the transition table's shape. Together
//! they fail if a second state vocabulary grows beside this one.
//!
//! These tests are deliberately written against the **public** surface
//! (`Store`), not against SQL, so a refactor that keeps the contract passes
//! and one that quietly reintroduces a second notion does not.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::super::entities::*;
use super::super::ids::*;
use super::super::jobs::*;
use super::super::schema;
use super::super::schema::MIGRATIONS;
use super::super::store::Store;
use super::super::store_jobs::{MoveJob, NewJob};
use super::fixtures::*;

/// A job plus everything a lifecycle test needs to drive it, so no test has to
/// restate the boilerplate that is not its subject.
///
/// `store` is an `Option` so a test can genuinely **drop the connection** and
/// reopen the file, which is what a process death is. A `Store` held in a
/// plain field could only be moved out, and then the fixture that names the
/// path would be half-moved — so the restart claim would be untestable and
/// silently untested.
struct Fixture {
    _dir: TempDir,
    db: PathBuf,
    scratch: PathBuf,
    store: Option<Store>,
    project: ProjectId,
}

impl Fixture {
    /// Open a **file-backed** store. File-backed because claim 1 is about a
    /// process dying, and an in-memory database cannot outlive the process
    /// that would have to be killed.
    fn new() -> Self {
        let dir = TempDir::new().expect("tempdir");
        let db = dir.path().join("creator.db");
        let scratch = dir.path().join("scratch");
        let store = Store::open(&db).expect("open");
        let p = project("stream");
        store.insert_project(&p).expect("project");
        Self {
            _dir: dir,
            db,
            scratch,
            store: Some(store),
            project: p.id,
        }
    }

    /// The open store. A test that has killed it has a bug, and the panic
    /// says so rather than reporting a confusing downstream failure.
    fn s(&self) -> &Store {
        self.store.as_ref().expect("the store is open")
    }

    /// The connection goes away without any orderly shutdown, as a process
    /// death does. No flush, no close, no chance to write a final state.
    fn kill(&mut self) {
        self.store = None;
    }

    /// The same database, opened again — a restart, with nothing carried over
    /// except the file.
    fn restart(&self) -> Store {
        Store::open(&self.db).expect("reopen")
    }

    fn new_job(&self, subject: &str) -> NewJob {
        NewJob {
            id: JobId::generate(),
            project_id: self.project,
            subject_kind: "render".into(),
            subject_id: subject.into(),
            cache_key: cache_key_for(subject, None),
            scratch_root: self.scratch.join(subject),
            max_attempts: MAX_ATTEMPTS,
            at: at("2026-09-07T00:00:00Z"),
        }
    }

    fn submit(&self, subject: &str) -> Job {
        self.s().submit_job(&self.new_job(subject)).expect("submit")
    }
}

/// A cache key over a fixed input set. Derived from literals rather than a
/// clock, so the key is stable across runs and machines.
fn cache_key_for(subject: &str, revision: Option<u32>) -> CacheKey {
    let assets = [SourceAssetId::from_uuid(uuid::Uuid::from_u128(1))];
    let transcripts: [TranscriptRevisionId; 0] = [];
    let composition = revision
        .map(|n| CompositionRevisionId::from_uuid(uuid::Uuid::from_u128(u128::from(n) + 1_000)));
    CacheKey::of(&JobInputs {
        subject_kind: "render",
        subject_id: subject,
        assets: &assets,
        transcript_revisions: &transcripts,
        composition_revision: composition,
        settings: "1080p/h264",
        tool_versions: &[("ffmpeg", "7.1")],
    })
}

/// A real file on disk whose bytes the artifact row will claim. The content
/// hash is computed from the bytes actually written, so a test can only
/// truncate it by really truncating it.
fn write_output(dir: &Path, name: &str, bytes: &[u8]) -> (PathBuf, String, i64) {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("mkdir");
    }
    std::fs::write(&path, bytes).expect("write output");
    let hash = sha256_hex(bytes);
    (path, hash, bytes.len() as i64)
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut d = Sha256::new();
    d.update(bytes);
    d.finalize().iter().fold(String::new(), |mut hex, b| {
        use std::fmt::Write as _;
        let _ = write!(hex, "{b:02x}");
        hex
    })
}

fn artifact_row(project: ProjectId, path: &Path, hash: &str, bytes: i64) -> Artifact {
    Artifact {
        id: ArtifactId::generate(),
        project_id: project,
        variant_id: None,
        render_plan_id: None,
        kind: "render".into(),
        content_hash: hash.into(),
        locator: Some(path.to_path_buf()),
        bytes: Some(bytes as u64),
        width: Some(1920),
        height: Some(1080),
        duration: Some(dur(1_000)),
        created_at: at("2026-09-07T00:00:00Z"),
    }
}

fn move_to(store: &Store, job: JobId, to: JobState, reason: &'static str) -> Job {
    store
        .move_job(&MoveJob {
            job,
            to,
            reason,
            error: None,
            at: "2026-09-07T00:00:01Z",
        })
        .expect("move")
}

// ---------------------------------------------------------------------------
// Claim 1: restart-safe
// ---------------------------------------------------------------------------

#[test]
fn restart_resumes_an_interrupted_job() {
    // Given a job that was executing when the process died.
    let mut f = Fixture::new();
    let job = f.submit("rec-a");
    let running = move_to(f.s(), job.id, JobState::Running, "run");
    assert_eq!(running.state, JobState::Running);

    // When the process dies and a new one opens the same database.
    f.kill();
    let reopened = f.restart();
    let recovered = reopened
        .recover_jobs("2026-09-07T00:01:00Z")
        .expect("recover");

    // Then the job's state CHANGED — it is queued again, not merely still
    // present. A job that survived the crash *as running* is the defect this
    // test exists to catch, and it is a state no reader may trust.
    assert_eq!(recovered, 1, "exactly the one interrupted job is re-queued");
    let after = reopened.job(job.id).expect("read").expect("job exists");
    assert_eq!(
        after.state,
        JobState::Pending,
        "an interrupted job must come back queued, never still 'running'"
    );

    // And the change is recorded, so a reader can tell recovery from a job
    // that merely never started.
    let log = reopened.transitions(job.id).expect("log");
    let recovery = log.last().expect("a recovery row");
    assert_eq!(recovery.to, JobState::Pending);
    assert_eq!(recovery.from, Some(JobState::Running));
    assert_eq!(recovery.reason, "recover", "the log must say why it moved");
}

#[test]
fn recovery_is_idempotent() {
    // Given a job recovered once already.
    let mut f = Fixture::new();
    let job = f.submit("rec-a");
    move_to(f.s(), job.id, JobState::Running, "run");
    f.kill();
    let reopened = f.restart();
    assert_eq!(reopened.recover_jobs("t1").expect("first"), 1);

    // When recovery runs again — a daemon that opens, recovers and re-opens.
    let again = reopened.recover_jobs("t2").expect("second");

    // Then there is nothing left to recover and the log did not grow: a
    // second pass must not invent a second `pending -> pending` row.
    assert_eq!(again, 0, "a job already queued is not re-recovered");
    let log = reopened.transitions(job.id).expect("log");
    assert_eq!(
        log.iter().filter(|t| t.reason == "recover").count(),
        1,
        "recovery is recorded once, not once per open"
    );
}

#[test]
fn recovery_leaves_every_settled_state_alone() {
    // Given one job in each state a reader is entitled to trust. Each is
    // reached through the transition table's own edges, because a state
    // reachable only by breaking that table is not a state this machine
    // produces.
    let mut f = Fixture::new();
    let mut settled: Vec<(JobId, JobState)> = Vec::new();

    // pending: submitted and left alone.
    let job = f.submit("settled-pending");
    settled.push((job.id, JobState::Pending));

    // skipped: an earlier stage failed, so this one never runs.
    let job = f.submit("settled-skipped");
    move_to(f.s(), job.id, JobState::Skipped, "skip");
    settled.push((job.id, JobState::Skipped));

    // cancelled: asked to stop before it ran.
    let job = f.submit("settled-cancelled");
    move_to(f.s(), job.id, JobState::Cancelled, "cancel");
    settled.push((job.id, JobState::Cancelled));

    // failed: one attempt failed, retries remain.
    let job = f.submit("settled-failed");
    move_to(f.s(), job.id, JobState::Running, "run");
    f.s().fail_job(job.id, "boom", "t1").expect("fail");
    settled.push((job.id, JobState::Failed));

    // exhausted: the retry budget was spent.
    let mut spec = f.new_job("settled-exhausted");
    spec.id = JobId::generate();
    spec.cache_key = cache_key_for("settled-exhausted", Some(9));
    spec.max_attempts = 1;
    let job = f.s().submit_job(&spec).expect("submit");
    move_to(f.s(), job.id, JobState::Running, "run");
    f.s().fail_job(job.id, "boom", "t1").expect("fail");
    settled.push((job.id, JobState::Exhausted));

    // When recovery runs.
    f.kill();
    let reopened = f.restart();
    let count = reopened.recover_jobs("t1").expect("recover");

    // Then nothing moved: recovery re-queues only what a crash froze.
    assert_eq!(count, 0, "no settled job is re-queued");
    for (id, want) in settled {
        let job = reopened.job(id).expect("read").expect("exists");
        assert_eq!(job.state, want, "job {id} must keep its settled state");
    }
}

#[test]
fn a_job_whose_record_outlives_its_process_resolves_to_a_defined_state() {
    // Given a job row left `running` by a process that is definitively gone —
    // the "abandoned record" case, where nothing will ever call recover for it
    // because no other caller knows the job existed.
    let mut f = Fixture::new();
    let job = f.submit("orphan");
    move_to(f.s(), job.id, JobState::Running, "run");
    f.kill();

    // When any later open happens.
    let reopened = f.restart();
    reopened.recover_jobs("t1").expect("recover");

    // Then it resolves to a state the machine defines and can leave again —
    // `pending`, which has outgoing edges. It is not stranded in `running`.
    let after = reopened.job(job.id).expect("read").expect("exists");
    assert!(!after.is_final());
    assert!(
        JobState::ALL.contains(&after.state),
        "the resolved state is one the machine defines"
    );
    assert!(
        TRANSITIONS.iter().any(|(from, _)| *from == after.state),
        "the resolved state still has a way out"
    );
}

// ---------------------------------------------------------------------------
// Claim 2: one lifecycle
// ---------------------------------------------------------------------------

#[test]
fn every_state_appears_in_the_migration_check() {
    // Given the states this build defines.
    let names: Vec<&str> = JobState::ALL.iter().map(|s| s.as_str()).collect();

    // When compared against the constraint the migration actually wrote.
    let sql = include_str!("../schema_v3.sql");

    // Then every one of them is spelled in the schema's CHECK list, so a new
    // state cannot be added to the enum without the database being able to
    // store it — the two halves of "one lifecycle" cannot drift apart.
    for name in &names {
        assert!(
            sql.contains(&format!("'{name}'")),
            "JobState::{name} is not in schema_v3.sql's state CHECK list; a state the \
             database cannot hold is a second lifecycle in waiting"
        );
    }
    // And the reverse: the schema invents no state the machine lacks, which
    // would be a value that parses to nothing and reads as a wrong row.
    for state in [
        "pending",
        "running",
        "done",
        "failed",
        "exhausted",
        "cancelled",
        "skipped",
    ] {
        assert!(
            names.contains(&state),
            "the schema defines '{state}' but JobState does not"
        );
    }
}

#[test]
fn only_done_has_no_way_out() {
    // Given every legal edge.
    for state in JobState::ALL {
        let outgoing = TRANSITIONS
            .iter()
            .filter(|(from, _)| *from == state)
            .count();

        // Then `done` is the only terminal state, which is what makes
        // `is_final` mean one thing rather than a convention.
        if state == JobState::Done {
            assert_eq!(outgoing, 0, "a completed job must not be re-entered");
            assert!(state.is_final());
        } else {
            assert!(outgoing > 0, "{state} is a dead end nothing leaves");
            assert!(!state.is_final(), "{state} claims to be final but is not");
        }
    }
}

#[test]
fn an_illegal_transition_is_refused() {
    // Given a job that has already completed.
    let f = Fixture::new();
    let job = f.submit("rec-a");
    move_to(f.s(), job.id, JobState::Running, "run");
    let (path, hash, bytes) = write_output(&f.scratch, "out.mkv", b"done already");
    let artifact = artifact_row(f.project, &path, &hash, bytes);
    f.s().insert_artifact(&artifact).expect("artifact");
    let done = f.s().complete(job.id, artifact.id, "t1").expect("complete");
    assert_eq!(done.state, JobState::Done);

    // When something tries to move it back to running.
    let attempt = f.s().move_job(&MoveJob {
        job: job.id,
        to: JobState::Running,
        reason: "run",
        error: None,
        at: "t2",
    });

    // Then the transition table refuses it, so a completed job cannot be
    // silently reopened by a stale worker.
    assert!(
        attempt.is_err(),
        "done -> running is not a legal edge and must not be writable"
    );
    let after = f.s().job(job.id).expect("read").expect("exists");
    assert_eq!(after.state, JobState::Done, "the state is unchanged");
}

// ---------------------------------------------------------------------------
// Isolated scratch, immutable outputs
// ---------------------------------------------------------------------------

#[test]
fn a_database_that_never_reached_the_latest_version_rolls_back_cleanly() {
    // A version 2 database is the normal state of a file written by a build
    // that predates the job lifecycle, opened by one that carries version 3.
    // The migration list is the *caller's*, and it is longer than what this
    // file went through.
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("creator.db");
    let mut conn = rusqlite::Connection::open(&path).expect("conn");
    schema::migrate(&mut conn, &MIGRATIONS[..2]).expect("migrate to version 2");
    assert_eq!(
        schema::applied_version(&conn).expect("version"),
        2,
        "the fixture really is a version 2 database"
    );
    assert!(
        !table_exists(&conn, "creator_jobs"),
        "and it really has no job table yet"
    );

    // When it is rolled back with the full list, which includes a version 3
    // this database never applied.
    let rolled = schema::rollback(&mut conn, 1, MIGRATIONS).expect("the down applies");

    // Then only the version it actually reached is reversed. Running version
    // 3's `down` first used to fail on `DROP COLUMN current`, a column
    // version 3 never added, and left the rollback dead.
    assert_eq!(rolled, 1, "and the version line says so");
    assert_eq!(schema::applied_version(&conn).expect("version"), 1);
}

fn table_exists(conn: &rusqlite::Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
        [name],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n > 0)
    .unwrap_or(false)
}

#[test]
fn two_attempts_of_one_job_get_separate_scratch() {
    // Given one job with a prepared first attempt.
    let f = Fixture::new();
    let job = f.submit("rec-a");
    let first = f.s().prepare_attempt(&job, 0).expect("attempt 0");
    std::fs::write(first.join("cut-0.mkv"), b"first").expect("write");
    let second = f.s().prepare_attempt(&job, 1).expect("attempt 1");
    std::fs::write(second.join("cut-0.mkv"), b"second").expect("write");

    // When one attempt is released.
    f.s().release_attempt(&job, 0).expect("release");

    // Then the other's file survives with its own bytes — releasing an attempt
    // cleans up that attempt only.
    assert!(!first.exists(), "the released attempt is gone");
    assert!(second.exists(), "the live attempt's directory is untouched");
    assert_eq!(
        std::fs::read(second.join("cut-0.mkv")).expect("read"),
        b"second",
        "the live attempt's bytes are its own"
    );
}

#[test]
fn two_jobs_on_one_recording_get_separate_scratch() {
    // Given two jobs for the same recording under *different* inputs — a
    // second render of one recording, which is a different composition
    // revision and therefore a different job. (Two jobs on identical inputs
    // are refused outright by the one-unsettled-per-key index, and that is its
    // own test.)
    let f = Fixture::new();
    let mut first = f.new_job("same-recording");
    first.cache_key = cache_key_for("same-recording", Some(1));
    let a = f.s().submit_job(&first).expect("submit a");
    let mut second = f.new_job("same-recording");
    second.cache_key = cache_key_for("same-recording", Some(2));
    let b = f.s().submit_job(&second).expect("submit b");
    let da = f.s().prepare_attempt(&a, 0).expect("a");
    let db = f.s().prepare_attempt(&b, 0).expect("b");

    // Then the directories differ even at the same attempt number, so a
    // cleanup of one cannot empty the other's working set.
    assert_ne!(da, db, "a job id is part of the scratch path");
    f.s().release_attempt(&a, 0).expect("release a");
    assert!(
        db.exists(),
        "releasing job A must not remove job B's scratch"
    );
}

#[test]
fn output_locations_are_immutable_per_attempt() {
    // Given one job and its scratch root.
    let f = Fixture::new();
    let job = f.submit("rec-a");

    // When two attempts produce output.
    let first = immutable_output_path(&f.scratch, &job.id, 0);
    let second = immutable_output_path(&f.scratch, &job.id, 1);

    // Then the locations differ, so a retry cannot overwrite the output of the
    // attempt before it — a new render writes a new artifact, never a
    // replacement.
    assert_ne!(first, second, "each attempt owns its output location");
    assert!(
        first
            .to_string_lossy()
            .contains(job.id.to_string().as_str()),
        "the job id is in the path, so no two jobs share one"
    );
}

// ---------------------------------------------------------------------------
// Cache keys: semantic invalidation, not size/mtime
// ---------------------------------------------------------------------------

#[test]
fn a_changed_composition_revision_invalidates_the_key() {
    // Given one key per revision of the same composition.
    let r1 = cache_key_for("rec-a", Some(1));
    let r2 = cache_key_for("rec-a", Some(2));

    // Then they differ, so revision 2 cannot reuse revision 1's output.
    assert_ne!(r1, r2, "a new composition revision is a new job");
    assert_eq!(r1, cache_key_for("rec-a", Some(1)), "the key is stable");
}

#[test]
fn identical_inputs_reuse_the_earlier_output() {
    // Given a job that completed and produced a validated artifact.
    let f = Fixture::new();
    let key = cache_key_for("rec-a", Some(1));
    let (path, hash, bytes) = write_output(&f.scratch, "out.mkv", b"rendered bytes");
    let artifact = artifact_row(f.project, &path, &hash, bytes);
    f.s().insert_artifact(&artifact).expect("artifact");
    f.s()
        .validate_artifact(artifact.id, "t1")
        .expect("validate");

    let mut spec = f.new_job("rec-a");
    spec.id = JobId::generate();
    spec.cache_key = key.clone();
    f.s().submit_job(&spec).expect("submit");
    let job = f.s().job(spec.id).expect("read").expect("exists");
    move_to(f.s(), job.id, JobState::Running, "run");
    f.s().complete(job.id, artifact.id, "t1").expect("complete");

    // When the same inputs are submitted again.
    let found = f.s().reusable(&key).expect("lookup");

    // Then the earlier artifact is offered, so an unchanged set of inputs
    // does not render again.
    assert_eq!(found, Some(artifact.id));
}

#[test]
fn a_completed_job_whose_artifact_does_not_validate_is_not_reusable() {
    // Given a job marked done whose artifact was never validated — the case a
    // "job complete" flag alone would wrongly serve.
    let f = Fixture::new();
    let key = cache_key_for("rec-a", None);
    let (path, hash, bytes) = write_output(&f.scratch, "out.mkv", b"rendered bytes");
    let artifact = artifact_row(f.project, &path, &hash, bytes);
    f.s().insert_artifact(&artifact).expect("artifact");

    let mut spec = f.new_job("rec-a");
    spec.cache_key = key.clone();
    f.s().submit_job(&spec).expect("submit");
    move_to(f.s(), spec.id, JobState::Running, "run");
    f.s()
        .complete(spec.id, artifact.id, "t1")
        .expect("complete");

    // When the cache is consulted.
    let found = f.s().reusable(&key).expect("lookup");

    // Then nothing is offered: completion is not validation.
    assert_eq!(found, None, "an unvalidated artifact is not a cache hit");
}

#[test]
fn two_unsettled_jobs_may_not_share_a_cache_key() {
    // Given one job already queued on a key.
    let f = Fixture::new();
    let mut spec = f.new_job("rec-a");
    spec.cache_key = cache_key_for("dup", None);
    f.s().submit_job(&spec).expect("submit");

    // When a second job is submitted on the same key.
    let mut second = f.new_job("rec-a");
    second.cache_key = spec.cache_key.clone();
    let result = f.s().submit_job(&second);

    // Then it is refused: the same inputs must not be rendered twice at once.
    assert!(result.is_err(), "one unsettled job per cache key");
}

// ---------------------------------------------------------------------------
// Validated output before completion
// ---------------------------------------------------------------------------

#[test]
fn a_truncated_output_is_never_marked_complete() {
    // Given an artifact whose row records the full size and content hash.
    let f = Fixture::new();
    let (path, hash, bytes) = write_output(&f.scratch, "out.mkv", b"the whole render");
    let artifact = artifact_row(f.project, &path, &hash, bytes);
    f.s().insert_artifact(&artifact).expect("artifact");
    let job = f.submit("rec-a");
    move_to(f.s(), job.id, JobState::Running, "run");

    // When the render is truncated on disk — the byte count no longer matches.
    std::fs::write(&path, b"the whole ren").expect("truncate");

    // Then validation refuses it, so the job cannot be completed on it.
    let verdict = f.s().validate_artifact(artifact.id, "t1");
    assert!(
        verdict.is_err(),
        "a truncated render must not validate, so no job can point at it"
    );
    let msg = verdict.unwrap_err().to_string();
    assert!(
        msg.contains("truncated"),
        "the refusal names the operator-facing fact: {msg}"
    );
}

#[test]
fn an_empty_output_is_never_marked_complete() {
    // Given a zero-byte render.
    let f = Fixture::new();
    let (path, hash, bytes) = write_output(&f.scratch, "out.mkv", b"");
    let artifact = artifact_row(f.project, &path, &hash, bytes);
    f.s().insert_artifact(&artifact).expect("artifact");

    // When it is validated.
    let verdict = f.s().validate_artifact(artifact.id, "t1");

    // Then it is refused as empty, which is a different problem from a
    // truncation and is reported as one.
    assert!(verdict.is_err());
    assert!(verdict.unwrap_err().to_string().contains("empty"));
}

#[test]
fn a_corrupted_output_is_never_marked_complete() {
    // Given an artifact of the right length but different bytes.
    let f = Fixture::new();
    let (path, hash, bytes) = write_output(&f.scratch, "out.mkv", b"the whole render");
    let artifact = artifact_row(f.project, &path, &hash, bytes);
    f.s().insert_artifact(&artifact).expect("artifact");
    std::fs::write(&path, b"a whole render!").expect("rewrite same length");

    // When it is validated.
    let verdict = f.s().validate_artifact(artifact.id, "t1");

    // Then the content hash catches it even though the size check passed.
    assert!(verdict.is_err(), "length alone is not identity");
}

#[test]
fn a_missing_output_is_never_marked_complete() {
    // Given an artifact whose file is not there.
    let f = Fixture::new();
    let (path, hash, bytes) = write_output(&f.scratch, "out.mkv", b"rendered");
    let artifact = artifact_row(f.project, &path, &hash, bytes);
    f.s().insert_artifact(&artifact).expect("artifact");
    std::fs::remove_file(&path).expect("remove");

    // When it is validated.
    let verdict = f.s().validate_artifact(artifact.id, "t1");

    // Then it is refused as unreadable, not reported as a content mismatch.
    assert!(verdict.is_err());
    assert!(verdict.unwrap_err().to_string().contains("not readable"));
}

#[test]
fn a_validated_output_becomes_reusable_and_completion_is_separate() {
    // Given a completed job whose artifact validates.
    let f = Fixture::new();
    let (path, hash, bytes) = write_output(&f.scratch, "out.mkv", b"final bytes");
    let artifact = artifact_row(f.project, &path, &hash, bytes);
    f.s().insert_artifact(&artifact).expect("artifact");
    f.s()
        .validate_artifact(artifact.id, "t1")
        .expect("validate");
    let job = f.submit("rec-a");
    move_to(f.s(), job.id, JobState::Running, "run");
    f.s().complete(job.id, artifact.id, "t1").expect("complete");

    // When it is merely completed.
    let current = f
        .s()
        .current_artifact(f.project, "render")
        .expect("read current");

    // Then nothing is current yet: completing a render makes an output
    // available, it does not switch the project over to it.
    assert_eq!(current, None, "completion is not promotion");
}

#[test]
fn make_current_promotes_exactly_one_artifact() {
    // Given two validated artifacts of the same kind for one project.
    let f = Fixture::new();
    let (p1, h1, b1) = write_output(&f.scratch, "a.mkv", b"first render");
    let (p2, h2, b2) = write_output(&f.scratch, "b.mkv", b"second render");
    let a1 = artifact_row(f.project, &p1, &h1, b1);
    let a2 = artifact_row(f.project, &p2, &h2, b2);
    for a in [&a1, &a2] {
        f.s().insert_artifact(a).expect("artifact");
        f.s().validate_artifact(a.id, "t1").expect("validate");
    }

    // When the first is promoted, then the second.
    f.s().make_current(a1.id).expect("promote 1");
    assert_eq!(
        f.s().current_artifact(f.project, "render").expect("read"),
        Some(a1.id)
    );
    f.s().make_current(a2.id).expect("promote 2");

    // Then only the second is current — promoting demotes, so the project's
    // current output is never ambiguous.
    assert_eq!(
        f.s().current_artifact(f.project, "render").expect("read"),
        Some(a2.id)
    );
}

// ---------------------------------------------------------------------------
// Malformed input: none of it may resolve to a wrong row
// ---------------------------------------------------------------------------

#[test]
fn the_schema_refuses_to_store_an_undefined_state() {
    // The CHECK constraint is the first line of defence, and this test exists
    // to prove it is not a vacuous one: a state the machine does not define
    // must not be writable at all.
    let f = Fixture::new();
    let job = f.submit("rec-a");
    let planted = f.s().raw_execute(
        "UPDATE creator_jobs SET state='halfway' WHERE id=?1",
        &[job.id.to_string()],
    );

    // Then the database rejects it outright.
    assert!(
        planted.is_err(),
        "the CHECK list must refuse a state JobState does not define"
    );
    let msg = planted.unwrap_err().to_string();
    assert!(msg.contains("CHECK"), "it is the CHECK that refuses: {msg}");
}

#[test]
fn an_undefined_state_value_is_refused_not_defaulted() {
    // The CHECK constraint makes the corrupt value unwritable through this
    // schema, so reproducing it needs a database that lacks the constraint —
    // which is what a file written by a different build, or a hand-edited
    // one, looks like to the reader. Without this test the strict reader
    // would be untested, and a "lenient" one that mapped it to `pending`
    // would hand a caller a job in a state its history never recorded.
    let conn = rusqlite::Connection::open_in_memory().expect("conn");
    conn.execute_batch(
        "CREATE TABLE creator_jobs (
           id TEXT PRIMARY KEY, project_id TEXT NOT NULL, subject_kind TEXT NOT NULL,
           subject_id TEXT NOT NULL, cache_key TEXT NOT NULL, state TEXT NOT NULL,
           attempt INTEGER NOT NULL, max_attempts INTEGER NOT NULL, scratch_root TEXT NOT NULL,
           last_error TEXT, artifact_id TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
         CREATE TABLE creator_job_transitions (
           job_id TEXT NOT NULL, seq INTEGER NOT NULL, from_state TEXT, to_state TEXT NOT NULL,
           attempt INTEGER NOT NULL, error TEXT, reason TEXT NOT NULL, recorded_at TEXT NOT NULL);",
    )
    .expect("schema without the CHECK");
    let id = JobId::generate();
    let project = ProjectId::generate();
    conn.execute(
        "INSERT INTO creator_jobs VALUES (?1,?2,'render','rec-a',?3,'halfway',0,3,'',NULL,NULL,'t','t')",
        rusqlite::params![id.to_string(), project.to_string(), "0".repeat(64)],
    )
    .expect("a state this schema would have refused");
    conn.execute(
        "INSERT INTO creator_job_transitions VALUES (?1,0,NULL,'halfway',0,NULL,'submit','t')",
        [id.to_string()],
    )
    .expect("log row");
    let store = Store::from_connection(conn);

    // When the corrupt row is read back.
    let job = store.job(id);
    let log = store.transitions(id);

    // Then both are errors, not rows. Defaulting to `pending` — as the code
    // did before this test existed — is the one wrong answer available here.
    assert!(
        job.is_err(),
        "an unknown job state must not resolve to a row"
    );
    assert!(
        log.is_err(),
        "an unknown state in the log must not be read as some other state"
    );
}

#[test]
fn a_duplicate_job_id_is_refused() {
    // Given a job already submitted.
    let f = Fixture::new();
    let spec = f.new_job("rec-a");
    f.s().submit_job(&spec).expect("submit");

    // When the same id is submitted again.
    let again = f.s().submit_job(&spec);

    // Then it is refused rather than shadowing the first.
    assert!(again.is_err(), "two jobs cannot share one id");
    assert_eq!(f.s().jobs(f.project).expect("list").len(), 1);
}

#[test]
fn a_job_naming_an_unknown_artifact_is_refused() {
    // Given a running job and an artifact id that does not exist.
    let f = Fixture::new();
    let job = f.submit("rec-a");
    move_to(f.s(), job.id, JobState::Running, "run");

    // When it is completed against that id.
    let result = f.s().complete(job.id, ArtifactId::generate(), "t1");

    // Then the foreign key refuses it, so a job cannot claim an artifact that
    // is not there — the "one entity's data leaking into another's" shape.
    assert!(result.is_err(), "completion requires a real artifact");
    let after = f.s().job(job.id).expect("read").expect("exists");
    assert_eq!(after.state, JobState::Running, "the job did not complete");
}

#[test]
fn a_job_with_no_retry_budget_is_refused() {
    // Given a job that may not be retried.
    let f = Fixture::new();
    let mut spec = f.new_job("rec-a");
    spec.max_attempts = 0;

    // When it is submitted.
    let result = f.s().submit_job(&spec);

    // Then it is refused: a job that can never fail usefully is a job whose
    // `failed` state is unreachable, which is a broken state machine.
    assert!(result.is_err());
}

#[test]
fn moving_an_absent_job_is_an_error() {
    // Given no such job.
    let f = Fixture::new();

    // When it is moved.
    let result = f.s().move_job(&MoveJob {
        job: JobId::generate(),
        to: JobState::Running,
        reason: "run",
        error: None,
        at: "t1",
    });

    // Then it is an error, not a silent insert.
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// The retry budget and the log
// ---------------------------------------------------------------------------

#[test]
fn the_attempt_budget_ends_in_exhausted_and_a_reset_restores_it() {
    // Given a job with a budget of two attempts.
    let f = Fixture::new();
    let mut spec = f.new_job("rec-a");
    spec.max_attempts = 2;
    f.s().submit_job(&spec).expect("submit");

    // When both attempts fail.
    move_to(f.s(), spec.id, JobState::Running, "run");
    let first = f.s().fail_job(spec.id, "boom 1", "t1").expect("fail 1");
    assert_eq!(first.state, JobState::Failed, "retries remain after one");
    assert_eq!(first.attempt, 1);
    move_to(f.s(), spec.id, JobState::Running, "run");
    let second = f.s().fail_job(spec.id, "boom 2", "t2").expect("fail 2");
    assert_eq!(second.state, JobState::Exhausted, "the budget is spent");
    assert_eq!(second.attempt, 2);

    // And then it is reset explicitly.
    let reset = f.s().retry_job(spec.id, "t3").expect("retry");
    assert_eq!(reset.state, JobState::Pending);
    assert_eq!(reset.attempt, 0, "a manual reset restores the full budget");
    assert!(reset.last_error.is_none(), "the stale error is cleared");
}

#[test]
fn the_log_records_every_transition_in_order() {
    // Given a job driven through several states.
    let f = Fixture::new();
    let job = f.submit("rec-a");
    move_to(f.s(), job.id, JobState::Running, "run");
    f.s().fail_job(job.id, "boom", "t1").expect("fail");
    let (path, hash, bytes) = write_output(&f.scratch, "out.mkv", b"second try");
    let artifact = artifact_row(f.project, &path, &hash, bytes);
    f.s().insert_artifact(&artifact).expect("artifact");
    move_to(f.s(), job.id, JobState::Running, "run");
    f.s().complete(job.id, artifact.id, "t2").expect("complete");

    // When the log is read.
    let log = f.s().transitions(job.id).expect("log");

    // Then it is a contiguous, ordered account starting from creation, so
    // "what did this job do" never needs a second source. Five writes happened:
    // submit, run, fail, run, complete.
    let seqs: Vec<i64> = log.iter().map(|t| t.seq).collect();
    assert_eq!(seqs, vec![0, 1, 2, 3, 4], "seq is contiguous from 0");
    assert_eq!(log[0].from, None, "creation has no prior state");
    assert_eq!(log[0].reason, "submit");
    assert_eq!(log[0].to, JobState::Pending);
    let chain: Vec<Option<JobState>> = log.iter().map(|t| t.from).collect();
    let tos: Vec<JobState> = log.iter().map(|t| t.to).collect();
    for i in 1..log.len() {
        assert_eq!(
            chain[i],
            Some(tos[i - 1]),
            "each row's prior state is the previous row's state"
        );
    }
    assert!(log.iter().any(|t| t.error.as_deref() == Some("boom")));
}

#[test]
fn the_artifact_a_job_points_at_is_the_one_it_completed_with() {
    // Given a completed job.
    let f = Fixture::new();
    let (path, hash, bytes) = write_output(&f.scratch, "out.mkv", b"final");
    let artifact = artifact_row(f.project, &path, &hash, bytes);
    f.s().insert_artifact(&artifact).expect("artifact");
    let job = f.submit("rec-a");
    move_to(f.s(), job.id, JobState::Running, "run");
    let done = f.s().complete(job.id, artifact.id, "t1").expect("complete");

    // Then the job carries it, which is what makes the job the unit of record
    // and the artifact merely its output.
    assert_eq!(done.artifact_id, Some(artifact.id));
    assert_eq!(done.state, JobState::Done);
    assert!(done.is_final());
}

#[test]
fn jobs_are_listed_for_a_project_and_scoped_to_it() {
    // Given two projects' worth of jobs.
    let f = Fixture::new();
    let other = project("other");
    f.s().insert_project(&other).expect("project");
    f.submit("mine");
    let mut foreign = f.new_job("theirs");
    foreign.project_id = other.id;
    f.s().submit_job(&foreign).expect("submit");

    // When they are listed.
    let mine = f.s().jobs(f.project).expect("list");
    let theirs = f.s().jobs(other.id).expect("list");

    // Then each project sees only its own — a job id from another project can
    // never be returned by a project's listing.
    assert_eq!(mine.len(), 1);
    assert_eq!(theirs.len(), 1);
    assert_ne!(mine[0].id, theirs[0].id);
    assert_eq!(mine[0].project_id, f.project);
    assert_eq!(theirs[0].project_id, other.id);
}
