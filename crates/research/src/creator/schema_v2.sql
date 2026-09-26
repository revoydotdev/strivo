-- Version 2: the canonical time model.
--
-- `duration_ms` becomes `duration_us`: integer microseconds, the unit
-- strivo-editor's time model uses and the only unit a Creator timestamp is
-- allowed to be in. The conversion is the same arithmetic the Rust side
-- applies in `MediaTime::from_millis`, so the two cannot disagree about the
-- scale, and a NULL duration stays NULL rather than becoming a fabricated
-- zero.
--
-- The old column is DROPPED, not left beside the new one. A database that
-- carried both would have two answers to "how long is this source", and
-- picking one at read time is exactly the drift this migration removes. A
-- caller that has not been converted is a caller to convert, not a reason to
-- keep a compatibility column.
--
-- `creator_clock_segments` is the wall-clock to media-time relationship of a
-- capture, one row per capture part: the four-minute hole a reconnect leaves
-- in the wall clock, with the media timeline running straight through it,
-- is a list of segments and cannot be a single rate. The CHECK constraints
-- are the same invariants `ClockSegment::validate` enforces, so a bad row
-- cannot be written with raw SQL either.

ALTER TABLE creator_source_assets ADD COLUMN duration_us INTEGER
  CHECK(duration_us IS NULL OR duration_us >= 0);
UPDATE creator_source_assets SET duration_us = duration_ms * 1000
  WHERE duration_ms IS NOT NULL;
ALTER TABLE creator_source_assets DROP COLUMN duration_ms;

ALTER TABLE creator_artifacts ADD COLUMN duration_us INTEGER
  CHECK(duration_us IS NULL OR duration_us >= 0);
UPDATE creator_artifacts SET duration_us = duration_ms * 1000
  WHERE duration_ms IS NOT NULL;
ALTER TABLE creator_artifacts DROP COLUMN duration_ms;

CREATE TABLE IF NOT EXISTS creator_clock_segments (
 id TEXT PRIMARY KEY,
 capture_session_id TEXT NOT NULL
   REFERENCES creator_capture_sessions(id) ON DELETE CASCADE,
 project_id TEXT NOT NULL REFERENCES creator_projects(id) ON DELETE CASCADE,
 seq INTEGER NOT NULL CHECK(seq >= 0),
 wall_start_ms INTEGER NOT NULL CHECK(wall_start_ms >= 0),
 media_start_us INTEGER NOT NULL CHECK(media_start_us >= 0),
 media_len_us INTEGER NOT NULL CHECK(media_len_us > 0),
 rate_num INTEGER NOT NULL CHECK(rate_num > 0),
 rate_den INTEGER NOT NULL CHECK(rate_den > 0),
 UNIQUE(capture_session_id, seq)
);
CREATE INDEX IF NOT EXISTS idx_creator_clock_segments_project
  ON creator_clock_segments(project_id);
