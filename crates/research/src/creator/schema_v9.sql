-- Version 9: the release package and the truthful publication lifecycle.
--
-- Two things were wrong with the delivery chain as version 1 left it, and both
-- are contract problems rather than missing columns.
--
-- 1. `creator_publications.state` carried a seven-value CHECK
--    (`draft,queued,uploading,uploaded,processing,published,failed`) with no way
--    to say *the provider has not told us*. An upload whose response was lost
--    had to be recorded as `failed`, and re-uploading a `failed` row is how a
--    duplicate posting appears. So the vocabulary is replaced by the fourteen
--    states of `creator::publication::PublicationState`, which include
--    `unknown_remote_state` and `needs_reauthorization` as first-class answers.
--    `queued` and `uploaded` are not in the new set and are mapped on the way in:
--    `queued` is a local state meaning the bytes are ready, and `uploaded` means
--    the provider acknowledged receipt and is transcoding. Both mappings are
--    exact; nothing is invented.
--
-- 2. `creator_release_packages` named only an artifact, so an approval record
--    could not say *which edit* and *which destination shape* it approved. The
--    report (§4.6) asks for "approved composition revision + destination variant
--    + rendered artifact + metadata + approval record". The two revision columns
--    are added NULLABLE and left NULL for a pre-existing row: back-filling them
--    would be a guess about a package nobody re-reviewed, and an approval that
--    cannot be traced is reported as untraceable rather than reconstructed.
--
-- **`approved_fingerprint` appears on both tables, and that is the point.** The
-- package stores the fingerprint it approved; a publication stores the
-- fingerprint it was authorised against, copied at the moment it left
-- `approved`. A later edit moves the package's fingerprint and cannot move the
-- publication's, so "a scheduled package was silently altered" stops being
-- something a caller has to be trusted not to do.
--
-- `content_fingerprint` is `''` for a pre-existing row, which reads as "no
-- fingerprint was ever recorded" and is exactly that: an old approval cannot
-- claim to have approved content it never hashed.
--
-- The publications table is **rebuilt** rather than altered, because a CHECK
-- constraint cannot be widened in place. The order is child-first for a
-- reason: `creator_metric_observations` holds a foreign key to
-- `creator_publications`, so it is copied to a staging table pointing at the
-- staging publications table, the old child is dropped, the old parent is
-- dropped, and only then are the staging tables renamed into place. Dropping a
-- parent while a child still names it is an integrity violation under the
-- `PRAGMA foreign_keys=ON` that `Store::open` sets, and SQLite runs a
-- migration inside one transaction where that pragma cannot be turned off.
--
-- `creator_publication_events` is the append-only action log P01 asks for: a
-- transition is one row here written in the same transaction as the state
-- change, so a publication can never be observed in a state its own log does
-- not record. It is the same shape as `creator_job_transitions`, and `seq` is
-- assigned by the store from `MAX(seq) + 1` within a publication.

CREATE TABLE creator_publications_v9 (
 id TEXT PRIMARY KEY,
 project_id TEXT NOT NULL REFERENCES creator_projects(id) ON DELETE CASCADE,
 release_package_id TEXT NOT NULL REFERENCES creator_release_packages(id) ON DELETE CASCADE,
 destination TEXT NOT NULL, account TEXT NOT NULL DEFAULT '',
 state TEXT NOT NULL DEFAULT 'draft'
   CHECK(state IN ('draft','needs_review','approved','rendering','ready','uploading',
                   'processing','scheduled','published','failed','cancelled',
                   'needs_reauthorization','unknown_remote_state','removed')),
 scheduled_at TEXT, timezone TEXT NOT NULL DEFAULT 'UTC', remote_id TEXT,
 uploaded_at TEXT, published_at TEXT, last_error TEXT, created_at TEXT NOT NULL,
 -- The package content fingerprint this posting was authorised against, copied
 -- when it left `approved`. NULL for a row that predates this version.
 approved_fingerprint TEXT
);

INSERT INTO creator_publications_v9
 (id,project_id,release_package_id,destination,account,state,scheduled_at,timezone,
  remote_id,uploaded_at,published_at,last_error,created_at,approved_fingerprint)
SELECT id,project_id,release_package_id,destination,account,
       CASE state
         WHEN 'queued' THEN 'ready'
         WHEN 'uploaded' THEN 'processing'
         ELSE state
       END,
       scheduled_at,timezone,remote_id,uploaded_at,published_at,last_error,created_at,NULL
FROM creator_publications;

