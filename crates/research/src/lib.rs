//! Local-first research data kernel for Strivo Pro.
//!
//! SQLite owns mutable research metadata and evidence links. Stable UUIDs let
//! later analytical tiers project high-volume events into Parquet/DuckDB
//! without changing researcher-authored references.

use std::path::Path;

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

pub mod search;

const SCHEMA_VERSION: i64 = 1;
const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS schema_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS projects (
 id TEXT PRIMARY KEY, name TEXT NOT NULL, description TEXT NOT NULL DEFAULT '',
 created_at TEXT NOT NULL, updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS sources (
 id TEXT PRIMARY KEY,
 project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
 recording_id TEXT, kind TEXT NOT NULL, title TEXT NOT NULL, uri TEXT,
 duration_ms INTEGER, attributes_json TEXT NOT NULL DEFAULT '{}',
 created_at TEXT NOT NULL, UNIQUE(project_id, recording_id)
);
CREATE INDEX IF NOT EXISTS idx_sources_project ON sources(project_id);
CREATE TABLE IF NOT EXISTS cases (
 id TEXT PRIMARY KEY,
 project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
 name TEXT NOT NULL, description TEXT NOT NULL DEFAULT '',
 attributes_json TEXT NOT NULL DEFAULT '{}', created_at TEXT NOT NULL,
 UNIQUE(project_id, name)
);
CREATE TABLE IF NOT EXISTS source_cases (
 source_id TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
 case_id TEXT NOT NULL REFERENCES cases(id) ON DELETE CASCADE,
 PRIMARY KEY(source_id, case_id)
);
CREATE TABLE IF NOT EXISTS codes (
 id TEXT PRIMARY KEY,
 project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
 parent_id TEXT REFERENCES codes(id) ON DELETE RESTRICT,
 name TEXT NOT NULL, description TEXT NOT NULL DEFAULT '',
 color TEXT NOT NULL DEFAULT '#7c5cff', created_at TEXT NOT NULL,
 UNIQUE(project_id, parent_id, name)
);
CREATE INDEX IF NOT EXISTS idx_codes_project_parent ON codes(project_id, parent_id);
CREATE TABLE IF NOT EXISTS provenance (
 id TEXT PRIMARY KEY, producer TEXT NOT NULL, producer_version TEXT NOT NULL,
 method TEXT NOT NULL, model TEXT, parameters_json TEXT NOT NULL DEFAULT '{}',
 input_digest TEXT, created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS signals (
 id TEXT PRIMARY KEY,
 project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
 source_id TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
 start_ms INTEGER NOT NULL CHECK(start_ms >= 0),
 end_ms INTEGER NOT NULL CHECK(end_ms >= start_ms),
 kind TEXT NOT NULL, label TEXT NOT NULL DEFAULT '',
 payload_json TEXT NOT NULL DEFAULT '{}',
 confidence REAL CHECK(confidence IS NULL OR (confidence >= 0 AND confidence <= 1)),
 provenance_id TEXT REFERENCES provenance(id) ON DELETE RESTRICT,
 created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_signals_source_time ON signals(source_id, start_ms, end_ms);
CREATE INDEX IF NOT EXISTS idx_signals_project_kind ON signals(project_id, kind);
CREATE TABLE IF NOT EXISTS codings (
 id TEXT PRIMARY KEY,
 project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
 source_id TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
 code_id TEXT NOT NULL REFERENCES codes(id) ON DELETE RESTRICT,
 start_ms INTEGER NOT NULL CHECK(start_ms >= 0),
 end_ms INTEGER NOT NULL CHECK(end_ms >= start_ms),
 excerpt TEXT NOT NULL DEFAULT '', note TEXT NOT NULL DEFAULT '',
 author TEXT NOT NULL,
 origin TEXT NOT NULL CHECK(origin IN ('human', 'model', 'import')),
 confidence REAL CHECK(confidence IS NULL OR (confidence >= 0 AND confidence <= 1)),
 provenance_id TEXT REFERENCES provenance(id) ON DELETE RESTRICT,
 created_at TEXT NOT NULL, updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_codings_source_time ON codings(source_id, start_ms, end_ms);
CREATE INDEX IF NOT EXISTS idx_codings_code ON codings(code_id);
CREATE TABLE IF NOT EXISTS memos (
 id TEXT PRIMARY KEY,
 project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
 source_id TEXT REFERENCES sources(id) ON DELETE CASCADE,
 coding_id TEXT REFERENCES codings(id) ON DELETE CASCADE,
 title TEXT NOT NULL, body TEXT NOT NULL, author TEXT NOT NULL,
 created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
 CHECK(source_id IS NOT NULL OR coding_id IS NOT NULL)
);
CREATE TABLE IF NOT EXISTS relationships (
 id TEXT PRIMARY KEY,
 project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
 from_kind TEXT NOT NULL, from_id TEXT NOT NULL,
 to_kind TEXT NOT NULL, to_id TEXT NOT NULL,
 relation TEXT NOT NULL, note TEXT NOT NULL DEFAULT '',
 author TEXT NOT NULL, created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_relationships_project ON relationships(project_id);
"#;

#[derive(Debug, Error)]
pub enum ResearchError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("invalid research data: {0}")]
    Validation(String),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("filesystem error: {0}")]
    Io(#[from] std::io::Error),
}
pub type Result<T> = std::result::Result<T, ResearchError>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Project {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Recording,
    Video,
    Audio,
    Chat,
    Document,
    Dataset,
}
impl SourceKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Recording => "recording",
            Self::Video => "video",
            Self::Audio => "audio",
            Self::Chat => "chat",
            Self::Document => "document",
            Self::Dataset => "dataset",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NewSource {
    pub id: Uuid,
    pub project_id: Uuid,
    pub recording_id: Option<Uuid>,
    pub kind: SourceKind,
    pub title: String,
    pub uri: Option<String>,
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub attributes: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Code {
    pub id: Uuid,
    pub project_id: Uuid,
    pub parent_id: Option<Uuid>,
    pub name: String,
    pub description: String,
    pub color: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Provenance {
    pub id: Uuid,
    pub producer: String,
    pub producer_version: String,
    pub method: String,
    pub model: Option<String>,
    #[serde(default)]
    pub parameters: Value,
    pub input_digest: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NewSignal {
    pub id: Uuid,
    pub project_id: Uuid,
    pub source_id: Uuid,
    pub start_ms: u64,
    pub end_ms: u64,
    pub kind: String,
    pub label: String,
    #[serde(default)]
    pub payload: Value,
    pub confidence: Option<f64>,
    pub provenance_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CodingOrigin {
    Human,
    Model,
    Import,
}
impl CodingOrigin {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Model => "model",
            Self::Import => "import",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NewCoding {
    pub id: Uuid,
    pub project_id: Uuid,
    pub source_id: Uuid,
    pub code_id: Uuid,
    pub start_ms: u64,
    pub end_ms: u64,
    pub excerpt: String,
    pub note: String,
    pub author: String,
    pub origin: CodingOrigin,
    pub confidence: Option<f64>,
    pub provenance_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Source {
    pub id: Uuid,
    pub project_id: Uuid,
    pub recording_id: Option<Uuid>,
    pub kind: String,
    pub title: String,
    pub uri: Option<String>,
    pub duration_ms: Option<u64>,
    pub attributes: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Signal {
    pub id: Uuid,
    pub project_id: Uuid,
    pub source_id: Uuid,
    pub start_ms: u64,
    pub end_ms: u64,
    pub kind: String,
    pub label: String,
    pub payload: Value,
    pub confidence: Option<f64>,
    pub provenance_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Coding {
    pub id: Uuid,
    pub project_id: Uuid,
    pub source_id: Uuid,
    pub code_id: Uuid,
    pub start_ms: u64,
    pub end_ms: u64,
    pub excerpt: String,
    pub note: String,
    pub author: String,
    pub origin: String,
    pub confidence: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectExport {
    pub schema_version: i64,
    pub project: Project,
    pub sources: Vec<Source>,
    pub codes: Vec<Code>,
    pub signals: Vec<Signal>,
    pub codings: Vec<Coding>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct MigrationReport {
    pub source: String,
    pub examined: u64,
    pub inserted: u64,
    pub skipped: u64,
    pub errors: Vec<String>,
    pub digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResearchCase {
    pub id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub attributes: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Memo {
    pub id: Uuid,
    pub project_id: Uuid,
    pub source_id: Option<Uuid>,
    pub coding_id: Option<Uuid>,
    pub title: String,
    pub body: String,
    pub author: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Relationship {
    pub id: Uuid,
    pub project_id: Uuid,
    pub from_kind: String,
    pub from_id: Uuid,
    pub to_kind: String,
    pub to_id: Uuid,
    pub relation: String,
    pub note: String,
    pub author: String,
}

pub struct ResearchStore {
    conn: Connection,
}

impl ResearchStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        if path.as_ref() != Path::new(":memory:") {
            if let Some(parent) = path.as_ref().parent() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA foreign_keys=ON;
             PRAGMA busy_timeout=30000;",
        )?;
        conn.execute_batch(SCHEMA)?;
        conn.execute(
            "INSERT INTO schema_meta(key,value) VALUES('schema_version',?1)
             ON CONFLICT(key) DO NOTHING",
            [SCHEMA_VERSION.to_string()],
        )?;
        Ok(Self { conn })
    }

    pub fn schema_version(&self) -> Result<i64> {
        let raw: String = self.conn.query_row(
            "SELECT value FROM schema_meta WHERE key='schema_version'",
            [],
            |row| row.get(0),
        )?;
        raw.parse()
            .map_err(|_| ResearchError::Validation("invalid schema version".into()))
    }

    pub fn create_project(&self, name: &str, description: &str) -> Result<Project> {
        if name.trim().is_empty() {
            return Err(ResearchError::Validation(
                "project name cannot be empty".into(),
            ));
        }
        let now = Utc::now();
        let project = Project {
            id: Uuid::new_v4(),
            name: name.trim().into(),
            description: description.trim().into(),
            created_at: now,
            updated_at: now,
        };
        self.conn.execute(
            "INSERT INTO projects VALUES(?1,?2,?3,?4,?5)",
            params![
                project.id.to_string(),
                project.name,
                project.description,
                project.created_at.to_rfc3339(),
                project.updated_at.to_rfc3339()
            ],
        )?;
        Ok(project)
    }

    pub fn upsert_source(&self, source: &NewSource) -> Result<()> {
        object(&source.attributes, "source attributes")?;
        self.conn.execute(
            "INSERT INTO sources VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)
             ON CONFLICT(id) DO UPDATE SET title=excluded.title,uri=excluded.uri,
             duration_ms=excluded.duration_ms,attributes_json=excluded.attributes_json",
            params![
                source.id.to_string(),
                source.project_id.to_string(),
                source.recording_id.map(|id| id.to_string()),
                source.kind.as_str(),
                source.title.trim(),
                source.uri,
                optional_i64(source.duration_ms, "duration")?,
                serde_json::to_string(&source.attributes)?,
                Utc::now().to_rfc3339()
            ],
        )?;
        Ok(())
    }

    pub fn create_code(&self, code: &Code) -> Result<()> {
        if code.name.trim().is_empty() || !valid_color(&code.color) {
            return Err(ResearchError::Validation(
                "code requires a name and #RRGGBB color".into(),
            ));
        }
        if let Some(parent_id) = code.parent_id {
            let owner: Option<String> = self
                .conn
                .query_row(
                    "SELECT project_id FROM codes WHERE id=?1",
                    [parent_id.to_string()],
                    |row| row.get(0),
                )
                .optional()?;
            if owner.as_deref() != Some(code.project_id.to_string().as_str()) {
                return Err(ResearchError::Validation(
                    "parent code must belong to the same project".into(),
                ));
            }
        }
        self.conn.execute(
            "INSERT INTO codes VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![
                code.id.to_string(),
                code.project_id.to_string(),
                code.parent_id.map(|id| id.to_string()),
                code.name.trim(),
                code.description.trim(),
                code.color,
                Utc::now().to_rfc3339()
            ],
        )?;
        Ok(())
    }

    pub fn create_case(&self, case: &ResearchCase) -> Result<()> {
        object(&case.attributes, "case attributes")?;
        if case.name.trim().is_empty() {
            return Err(ResearchError::Validation("case name is empty".into()));
        }
        self.conn.execute(
            "INSERT INTO cases VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                case.id.to_string(),
                case.project_id.to_string(),
                case.name.trim(),
                case.description.trim(),
                serde_json::to_string(&case.attributes)?,
                Utc::now().to_rfc3339()
            ],
        )?;
        Ok(())
    }

    pub fn assign_source_case(&self, source_id: Uuid, case_id: Uuid) -> Result<()> {
        let changed = self.conn.execute(
            "INSERT OR IGNORE INTO source_cases(source_id,case_id)
             SELECT s.id,c.id FROM sources s,cases c
             WHERE s.id=?1 AND c.id=?2 AND s.project_id=c.project_id",
            params![source_id.to_string(), case_id.to_string()],
        )?;
        if changed == 0 {
            let exists: i64 = self.conn.query_row(
                "SELECT COUNT(*) FROM source_cases WHERE source_id=?1 AND case_id=?2",
                params![source_id.to_string(), case_id.to_string()],
                |row| row.get(0),
            )?;
            if exists == 0 {
                return Err(ResearchError::Validation(
                    "source and case must belong to the same project".into(),
                ));
            }
        }
        Ok(())
    }

    pub fn create_case_with_sources(
        &mut self,
        case: &ResearchCase,
        source_ids: &[Uuid],
    ) -> Result<()> {
        object(&case.attributes, "case attributes")?;
        if case.name.trim().is_empty() {
            return Err(ResearchError::Validation("case name is empty".into()));
        }
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO cases VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                case.id.to_string(),
                case.project_id.to_string(),
                case.name.trim(),
                case.description.trim(),
                serde_json::to_string(&case.attributes)?,
                Utc::now().to_rfc3339()
            ],
        )?;
        for source_id in source_ids {
            let changed = tx.execute(
                "INSERT INTO source_cases(source_id,case_id)
                 SELECT id,?2 FROM sources WHERE id=?1 AND project_id=?3",
                params![
                    source_id.to_string(),
                    case.id.to_string(),
                    case.project_id.to_string()
                ],
            )?;
            if changed != 1 {
                return Err(ResearchError::Validation(
                    "case source must belong to the same project".into(),
                ));
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn add_memo(&self, memo: &Memo) -> Result<()> {
        if memo.title.trim().is_empty() || memo.author.trim().is_empty() {
            return Err(ResearchError::Validation(
                "memo title and author are required".into(),
            ));
        }
        if memo.source_id.is_none() && memo.coding_id.is_none() {
            return Err(ResearchError::Validation(
                "memo must attach to a source or coding".into(),
            ));
        }
        if let Some(source_id) = memo.source_id {
            ensure_object_project(&self.conn, "sources", source_id, memo.project_id)?;
        }
        if let Some(coding_id) = memo.coding_id {
            ensure_object_project(&self.conn, "codings", coding_id, memo.project_id)?;
        }
        let now = Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO memos VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?8)",
            params![
                memo.id.to_string(),
                memo.project_id.to_string(),
                memo.source_id.map(|id| id.to_string()),
                memo.coding_id.map(|id| id.to_string()),
                memo.title.trim(),
                memo.body,
                memo.author.trim(),
                now
            ],
        )?;
        Ok(())
    }

    pub fn add_relationship(&self, relationship: &Relationship) -> Result<()> {
        if relationship.relation.trim().is_empty() || relationship.author.trim().is_empty() {
            return Err(ResearchError::Validation(
                "relationship type and author are required".into(),
            ));
        }
        ensure_object_project(
            &self.conn,
            &relationship.from_kind,
            relationship.from_id,
            relationship.project_id,
        )?;
        ensure_object_project(
            &self.conn,
            &relationship.to_kind,
            relationship.to_id,
            relationship.project_id,
        )?;
        self.conn.execute(
            "INSERT INTO relationships VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                relationship.id.to_string(),
                relationship.project_id.to_string(),
                relationship.from_kind.trim(),
                relationship.from_id.to_string(),
                relationship.to_kind.trim(),
                relationship.to_id.to_string(),
                relationship.relation.trim(),
                relationship.note,
                relationship.author.trim(),
                Utc::now().to_rfc3339()
            ],
        )?;
        Ok(())
    }

    pub fn add_provenance(&self, value: &Provenance) -> Result<()> {
        object(&value.parameters, "provenance parameters")?;
        self.conn.execute(
            "INSERT INTO provenance VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                value.id.to_string(),
                value.producer,
                value.producer_version,
                value.method,
                value.model,
                serde_json::to_string(&value.parameters)?,
                value.input_digest,
                Utc::now().to_rfc3339()
            ],
        )?;
        Ok(())
    }

    pub fn append_signal(&self, signal: &NewSignal) -> Result<()> {
        insert_signal(&self.conn, signal)?;
        Ok(())
    }

    /// Append a bounded transaction of observations. Extractors should batch
    /// high-volume output so one transcript/chat import does not fsync per row.
    pub fn append_signals(&mut self, signals: &[NewSignal]) -> Result<usize> {
        if signals.len() > 10_000 {
            return Err(ResearchError::Validation(
                "signal batches are capped at 10000 rows".into(),
            ));
        }
        let tx = self.conn.transaction()?;
        for signal in signals {
            insert_signal(&tx, signal)?;
        }
        tx.commit()?;
        Ok(signals.len())
    }

    pub fn add_coding(&mut self, coding: &NewCoding) -> Result<()> {
        range(coding.start_ms, coding.end_ms)?;
        confidence(coding.confidence)?;
        if coding.author.trim().is_empty() {
            return Err(ResearchError::Validation("coding author is empty".into()));
        }
        let tx = self.conn.transaction()?;
        ensure_same_project(&tx, coding)?;
        let now = Utc::now().to_rfc3339();
        tx.execute(
            "INSERT INTO codings VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?13)",
            params![
                coding.id.to_string(),
                coding.project_id.to_string(),
                coding.source_id.to_string(),
                coding.code_id.to_string(),
                integer(coding.start_ms, "coding start")?,
                integer(coding.end_ms, "coding end")?,
                coding.excerpt,
                coding.note,
                coding.author.trim(),
                coding.origin.as_str(),
                coding.confidence,
                coding.provenance_id.map(|id| id.to_string()),
                now
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn signal_count(&self, project_id: Uuid) -> Result<u64> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM signals WHERE project_id=?1",
            [project_id.to_string()],
            |row| row.get(0),
        )?;
        Ok(count.max(0) as u64)
    }

    pub fn list_projects(&self) -> Result<Vec<Project>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,name,description,created_at,updated_at
             FROM projects ORDER BY updated_at DESC,id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Project {
                id: parse_uuid(row.get::<_, String>(0)?)?,
                name: row.get(1)?,
                description: row.get(2)?,
                created_at: parse_time(row.get::<_, String>(3)?)?,
                updated_at: parse_time(row.get::<_, String>(4)?)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn list_sources(&self, project_id: Uuid) -> Result<Vec<Source>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,project_id,recording_id,kind,title,uri,duration_ms,attributes_json
             FROM sources WHERE project_id=?1 ORDER BY created_at,id",
        )?;
        let rows = stmt.query_map([project_id.to_string()], |row| {
            let duration: Option<i64> = row.get(6)?;
            Ok(Source {
                id: parse_uuid(row.get::<_, String>(0)?)?,
                project_id: parse_uuid(row.get::<_, String>(1)?)?,
                recording_id: row
                    .get::<_, Option<String>>(2)?
                    .map(parse_uuid)
                    .transpose()?,
                kind: row.get(3)?,
                title: row.get(4)?,
                uri: row.get(5)?,
                duration_ms: duration.map(|value| value.max(0) as u64),
                attributes: parse_json(row.get::<_, String>(7)?)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn list_codes(&self, project_id: Uuid) -> Result<Vec<Code>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,project_id,parent_id,name,description,color
             FROM codes WHERE project_id=?1 ORDER BY parent_id,name,id",
        )?;
        let rows = stmt.query_map([project_id.to_string()], |row| {
            Ok(Code {
                id: parse_uuid(row.get::<_, String>(0)?)?,
                project_id: parse_uuid(row.get::<_, String>(1)?)?,
                parent_id: row
                    .get::<_, Option<String>>(2)?
                    .map(parse_uuid)
                    .transpose()?,
                name: row.get(3)?,
                description: row.get(4)?,
                color: row.get(5)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn list_signals(
        &self,
        project_id: Uuid,
        source_id: Option<Uuid>,
        kind: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Signal>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,project_id,source_id,start_ms,end_ms,kind,label,payload_json,
                    confidence,provenance_id
             FROM signals
             WHERE project_id=?1
               AND (?2 IS NULL OR source_id=?2)
               AND (?3 IS NULL OR kind=?3)
             ORDER BY start_ms,id LIMIT ?4 OFFSET ?5",
        )?;
        let rows = stmt.query_map(
            params![
                project_id.to_string(),
                source_id.map(|id| id.to_string()),
                kind,
                limit.clamp(1, 1_000) as i64,
                offset as i64
            ],
            |row| {
                Ok(Signal {
                    id: parse_uuid(row.get::<_, String>(0)?)?,
                    project_id: parse_uuid(row.get::<_, String>(1)?)?,
                    source_id: parse_uuid(row.get::<_, String>(2)?)?,
                    start_ms: row.get::<_, i64>(3)?.max(0) as u64,
                    end_ms: row.get::<_, i64>(4)?.max(0) as u64,
                    kind: row.get(5)?,
                    label: row.get(6)?,
                    payload: parse_json(row.get::<_, String>(7)?)?,
                    confidence: row.get(8)?,
                    provenance_id: row
                        .get::<_, Option<String>>(9)?
                        .map(parse_uuid)
                        .transpose()?,
                })
            },
        )?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn list_codings(&self, project_id: Uuid) -> Result<Vec<Coding>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,project_id,source_id,code_id,start_ms,end_ms,excerpt,note,
                    author,origin,confidence
             FROM codings WHERE project_id=?1 ORDER BY source_id,start_ms,id",
        )?;
        let rows = stmt.query_map([project_id.to_string()], |row| {
            Ok(Coding {
                id: parse_uuid(row.get::<_, String>(0)?)?,
                project_id: parse_uuid(row.get::<_, String>(1)?)?,
                source_id: parse_uuid(row.get::<_, String>(2)?)?,
                code_id: parse_uuid(row.get::<_, String>(3)?)?,
                start_ms: row.get::<_, i64>(4)?.max(0) as u64,
                end_ms: row.get::<_, i64>(5)?.max(0) as u64,
                excerpt: row.get(6)?,
                note: row.get(7)?,
                author: row.get(8)?,
                origin: row.get(9)?,
                confidence: row.get(10)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn export_project(&self, project_id: Uuid) -> Result<ProjectExport> {
        let project = self
            .list_projects()?
            .into_iter()
            .find(|project| project.id == project_id)
            .ok_or_else(|| ResearchError::Validation("project not found".into()))?;
        let signals = self.list_signals(project_id, None, None, 1_000, 0)?;
        if signals.len() as u64 != self.signal_count(project_id)? {
            return Err(ResearchError::Validation(
                "project export exceeds the 1000-signal portable limit".into(),
            ));
        }
        Ok(ProjectExport {
            schema_version: self.schema_version()?,
            project,
            sources: self.list_sources(project_id)?,
            codes: self.list_codes(project_id)?,
            signals,
            codings: self.list_codings(project_id)?,
        })
    }

    /// Import legacy Crunchr videos and transcript segments. Stable UUIDv5
    /// identifiers make retries idempotent and the report digest makes count
    /// reconciliation auditable.
    pub fn migrate_crunchr(
        &mut self,
        path: impl AsRef<Path>,
        project_id: Uuid,
    ) -> Result<MigrationReport> {
        let legacy = Connection::open_with_flags(
            path.as_ref(),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        let mut rows = legacy.prepare(
            "SELECT v.recording_id,v.channel_name,v.title,v.video_path,
                    s.segment_index,s.start_sec,s.end_sec,s.text,s.speaker,s.confidence
             FROM videos v JOIN segments s ON s.video_id=v.id
             ORDER BY v.recording_id,s.segment_index",
        )?;
        let data = rows
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, f64>(5)?,
                    row.get::<_, f64>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<f64>>(9)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut report = MigrationReport {
            source: path.as_ref().display().to_string(),
            ..MigrationReport::default()
        };
        let tx = self.conn.transaction()?;
        let mut digest = Sha256::new();
        for (recording, channel, title, uri, index, start, end, text, speaker, conf) in data {
            report.examined += 1;
            digest.update(format!("{recording}:{index}:{start}:{end}:{text}"));
            let mut source_id = stable_id("crunchr-source", &recording);
            let recording_id = Uuid::parse_str(&recording).ok();
            tx.execute(
                "INSERT OR IGNORE INTO sources VALUES(?1,?2,?3,'recording',?4,?5,NULL,?6,?7)",
                params![
                    source_id.to_string(),
                    project_id.to_string(),
                    recording_id.map(|id| id.to_string()),
                    title,
                    uri,
                    serde_json::json!({"channel": channel}).to_string(),
                    Utc::now().to_rfc3339()
                ],
            )?;
            if let Some(recording_id) = recording_id {
                let stored: String = tx.query_row(
                    "SELECT id FROM sources WHERE project_id=?1 AND recording_id=?2",
                    params![project_id.to_string(), recording_id.to_string()],
                    |row| row.get(0),
                )?;
                source_id = Uuid::parse_str(&stored)
                    .map_err(|_| ResearchError::Validation("invalid source UUID".into()))?;
            }
            let signal_id = stable_id("crunchr-segment", &format!("{recording}:{index}"));
            let inserted = tx.execute(
                "INSERT OR IGNORE INTO signals VALUES(?1,?2,?3,?4,?5,'transcript.utterance',?6,?7,?8,NULL,?9)",
                params![
                    signal_id.to_string(),
                    project_id.to_string(),
                    source_id.to_string(),
                    seconds_ms(start),
                    seconds_ms(end).max(seconds_ms(start)),
                    speaker.unwrap_or_else(|| "Speaker".into()),
                    serde_json::json!({"text": text, "segment_index": index}).to_string(),
                    conf.filter(|value| value.is_finite()).map(|value| value.clamp(0.0, 1.0)),
                    Utc::now().to_rfc3339()
                ],
            )?;
            if inserted == 1 {
                report.inserted += 1;
            } else {
                report.skipped += 1;
            }
        }
        tx.commit()?;
        report.digest = format!("{:x}", digest.finalize());
        Ok(report)
    }

    pub fn migrate_cuepoints(
        &mut self,
        path: impl AsRef<Path>,
        project_id: Uuid,
    ) -> Result<MigrationReport> {
        let legacy = open_legacy(path.as_ref())?;
        let mut stmt = legacy.prepare(
            "SELECT recording_id,threshold,generated_at,points
             FROM cuepoints ORDER BY recording_id,threshold",
        )?;
        let sets = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, f64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut report = MigrationReport {
            source: path.as_ref().display().to_string(),
            ..MigrationReport::default()
        };
        let tx = self.conn.transaction()?;
        let mut digest = Sha256::new();
        for (recording, threshold, generated_at, raw) in sets {
            let points: Vec<Value> = serde_json::from_str(&raw)?;
            let source_id = ensure_legacy_source(&tx, project_id, &recording)?;
            for (index, point) in points.into_iter().enumerate() {
                report.examined += 1;
                let time = point
                    .get("time_sec")
                    .and_then(Value::as_f64)
                    .unwrap_or_default();
                digest.update(format!("{recording}:{threshold}:{index}:{raw}"));
                let id = stable_id("cuepoint", &format!("{recording}:{threshold}:{index}"));
                let inserted = tx.execute(
                    "INSERT OR IGNORE INTO signals VALUES(
                     ?1,?2,?3,?4,?4,'visual.scene_change','Scene change',?5,NULL,NULL,?6)",
                    params![
                        id.to_string(),
                        project_id.to_string(),
                        source_id.to_string(),
                        seconds_ms(time),
                        serde_json::json!({
                            "threshold": threshold,
                            "generated_at": generated_at,
                            "frame": point.get("frame").cloned().unwrap_or(Value::Null)
                        })
                        .to_string(),
                        Utc::now().to_rfc3339()
                    ],
                )?;
                if inserted == 1 {
                    report.inserted += 1;
                } else {
                    report.skipped += 1;
                }
            }
        }
        tx.commit()?;
        report.digest = format!("{:x}", digest.finalize());
        Ok(report)
    }

    pub fn migrate_viewguard(
        &mut self,
        path: impl AsRef<Path>,
        project_id: Uuid,
    ) -> Result<MigrationReport> {
        let legacy = open_legacy(path.as_ref())?;
        let mut stmt = legacy.prepare(
            "SELECT channel_id,detector,score,confidence,ts,evidence
             FROM signals ORDER BY channel_id,ts,id",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, f64>(2)?,
                    row.get::<_, f64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut report = MigrationReport {
            source: path.as_ref().display().to_string(),
            ..MigrationReport::default()
        };
        let tx = self.conn.transaction()?;
        let mut digest = Sha256::new();
        let mut origins = std::collections::HashMap::<String, DateTime<Utc>>::new();
        for (channel, detector, score, conf, timestamp, evidence) in rows {
            report.examined += 1;
            digest.update(format!("{channel}:{detector}:{timestamp}:{score}:{conf}"));
            let parsed = DateTime::parse_from_rfc3339(&timestamp)
                .map(|value| value.with_timezone(&Utc))
                .map_err(|_| ResearchError::Validation("invalid Viewguard timestamp".into()))?;
            let origin = origins.entry(channel.clone()).or_insert(parsed);
            let relative_ms = parsed
                .signed_duration_since(*origin)
                .num_milliseconds()
                .max(0);
            let source_id = ensure_channel_source(&tx, project_id, &channel)?;
            let id = stable_id("viewguard", &format!("{channel}:{detector}:{timestamp}"));
            let evidence_value = evidence
                .as_deref()
                .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
                .unwrap_or(Value::Null);
            let inserted = tx.execute(
                "INSERT OR IGNORE INTO signals VALUES(
                 ?1,?2,?3,?4,?4,'audience.anomaly',?5,?6,?7,NULL,?8)",
                params![
                    id.to_string(),
                    project_id.to_string(),
                    source_id.to_string(),
                    relative_ms,
                    detector,
                    serde_json::json!({
                        "score": score,
                        "observed_at": timestamp,
                        "evidence": evidence_value
                    })
                    .to_string(),
                    conf.clamp(0.0, 1.0),
                    Utc::now().to_rfc3339()
                ],
            )?;
            if inserted == 1 {
                report.inserted += 1;
            } else {
                report.skipped += 1;
            }
        }
        tx.commit()?;
        report.digest = format!("{:x}", digest.finalize());
        Ok(report)
    }
}

fn parse_uuid(value: String) -> rusqlite::Result<Uuid> {
    Uuid::parse_str(&value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            value.len(),
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}
fn parse_time(value: String) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                value.len(),
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })
}
fn parse_json(value: String) -> rusqlite::Result<Value> {
    serde_json::from_str(&value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            value.len(),
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}
fn stable_id(domain: &str, value: &str) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("strivo:{domain}:{value}").as_bytes(),
    )
}
fn seconds_ms(value: f64) -> i64 {
    if value.is_finite() {
        (value.max(0.0) * 1_000.0).round().min(i64::MAX as f64) as i64
    } else {
        0
    }
}
fn open_legacy(path: &Path) -> Result<Connection> {
    Ok(Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?)
}
fn ensure_legacy_source(tx: &Transaction<'_>, project_id: Uuid, recording: &str) -> Result<Uuid> {
    let id = stable_id("crunchr-source", recording);
    tx.execute(
        "INSERT OR IGNORE INTO sources VALUES(
         ?1,?2,?3,'recording',?4,NULL,NULL,'{}',?5)",
        params![
            id.to_string(),
            project_id.to_string(),
            Uuid::parse_str(recording).ok().map(|id| id.to_string()),
            recording,
            Utc::now().to_rfc3339()
        ],
    )?;
    Ok(id)
}
fn ensure_channel_source(tx: &Transaction<'_>, project_id: Uuid, channel: &str) -> Result<Uuid> {
    let id = stable_id("viewguard-channel", channel);
    tx.execute(
        "INSERT OR IGNORE INTO sources VALUES(
         ?1,?2,NULL,'dataset',?3,NULL,NULL,?4,?5)",
        params![
            id.to_string(),
            project_id.to_string(),
            format!("Viewguard · {channel}"),
            serde_json::json!({"channel_id": channel}).to_string(),
            Utc::now().to_rfc3339()
        ],
    )?;
    Ok(id)
}

fn ensure_same_project(tx: &Transaction<'_>, coding: &NewCoding) -> Result<()> {
    let source: String = tx.query_row(
        "SELECT project_id FROM sources WHERE id=?1",
        [coding.source_id.to_string()],
        |row| row.get(0),
    )?;
    let code: String = tx.query_row(
        "SELECT project_id FROM codes WHERE id=?1",
        [coding.code_id.to_string()],
        |row| row.get(0),
    )?;
    if source != coding.project_id.to_string() || code != source {
        return Err(ResearchError::Validation(
            "project, source, and code must match".into(),
        ));
    }
    Ok(())
}

fn insert_signal(conn: &Connection, signal: &NewSignal) -> Result<()> {
    range(signal.start_ms, signal.end_ms)?;
    confidence(signal.confidence)?;
    object(&signal.payload, "signal payload")?;
    conn.execute(
        "INSERT INTO signals VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        params![
            signal.id.to_string(),
            signal.project_id.to_string(),
            signal.source_id.to_string(),
            integer(signal.start_ms, "signal start")?,
            integer(signal.end_ms, "signal end")?,
            signal.kind.trim(),
            signal.label.trim(),
            serde_json::to_string(&signal.payload)?,
            signal.confidence,
            signal.provenance_id.map(|id| id.to_string()),
            Utc::now().to_rfc3339()
        ],
    )?;
    Ok(())
}

fn ensure_object_project(conn: &Connection, table: &str, id: Uuid, project_id: Uuid) -> Result<()> {
    let sql = match table {
        "source" | "sources" => "SELECT project_id FROM sources WHERE id=?1",
        "case" | "cases" => "SELECT project_id FROM cases WHERE id=?1",
        "code" | "codes" => "SELECT project_id FROM codes WHERE id=?1",
        "coding" | "codings" => "SELECT project_id FROM codings WHERE id=?1",
        "memo" | "memos" => "SELECT project_id FROM memos WHERE id=?1",
        _ => {
            return Err(ResearchError::Validation(
                "unsupported research object".into(),
            ))
        }
    };
    let owner: Option<String> = conn
        .query_row(sql, [id.to_string()], |row| row.get(0))
        .optional()?;
    if owner.as_deref() != Some(project_id.to_string().as_str()) {
        return Err(ResearchError::Validation(
            "research objects must belong to the same project".into(),
        ));
    }
    Ok(())
}

fn range(start: u64, end: u64) -> Result<()> {
    if end < start {
        return Err(ResearchError::Validation("invalid evidence range".into()));
    }
    Ok(())
}
fn confidence(value: Option<f64>) -> Result<()> {
    if value.is_some_and(|n| !n.is_finite() || !(0.0..=1.0).contains(&n)) {
        return Err(ResearchError::Validation("invalid confidence".into()));
    }
    Ok(())
}
fn object(value: &Value, label: &str) -> Result<()> {
    if !value.is_object() {
        return Err(ResearchError::Validation(format!(
            "{label} must be an object"
        )));
    }
    Ok(())
}
fn valid_color(value: &str) -> bool {
    value.len() == 7
        && value.starts_with('#')
        && value[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
}
fn integer(value: u64, label: &str) -> Result<i64> {
    i64::try_from(value).map_err(|_| ResearchError::Validation(format!("{label} is too large")))
}
fn optional_i64(value: Option<u64>, label: &str) -> Result<Option<i64>> {
    value.map(|value| integer(value, label)).transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(project_id: Uuid) -> NewSource {
        NewSource {
            id: Uuid::new_v4(),
            project_id,
            recording_id: Some(Uuid::new_v4()),
            kind: SourceKind::Recording,
            title: "Episode 1".into(),
            uri: Some("file:///episode.mkv".into()),
            duration_ms: Some(60_000),
            attributes: serde_json::json!({"channel": "researcher"}),
        }
    }

    #[test]
    fn schema_is_idempotent_and_versioned() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("research.db");
        assert_eq!(
            ResearchStore::open(&path)
                .unwrap()
                .schema_version()
                .unwrap(),
            1
        );
        assert_eq!(
            ResearchStore::open(&path)
                .unwrap()
                .schema_version()
                .unwrap(),
            1
        );
    }

    #[test]
    fn append_only_signal_preserves_provenance() {
        let store = ResearchStore::open(":memory:").unwrap();
        let project = store.create_project("Study", "").unwrap();
        let source = source(project.id);
        store.upsert_source(&source).unwrap();
        let provenance = Provenance {
            id: Uuid::new_v4(),
            producer: "crunchr".into(),
            producer_version: "0.3.0".into(),
            method: "transcribe".into(),
            model: Some("whisper".into()),
            parameters: serde_json::json!({"language": "en"}),
            input_digest: Some("sha256:abc".into()),
        };
        store.add_provenance(&provenance).unwrap();
        let signal = NewSignal {
            id: Uuid::new_v4(),
            project_id: project.id,
            source_id: source.id,
            start_ms: 100,
            end_ms: 900,
            kind: "transcript.utterance".into(),
            label: "Speaker 1".into(),
            payload: serde_json::json!({"text": "hello"}),
            confidence: Some(0.97),
            provenance_id: Some(provenance.id),
        };
        store.append_signal(&signal).unwrap();
        assert_eq!(store.signal_count(project.id).unwrap(), 1);
        assert!(store.append_signal(&signal).is_err());
    }

    #[test]
    fn hierarchical_codes_support_time_ranged_human_coding() {
        let mut store = ResearchStore::open(":memory:").unwrap();
        let project = store.create_project("Study", "").unwrap();
        let source = source(project.id);
        store.upsert_source(&source).unwrap();
        let parent = Code {
            id: Uuid::new_v4(),
            project_id: project.id,
            parent_id: None,
            name: "Community".into(),
            description: String::new(),
            color: "#7c5cff".into(),
        };
        store.create_code(&parent).unwrap();
        let child = Code {
            id: Uuid::new_v4(),
            project_id: project.id,
            parent_id: Some(parent.id),
            name: "Belonging".into(),
            description: String::new(),
            color: "#00e5ff".into(),
        };
        store.create_code(&child).unwrap();
        store
            .add_coding(&NewCoding {
                id: Uuid::new_v4(),
                project_id: project.id,
                source_id: source.id,
                code_id: child.id,
                start_ms: 1_000,
                end_ms: 2_500,
                excerpt: "we built this together".into(),
                note: "Collective identity".into(),
                author: "analyst@example.test".into(),
                origin: CodingOrigin::Human,
                confidence: None,
                provenance_id: None,
            })
            .unwrap();
    }

    #[test]
    fn rejects_invalid_and_cross_project_codings() {
        let mut store = ResearchStore::open(":memory:").unwrap();
        let first = store.create_project("First", "").unwrap();
        let second = store.create_project("Second", "").unwrap();
        let source = source(first.id);
        store.upsert_source(&source).unwrap();
        let code = Code {
            id: Uuid::new_v4(),
            project_id: second.id,
            parent_id: None,
            name: "Other".into(),
            description: String::new(),
            color: "#ffffff".into(),
        };
        store.create_code(&code).unwrap();
        let coding = NewCoding {
            id: Uuid::new_v4(),
            project_id: first.id,
            source_id: source.id,
            code_id: code.id,
            start_ms: 10,
            end_ms: 5,
            excerpt: String::new(),
            note: String::new(),
            author: "analyst".into(),
            origin: CodingOrigin::Model,
            confidence: Some(2.0),
            provenance_id: None,
        };
        assert!(store.add_coding(&coding).is_err());
        let mut cross_project = coding;
        cross_project.end_ms = 50;
        cross_project.confidence = Some(0.5);
        assert!(store.add_coding(&cross_project).is_err());
    }

    #[test]
    fn export_round_trips_complete_small_project() {
        let store = ResearchStore::open(":memory:").unwrap();
        let project = store.create_project("Export me", "portable").unwrap();
        let source = source(project.id);
        store.upsert_source(&source).unwrap();
        let export = store.export_project(project.id).unwrap();
        assert_eq!(export.schema_version, 1);
        assert_eq!(export.project, project);
        assert_eq!(export.sources.len(), 1);
        assert!(export.signals.is_empty());
    }

    #[test]
    fn crunchr_migration_is_idempotent_and_reconciled() {
        let dir = tempfile::tempdir().unwrap();
        let legacy_path = dir.path().join("crunchr.db");
        let legacy = Connection::open(&legacy_path).unwrap();
        legacy
            .execute_batch(
                "CREATE TABLE videos(
                    id INTEGER PRIMARY KEY, recording_id TEXT, channel_name TEXT,
                    title TEXT, video_path TEXT);
                 CREATE TABLE segments(
                    video_id INTEGER, segment_index INTEGER, start_sec REAL,
                    end_sec REAL, text TEXT, speaker TEXT, confidence REAL);
                 INSERT INTO videos VALUES(1,'00000000-0000-0000-0000-000000000001',
                    'channel','Episode','/episode.mkv');
                 INSERT INTO segments VALUES(1,0,1.25,2.5,'hello','Alice',0.9);
                 INSERT INTO segments VALUES(1,1,2.5,4.0,'world','Bob',NULL);",
            )
            .unwrap();
        drop(legacy);
        let mut store = ResearchStore::open(":memory:").unwrap();
        let project = store.create_project("Migrated", "").unwrap();
        let first = store.migrate_crunchr(&legacy_path, project.id).unwrap();
        assert_eq!((first.examined, first.inserted, first.skipped), (2, 2, 0));
        assert_eq!(store.signal_count(project.id).unwrap(), 2);
        let second = store.migrate_crunchr(&legacy_path, project.id).unwrap();
        assert_eq!(
            (second.examined, second.inserted, second.skipped),
            (2, 0, 2)
        );
        assert_eq!(first.digest, second.digest);
        let signals = store
            .list_signals(project.id, None, Some("transcript.utterance"), 10, 0)
            .unwrap();
        assert_eq!(signals[0].start_ms, 1_250);
        assert_eq!(signals[0].label, "Alice");
    }

    #[test]
    fn visual_and_audience_adapters_emit_normalized_signals() {
        let dir = tempfile::tempdir().unwrap();
        let cue_path = dir.path().join("cuepoints.db");
        let cue = Connection::open(&cue_path).unwrap();
        cue.execute_batch(
            "CREATE TABLE cuepoints(
                recording_id TEXT,threshold REAL,generated_at TEXT,points TEXT);
             INSERT INTO cuepoints VALUES(
                'rec-1',0.4,'2026-01-01T00:00:00Z',
                '[{\"time_sec\":1.5,\"frame\":45}]');",
        )
        .unwrap();
        drop(cue);
        let view_path = dir.path().join("viewguard.db");
        let view = Connection::open(&view_path).unwrap();
        view.execute_batch(
            "CREATE TABLE signals(
                id INTEGER,channel_id TEXT,detector TEXT,score REAL,
                confidence REAL,ts TEXT,evidence TEXT);
             INSERT INTO signals VALUES(
                1,'chan','spike',0.8,0.9,'2026-01-01T00:00:00Z','{\"delta\":42}');",
        )
        .unwrap();
        drop(view);
        let mut store = ResearchStore::open(":memory:").unwrap();
        let project = store.create_project("Signals", "").unwrap();
        assert_eq!(
            store
                .migrate_cuepoints(&cue_path, project.id)
                .unwrap()
                .inserted,
            1
        );
        assert_eq!(
            store
                .migrate_viewguard(&view_path, project.id)
                .unwrap()
                .inserted,
            1
        );
        let signals = store.list_signals(project.id, None, None, 10, 0).unwrap();
        assert_eq!(signals.len(), 2);
        assert!(signals
            .iter()
            .any(|signal| signal.kind == "visual.scene_change"));
        assert!(signals
            .iter()
            .any(|signal| signal.kind == "audience.anomaly"));
    }

    #[test]
    fn cases_memos_and_relationships_preserve_research_context() {
        let store = ResearchStore::open(":memory:").unwrap();
        let project = store.create_project("Context", "").unwrap();
        let source = source(project.id);
        store.upsert_source(&source).unwrap();
        let case = ResearchCase {
            id: Uuid::new_v4(),
            project_id: project.id,
            name: "Core community".into(),
            description: String::new(),
            attributes: serde_json::json!({"cohort": "long_term"}),
        };
        store.create_case(&case).unwrap();
        store.assign_source_case(source.id, case.id).unwrap();
        store
            .add_memo(&Memo {
                id: Uuid::new_v4(),
                project_id: project.id,
                source_id: Some(source.id),
                coding_id: None,
                title: "Reflexive note".into(),
                body: "Researcher position and assumptions.".into(),
                author: "analyst".into(),
            })
            .unwrap();
        store
            .add_relationship(&Relationship {
                id: Uuid::new_v4(),
                project_id: project.id,
                from_kind: "source".into(),
                from_id: source.id,
                to_kind: "case".into(),
                to_id: case.id,
                relation: "belongs_to".into(),
                note: String::new(),
                author: "analyst".into(),
            })
            .unwrap();
    }

    #[test]
    fn case_assignment_is_atomic_across_projects() {
        let mut store = ResearchStore::open(":memory:").unwrap();
        let first = store.create_project("First", "").unwrap();
        let second = store.create_project("Second", "").unwrap();
        let foreign = source(second.id);
        store.upsert_source(&foreign).unwrap();
        let case = ResearchCase {
            id: Uuid::new_v4(),
            project_id: first.id,
            name: "Must roll back".into(),
            description: String::new(),
            attributes: serde_json::json!({}),
        };
        assert!(store
            .create_case_with_sources(&case, &[foreign.id])
            .is_err());
        assert!(store.create_case(&case).is_ok());
    }

    #[test]
    fn concurrent_batched_writers_reconcile_without_loss() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("research.db");
        let store = ResearchStore::open(&path).unwrap();
        let project = store.create_project("Concurrent", "").unwrap();
        let source = source(project.id);
        store.upsert_source(&source).unwrap();
        drop(store);
        std::thread::scope(|scope| {
            for worker in 0..4 {
                let path = path.clone();
                let source = source.clone();
                scope.spawn(move || {
                    let mut store = ResearchStore::open(path).unwrap();
                    let batch = (0..100)
                        .map(|index| NewSignal {
                            id: stable_id("concurrency", &format!("{worker}:{index}")),
                            project_id: source.project_id,
                            source_id: source.id,
                            start_ms: index,
                            end_ms: index,
                            kind: "test.concurrent".into(),
                            label: worker.to_string(),
                            payload: serde_json::json!({}),
                            confidence: None,
                            provenance_id: None,
                        })
                        .collect::<Vec<_>>();
                    store.append_signals(&batch).unwrap();
                });
            }
        });
        let store = ResearchStore::open(&path).unwrap();
        assert_eq!(store.signal_count(project.id).unwrap(), 400);
    }

    #[test]
    #[ignore = "release-scale benchmark; run explicitly on representative storage"]
    fn million_signal_scale_gate() {
        let mut store = ResearchStore::open(":memory:").unwrap();
        let project = store.create_project("Scale", "").unwrap();
        let source = source(project.id);
        store.upsert_source(&source).unwrap();
        for chunk in 0..100 {
            let batch = (0..10_000)
                .map(|index| {
                    let ordinal = chunk * 10_000 + index;
                    NewSignal {
                        id: stable_id("scale", &ordinal.to_string()),
                        project_id: project.id,
                        source_id: source.id,
                        start_ms: ordinal,
                        end_ms: ordinal,
                        kind: "benchmark.signal".into(),
                        label: String::new(),
                        payload: serde_json::json!({}),
                        confidence: None,
                        provenance_id: None,
                    }
                })
                .collect::<Vec<_>>();
            store.append_signals(&batch).unwrap();
        }
        assert_eq!(store.signal_count(project.id).unwrap(), 1_000_000);
    }
}
