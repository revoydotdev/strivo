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
use thiserror::Error;
use uuid::Uuid;

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
        range(signal.start_ms, signal.end_ms)?;
        confidence(signal.confidence)?;
        object(&signal.payload, "signal payload")?;
        self.conn.execute(
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
}
