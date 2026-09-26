-- Version 3: one durable job and artifact lifecycle.
--
-- `creator_jobs` is the DURABLE SPELLING of the pipeline stage state machine,
-- not a second machine. The seven state names below are exactly
-- `strivo_core::pipeline::stage::StageState`'s, the CHECK list is that enum's
-- variants, and `crates/strivo-web/src/creator_schema.rs` carries a test that
-- drives the real `PipelineRegistry` and asserts this table agrees with it on
-- every (state, operation) pair. `strivo-research` cannot `use` the daemon's
-- enum — the daemon depends on this crate, not the reverse — so the
-- correspondence is enforced by that test rather than by a `use`, which is the
-- only honest way to avoid a second vocabulary across a dependency cycle.
--
-- `creator_job_transitions` is the durability. Every state change writes its
-- log row in the SAME transaction, so a job can never be observed in a state
-- its own history does not record: "never lands in a terminal state it did
-- not pass through" is a property of the write, not of a reader's care.
-- `seq` is a per-job counter rather than a timestamp, because ordering must
-- not depend on the wall clock. `reason` names which operation moved it —
-- 'submit', 'run', 'complete', 'fail', 'retry', 'cancel', 'skip', 'recover' —
-- so an interrupted job says on reopen *why* it was re-queued.
--
-- `cache_key` is content-addressed over the job's declared inputs: source
-- asset identities, transcript and composition revisions, the selected
-- settings, and the tool and model versions. A changed composition revision
-- is therefore a different job, and an unchanged one reuses the earlier
-- output. The partial unique index allows only one UNSETTLED job per key, so
-- the same inputs cannot be rendered twice at once, while any number of
-- settled jobs may share a key — that is the cache.
--
-- `scratch_root` is per job and each attempt gets `attempt-<n>` beneath it, so
-- two concurrent attempts cannot overwrite or clean each other's working
-- directory. Outputs never land there: an output location is immutable and is
-- recorded on the artifact row.
--
-- On the artifact side, `validated_at` is when its bytes were checked, and
-- `current` is the SEPARATE "make current" action: completing a render makes
-- an output available, it does not switch the project over to it. The partial
-- unique index is the enforcement that at most one artifact per
-- (project, kind) is current, which a `UPDATE` in application code alone
-- could not promise.
--
-- `subject_id` is deliberately NOT a foreign key, for the same reason
-- `creator_project_research_links.research_project_id` is not: a job's
-- subject is polymorphic (a render of a variant, a transcript of a source, a
-- publish of a release package) and a key would have to name one of them,
-- silently redefining the other two.

CREATE TABLE IF NOT EXISTS creator_jobs (
 id TEXT PRIMARY KEY,
 project_id TEXT NOT NULL REFERENCES creator_projects(id) ON DELETE CASCADE,
 -- `render`, `transcribe`, `publish`, …
 subject_kind TEXT NOT NULL,
 subject_id TEXT NOT NULL,
 cache_key TEXT NOT NULL CHECK(length(cache_key) = 64),
 state TEXT NOT NULL DEFAULT 'pending'
   CHECK(state IN ('pending','running','done','failed','exhausted','cancelled','skipped')),
 -- Counts FAILURES, not entries into `running`: the daemon's
 -- `PipelineRegistry::mark_stage_failed` increments here and nowhere else, so
 -- the two counters cannot mean different things.
 attempt INTEGER NOT NULL DEFAULT 0 CHECK(attempt >= 0),
 max_attempts INTEGER NOT NULL DEFAULT 3 CHECK(max_attempts > 0),
 scratch_root TEXT NOT NULL,
 last_error TEXT,
 artifact_id TEXT REFERENCES creator_artifacts(id) ON DELETE SET NULL,
 created_at TEXT NOT NULL,
 updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_creator_jobs_project ON creator_jobs(project_id);
CREATE INDEX IF NOT EXISTS idx_creator_jobs_cache_key ON creator_jobs(cache_key);
CREATE UNIQUE INDEX IF NOT EXISTS idx_creator_jobs_one_unsettled
  ON creator_jobs(cache_key)
  WHERE state IN ('pending','running','failed');

CREATE TABLE IF NOT EXISTS creator_job_transitions (
 job_id TEXT NOT NULL REFERENCES creator_jobs(id) ON DELETE CASCADE,
 seq INTEGER NOT NULL CHECK(seq >= 0),
 from_state TEXT
   CHECK(from_state IS NULL OR
         from_state IN ('pending','running','done','failed','exhausted','cancelled','skipped')),
 to_state TEXT NOT NULL
   CHECK(to_state IN ('pending','running','done','failed','exhausted','cancelled','skipped')),
 attempt INTEGER NOT NULL CHECK(attempt >= 0),
 error TEXT,
 reason TEXT NOT NULL,
 recorded_at TEXT NOT NULL,
 PRIMARY KEY (job_id, seq)
);

ALTER TABLE creator_artifacts ADD COLUMN validated_at TEXT;
ALTER TABLE creator_artifacts ADD COLUMN current INTEGER NOT NULL DEFAULT 0
  CHECK(current IN (0,1));
CREATE UNIQUE INDEX IF NOT EXISTS idx_creator_artifacts_one_current
  ON creator_artifacts(project_id, kind) WHERE current = 1;
