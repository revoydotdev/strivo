//! Moments projection — the coding == clip-candidate bridge (CE-Fusion F2).
//!
//! Strategy §3's load-bearing insight: a research coding and a clip
//! candidate are the same data structure. This module renders codings AND
//! high-confidence detection signals through one creator-vocabulary "moment"
//! projection, and lets creating a moment write a real coding through the
//! existing [`crate::ResearchStore::add_coding`] path. Strategy §6 rule:
//! renderings, never forks — the kernel schema keeps its canonical research
//! names; this module adds no tables and no columns.

use chrono::Utc;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{CodingOrigin, NewCoding, ResearchError, ResearchStore, Result};

/// Signal kinds that qualify as clip candidates ("detections") in the
/// moments projection. `transcript.utterance` is deliberately excluded —
/// it is raw evidence, not a moment in its own right (strategy §3:
/// extractors emit signals, a human or a detector flags a moment).
/// `chat.density` would join this list once chat-density writes into the
/// signal store (today it computes on demand and is not yet a kernel
/// signal producer — see `strivo-web/src/routes/plugins.rs::chat_density_compute`).
const DETECTION_KINDS: [&str; 2] = ["visual.scene_change", "audience.anomaly"];

/// Default tag name used when [`ResearchStore::create_moment`] is called
/// without a `tag`. Chosen so a tag-less moment still lands on a
/// meaningful, findable code rather than an anonymous one.
pub const DEFAULT_MOMENT_CODE: &str = "Moment";

/// Matches the schema's default code color (`codes.color` column default)
/// so a projection-created code looks identical to a manually created one.
const DEFAULT_MOMENT_CODE_COLOR: &str = "#7c5cff";

/// Author recorded on codings written through the moments projection.
/// The creator surface does not yet carry a per-user identity into the
/// kernel (`author` is a required, non-empty free-text field on every
/// coding); this constant is the documented placeholder until one exists.
const DEFAULT_MOMENT_AUTHOR: &str = "creator";

/// Where a moment came from: a human/model/import coding, or a
/// high-confidence machine detection signal.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MomentOrigin {
    Coding,
    Detection,
}

/// One creator-vocabulary "moment": a time range on a source worth
/// clipping, whether a researcher coded it or an extractor detected it.
/// Renders [`crate::Coding`] and clip-worthy [`crate::Signal`] rows behind
/// one shape; never stored directly.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Moment {
    pub id: Uuid,
    pub source_id: Uuid,
    pub t_start_ms: u64,
    pub t_end_ms: u64,
    pub label: String,
    pub origin: MomentOrigin,
    /// Code name for a coding-origin moment; signal kind for a
    /// detection-origin moment.
    pub kind: String,
    pub confidence: Option<f64>,
    pub coding_id: Option<Uuid>,
    pub signal_id: Option<Uuid>,
    pub code_id: Option<Uuid>,
}

