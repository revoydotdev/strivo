//! The job and artifact lifecycle: one durable state machine, shared by
//! creator operations and by the artifacts they produce.
//!
//! **The state names are the daemon's, not new ones.**
//! `strivo-research` sits below `strivo-core` in the dependency graph — the
//! daemon depends on this crate, never the reverse — so this module cannot
//! `use` `strivo_core::pipeline::stage::StageState`. The alternative was a
//! second vocabulary, which
//! `docs/CREATOR-EDITION-REPORT-2026-09-07.md:204` forbids in as many words.
//! So the states here are the *durable encoding* of that machine, and
//! `crates/strivo-web/src/creator_schema.rs` carries the test that drives the
//! real `PipelineRegistry` and asserts the two agree on every (state,
//! operation) pair. Adding a variant to one without the other fails the
//! Creator build; that test is the single point of contact, and it is why this
//! is reuse rather than duplication.
//!
//! **The retry ladder, the resource locks and the cancellation token stay in
//! the daemon.** `Stage::backoff_after`, `ResourceRegistry` and
//! `CancellationToken` are not reimplemented here, because a second copy of
//! the backoff ladder is a second answer to "when do we try again". This
//! module persists the *state*; the runtime schedules it.
//!
//! What the pipeline registry does not have, and what this module adds, is
//! durability with a history: a transition is an append-only row written in
//! the same transaction as the state change, so a job can never be observed
//! in a state its own log does not record, and an interrupted job is
//! re-queued on the next open by [`Store::recover_jobs`](super::store_jobs) —
//! the same rule `PipelineRegistry::open` applies to an interrupted stage.

use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::ids::*;
use crate::{ResearchError, Result};

/// Default retry budget, the same value `Stage::new` installs. Asserted
/// against a real `Stage` in the edition test, so this cannot drift from the
/// daemon's without that test failing.
pub const MAX_ATTEMPTS: u8 = 3;

/// The seven states of a creator job. Each is `StageState`'s counterpart;
/// see the module doc for why they are re-declared rather than imported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    /// Accepted, not started.
    Pending,
    /// An attempt is executing. **Not durable across a crash**: process death
    /// proves nothing is running, so the next open re-queues it.
    Running,
    /// Finished, and its output validated. The only state with no way out.
    Done,
    /// An attempt failed; retries remain. Carries `attempt`, not a fresh
    /// counter — the counter is the job's, so a reader cannot see two
    /// attempts disagree about which try this was.
    Failed,
    /// Retries are spent. A reset is the only way out, and it is explicit.
    Exhausted,
    /// Asked to stop. Resettable, never resumed implicitly.
    Cancelled,
    /// Not run because an input failed. Resettable, never resumed implicitly.
    Skipped,
}

impl JobState {
    /// Every state, in the order the transition table's rows are grouped.
    pub const ALL: [JobState; 7] = [
        JobState::Pending,
        JobState::Running,
        JobState::Done,
        JobState::Failed,
        JobState::Exhausted,
        JobState::Cancelled,
        JobState::Skipped,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Exhausted => "exhausted",
            Self::Cancelled => "cancelled",
            Self::Skipped => "skipped",
        }
    }

    /// Whether the scheduler still waits on this state. Mirrors
    /// `Stage::is_terminal`, and deliberately not the same question as
    /// [`JobState::is_final`]: a stage the pipeline stops waiting for can
    /// still be reset.
    #[must_use]
    pub const fn is_settled(self) -> bool {
        matches!(
            self,
            Self::Done | Self::Exhausted | Self::Cancelled | Self::Skipped
        )
    }

    /// Whether anything can move this state again. Only `done` cannot, and
    /// that is the invariant the transition table exists to keep: a job
    /// reaches a state nothing leaves only by passing through the states in
    /// between.
    #[must_use]
    pub const fn is_final(self) -> bool {
        matches!(self, Self::Done)
    }
}

impl fmt::Display for JobState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for JobState {
    type Err = ResearchError;

