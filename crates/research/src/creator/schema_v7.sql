-- Version 7: the edit history — one chain, and undo, redo, branch and snapshot
-- as that one chain read four ways.
--
-- One row per *action*, not per surface. A transcript correction and a
-- timeline trim are two rows here, in the order they happened, and one undo
-- moves across both. That is the whole of "one undo history across transcript,
-- timeline and inspector": there is no second stack to reconcile because there
-- is only one list.
--
-- What a row *references*, not what it stores. `composition_revision_id` points
-- at the immutable revision todo 28 already keeps in
-- `creator_composition_revisions`, so an undo is that same revision id again —
-- restoring a revision, never re-running an inverse edit. The transcript half is
-- a PATCH against `transcript_revision_id`, in todo 42's own `TranscriptDraft`
-- vocabulary, not a copy of the document: a copy would be a second source of
-- truth that can disagree with the revision it claims to edit.
--
-- `parent_id` and `replaced_id` are deliberately NOT foreign keys, for the
-- reason version 4 gave for the workspace's selection and version 1 did *not*
-- follow for `creator_composition_revisions.parent_revision_id`: with
-- `ON DELETE SET NULL` a deleted entry would silently re-root the history, and
-- the next read would be indistinguishable from a history that had never been
-- branched. Left unkeyed, the dangling id survives and the walk reports it.
--
-- The primary key is the entry. There is exactly one identity per action, so
-- "the entry named X" has one answer.

CREATE TABLE IF NOT EXISTS creator_history (
 history_id TEXT PRIMARY KEY,
 project_id TEXT NOT NULL REFERENCES creator_projects(id) ON DELETE CASCADE,
 -- The entry this was made against. NULL only at the root of a history.
 parent_id TEXT,
 -- The entry this one puts back, when it is an undo or a redo. NULL for an
 -- ordinary commit, and a NULL here is what makes "nothing to redo" a state
 -- rather than a guess.
 replaced_id TEXT,
 composition_revision_id TEXT NOT NULL,
 transcript_revision_id TEXT,
 transcript_patch_json TEXT NOT NULL DEFAULT 'null',
 label TEXT NOT NULL,
 created_at TEXT NOT NULL,
 -- A revision id with no entry, or an entry that names no composition, is not
 -- an action. A transcript patch without the revision it patches against is an
 -- orphan, and is refused here rather than read as "no text change".
 CHECK((transcript_revision_id IS NULL) = (transcript_patch_json = 'null'))
);
CREATE INDEX IF NOT EXISTS idx_creator_history_project
 ON creator_history(project_id, created_at, history_id);
