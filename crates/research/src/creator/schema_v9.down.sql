-- Version 9, reversed: the fourteen-state vocabulary and the action log go away.
--
-- **The `state` CHECK is not narrowed back, and that is a decision rather than
-- an omission.** The seven-state vocabulary has no way to hold
-- `unknown_remote_state` or `needs_reauthorization`; putting either back would
-- mean writing `failed`, which is the false claim this whole version exists to
-- remove, or `published`, which is worse. A wider CHECK is also harmless to the
-- build that comes before this one: `Publication.state` is a `String` there, so
-- it reads a name it does not know as an opaque value rather than crashing or
-- silently normalising it. So the reverse step drops what it can drop exactly
-- and leaves the wider vocabulary standing.
--
-- **What this destroys, stated plainly:** `creator_publication_events` is
-- dropped whole, and with it the record of every upload attempt, every
-- ambiguous outcome and every provider word. A publication row survives saying
-- `unknown_remote_state` with no log explaining how it got there. That is the
-- same honest cost as version 5's and version 8's `down`, and the same reason
-- `docs/RESEARCH-PLATFORM-ROADMAP.md:72` permits a destructive `down` only
-- rehearsed from a backup.
--
-- `approved_fingerprint` on `creator_publications` is kept rather than dropped:
-- dropping a column that an older build does not know about is free, but the
-- column is the evidence that a posting was authorised against specific
-- content, and an evidence column that disappears on a downgrade is an
-- evidence column nobody should rely on. The package's added columns ARE
-- dropped, because they are inputs a caller re-supplies on the way back up.
--
-- The version line is NOT stamped here. It belongs to `schema::rollback`.

DROP INDEX IF EXISTS idx_creator_publication_events_publication;

DROP TABLE IF EXISTS creator_publication_events;

ALTER TABLE creator_release_packages DROP COLUMN approved_fingerprint;

ALTER TABLE creator_release_packages DROP COLUMN content_fingerprint;

ALTER TABLE creator_release_packages DROP COLUMN source_ids_json;

ALTER TABLE creator_release_packages DROP COLUMN variant_id;

ALTER TABLE creator_release_packages DROP COLUMN composition_revision_id;