CREATE TABLE creator_metric_observations_v9 (
 id TEXT PRIMARY KEY,
 project_id TEXT NOT NULL REFERENCES creator_projects(id) ON DELETE CASCADE,
 publication_id TEXT REFERENCES creator_publications_v9(id) ON DELETE SET NULL,
 release_package_id TEXT REFERENCES creator_release_packages(id) ON DELETE SET NULL,
 artifact_id TEXT REFERENCES creator_artifacts(id) ON DELETE SET NULL,
 composition_revision_id TEXT REFERENCES creator_composition_revisions(id) ON DELETE SET NULL,
 provider TEXT NOT NULL, metric TEXT NOT NULL, definition_version TEXT NOT NULL DEFAULT '',
 scope TEXT NOT NULL DEFAULT '', unit TEXT NOT NULL DEFAULT '', denominator TEXT,
 interval_start TEXT NOT NULL, interval_end TEXT NOT NULL,
 timezone TEXT NOT NULL DEFAULT 'UTC',
 value REAL, coverage REAL, retrieved_at TEXT NOT NULL,
 source_query_json TEXT NOT NULL DEFAULT '{}',
 transform_json TEXT NOT NULL DEFAULT '{}', chart_json TEXT NOT NULL DEFAULT '{}'
);

INSERT INTO creator_metric_observations_v9
 SELECT id,project_id,publication_id,release_package_id,artifact_id,composition_revision_id,
        provider,metric,definition_version,scope,unit,denominator,interval_start,interval_end,
        timezone,value,coverage,retrieved_at,source_query_json,transform_json,chart_json
FROM creator_metric_observations;

DROP TABLE creator_metric_observations;

DROP TABLE creator_publications;

ALTER TABLE creator_publications_v9 RENAME TO creator_publications;

ALTER TABLE creator_metric_observations_v9 RENAME TO creator_metric_observations;

CREATE INDEX IF NOT EXISTS idx_creator_publications_package
 ON creator_publications(release_package_id);

CREATE INDEX IF NOT EXISTS idx_creator_metric_observations_publication
 ON creator_metric_observations(publication_id, metric);

-- **Version 5's live-status trigger is reinstalled here, and its absence was a
-- real defect this migration shipped once.** A trigger belongs to the table it
-- is declared on, so rebuilding that table drops it: the first run of this
-- migration left `creator_metric_observations` able to accept
-- `concurrent_viewers` again, and the only thing that noticed was
-- `tests/observations.rs`'s "the schema, not a Rust `if`" assertion. A refusal
-- enforced by Rust instead is a refusal a hand-edited statement, a future
-- writer or a re-import routes straight around, so the trigger is re-declared
-- verbatim rather than left to version 5's authors to notice. It is `IF NOT
-- EXISTS` so re-running this migration is a no-op, and version 9's `down` does
-- NOT drop it: it is version 5's object, and version 5's own `down` removes it.
CREATE TRIGGER IF NOT EXISTS creator_metric_observations_reject_live_status
 BEFORE INSERT ON creator_metric_observations
 WHEN NEW.metric IN ('concurrent_viewers','viewer_count','live_viewers',
                     'is_live','live_status','followers')
BEGIN
 SELECT RAISE(ABORT,
   'live-status/live-status: this metric is a live-status reading, not a measured outcome. '
   || 'It belongs in creator_viewer_samples, which carries it with its own uncertainty bound.');
END;

-- The package's own inputs. Nullable revision columns, see the note above.
ALTER TABLE creator_release_packages ADD COLUMN composition_revision_id TEXT
  REFERENCES creator_composition_revisions(id);

ALTER TABLE creator_release_packages ADD COLUMN variant_id TEXT
  REFERENCES creator_variants(id);

-- The source set the rights gate is run over. `[]` means "no source was
-- recorded", which is a real and reportable answer: `publication_report` over
-- an empty list is vacuously publishable, so a caller that means to gate must
-- say so here. See `creator::publication` for why the set is declared rather
-- than derived.
ALTER TABLE creator_release_packages ADD COLUMN source_ids_json TEXT NOT NULL DEFAULT '[]';

ALTER TABLE creator_release_packages ADD COLUMN content_fingerprint TEXT NOT NULL DEFAULT '';

ALTER TABLE creator_release_packages ADD COLUMN approved_fingerprint TEXT;

-- The action log. `detail` is untrusted remote or caller text, carried
-- verbatim and never interpreted; bounded so a provider cannot fill a disk.
CREATE TABLE IF NOT EXISTS creator_publication_events (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 publication_id TEXT NOT NULL REFERENCES creator_publications(id) ON DELETE CASCADE,
 seq INTEGER NOT NULL CHECK(seq > 0),
 from_state TEXT CHECK(from_state IS NULL OR from_state IN
   ('draft','needs_review','approved','rendering','ready','uploading','processing',
    'scheduled','published','failed','cancelled','needs_reauthorization',
    'unknown_remote_state','removed')),
 to_state TEXT NOT NULL CHECK(to_state IN
   ('draft','needs_review','approved','rendering','ready','uploading','processing',
    'scheduled','published','failed','cancelled','needs_reauthorization',
    'unknown_remote_state','removed')),
 detail TEXT NOT NULL DEFAULT '' CHECK(LENGTH(detail) <= 4096),
 recorded_at TEXT NOT NULL,
 UNIQUE(publication_id, seq)
);

CREATE INDEX IF NOT EXISTS idx_creator_publication_events_publication
 ON creator_publication_events(publication_id, seq);
