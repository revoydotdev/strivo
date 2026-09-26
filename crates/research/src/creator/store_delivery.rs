//! The delivery chain: variant → render plan → artifact → release package →
//! publication → metric observation.
//!
//! Inserts, plus the one `get` a caller has earned. Every type in this file has
//! a durable identity and a schema row, and the things that *read* them back are
//! the jobs and routes that todo 23/24 build — so a `get` with no caller is not
//! written. [`Store::artifact`] is the exception: import's manifest is an
//! artifact row, and a test that cannot read its own manifest back is a test
//! that proves the job points at nothing.

use std::path::Path;

use rusqlite::params;

use super::entities::*;
use super::ids::{ArtifactId, ProjectId, RenderPlanId, VariantId};
use super::store::Store;
use crate::Result;
use strivo_editor::time::MediaTime;

impl Store {
    pub fn insert_variant(&self, v: &Variant) -> Result<()> {
        self.conn.execute(
            "INSERT INTO creator_variants
             (id,project_id,composition_revision_id,name,format_overrides_json,created_at)
             VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                v.id.to_string(),
                v.project_id.to_string(),
                v.composition_revision_id.to_string(),
                v.name,
                v.format_overrides_json,
                v.created_at,
            ],
        )?;
        Ok(())
    }

    pub fn insert_render_plan(&self, r: &RenderPlan) -> Result<()> {
        self.conn.execute(
            "INSERT INTO creator_render_plans
             (id,project_id,variant_id,renderer,renderer_version,resolved_inputs_json,
              content_hash,created_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                r.id.to_string(),
                r.project_id.to_string(),
                r.variant_id.to_string(),
                r.renderer,
                r.renderer_version,
                r.resolved_inputs_json,
                r.content_hash,
                r.created_at,
            ],
        )?;
        Ok(())
    }

    pub fn insert_artifact(&self, a: &Artifact) -> Result<()> {
        self.conn.execute(
            "INSERT INTO creator_artifacts
             (id,project_id,variant_id,render_plan_id,kind,content_hash,locator,bytes,
              width,height,duration_us,created_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            params![
                a.id.to_string(),
                a.project_id.to_string(),
                a.variant_id.map(|i| i.to_string()),
                a.render_plan_id.map(|i| i.to_string()),
                a.kind,
                a.content_hash,
                a.locator.as_ref().map(|p| p.display().to_string()),
                a.bytes.map(|b| b as i64),
                a.width.map(i64::from),
                a.height.map(i64::from),
                a.duration.map(|d| d.micros()),
                a.created_at,
            ],
        )?;
        Ok(())
    }

    /// One artifact by its identity, or `None` for an id that names no row.
    /// The `creator_artifacts` row is the only place an artifact's bytes are
    /// described, so a reader that cannot ask for it is a reader inferring an
    /// artifact's shape from the job that claims to have made it.
    pub fn artifact(&self, id: ArtifactId) -> Result<Option<Artifact>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id,project_id,variant_id,render_plan_id,kind,content_hash,locator,bytes,
                        width,height,duration_us,created_at
                 FROM creator_artifacts WHERE id=?1",
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
                    Ok(Artifact {
                        id: ArtifactId::from_uuid(uuid(0, r.get(0)?)?),
                        project_id: ProjectId::from_uuid(uuid(1, r.get(1)?)?),
                        variant_id: r
                            .get::<_, Option<String>>(2)?
                            .map(|v| uuid(2, v))
                            .transpose()?
                            .map(VariantId::from_uuid),
                        render_plan_id: r
                            .get::<_, Option<String>>(3)?
                            .map(|v| uuid(3, v))
                            .transpose()?
                            .map(RenderPlanId::from_uuid),
                        kind: r.get(4)?,
                        content_hash: r.get(5)?,
                        locator: r.get::<_, Option<String>>(6)?.map(std::path::PathBuf::from),
                        bytes: r.get::<_, Option<i64>>(7)?.map(|b| b as u64),
                        width: r.get::<_, Option<i64>>(8)?.map(|w| w as u32),
                        height: r.get::<_, Option<i64>>(9)?.map(|h| h as u32),
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
            .ok())
    }

    /// The ids of a project's outputs, oldest first. A candidate set for a
    /// cleanup pass, so it reads ids rather than every row.
    pub fn artifact_ids_of(&self, project: ProjectId) -> Result<Vec<ArtifactId>> {
        let mut stmt = self.conn.prepare(
            "SELECT id FROM creator_artifacts WHERE project_id=?1 ORDER BY created_at, id",
        )?;
        let rows = stmt.query_map([project.to_string()], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?.parse()?);
        }
        Ok(out)
    }

    /// One artifact by its identity, scoped to the project that owns it.
    ///
    /// Two lookups rather than one query because a cleanup pass must never
    /// reach outside the project it was asked about, and `None` here is how
    /// that is enforced: an id belonging to a neighbour is indistinguishable
    /// from one that does not exist, so a plan cannot act on it.
    pub fn artifact_in_project(
        &self,
        id: ArtifactId,
        project: ProjectId,
    ) -> Result<Option<Artifact>> {
        Ok(self.artifact(id)?.filter(|a| a.project_id == project))
    }

    /// Remove an artifact row, reporting whether one was there.
    ///
    /// A row is a fact, not a file: this deletes the row and nothing else. The
    /// bytes at its `locator` are removed by the caller, and only once this has
    /// confirmed no other row still names the same location.
    pub fn delete_artifact(&self, id: ArtifactId) -> Result<bool> {
        Ok(self.conn.execute(
            "DELETE FROM creator_artifacts WHERE id=?1",
            [id.to_string()],
        )? > 0)
    }

    /// Whether any surviving row — a source or an output — still names this
    /// location.
    ///
    /// Read across both tables because a locator is a *location*: the same bytes
    /// can be registered as a source in one project and as an output in
    /// another, and a cleanup that removed the file on the strength of one row
    /// would take the other with it.
    pub fn locator_is_referenced(&self, locator: &Path) -> Result<bool> {
        let at = locator.display().to_string();
        for sql in [
            "SELECT EXISTS(SELECT 1 FROM creator_source_assets WHERE locator=?1)",
            "SELECT EXISTS(SELECT 1 FROM creator_artifacts WHERE locator=?1)",
        ] {
            if self.conn.query_row(sql, [&at], |r| r.get::<_, i64>(0))? != 0 {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn insert_release_package(&self, r: &ReleasePackage) -> Result<()> {
        self.conn.execute(
            "INSERT INTO creator_release_packages
             (id,project_id,artifact_id,metadata_json,approved_by,approved_at,state,created_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                r.id.to_string(),
                r.project_id.to_string(),
                r.artifact_id.to_string(),
                r.metadata_json,
                r.approved_by,
                r.approved_at,
                r.state,
                r.created_at,
            ],
        )?;
        Ok(())
    }

    pub fn insert_publication(&self, p: &Publication) -> Result<()> {
        self.conn.execute(
            "INSERT INTO creator_publications
             (id,project_id,release_package_id,destination,account,state,scheduled_at,timezone,
              remote_id,uploaded_at,published_at,last_error,created_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            params![
                p.id.to_string(),
                p.project_id.to_string(),
                p.release_package_id.to_string(),
                p.destination,
                p.account,
                p.state,
                p.scheduled_at,
                p.timezone,
                p.remote_id,
                p.uploaded_at,
                p.published_at,
                p.last_error,
                p.created_at,
            ],
        )?;
        Ok(())
    }

    pub fn insert_metric_observation(&self, m: &MetricObservation) -> Result<()> {
        self.conn.execute(
            "INSERT INTO creator_metric_observations
             (id,project_id,publication_id,release_package_id,artifact_id,composition_revision_id,
              provider,metric,definition_version,scope,unit,denominator,interval_start,interval_end,
              timezone,value,coverage,retrieved_at,source_query_json,transform_json,chart_json)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21)",
            params![
                m.id.to_string(),
                m.project_id.to_string(),
                m.publication_id.map(|i| i.to_string()),
                m.release_package_id.map(|i| i.to_string()),
                m.artifact_id.map(|i| i.to_string()),
                m.composition_revision_id.map(|i| i.to_string()),
                m.provider,
                m.metric,
                m.definition_version,
                m.scope,
                m.unit,
                m.denominator,
                m.interval_start,
                m.interval_end,
                m.timezone,
                m.value,
                m.coverage,
                m.retrieved_at,
                m.source_query_json,
                m.transform_json,
                m.chart_json,
            ],
        )?;
        Ok(())
    }
}
