//! Source assets, capture sessions, transcript revisions and composition
//! revisions — the rows that carry a project's actual work. The project and
//! research-link rows are in [`super::store`].

use std::path::Path;

use rusqlite::{params, OptionalExtension};
use strivo_editor::time::MediaTime;

use super::entities::*;
use super::ids::*;
use super::store::Store;
use crate::{ResearchError, Result};

impl Store {
    pub fn insert_source_asset(&self, s: &SourceAsset) -> Result<()> {
        self.conn.execute(
            "INSERT INTO creator_source_assets
             (id,project_id,kind,title,media_fingerprint,locator,origin,rights_json,
              track_type,codec,duration_us,created_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            params![
                s.id.to_string(),
                s.project_id.to_string(),
                s.kind,
                s.title,
                s.media_fingerprint,
                s.locator.as_ref().map(|p| p.display().to_string()),
                s.origin,
                s.rights_json,
                s.track_type,
                s.codec,
                s.duration.map(|d| d.micros()),
                s.created_at,
            ],
        )?;
        Ok(())
    }

    /// Look a source up by its **identity**. A `ProjectId` cannot be passed
    /// here, and an id that names no row is `None` — never a wrong row.
    pub fn source_asset(&self, id: SourceAssetId) -> Result<Option<SourceAsset>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id,project_id,kind,title,media_fingerprint,locator,origin,rights_json,
                        track_type,codec,duration_us,created_at
                 FROM creator_source_assets WHERE id=?1",
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
                    Ok(SourceAsset {
                        id: SourceAssetId::from_uuid(uuid(0, r.get(0)?)?),
                        project_id: ProjectId::from_uuid(uuid(1, r.get(1)?)?),
                        kind: r.get(2)?,
                        title: r.get(3)?,
                        media_fingerprint: r.get(4)?,
                        locator: r.get::<_, Option<String>>(5)?.map(std::path::PathBuf::from),
                        origin: r.get(6)?,
                        rights_json: r.get(7)?,
                        track_type: r.get(8)?,
                        codec: r.get(9)?,
                        duration: r
                            .get::<_, Option<i64>>(10)?
                            .map(MediaTime::from_micros)
                            .transpose()
                            .map_err(|e| {
                                rusqlite::Error::InvalidColumnType(
                                    10,
                                    e.to_string(),
                                    rusqlite::types::Type::Integer,
                                )
                            })?,
                        created_at: r.get(11)?,
                    })
                },
            )
            .optional()?)
    }

    /// Every source in a project. Many, and none of them identified by path.
    pub fn sources_of(&self, project: ProjectId) -> Result<Vec<SourceAsset>> {
        let mut stmt = self.conn.prepare(
            "SELECT id FROM creator_source_assets WHERE project_id=?1 ORDER BY created_at, id",
        )?;
        let ids: Vec<String> = stmt
            .query_map([project.to_string()], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<_, _>>()?;
        ids.into_iter()
            .map(|raw| {
                let id: SourceAssetId = raw.parse()?;
                self.source_asset(id)?
                    .ok_or_else(|| ResearchError::Validation(format!("source {raw} vanished")))
            })
            .collect()
    }

    /// Record where a source's bytes are now. This is the *only* way a move
    /// is expressed, and it touches no identity: `id` and
    /// `media_fingerprint` are not in the statement.
    pub fn set_source_locator(&self, id: SourceAssetId, locator: Option<&Path>) -> Result<()> {
        self.conn.execute(
            "UPDATE creator_source_assets SET locator=?2 WHERE id=?1",
            params![id.to_string(), locator.map(|p| p.display().to_string())],
        )?;
        Ok(())
    }

    pub fn insert_capture_session(&self, c: &CaptureSession) -> Result<()> {
        self.conn.execute(
            "INSERT INTO creator_capture_sessions
             (id,project_id,source_asset_id,started_at,ended_at,finalized,attributes_json,created_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                c.id.to_string(),
                c.project_id.to_string(),
                c.source_asset_id.map(|i| i.to_string()),
                c.started_at,
                c.ended_at,
                i64::from(c.finalized),
                c.attributes_json,
                c.created_at,
            ],
        )?;
        Ok(())
    }

    pub fn insert_transcript_revision(&self, t: &TranscriptRevision) -> Result<()> {
        self.conn.execute(
            "INSERT INTO creator_transcript_revisions
             (id,project_id,source_asset_id,revision,provider,model_version,author,body_json,created_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                t.id.to_string(),
                t.project_id.to_string(),
                t.source_asset_id.to_string(),
                i64::from(t.revision),
                t.provider,
                t.model_version,
                t.author,
                t.body_json,
                t.created_at,
            ],
        )?;
        Ok(())
    }

    /// Transcripts that name this source — the links that must survive a
    /// move of the file.
    pub fn transcript_revisions_of_source(
        &self,
        source: SourceAssetId,
    ) -> Result<Vec<TranscriptRevision>> {
        let mut stmt = self.conn.prepare(
            "SELECT id FROM creator_transcript_revisions WHERE source_asset_id=?1 ORDER BY revision",
        )?;
        let ids: Vec<String> = stmt
            .query_map([source.to_string()], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<_, _>>()?;
        ids.into_iter()
            .map(|raw| {
                let full = self
                    .conn
                    .query_row(
                        "SELECT project_id,revision,provider,model_version,author,body_json,created_at
                         FROM creator_transcript_revisions WHERE id=?1",
                        [&raw],
                        |r| {
                            Ok((
                                r.get::<_, String>(0)?,
                                r.get::<_, i64>(1)?,
                                r.get::<_, String>(2)?,
                                r.get::<_, String>(3)?,
                                r.get::<_, String>(4)?,
                                r.get::<_, String>(5)?,
                                r.get::<_, String>(6)?,
                            ))
                        },
                    )
                    .optional()?
                    .ok_or_else(|| ResearchError::Validation(format!("transcript {raw} vanished")))?;
                let id: TranscriptRevisionId = raw.parse()?;
                let project: ProjectId = full.0.parse()?;
                Ok(TranscriptRevision {
                    id,
                    project_id: project,
                    source_asset_id: source,
                    revision: full.1 as u32,
                    provider: full.2,
                    model_version: full.3,
                    author: full.4,
                    body_json: full.5,
                    created_at: full.6,
                })
            })
            .collect()
    }

    pub fn insert_composition_revision(&self, c: &CompositionRevision) -> Result<()> {
        self.conn.execute(
            "INSERT INTO creator_composition_revisions
             (id,project_id,parent_revision_id,revision,author,timeline_json,output_profile,created_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                c.id.to_string(),
                c.project_id.to_string(),
                c.parent_revision_id.map(|i| i.to_string()),
                i64::from(c.revision),
                c.author,
                c.timeline_json,
                c.output_profile,
                c.created_at,
            ],
        )?;
        Ok(())
    }

    /// Every composition revision in a project, oldest first.
    pub fn composition_revisions_of(&self, project: ProjectId) -> Result<Vec<CompositionRevision>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,parent_revision_id,revision,author,timeline_json,output_profile,created_at
             FROM creator_composition_revisions WHERE project_id=?1 ORDER BY created_at, id",
        )?;
        let rows = stmt.query_map([project.to_string()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, parent, revision, author, timeline, profile, created) = row?;
            out.push(CompositionRevision {
                id: id.parse()?,
                project_id: project,
                parent_revision_id: parent.as_deref().map(str::parse).transpose()?,
                revision: revision as u32,
                author,
                timeline_json: timeline,
                output_profile: profile,
                created_at: created,
            });
        }
        Ok(out)
    }
}
