CREATE TABLE IF NOT EXISTS schema_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);

CREATE TABLE IF NOT EXISTS creator_projects (
 id TEXT PRIMARY KEY, name TEXT NOT NULL, description TEXT NOT NULL DEFAULT '',
 channel TEXT NOT NULL DEFAULT '', series TEXT NOT NULL DEFAULT '',
 settings_json TEXT NOT NULL DEFAULT '{}',
 created_at TEXT NOT NULL, updated_at TEXT NOT NULL
);

-- The link to the research kernel. `research_project_id` is deliberately NOT
-- a foreign key: the two project models are related by stable ID, and forcing
-- a key would be the report's "silently redefine it as a single recording".
-- The primary key makes the relationship many-to-many, which one-to-one
-- cannot express.
CREATE TABLE IF NOT EXISTS creator_project_research_links (
 creator_project_id TEXT NOT NULL REFERENCES creator_projects(id) ON DELETE CASCADE,
 research_project_id TEXT NOT NULL,
 relation TEXT NOT NULL DEFAULT 'linked',
 created_at TEXT NOT NULL,
 PRIMARY KEY (creator_project_id, research_project_id)
);

-- `locator` is a resolvable location, never an identity: no unique
-- constraint, never a key. `media_fingerprint` is the content identity and
-- survives the file moving.
CREATE TABLE IF NOT EXISTS creator_source_assets (
 id TEXT PRIMARY KEY,
 project_id TEXT NOT NULL REFERENCES creator_projects(id) ON DELETE CASCADE,
 kind TEXT NOT NULL, title TEXT NOT NULL DEFAULT '',
 media_fingerprint TEXT NOT NULL, locator TEXT,
 origin TEXT NOT NULL DEFAULT 'import', rights_json TEXT NOT NULL DEFAULT '{}',
 track_type TEXT NOT NULL DEFAULT '', codec TEXT NOT NULL DEFAULT '',
 duration_ms INTEGER, created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_creator_source_assets_project
 ON creator_source_assets(project_id);

CREATE TABLE IF NOT EXISTS creator_capture_sessions (
 id TEXT PRIMARY KEY,
 project_id TEXT NOT NULL REFERENCES creator_projects(id) ON DELETE CASCADE,
 source_asset_id TEXT REFERENCES creator_source_assets(id) ON DELETE SET NULL,
 started_at TEXT NOT NULL, ended_at TEXT, finalized INTEGER NOT NULL DEFAULT 0,
 attributes_json TEXT NOT NULL DEFAULT '{}', created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_creator_capture_sessions_project
 ON creator_capture_sessions(project_id);

CREATE TABLE IF NOT EXISTS creator_transcript_revisions (
 id TEXT PRIMARY KEY,
 project_id TEXT NOT NULL REFERENCES creator_projects(id) ON DELETE CASCADE,
 source_asset_id TEXT NOT NULL REFERENCES creator_source_assets(id) ON DELETE CASCADE,
 revision INTEGER NOT NULL, provider TEXT NOT NULL DEFAULT '',
 model_version TEXT NOT NULL DEFAULT '', author TEXT NOT NULL DEFAULT '',
 body_json TEXT NOT NULL DEFAULT '{}', created_at TEXT NOT NULL,
 UNIQUE(source_asset_id, revision)
);
CREATE INDEX IF NOT EXISTS idx_creator_transcript_revisions_source
 ON creator_transcript_revisions(source_asset_id);

CREATE TABLE IF NOT EXISTS creator_composition_revisions (
 id TEXT PRIMARY KEY,
 project_id TEXT NOT NULL REFERENCES creator_projects(id) ON DELETE CASCADE,
 parent_revision_id TEXT REFERENCES creator_composition_revisions(id) ON DELETE SET NULL,
 revision INTEGER NOT NULL, author TEXT NOT NULL DEFAULT '',
 timeline_json TEXT NOT NULL DEFAULT '{}', output_profile TEXT NOT NULL DEFAULT '',
 created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_creator_composition_revisions_project
 ON creator_composition_revisions(project_id, created_at);

CREATE TABLE IF NOT EXISTS creator_variants (
 id TEXT PRIMARY KEY,
 project_id TEXT NOT NULL REFERENCES creator_projects(id) ON DELETE CASCADE,
 composition_revision_id TEXT NOT NULL
   REFERENCES creator_composition_revisions(id) ON DELETE CASCADE,
 name TEXT NOT NULL DEFAULT '', format_overrides_json TEXT NOT NULL DEFAULT '{}',
 created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS creator_render_plans (
 id TEXT PRIMARY KEY,
 project_id TEXT NOT NULL REFERENCES creator_projects(id) ON DELETE CASCADE,
 variant_id TEXT NOT NULL REFERENCES creator_variants(id) ON DELETE CASCADE,
 renderer TEXT NOT NULL DEFAULT '', renderer_version TEXT NOT NULL DEFAULT '',
 resolved_inputs_json TEXT NOT NULL DEFAULT '{}', content_hash TEXT NOT NULL,
 created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS creator_artifacts (
 id TEXT PRIMARY KEY,
 project_id TEXT NOT NULL REFERENCES creator_projects(id) ON DELETE CASCADE,
 variant_id TEXT REFERENCES creator_variants(id) ON DELETE SET NULL,
 render_plan_id TEXT REFERENCES creator_render_plans(id) ON DELETE SET NULL,
 kind TEXT NOT NULL, content_hash TEXT NOT NULL, locator TEXT,
 bytes INTEGER, width INTEGER, height INTEGER, duration_ms INTEGER,
 created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_creator_artifacts_project
 ON creator_artifacts(project_id);

CREATE TABLE IF NOT EXISTS creator_release_packages (
 id TEXT PRIMARY KEY,
 project_id TEXT NOT NULL REFERENCES creator_projects(id) ON DELETE CASCADE,
 artifact_id TEXT NOT NULL REFERENCES creator_artifacts(id) ON DELETE CASCADE,
 metadata_json TEXT NOT NULL DEFAULT '{}', approved_by TEXT NOT NULL DEFAULT '',
 approved_at TEXT, state TEXT NOT NULL DEFAULT 'draft'
   CHECK(state IN ('draft','approved','superseded')),
 created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS creator_publications (
 id TEXT PRIMARY KEY,
 project_id TEXT NOT NULL REFERENCES creator_projects(id) ON DELETE CASCADE,
 release_package_id TEXT NOT NULL REFERENCES creator_release_packages(id) ON DELETE CASCADE,
 destination TEXT NOT NULL, account TEXT NOT NULL DEFAULT '',
 state TEXT NOT NULL DEFAULT 'draft'
   CHECK(state IN ('draft','queued','uploading','uploaded','processing','published','failed')),
 scheduled_at TEXT, timezone TEXT NOT NULL DEFAULT 'UTC', remote_id TEXT,
 uploaded_at TEXT, published_at TEXT, last_error TEXT, created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_creator_publications_package
 ON creator_publications(release_package_id);

CREATE TABLE IF NOT EXISTS creator_metric_observations (
 id TEXT PRIMARY KEY,
 project_id TEXT NOT NULL REFERENCES creator_projects(id) ON DELETE CASCADE,
 publication_id TEXT REFERENCES creator_publications(id) ON DELETE SET NULL,
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
CREATE INDEX IF NOT EXISTS idx_creator_metric_observations_publication
 ON creator_metric_observations(publication_id, metric);
