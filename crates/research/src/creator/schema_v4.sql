-- Version 4: the persistent project workspace.
--
-- One row per creator project, and it is the whole of "which revision is
-- selected". There is no per-view row, no per-pane row and no
-- session-storage mirror: a second place recording a selection is a second
-- answer to "what is open", and the disagreement between the two is what a
-- project-centric workspace exists to end. Every view — player, transcript,
-- timeline, inspector, task status — reads this row, so they cannot disagree
-- about it.
--
-- The primary key is the project. That is the whole coexistence claim in one
-- line: there is exactly one row a project can have, so there is exactly one
-- selection for every view to read.
--
-- `selected_composition_revision_id` is deliberately NOT a foreign key, for
-- the same reason `creator_jobs.subject_id` and
-- `creator_project_research_links.research_project_id` are not. With
-- `ON DELETE SET NULL` a deleted revision would silently clear the selection,
-- and the next read would be indistinguishable from a project nobody had
-- selected anything in — a quiet data loss that looks like a fresh project.
-- Left unkeyed, the dangling id survives and the read reports it.
--
-- `selected_fingerprint` is the selected revision's content hash at the moment
-- of selection, so a revision edited after the fact is *detectably* stale
-- rather than silently rendered as the selection. It is a cache-validation
-- field, not a second copy of the revision: nothing reads it to decide what
-- the revision is, only whether what it says still matches the row.
--
-- Time is integer microseconds, the single Creator representation. There is no
-- millisecond column and no float: schema version 2 already replaced
-- `duration_ms` with `duration_us` precisely so a database would not carry two
-- answers to "when".
--
-- `unsaved_json` and `dirty` are written by the SAME statement everywhere they
-- change, so unsaved work can never be recorded without the flag that says it
-- is unsaved, or the flag without the work.

CREATE TABLE IF NOT EXISTS creator_workspace (
 project_id TEXT PRIMARY KEY REFERENCES creator_projects(id) ON DELETE CASCADE,
 selected_composition_revision_id TEXT,
 selected_fingerprint TEXT,
 playhead_us INTEGER CHECK(playhead_us IS NULL OR playhead_us >= 0),
 selection_start_us INTEGER CHECK(selection_start_us IS NULL OR selection_start_us >= 0),
 selection_end_us INTEGER CHECK(selection_end_us IS NULL OR selection_end_us >= 0),
 active_view TEXT NOT NULL DEFAULT 'player'
   CHECK(active_view IN ('player','transcript','timeline','inspector','tasks')),
 unsaved_json TEXT NOT NULL DEFAULT '{}',
 dirty INTEGER NOT NULL DEFAULT 0 CHECK(dirty IN (0,1)),
 updated_at TEXT NOT NULL,
 -- A range is either both endpoints or neither, and never inverted: half a
 -- range is a selection whose other end is unknown, which a view would have to
 -- guess.
 CHECK((selection_start_us IS NULL) = (selection_end_us IS NULL)),
 CHECK(selection_start_us IS NULL OR selection_end_us >= selection_start_us),
 -- A fingerprint without a revision describes nothing, and a revision without
 -- a fingerprint cannot be checked for staleness. Both are refused here rather
 -- than read as "no selection".
 CHECK((selected_composition_revision_id IS NULL) = (selected_fingerprint IS NULL))
);
