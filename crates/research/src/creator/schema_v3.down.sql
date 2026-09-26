-- Version 3, reversed: the job lifecycle's tables and the artifact's
-- validation columns go away.
--
-- **The drop of `creator_jobs` destroys the job history**, which is the
-- honest cost of reversing a version that introduced it: the transitions this
-- version made recoverable are the transitions it takes with it. That is why
-- `docs/RESEARCH-PLATFORM-ROADMAP.md:72` permits a destructive `down` only
-- when rehearsed from a backup, and why the rehearsal copies the file first
-- rather than pointing `down` at the live database.
--
-- The indexes are dropped explicitly rather than relying on `DROP TABLE`:
-- SQLite refuses `ALTER TABLE … DROP COLUMN` for a column an index mentions,
-- so `current` could not be dropped while `idx_creator_artifacts_one_current`
-- still existed. Reversing the order of creation is the whole reverse
-- migration.
--
-- The version line is NOT stamped here. It belongs to `schema::rollback`,
-- which writes it in the same transaction as this file's statements — a `down`
-- that stamped it itself would make the line's position depend on which
-- caller applied the SQL.

DROP INDEX IF EXISTS idx_creator_artifacts_one_current;
ALTER TABLE creator_artifacts DROP COLUMN current;
ALTER TABLE creator_artifacts DROP COLUMN validated_at;

DROP INDEX IF EXISTS idx_creator_jobs_one_unsettled;
DROP INDEX IF EXISTS idx_creator_jobs_cache_key;
DROP INDEX IF EXISTS idx_creator_jobs_project;
DROP TABLE IF EXISTS creator_job_transitions;
DROP TABLE IF EXISTS creator_jobs;
