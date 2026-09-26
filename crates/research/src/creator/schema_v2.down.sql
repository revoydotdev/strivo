-- Version 2, reversed: `duration_us` becomes `duration_ms` again and the
-- clock-segment table goes away.
--
-- **The reverse conversion is lossy and says so.** `duration_us / 1000`
-- truncates, so a sub-millisecond duration comes back shortened — 1 500 us
-- returns as 1 ms. That is a property of the old unit, not of this
-- statement, and it is why `docs/RESEARCH-PLATFORM-ROADMAP.md:72` permits a
-- destructive `down` only when rehearsed from a backup: the backup is the
-- copy that keeps the exact values. A test asserts the truncation rather
-- than leaving it to be discovered.
--
-- Everything else is exactly reversible: the clock table is dropped whole,
-- and the version line is set by the runner, not here.

ALTER TABLE creator_artifacts ADD COLUMN duration_ms INTEGER;
UPDATE creator_artifacts SET duration_ms = duration_us / 1000
  WHERE duration_us IS NOT NULL;
ALTER TABLE creator_artifacts DROP COLUMN duration_us;

ALTER TABLE creator_source_assets ADD COLUMN duration_ms INTEGER;
UPDATE creator_source_assets SET duration_ms = duration_us / 1000
  WHERE duration_us IS NOT NULL;
ALTER TABLE creator_source_assets DROP COLUMN duration_us;

DROP INDEX IF EXISTS idx_creator_clock_segments_project;
DROP TABLE IF EXISTS creator_clock_segments;