impl ResearchStore {
    /// List moments for a project: every coding, plus clip-worthy detection
    /// signals, merged into one deterministically ordered stream.
    ///
    /// `min_confidence` filters detections only — codings always pass,
    /// since a human/model/import coding is already a confirmed moment, not
    /// a candidate awaiting confidence triage. A detection with no
    /// recorded confidence is excluded whenever `min_confidence` is set: an
    /// unscored detection cannot be shown to clear a bar it was never
    /// measured against.
    ///
    /// Ordering is `(source_id, t_start_ms, id)`, both within and across
    /// origins. Codings and detections are fetched in full for the
    /// project/source scope and merged in memory before the `limit`/
    /// `offset` window is applied, so pagination is correct across the
    /// coding/detection boundary — mirroring [`crate::ResearchStore::list_codings`],
    /// which is unpaginated for the same local-corpus-scale reason.
    pub fn list_moments(
        &self,
        project_id: Uuid,
        source_id: Option<Uuid>,
        min_confidence: Option<f64>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<Moment>> {
        if !(1..=500).contains(&limit) {
            return Err(ResearchError::Validation(
                "moments limit must be between 1 and 500".into(),
            ));
        }
        crate::confidence(min_confidence)?;
        if let Some(source_id) = source_id {
            crate::ensure_object_project(&self.conn, "sources", source_id, project_id)?;
        }

        let mut moments = self.coding_moments(project_id, source_id)?;
        moments.extend(self.detection_moments(project_id, source_id, min_confidence)?);
        moments.sort_by(|a, b| {
            (a.source_id, a.t_start_ms, a.id).cmp(&(b.source_id, b.t_start_ms, b.id))
        });

        Ok(moments
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .collect())
    }

    fn coding_moments(&self, project_id: Uuid, source_id: Option<Uuid>) -> Result<Vec<Moment>> {
        let mut stmt = self.conn.prepare(
            "SELECT c.id,c.source_id,c.start_ms,c.end_ms,c.excerpt,c.confidence,c.code_id,co.name
             FROM codings c JOIN codes co ON co.id=c.code_id
             WHERE c.project_id=?1 AND (?2 IS NULL OR c.source_id=?2)",
        )?;
        let rows = stmt.query_map(
            params![project_id.to_string(), source_id.map(|id| id.to_string())],
            |row| {
                let id: Uuid = crate::parse_uuid(row.get::<_, String>(0)?)?;
                let code_id: Uuid = crate::parse_uuid(row.get::<_, String>(6)?)?;
                let excerpt: String = row.get(4)?;
                let code_name: String = row.get(7)?;
                let label = if excerpt.trim().is_empty() {
                    code_name.clone()
                } else {
                    excerpt
                };
                Ok(Moment {
                    id,
                    source_id: crate::parse_uuid(row.get::<_, String>(1)?)?,
                    t_start_ms: row.get::<_, i64>(2)?.max(0) as u64,
                    t_end_ms: row.get::<_, i64>(3)?.max(0) as u64,
                    label,
                    origin: MomentOrigin::Coding,
                    kind: code_name,
                    confidence: row.get(5)?,
                    coding_id: Some(id),
                    signal_id: None,
                    code_id: Some(code_id),
                })
            },
        )?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    fn detection_moments(
        &self,
        project_id: Uuid,
        source_id: Option<Uuid>,
        min_confidence: Option<f64>,
    ) -> Result<Vec<Moment>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,source_id,start_ms,end_ms,kind,label,confidence
             FROM signals
             WHERE project_id=?1
               AND (?2 IS NULL OR source_id=?2)
               AND kind IN (?3,?4)
               AND (?5 IS NULL OR (confidence IS NOT NULL AND confidence>=?5))",
        )?;
        let rows = stmt.query_map(
            params![
                project_id.to_string(),
                source_id.map(|id| id.to_string()),
                DETECTION_KINDS[0],
                DETECTION_KINDS[1],
                min_confidence
            ],
            |row| {
                let id: Uuid = crate::parse_uuid(row.get::<_, String>(0)?)?;
                Ok(Moment {
                    id,
                    source_id: crate::parse_uuid(row.get::<_, String>(1)?)?,
                    t_start_ms: row.get::<_, i64>(2)?.max(0) as u64,
                    t_end_ms: row.get::<_, i64>(3)?.max(0) as u64,
                    label: row.get(5)?,
                    origin: MomentOrigin::Detection,
                    kind: row.get(4)?,
                    confidence: row.get(6)?,
                    coding_id: None,
                    signal_id: Some(id),
                    code_id: None,
                })
            },
        )?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// Create a moment: find-or-create a [`crate::Code`] by `tag` (or
    /// [`DEFAULT_MOMENT_CODE`] when `tag` is `None`/blank), then write a
    /// human-origin [`crate::Coding`] through [`Self::add_coding`] so every
    /// existing kernel validation (time range, confidence, same-project
    /// source/code) applies unchanged. Repeat calls with the same tag reuse
    /// the same code — no duplicate codes.
    pub fn create_moment(
        &mut self,
        project_id: Uuid,
        source_id: Uuid,
        t_start_ms: u64,
        t_end_ms: u64,
        label: String,
        tag: Option<String>,
    ) -> Result<Moment> {
        let tag_name = tag
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(DEFAULT_MOMENT_CODE)
            .to_string();
        let code_id = self.find_or_create_code(project_id, &tag_name)?;
        let coding = NewCoding {
            id: Uuid::new_v4(),
            project_id,
            source_id,
            code_id,
            start_ms: t_start_ms,
            end_ms: t_end_ms,
            excerpt: label.clone(),
            note: String::new(),
            author: DEFAULT_MOMENT_AUTHOR.into(),
            origin: CodingOrigin::Human,
            confidence: None,
            provenance_id: None,
        };
        self.add_coding(&coding)?;
        Ok(Moment {
            id: coding.id,
            source_id,
            t_start_ms,
            t_end_ms,
            label,
            origin: MomentOrigin::Coding,
            kind: tag_name,
            confidence: None,
            coding_id: Some(coding.id),
            signal_id: None,
            code_id: Some(code_id),
        })
    }

    /// Idempotent find-or-create for a top-level (no parent) code by exact
    /// trimmed name.
    ///
    /// First reuse any existing top-level code with this name — whether it
    /// was created by an earlier `create_moment` call or directly through
    /// the research codebook. If none exists, mint one with a
    /// *deterministic* id derived from `(project_id, name)` and
    /// `INSERT OR IGNORE`. The schema's `UNIQUE(project_id, parent_id,
    /// name)` constraint does **not** actually dedupe here — SQL treats
    /// every `NULL` `parent_id` as distinct for uniqueness purposes, so two
    /// inserts with `parent_id IS NULL` never collide on it (confirmed by a
    /// failing test during development). Idempotency instead comes from
    /// colliding on the deterministic *primary key*, the same pattern
    /// [`crate::ensure_legacy_source`]-style migration helpers already use:
    /// even a first-call race between two callers minting the same new tag
    /// collapses to one row, because both computed the same id.
    fn find_or_create_code(&self, project_id: Uuid, name: &str) -> Result<Uuid> {
        let existing: Option<String> = self
            .conn
            .query_row(
                "SELECT id FROM codes WHERE project_id=?1 AND parent_id IS NULL AND name=?2
                 ORDER BY created_at,id LIMIT 1",
                params![project_id.to_string(), name],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(id) = existing {
            return crate::parse_uuid(id).map_err(Into::into);
        }
        let id = crate::stable_id("moment-code", &format!("{project_id}:{name}"));
        self.conn.execute(
            "INSERT OR IGNORE INTO codes(id,project_id,parent_id,name,description,color,created_at)
             VALUES(?1,?2,NULL,?3,'',?4,?5)",
            params![
                id.to_string(),
                project_id.to_string(),
                name,
                DEFAULT_MOMENT_CODE_COLOR,
                Utc::now().to_rfc3339()
            ],
        )?;
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Code, NewSignal, NewSource, SourceKind};

    /// On-disk (tempfile) store, per the brief's test convention.
    fn open_store() -> (tempfile::TempDir, ResearchStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = ResearchStore::open(dir.path().join("research.db")).unwrap();
        (dir, store)
    }

    fn add_source(store: &ResearchStore, project_id: Uuid) -> Uuid {
        let id = Uuid::new_v4();
        store
            .upsert_source(&NewSource {
                id,
                project_id,
                recording_id: None,
                kind: SourceKind::Recording,
                title: "Source".into(),
                uri: None,
                duration_ms: None,
                attributes: serde_json::json!({}),
            })
            .unwrap();
        id
    }

    fn add_code(store: &ResearchStore, project_id: Uuid, name: &str) -> Uuid {
        let code = Code {
            id: Uuid::new_v4(),
            project_id,
            parent_id: None,
            name: name.into(),
            description: String::new(),
            color: "#7c5cff".into(),
        };
        store.create_code(&code).unwrap();
        code.id
    }

    fn add_coding(
        store: &mut ResearchStore,
        project_id: Uuid,
        source_id: Uuid,
        code_id: Uuid,
        start_ms: u64,
    ) -> Uuid {
        let id = Uuid::new_v4();
        store
            .add_coding(&NewCoding {
                id,
                project_id,
                source_id,
                code_id,
                start_ms,
                end_ms: start_ms + 50,
                excerpt: String::new(),
                note: String::new(),
                author: "researcher".into(),
                origin: CodingOrigin::Human,
                confidence: None,
                provenance_id: None,
            })
            .unwrap();
        id
    }

    fn add_signal(
        store: &ResearchStore,
        project_id: Uuid,
        source_id: Uuid,
        kind: &str,
        start_ms: u64,
        confidence: Option<f64>,
    ) -> Uuid {
        let id = Uuid::new_v4();
        store
            .append_signal(&NewSignal {
                id,
                project_id,
                source_id,
                start_ms,
                end_ms: start_ms + 50,
                kind: kind.into(),
                label: format!("{kind} label"),
                payload: serde_json::json!({}),
                confidence,
                provenance_id: None,
            })
            .unwrap();
        id
    }

    #[test]
    fn merges_codings_and_detections_in_time_order() {
        let (_dir, mut store) = open_store();
        let project = store.create_project("Study", "").unwrap();
        let source = add_source(&store, project.id);
        let code = add_code(&store, project.id, "Highlight");

        let detection_early = add_signal(
            &store,
            project.id,
            source,
            "visual.scene_change",
            100,
            Some(0.9),
        );
        let coding_mid = add_coding(&mut store, project.id, source, code, 500);
        let detection_late = add_signal(
            &store,
            project.id,
            source,
            "audience.anomaly",
            900,
            Some(0.9),
        );

        let moments = store.list_moments(project.id, None, None, 10, 0).unwrap();
        let ids: Vec<Uuid> = moments.iter().map(|m| m.id).collect();
        assert_eq!(ids, vec![detection_early, coding_mid, detection_late]);
        assert_eq!(moments[0].origin, MomentOrigin::Detection);
        assert_eq!(moments[1].origin, MomentOrigin::Coding);
        assert_eq!(moments[1].kind, "Highlight");
        assert_eq!(moments[2].origin, MomentOrigin::Detection);
    }

    #[test]
    fn min_confidence_filters_detections_but_never_codings() {
        let (_dir, mut store) = open_store();
        let project = store.create_project("Study", "").unwrap();
        let source = add_source(&store, project.id);
        let code = add_code(&store, project.id, "Highlight");

        // Human coding: no confidence at all, must always pass.
        let coding = add_coding(&mut store, project.id, source, code, 100);
        let high = add_signal(
            &store,
            project.id,
            source,
            "visual.scene_change",
            200,
            Some(0.9),
        );
        let low = add_signal(
            &store,
            project.id,
            source,
            "audience.anomaly",
            300,
            Some(0.2),
        );
        let unscored = add_signal(&store, project.id, source, "visual.scene_change", 400, None);

        let moments = store
            .list_moments(project.id, None, Some(0.5), 10, 0)
            .unwrap();
        let ids: Vec<Uuid> = moments.iter().map(|m| m.id).collect();

        assert!(ids.contains(&coding), "coding must always pass the filter");
        assert!(ids.contains(&high));
        assert!(
            !ids.contains(&low),
            "low-confidence detection must be filtered out"
        );
        assert!(
            !ids.contains(&unscored),
            "an unscored detection cannot clear a confidence bar it was never measured against"
        );
    }

    #[test]
    fn source_id_filters_to_one_source() {
        let (_dir, mut store) = open_store();
        let project = store.create_project("Study", "").unwrap();
        let source_a = add_source(&store, project.id);
        let source_b = add_source(&store, project.id);
        let code = add_code(&store, project.id, "Highlight");
        let coding_a = add_coding(&mut store, project.id, source_a, code, 100);
        add_coding(&mut store, project.id, source_b, code, 100);
        add_signal(
            &store,
            project.id,
            source_a,
            "visual.scene_change",
            200,
            Some(0.9),
        );
        add_signal(
            &store,
            project.id,
            source_b,
            "visual.scene_change",
            200,
            Some(0.9),
        );

        let moments = store
            .list_moments(project.id, Some(source_a), None, 10, 0)
            .unwrap();
        assert_eq!(moments.len(), 2);
        assert!(moments.iter().all(|m| m.source_id == source_a));
        assert!(moments.iter().any(|m| m.id == coding_a));
    }

    #[test]
    fn pagination_is_correct_across_the_merge_boundary() {
        let (_dir, mut store) = open_store();
        let project = store.create_project("Study", "").unwrap();
        let source = add_source(&store, project.id);
        let code = add_code(&store, project.id, "Highlight");

        // Interleave 3 codings and 3 detections across time so a page
        // boundary is guaranteed to fall mid-origin-switch.
        let mut expected = Vec::new();
        for (i, start) in [0u64, 200, 400, 600, 800, 1000].iter().enumerate() {
            if i % 2 == 0 {
                expected.push(add_coding(&mut store, project.id, source, code, *start));
            } else {
                expected.push(add_signal(
                    &store,
                    project.id,
                    source,
                    "visual.scene_change",
                    *start,
                    Some(0.9),
                ));
            }
        }

        let page1 = store.list_moments(project.id, None, None, 2, 0).unwrap();
        let page2 = store.list_moments(project.id, None, None, 2, 2).unwrap();
        let page3 = store.list_moments(project.id, None, None, 2, 4).unwrap();
        let page4 = store.list_moments(project.id, None, None, 2, 6).unwrap();

        assert!(
            page4.is_empty(),
            "only 6 moments exist; a 7th/8th page is empty"
        );
        let paged: Vec<Uuid> = [page1, page2, page3, page4]
            .into_iter()
            .flatten()
            .map(|m| m.id)
            .collect();
        assert_eq!(paged, expected);
    }

    #[test]
    fn cross_project_isolation() {
        let (_dir, mut store) = open_store();
        let project_a = store.create_project("A", "").unwrap();
        let project_b = store.create_project("B", "").unwrap();
        let source_a = add_source(&store, project_a.id);
        let source_b = add_source(&store, project_b.id);
        let code_a = add_code(&store, project_a.id, "Highlight");
        let code_b = add_code(&store, project_b.id, "Highlight");
        add_coding(&mut store, project_a.id, source_a, code_a, 100);
        add_coding(&mut store, project_b.id, source_b, code_b, 100);
        add_signal(
            &store,
            project_a.id,
            source_a,
            "visual.scene_change",
            200,
            Some(0.9),
        );
        add_signal(
            &store,
            project_b.id,
            source_b,
            "visual.scene_change",
            200,
            Some(0.9),
        );

        let moments_a = store.list_moments(project_a.id, None, None, 10, 0).unwrap();
        let moments_b = store.list_moments(project_b.id, None, None, 10, 0).unwrap();
        assert_eq!(moments_a.len(), 2);
        assert_eq!(moments_b.len(), 2);
        assert!(moments_a.iter().all(|m| m.source_id == source_a));
        assert!(moments_b.iter().all(|m| m.source_id == source_b));
    }

    #[test]
    fn create_moment_writes_a_visible_coding() {
        let (_dir, mut store) = open_store();
        let project = store.create_project("Study", "").unwrap();
        let source = add_source(&store, project.id);

        let moment = store
            .create_moment(
                project.id,
                source,
                1_000,
                2_000,
                "Chat erupts".into(),
                Some("Highlight".into()),
            )
            .unwrap();

        assert_eq!(moment.origin, MomentOrigin::Coding);
        assert_eq!(moment.kind, "Highlight");
        assert_eq!(moment.label, "Chat erupts");

        let codings = store.list_codings(project.id).unwrap();
        assert_eq!(codings.len(), 1);
        assert_eq!(codings[0].id, moment.coding_id.unwrap());
        assert_eq!(codings[0].excerpt, "Chat erupts");

        let moments = store.list_moments(project.id, None, None, 10, 0).unwrap();
        assert_eq!(moments.len(), 1);
        assert_eq!(moments[0].id, moment.id);
    }

    #[test]
    fn create_moment_tag_find_or_create_is_idempotent() {
        let (_dir, mut store) = open_store();
        let project = store.create_project("Study", "").unwrap();
        let source = add_source(&store, project.id);

        let first = store
            .create_moment(
                project.id,
                source,
                0,
                1_000,
                "First".into(),
                Some("Highlight".into()),
            )
            .unwrap();
        let second = store
            .create_moment(
                project.id,
                source,
                2_000,
                3_000,
                "Second".into(),
                Some("Highlight".into()),
            )
            .unwrap();

        assert_eq!(first.code_id, second.code_id);
        let codes = store.list_codes(project.id).unwrap();
        assert_eq!(
            codes.iter().filter(|c| c.name == "Highlight").count(),
            1,
            "repeat tag must not create a duplicate code"
        );
    }

    #[test]
    fn create_moment_default_tag_is_used_when_none() {
        let (_dir, mut store) = open_store();
        let project = store.create_project("Study", "").unwrap();
        let source = add_source(&store, project.id);

        let moment = store
            .create_moment(project.id, source, 0, 1_000, "Untagged".into(), None)
            .unwrap();
        assert_eq!(moment.kind, DEFAULT_MOMENT_CODE);
        let codes = store.list_codes(project.id).unwrap();
        assert!(codes.iter().any(|c| c.name == DEFAULT_MOMENT_CODE));
    }

    #[test]
    fn create_moment_rejects_invalid_time_range() {
        let (_dir, mut store) = open_store();
        let project = store.create_project("Study", "").unwrap();
        let source = add_source(&store, project.id);
        let result = store.create_moment(project.id, source, 2_000, 1_000, "Bad".into(), None);
        assert!(matches!(result, Err(ResearchError::Validation(_))));
    }

    #[test]
    fn create_moment_rejects_source_from_another_project() {
        let (_dir, mut store) = open_store();
        let project_a = store.create_project("A", "").unwrap();
        let project_b = store.create_project("B", "").unwrap();
        let source_a = add_source(&store, project_a.id);
        let result = store.create_moment(
            project_b.id,
            source_a,
            0,
            1_000,
            "Cross-project".into(),
            None,
        );
        assert!(matches!(result, Err(ResearchError::Validation(_))));
    }

    #[test]
    fn list_moments_rejects_source_from_another_project() {
        let (_dir, store) = open_store();
        let project_a = store.create_project("A", "").unwrap();
        let project_b = store.create_project("B", "").unwrap();
        let source_a = add_source(&store, project_a.id);
        let result = store.list_moments(project_b.id, Some(source_a), None, 10, 0);
        assert!(matches!(result, Err(ResearchError::Validation(_))));
    }

    #[test]
    fn list_moments_rejects_out_of_range_limit() {
        let (_dir, store) = open_store();
        let project = store.create_project("Study", "").unwrap();
        assert!(matches!(
            store.list_moments(project.id, None, None, 0, 0),
            Err(ResearchError::Validation(_))
        ));
        assert!(matches!(
            store.list_moments(project.id, None, None, 501, 0),
            Err(ResearchError::Validation(_))
        ));
    }

    #[test]
    fn list_moments_rejects_invalid_min_confidence() {
        let (_dir, store) = open_store();
        let project = store.create_project("Study", "").unwrap();
        assert!(matches!(
            store.list_moments(project.id, None, Some(1.5), 10, 0),
            Err(ResearchError::Validation(_))
        ));
    }
}
