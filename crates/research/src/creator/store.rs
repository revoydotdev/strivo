//! The store handle, the project rows, and the stable-ID link to the research
//! kernel. Sources and edits are in [`super::store_sources`]; the delivery
//! chain (variant → render plan → artifact → package → publication → metric)
//! is in [`super::store_delivery`].

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};

use super::entities::Project;
use super::ids::*;
use super::schema;
use crate::Result;

/// Owns one connection to the Creator schema. PRAGMAs match the rest of the
/// workspace (`src/recording/persist.rs`, `crates/research`): WAL, normal
/// synchronous, foreign keys **on**, 30 s busy timeout. Foreign keys off
/// would make every `ON DELETE CASCADE` in the schema a comment.
pub struct Store {
    /// Shared with [`store_delivery`], which writes the same connection.
    pub(super) conn: Connection,
}

impl Store {
    /// Open `path`, creating its parent directory, and bring the Creator
    /// schema up to this build's version.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if path != Path::new(":memory:") {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let mut conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA foreign_keys=ON;
             PRAGMA busy_timeout=30000;",
        )?;
        schema::migrate(&mut conn, schema::MIGRATIONS)?;
        Ok(Self { conn })
    }

    /// In-memory store for tests.
    pub fn open_in_memory() -> Result<Self> {
        Self::open(":memory:")
    }

    /// Take over an existing connection without migrating it. For the
    /// migration tests, which need to observe version 0.
    #[cfg(test)]
    pub fn from_connection(conn: Connection) -> Self {
        Self { conn }
    }

    /// Run one statement, bypassing every writer. For the malformed-input
    /// tests, which have to produce the state a hand-edited or corrupt
    /// database holds — something no public method will write, because
    /// writing it is the defect.
    #[cfg(test)]
    pub fn raw_execute(&self, sql: &str, args: &[String]) -> Result<()> {
        let refs: Vec<&dyn rusqlite::ToSql> =
            args.iter().map(|a| a as &dyn rusqlite::ToSql).collect();
        self.conn.execute(sql, refs.as_slice())?;
        Ok(())
    }

    /// The version line currently recorded, for diagnostics and tests.
    pub fn schema_version(&self) -> Result<i64> {
        schema::applied_version(&self.conn)
    }

    /// Names of the Creator tables this database actually has. The PVR proof
    /// runs this against a PVR-created file and expects nothing.
    pub fn creator_tables(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'creator_%' ORDER BY name",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn insert_project(&self, p: &Project) -> Result<()> {
        self.conn.execute(
            "INSERT INTO creator_projects
             (id,name,description,channel,series,settings_json,created_at,updated_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                p.id.to_string(),
                p.name,
                p.description,
                p.channel,
                p.series,
                p.settings_json,
                p.created_at,
                p.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn project(&self, id: ProjectId) -> Result<Option<Project>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id,name,description,channel,series,settings_json,created_at,updated_at
                 FROM creator_projects WHERE id=?1",
                [id.to_string()],
                |r| {
                    let uuid = |i: usize, v: String| -> rusqlite::Result<uuid::Uuid> {
                        uuid::Uuid::parse_str(&v).map_err(|e| {
                            rusqlite::Error::InvalidColumnType(
                                i,
                                e.to_string(),
                                rusqlite::types::Type::Text,
                            )
                        })
                    };
                    Ok(Project {
                        id: ProjectId::from_uuid(uuid(0, r.get(0)?)?),
                        name: r.get(1)?,
                        description: r.get(2)?,
                        channel: r.get(3)?,
                        series: r.get(4)?,
                        settings_json: r.get(5)?,
                        created_at: r.get(6)?,
                        updated_at: r.get(7)?,
                    })
                },
            )
            .optional()?)
    }

    /// Link a research-kernel project by stable ID. Idempotent per pair, and
    /// deliberately many-to-many: the report forbids forcing this into a
    /// one-to-one relationship, because a cross-recording compilation spans
    /// projects and a research project may cover several creator projects.
    pub fn link_research_project(
        &self,
        creator: ProjectId,
        research: ResearchProjectId,
        created_at: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO creator_project_research_links
             (creator_project_id,research_project_id,relation,created_at)
             VALUES(?1,?2,'linked',?3)
             ON CONFLICT(creator_project_id,research_project_id) DO NOTHING",
            params![creator.to_string(), research.to_string(), created_at],
        )?;
        Ok(())
    }

    /// Every research project this creator project is linked to. A test
    /// asserting `len() != 1` is asserting the relationship is not one-to-one.
    pub fn research_links(&self, creator: ProjectId) -> Result<Vec<ResearchProjectId>> {
        let mut stmt = self.conn.prepare(
            "SELECT research_project_id FROM creator_project_research_links
             WHERE creator_project_id=?1 ORDER BY research_project_id",
        )?;
        let rows = stmt.query_map([creator.to_string()], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for id in rows {
            out.push(id?.parse()?);
        }
        Ok(out)
    }

    /// How many creator projects point at one research project id.
    pub fn research_link_inbound(&self, research: ResearchProjectId) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM creator_project_research_links WHERE research_project_id=?1",
            [research.to_string()],
            |r| r.get(0),
        )?)
    }
}