    fn from_str(s: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|state| state.as_str() == s)
            .ok_or_else(|| ResearchError::Validation(format!("not a job state: {s}")))
    }
}

/// The legal edges, read off the executor's own guards rather than invented
/// here. Every row names the `PipelineRegistry` method that admits it, so a
/// reviewer can check the claim rather than trust it — and the edition test
/// drives those methods and compares, so a wrong row is a failing test.
///
/// `Done` has no outgoing edge and is the only state with none, which is what
/// makes [`JobState::is_final`] mean something.
pub const TRANSITIONS: &[(JobState, JobState)] = &[
    // mark_stage_running / update_stage(Running)
    (JobState::Pending, JobState::Running),
    // cancel_pipeline
    (JobState::Pending, JobState::Cancelled),
    // update_stage(Skipped) / stop_siblings
    (JobState::Pending, JobState::Skipped),
    // complete_stage / mark_stage_done / update_stage(Done)
    (JobState::Running, JobState::Done),
    // mark_stage_failed, retries remaining / update_stage(Failed)
    (JobState::Running, JobState::Failed),
    // mark_stage_failed, retries spent / exhaust_stage
    (JobState::Running, JobState::Exhausted),
    // cancel_pipeline / update_stage(Cancelled)
    (JobState::Running, JobState::Cancelled),
    // PipelineRegistry::open: process death proves nothing is running
    (JobState::Running, JobState::Pending),
    // mark_stage_running admits a failed stage directly
    (JobState::Failed, JobState::Running),
    // retry_failed_stage (automatic) / retry_stage (manual)
    (JobState::Failed, JobState::Pending),
    // cancel_pipeline
    (JobState::Failed, JobState::Cancelled),
    // stop_siblings / update_stage(Skipped)
    (JobState::Failed, JobState::Skipped),
    // retry_stage resets a spent stage, and zeroes its attempt counter
    (JobState::Exhausted, JobState::Pending),
    // stop_siblings
    (JobState::Exhausted, JobState::Skipped),
    // retry_stage's sibling re-queue, when a spent sibling is retried
    (JobState::Cancelled, JobState::Pending),
    (JobState::Skipped, JobState::Pending),
];

/// Whether `from -> to` is one of the legal edges. The single answer every
/// writer consults, so an unlisted transition cannot be written by any of
/// them.
#[must_use]
pub fn is_legal(from: JobState, to: JobState) -> bool {
    TRANSITIONS.contains(&(from, to))
}

/// One row of `creator_job_transitions`, as read back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobTransition {
    pub job_id: JobId,
    pub seq: i64,
    pub from: Option<JobState>,
    pub to: JobState,
    pub attempt: u8,
    pub error: Option<String>,
    pub reason: String,
    pub recorded_at: String,
}

/// One row of `creator_jobs`, as read back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub id: JobId,
    pub project_id: ProjectId,
    pub subject_kind: String,
    pub subject_id: String,
    pub cache_key: CacheKey,
    pub state: JobState,
    /// Failures so far. Incremented by [`Store::fail_job`](super::store_jobs)
    /// and nowhere else, matching `PipelineRegistry::mark_stage_failed`.
    pub attempt: u8,
    pub max_attempts: u8,
    pub scratch_root: PathBuf,
    pub last_error: Option<String>,
    pub artifact_id: Option<ArtifactId>,
    pub created_at: String,
    pub updated_at: String,
}

impl Job {
    /// The isolated working directory for one attempt. Derived, never stored.
    ///
    /// The job id **is** in the path, not just the attempt number, so the
    /// isolation is structural rather than a promise the caller has to keep:
    /// two attempts on one job, and two jobs on one recording, cannot land in
    /// the same directory even when a caller hands both the same
    /// `scratch_root`. An earlier version of this derived only the attempt
    /// number, which made the guarantee a property of how carefully callers
    /// chose their scratch roots rather than of the path itself.
    #[must_use]
    pub fn attempt_dir(&self, attempt: u8) -> PathBuf {
        self.scratch_root
            .join(self.id.to_string())
            .join(format!("attempt-{attempt}"))
    }

    /// Whether any further transition is possible.
    #[must_use]
    pub fn is_final(&self) -> bool {
        self.state.is_final()
    }
}

/// A content-addressed identity for a job's inputs.
///
/// Computed over the declared inputs only — asset identities, transcript and
/// composition revisions, the selected settings, and the tool and model
/// versions — never over a file's size or mtime. Size and mtime are a valid
/// *optimisation* for spotting an unchanged file, and they are not semantic
/// invalidation for a versioned editor: a re-render that changes nothing on
/// disk is still a different job, and a file whose mtime moved is not
/// necessarily a different input.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CacheKey(String);

impl CacheKey {
    /// Hash the inputs in a canonical order. The orderings below (assets,
    /// transcript revisions, tool versions) are sorted before hashing because
    /// the *set* of inputs is the identity, not the order a caller happened
    /// to build it in.
    #[must_use]
    pub fn of(inputs: &JobInputs<'_>) -> Self {
        let mut digest = Sha256::new();
        digest.update(inputs.subject_kind.as_bytes());
        digest.update(b"\x1f");
        digest.update(inputs.subject_id.as_bytes());
        digest.update(b"\x1e");
        for id in sorted(inputs.assets) {
            digest.update(id.as_uuid().as_bytes());
            digest.update(b"\x1f");
        }
        digest.update(b"\x1e");
        for id in sorted(inputs.transcript_revisions) {
            digest.update(id.as_uuid().as_bytes());
            digest.update(b"\x1f");
        }
        digest.update(b"\x1e");
        if let Some(revision) = inputs.composition_revision {
            digest.update(revision.as_uuid().as_bytes());
        }
        digest.update(b"\x1e");
        digest.update(inputs.settings.as_bytes());
        digest.update(b"\x1e");
        let mut versions: Vec<&str> = inputs
            .tool_versions
            .iter()
            .flat_map(|(name, version)| [name, version])
            .copied()
            .collect();
        versions.sort_unstable();
        for part in versions {
            digest.update(part.as_bytes());
            digest.update(b"\x1f");
        }
        Self(
            digest
                .finalize()
                .iter()
                .fold(String::with_capacity(64), |mut hex, byte| {
                    hex.push_str(&format!("{byte:02x}"));
                    hex
                }),
        )
    }

    /// Adopt a key read back from the database. The one way a `CacheKey`
    /// enters the system without being computed from its inputs, so it is
    /// crate-internal: a caller cannot mint an identity by asserting a hash.
    pub(crate) fn from_raw(hex: String) -> Self {
        Self(hex)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CacheKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn sorted<T: Ord>(values: &[T]) -> Vec<&T> {
    let mut out: Vec<&T> = values.iter().collect();
    out.sort();
    out
}

/// Everything a job's identity is derived from. Grouped into one value
/// because these are not independent parameters: a cache key that could be
/// built from some of its inputs is a cache key that silently ignores the
/// rest.
#[derive(Debug, Clone, Copy)]
pub struct JobInputs<'a> {
    pub subject_kind: &'a str,
    pub subject_id: &'a str,
    /// Source asset **identities** (ids and fingerprints), never locators.
    pub assets: &'a [SourceAssetId],
    pub transcript_revisions: &'a [TranscriptRevisionId],
    pub composition_revision: Option<CompositionRevisionId>,
    /// The selected settings, already canonicalised by the caller.
    pub settings: &'a str,
    /// `(name, version)` for every tool and model the job invokes.
    pub tool_versions: &'a [(&'a str, &'a str)],
}

/// Where a completed job's output belongs. Immutable by construction: the path
/// is derived from the job id and the attempt that produced it, so a retry
/// cannot overwrite the output of the attempt before it, and no two jobs share
/// one.
#[must_use]
pub fn immutable_output_path(scratch_root: &Path, job: &JobId, attempt: u8) -> PathBuf {
    scratch_root
        .join("output")
        .join(format!("{job}-attempt-{attempt}"))
}
