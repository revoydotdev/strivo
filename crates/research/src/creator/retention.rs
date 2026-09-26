//! Retention: what may be dropped, what is protected, and what a caller has to
//! resolve by hand before anything goes.
//!
//! Report item O01 (`docs/CREATOR-EDITION-REPORT-2026-09-07.md:373`): *"restore
//! projects and source links from backup; deleting disposable proxies does not
//! delete originals; referenced assets are protected or explicitly resolved."*
//!
//! ## The distinction is the schema's, not this module's
//!
//! "Disposable proxy" and "original" are decided by a fact the database
//! already enforces, not by a spelling list this crate invented:
//!
//! * **An original is a `creator_source_assets` row, or a
//!   `creator_artifacts` row with no `render_plan_id`.** A `NULL` plan id is a
//!   foreign key saying nothing was compiled from this row — it was *registered*
//!   (todo 33's import manifest is exactly this) — so it is material that
//!   arrived rather than material a render produced.
//! * **Everything else is a derivative**, and a derivative is the only class a
//!   cleanup pass may act on.
//!
//! An original cannot be reached through this API at all, which is stronger than
//! being checked and refused: [`PlanRequest::drop`] is typed `&[ArtifactId]`, so
//! naming a source is a compile error, and a *source* id handed to the
//! collector is refused because `creator_artifacts` holds no row with that id.
//!
//! An earlier draft added a `disposable` column to `creator_artifacts` and
//! spelled "is this a proxy?" off the free-text `kind`. It is deliberately **not**
//! here: `kind` is free text with no `CHECK`, so it fails open on a typo, and
//! the typo that matters is the one that makes a render output collectable. The
//! distinction above costs no migration and cannot be misspelt.
//!
//! ## `current` is not used, and that is a finding
//!
//! `creator_artifacts.current` plus its partial unique index
//! `idx_creator_artifacts_one_current` look like the natural way to protect "the
//! output this project is on". **Nothing in the tree ever sets it** — grep finds
//! the column and the index and no `UPDATE`, and todo 24's note is explicit that
//! completing a render makes an output *available* rather than switching the
//! project over to it. A `Deliverable` class built on it would be an unreachable
//! branch, and a rule that can never fire is a comment. Until a writer exists,
//! the honest consequence is that a project's finished render is a derivative
//! like any other and is **offered** by the plan as unreferenced rather than
//! protected by class. That is what "reports as such before deletion" asks for,
//! but it is a weaker guarantee than a class would be, so it is reported in this
//! todo's evidence rather than papered over.
//!
//! ## Protected, or explicitly resolved — never silently collected
//!
//! A derivative something points at is reported with **what** points at it and
//! left alone. Collecting it anyway is possible, but only by naming that one id
//! in [`PlanRequest::resolved`]: an explicit, per-asset decision, whose effect
//! is written into the plan rather than inferred. An id that is not named stays
//! protected however many times the plan runs, so "clean up old proxies" cannot
//! quietly eat the one a release package still names.
//!
//! ## The plan is re-derived, never trusted
//!
//! [`collect`] re-runs [`plan`] and acts only on that fresh answer, so a plan
//! captured before another writer moved a reference cannot authorise a delete
//! the current graph does not permit. The [`Collection`] carries the plan it
//! acted on, and that is the one a caller should show.
//!
//! ## Bytes
//!
//! Removing bytes is the irreversible half, so it happens only for a row that
//! was collected, and **only when no surviving row still names that locator**.
//! Two artifacts may legitimately share a locator — a copy — and one project
//! deleting its row must not take the other one's file with it. Bytes are never
//! touched for a protected row, which is the byte-level half of "deleting a
//! proxy never deletes an original".

use std::collections::BTreeSet;
use std::path::PathBuf;

use super::entities::{Artifact, SourceAsset};
use super::ids::{ArtifactId, ProjectId};
use super::store::Store;
use crate::Result;

/// Which kind of thing a row is, for the purpose of deleting it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// The user's own material — a source asset, or an artifact nothing was
    /// compiled from. Never collectable, by any path, in any order.
    Original,
    /// Produced from a render plan. The only class a cleanup pass may act on,
    /// and only when nothing references it.
    Derivative,
}

impl Class {
    /// Why this class is or is not collectable, in the report's own words.
    #[must_use]
    pub fn reason(self) -> &'static str {
        match self {
            Self::Original => "original material — never collectable",
            Self::Derivative => "derived from a composition",
        }
    }

    /// Whether this class may be collected at all, references aside.
    #[must_use]
    pub fn is_collectable(self) -> bool {
        matches!(self, Self::Derivative)
    }
}

/// Decide what an artifact is, from the row's own columns.
///
/// Takes the whole [`Artifact`] rather than a project id, so there is no way to
/// ask the question about a row nobody read — and a `SourceAsset` is not an
/// `Artifact`, so a source cannot be classified as a derivative by handing over
/// the wrong type.
#[must_use]
pub fn classify(artifact: &Artifact) -> Class {
    if artifact.render_plan_id.is_none() {
        Class::Original
    } else {
        Class::Derivative
    }
}

/// One row that points at an artifact, and so is a reason to keep it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reference {
    /// A release package was built from it. Deleting the artifact cascades the
    /// package away, and with it an approval somebody gave.
    ReleasePackage(String),
    /// A publication was made from the package over it.
    Publication(String),
    /// A job's `artifact_id` names it.
    Job(String),
    /// A recorded metric reads it — a chart would lose its subject.
    Metric(String),
}

impl Reference {
    /// Stable machine code, the key a caller branches on.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::ReleasePackage(_) => "retention/release-package",
            Self::Publication(_) => "retention/publication",
            Self::Job(_) => "retention/job",
            Self::Metric(_) => "retention/metric",
        }
    }
}

impl std::fmt::Display for Reference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ReleasePackage(id) => write!(f, "release package {id}"),
            Self::Publication(id) => write!(f, "publication {id}"),
            Self::Job(id) => write!(f, "job {id}"),
            Self::Metric(id) => write!(f, "metric observation {id}"),
        }
    }
}

/// What the plan decided about one requested artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// The id names no `creator_artifacts` row **in this project**. Either it
    /// never existed, it belongs to another project, or it is a *source* — and
    /// the caller has to be able to tell "I asked to delete a recording and it
    /// was refused" from "I asked to delete something that is not there", so
    /// this is its own answer rather than folded into a refusal.
    Unknown,
    /// An original. Refused whatever the caller asked for, and the class says
    /// which rule.
    Refused { class: Class },
    /// A derivative nothing points at. **Offered** — the plan reports it and
    /// nothing is removed until [`collect`] runs.
    Unreferenced,
    /// A derivative something points at. Protected, with the reason named.
    Protected { by: Vec<Reference> },
    /// A derivative something points at, which the caller resolved by naming
    /// this exact id in [`PlanRequest::resolved`].
    Resolved { by: Vec<Reference> },
}

impl Action {
    /// Whether [`collect`] will remove this row.
    #[must_use]
    pub fn is_collected(&self) -> bool {
        matches!(self, Self::Unreferenced | Self::Resolved { .. })
    }
}

/// One requested artifact, and everything the report needs to explain itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub id: ArtifactId,
    pub kind: String,
    pub class: Class,
    pub action: Action,
    pub locator: Option<PathBuf>,
    pub bytes: Option<u64>,
}

impl Entry {
    /// A one-line explanation, safe to hand straight to a UI.
    #[must_use]
    pub fn explain(&self) -> String {
        let what = if self.kind.is_empty() {
            String::from("output")
        } else {
            self.kind.clone()
        };
        match &self.action {
            Action::Unknown => format!("{}: no such output in this project", self.id),
            Action::Refused { class } => {
                format!("{}: {} — kept ({what})", self.id, class.reason())
            }
            Action::Unreferenced => {
                format!(
                    "{}: {what} — referenced by nothing, so it can be collected",
                    self.id
                )
            }
            Action::Protected { by } | Action::Resolved { by } => {
                let why: Vec<String> = by.iter().map(ToString::to_string).collect();
                format!("{}: {what} — referenced by {}", self.id, why.join(", "))
            }
        }
    }
}

/// One cleanup request.
///
/// Grouped rather than three parameters because they are not independent: the
/// ids are what may go, and `resolved` is the caller's per-id override of the
/// protection the reference scan applies. A builder that could hand over one
/// without the other is one that can contradict itself.
#[derive(Debug, Clone, Copy)]
pub struct PlanRequest<'a> {
    /// Whose outputs are being considered. A plan never reaches outside the
    /// project it was asked about, so a cleanup cannot collect a neighbour's
    /// proxy by naming its id.
    pub project: ProjectId,
    /// The artifacts the caller wants gone — the proxies. Typed `ArtifactId`, so
    /// a [`super::ids::SourceAssetId`] cannot be passed by accident.
    pub drop: &'a [ArtifactId],
    /// Artifacts the caller has explicitly resolved: named one by one because
    /// something references them and the caller has decided that is acceptable.
    /// A blanket "ignore references" flag is deliberately absent.
    pub resolved: &'a [ArtifactId],
}

/// What a plan found. Nothing here has been removed yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub project: ProjectId,
    pub entries: Vec<Entry>,
    /// How many rows [`collect`] would remove.
    pub collectable: usize,
}

impl Plan {
    /// The entries that would be removed, in request order.
    #[must_use]
    pub fn collectable_entries(&self) -> Vec<&Entry> {
        self.entries
            .iter()
            .filter(|e| e.action.is_collected())
            .collect()
    }
}

/// Every row that points at `artifact`. The reasons a derivative is protected.
///
/// Four statements, not a generic graph walk: the schema names exactly these
/// four ways an artifact can be referenced, and a walk would have to be taught
/// about each new one. A reference kind added without a line here would fail
/// *open*, so the test module pairs this function with a check that reads the
/// schema's foreign keys naming `creator_artifacts` and fails when one has no
/// statement here.
/// One reference kind's query, and how it names the row it found.
type ReferenceQuery = (&'static str, fn(String) -> Reference);

const REFERENCE_QUERIES: &[ReferenceQuery] = &[
    (
        "SELECT id FROM creator_release_packages WHERE artifact_id=?1 ORDER BY id",
        Reference::ReleasePackage as fn(String) -> Reference,
    ),
    (
        "SELECT p.id FROM creator_publications p
         JOIN creator_release_packages r ON r.id = p.release_package_id
         WHERE r.artifact_id=?1 ORDER BY p.id",
        Reference::Publication as fn(String) -> Reference,
    ),
    (
        "SELECT id FROM creator_jobs WHERE artifact_id=?1 ORDER BY id",
        Reference::Job as fn(String) -> Reference,
    ),
    (
        "SELECT id FROM creator_metric_observations WHERE artifact_id=?1 ORDER BY id",
        Reference::Metric as fn(String) -> Reference,
    ),
];

fn references_to(store: &Store, artifact: ArtifactId) -> Result<Vec<Reference>> {
    let id = artifact.to_string();
    let mut refs = Vec::new();
    for (sql, wrap) in REFERENCE_QUERIES {
        let mut stmt = store.conn.prepare(sql)?;
        let rows = stmt.query_map([&id], |r| r.get::<_, String>(0))?;
        for row in rows {
            refs.push(wrap(row?));
        }
    }
    Ok(refs)
}

/// Report on every requested artifact without touching anything.
pub fn plan(store: &Store, request: &PlanRequest<'_>) -> Result<Plan> {
    let resolved: BTreeSet<ArtifactId> = request.resolved.iter().copied().collect();
    let mut entries = Vec::with_capacity(request.drop.len());
    let mut collectable = 0;
    for id in request.drop {
        // Scoped to the project: an id from another project is `Unknown` rather
        // than an entry about somebody else's output.
        let Some(row) = store.artifact_in_project(*id, request.project)? else {
            entries.push(Entry {
                id: *id,
                kind: String::new(),
                class: Class::Original,
                action: Action::Unknown,
                locator: None,
                bytes: None,
            });
            continue;
        };
        let class = classify(&row);
        let action = if !class.is_collectable() {
            Action::Refused { class }
        } else {
            let by = references_to(store, *id)?;
            if by.is_empty() {
                Action::Unreferenced
            } else if resolved.contains(id) {
                Action::Resolved { by }
            } else {
                Action::Protected { by }
            }
        };
        if action.is_collected() {
            collectable += 1;
        }
        entries.push(Entry {
            id: *id,
            kind: row.kind.clone(),
            class,
            action,
            locator: row.locator.clone(),
            bytes: row.bytes,
        });
    }
    Ok(Plan {
        project: request.project,
        entries,
        collectable,
    })
}

/// What [`collect`] actually did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collection {
    /// The plan that was acted on. Returned because a caller must be able to
    /// show what happened, and the plan it holds is the one that happened —
    /// never the one it asked for.
    pub plan: Plan,
    /// Artifact ids whose rows were removed.
    pub removed: Vec<ArtifactId>,
    /// Locators whose bytes were removed, in plan order.
    pub bytes_removed: Vec<PathBuf>,
}

/// Remove the rows the *current* graph says are collectable.
///
/// The plan is re-derived inside this call, so a plan captured before another
/// writer moved a reference cannot authorise a delete the present does not
/// permit. [`Action::Refused`] and [`Action::Protected`] rows are never
/// removed, whatever the caller asked for and in whatever order.
pub fn collect(store: &Store, request: &PlanRequest<'_>) -> Result<Collection> {
    let plan = plan(store, request)?;
    let mut removed = Vec::new();
    let mut bytes_removed = Vec::new();
    for entry in plan.collectable_entries() {
        store.delete_artifact(entry.id)?;
        removed.push(entry.id);
        let Some(locator) = &entry.locator else {
            continue;
        };
        // A locator is shared when the same bytes were registered twice. Only
        // remove the file once nothing in the store still names it.
        if store.locator_is_referenced(locator)? {
            continue;
        }
        match std::fs::remove_file(locator) {
            Ok(()) => bytes_removed.push(locator.clone()),
            // A file already gone is the outcome the caller wanted; any other
            // failure is reported rather than swallowed.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(crate::ResearchError::Validation(format!(
                    "{}: removed the row but not the bytes: {e}",
                    entry.id
                )))
            }
        }
    }
    Ok(Collection {
        plan,
        removed,
        bytes_removed,
    })
}

/// Every output a project holds, as identities.
///
/// The candidate set for a whole-project cleanup. Ids only — reading each row is
/// [`plan`]'s job, so a caller that only wants to know how many there are does
/// not pay for a row read per output.
pub fn outputs(store: &Store, project: ProjectId) -> Result<Vec<ArtifactId>> {
    store.artifact_ids_of(project)
}

/// Every source in a project, as originals. The counterpart to [`plan`]: a
/// library's inputs, listed for a report that has to say what a cleanup will
/// not touch.
pub fn originals(store: &Store, project: ProjectId) -> Result<Vec<SourceAsset>> {
    store.sources_of(project)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tempfile::TempDir;

    use super::*;
    use crate::creator::entities::{
        Artifact, CompositionRevision, Project, ReleasePackage, RenderPlan, SourceAsset, Variant,
    };
    use crate::creator::ids::{
        CompositionRevisionId, ReleasePackageId, RenderPlanId, SourceAssetId, VariantId,
    };

    const AT: &str = "2026-09-26T00:00:00Z";

    struct Stage {
        store: Store,
        project: Project,
        dir: TempDir,
        source: SourceAsset,
        registered: Artifact,
        proxy: Artifact,
    }

    fn project(name: &str) -> Project {
        Project {
            id: ProjectId::generate(),
            name: name.into(),
            description: String::new(),
            channel: String::new(),
            series: String::new(),
            settings_json: "{}".into(),
            created_at: AT.into(),
            updated_at: AT.into(),
        }
    }

    fn bytes_at(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).expect("bytes written");
        path
    }

    /// A store with one project, one source, one original artifact (no render
    /// plan — the import manifest's shape) and one derived proxy, both with real
    /// bytes on disk.
    fn stage() -> Stage {
        let dir = TempDir::new().expect("tempdir");
        let store = Store::open_in_memory().expect("store opens");
        let project = project("retention");
        store.insert_project(&project).expect("project");
        let source = SourceAsset {
            id: SourceAssetId::generate(),
            project_id: project.id,
            kind: "recording".into(),
            title: "session-01.mkv".into(),
            media_fingerprint: "sha256:original".into(),
            locator: Some(bytes_at(
                dir.path(),
                "session-01.mkv",
                "the original recording",
            )),
            origin: "capture".into(),
            rights_json: "{}".into(),
            track_type: "video".into(),
            codec: "h264".into(),
            duration: None,
            created_at: AT.into(),
        };
        store.insert_source_asset(&source).expect("source");

        let revision = CompositionRevision {
            id: CompositionRevisionId::generate(),
            project_id: project.id,
            parent_revision_id: None,
            revision: 1,
            author: "operator".into(),
            timeline_json: "{}".into(),
            output_profile: "long-form".into(),
            created_at: AT.into(),
        };
        store
            .insert_composition_revision(&revision)
            .expect("revision");
        let variant = Variant {
            id: VariantId::generate(),
            project_id: project.id,
            composition_revision_id: revision.id,
            name: "web".into(),
            format_overrides_json: "{}".into(),
            created_at: AT.into(),
        };
        store.insert_variant(&variant).expect("variant");
        let plan = RenderPlan {
            id: RenderPlanId::generate(),
            project_id: project.id,
            variant_id: variant.id,
            renderer: "ffmpeg".into(),
            renderer_version: "7".into(),
            resolved_inputs_json: "{}".into(),
            content_hash: "hash".into(),
            created_at: AT.into(),
        };
        store.insert_render_plan(&plan).expect("plan");

        let registered = Artifact {
            id: ArtifactId::generate(),
            project_id: project.id,
            variant_id: None,
            render_plan_id: None,
            kind: "import".into(),
            content_hash: "hash-import".into(),
            locator: Some(bytes_at(
                dir.path(),
                "import-manifest.mkv",
                "registered bytes",
            )),
            bytes: Some(16),
            width: None,
            height: None,
            duration: None,
            created_at: AT.into(),
        };
        store
            .insert_artifact(&registered)
            .expect("registered artifact");

        let proxy = Artifact {
            id: ArtifactId::generate(),
            project_id: project.id,
            variant_id: Some(variant.id),
            render_plan_id: Some(plan.id),
            kind: "proxy".into(),
            content_hash: "hash-proxy".into(),
            locator: Some(bytes_at(dir.path(), "proxy.mp4", "disposable proxy bytes")),
            bytes: Some(24),
            width: None,
            height: None,
            duration: None,
            created_at: AT.into(),
        };
        store.insert_artifact(&proxy).expect("proxy");

        Stage {
            store,
            project,
            dir,
            source,
            registered,
            proxy,
        }
    }

    impl Stage {
        fn package_over(&self, artifact: &Artifact) -> ReleasePackage {
            let package = ReleasePackage {
                id: ReleasePackageId::generate(),
                project_id: self.project.id,
                artifact_id: artifact.id,
                metadata_json: "{}".into(),
                approved_by: "operator".into(),
                approved_at: None,
                state: "approved".into(),
                created_at: AT.into(),
            };
            self.store
                .insert_release_package(&package)
                .expect("package");
            package
        }
    }

    fn req<'a>(
        project: ProjectId,
        drop: &'a [ArtifactId],
        resolved: &'a [ArtifactId],
    ) -> PlanRequest<'a> {
        PlanRequest {
            project,
            drop,
            resolved,
        }
    }

    // ── the structural distinction ──────────────────────────────────────────

    /// Given a row a render plan was compiled into, and a row none was
    /// When both are classified
    /// Then they are different classes, and the difference is read off the
    /// foreign key rather than off a name anybody chose.
    #[test]
    fn a_derived_output_and_a_registered_one_are_different_classes() {
        let stage = stage();
        let derived = stage.proxy.clone();
        let registered = stage.registered.clone();
        assert_eq!(
            classify(&derived),
            Class::Derivative,
            "an output a render plan produced is a derivative"
        );
        assert_eq!(
            classify(&registered),
            Class::Original,
            "a row nothing was compiled from is original material"
        );
        assert!(
            !Class::Original.is_collectable() && Class::Derivative.is_collectable(),
            "and only the derivative class is collectable at all"
        );
    }

    /// The load-bearing guarantee, and the reason it cannot be a happy-path
    /// ordering: both rows are named for removal **in the same call**, and the
    /// original survives anyway.
    #[test]
    fn deleting_a_proxy_leaves_the_original_intact() {
        let stage = stage();
        let proxy = stage.proxy.clone();
        let original = stage.registered.clone();
        let source = stage.source.clone();
        let drop = [proxy.id, original.id];

        let done = collect(&stage.store, &req(stage.project.id, &drop, &[])).expect("collect runs");

        assert_eq!(
            done.removed,
            vec![proxy.id],
            "only the derivative was removed"
        );
        assert_eq!(
            stage.store.artifact(original.id).expect("lookup"),
            Some(original.clone()),
            "the original row is still there"
        );
        assert!(
            original.locator.as_ref().expect("locator").is_file(),
            "and its bytes are still on disk"
        );
        assert_eq!(
            stage
                .store
                .source_asset(source.id)
                .expect("lookup")
                .map(|s| s.id),
            Some(source.id),
            "and no source row was touched at all"
        );
        assert!(
            source.locator.as_ref().expect("locator").is_file(),
            "the source's bytes are still on disk too"
        );
        assert!(
            !done
                .bytes_removed
                .contains(original.locator.as_ref().expect("locator")),
            "and the collector never reached for the original's bytes"
        );
    }

    /// The same claim from the other side: a source's id handed to the collector
    /// is refused, because `creator_artifacts` holds no row with it. The typed
    /// `&[ArtifactId]` makes naming a source a deliberate cast rather than an
    /// accident, and this is where the deliberate cast lands.
    #[test]
    fn a_source_id_named_for_removal_is_refused_rather_than_ignored() {
        let stage = stage();
        let source = stage.source.clone();
        let as_artifact = ArtifactId::from_uuid(source.id.as_uuid());
        let drop = [as_artifact];

        let plan = plan(&stage.store, &req(stage.project.id, &drop, &[])).expect("plan");

        assert_eq!(
            plan.entries[0].action,
            Action::Unknown,
            "a source id is not a collectable output, and is reported as its own answer"
        );
        assert_eq!(plan.collectable, 0, "nothing is offered for collection");
        let done = collect(&stage.store, &req(stage.project.id, &drop, &[])).expect("collect");
        assert!(done.removed.is_empty(), "so nothing was removed");
        assert!(
            stage
                .store
                .source_asset(source.id)
                .expect("lookup")
                .is_some(),
            "and the source is untouched"
        );
    }

    /// Given an id that belongs to a different project
    /// When it is named for removal here
    /// Then the plan does not reach outside the project it was asked about.
    #[test]
    fn a_cleanup_cannot_reach_another_projects_output_by_id() {
        let stage = stage();
        let mine = stage.proxy.clone();
        let other_project = project("somebody else");
        stage
            .store
            .insert_project(&other_project)
            .expect("other project");
        let theirs = Artifact {
            id: ArtifactId::generate(),
            project_id: other_project.id,
            variant_id: None,
            render_plan_id: None,
            kind: "render".into(),
            content_hash: "hash-theirs".into(),
            locator: None,
            bytes: None,
            width: None,
            height: None,
            duration: None,
            created_at: AT.into(),
        };
        stage
            .store
            .insert_artifact(&theirs)
            .expect("their artifact");
        let drop = [mine.id, theirs.id];

        let done = collect(&stage.store, &req(stage.project.id, &drop, &[])).expect("collect");

        assert_eq!(
            done.removed,
            vec![mine.id],
            "only this project's proxy went"
        );
        assert!(
            stage.store.artifact(theirs.id).expect("lookup").is_some(),
            "the other project's row is untouched"
        );
        let entry = done
            .plan
            .entries
            .iter()
            .find(|e| e.id == theirs.id)
            .expect("it is still reported, not silently dropped");
        assert_eq!(entry.action, Action::Unknown, "as unknown here");
    }

    // ── referenced, or explicitly resolved, never silently collected ─────────

    /// Given a proxy a release package was built from
    /// When a plan is made without naming it as resolved
    /// Then it is protected and the report says what points at it.
    #[test]
    fn a_referenced_output_is_reported_as_protected_with_its_referrers() {
        let stage = stage();
        let proxy = stage.proxy.clone();
        let package = stage.package_over(&proxy);
        let drop = [proxy.id];

        let plan = plan(&stage.store, &req(stage.project.id, &drop, &[])).expect("plan");

        assert_eq!(plan.collectable, 0, "a referenced output is not offered");
        assert_eq!(
            plan.entries[0].action,
            Action::Protected {
                by: vec![Reference::ReleasePackage(package.id.to_string())]
            },
            "and the refusal names the row that points at it"
        );
        assert!(
            plan.entries[0].explain().contains(&package.id.to_string()),
            "so a UI naming the reason can: {}",
            plan.entries[0].explain()
        );
    }

    /// The other half: naming the id resolves it, and the plan records that the
    /// caller decided rather than that nothing referenced it.
    #[test]
    fn a_referenced_output_is_collected_only_when_named_explicitly() {
        let stage = stage();
        let proxy = stage.proxy.clone();
        let package = stage.package_over(&proxy);
        let drop = [proxy.id];

        let collected = collect(&stage.store, &req(stage.project.id, &drop, &[])).expect("collect");
        assert!(collected.removed.is_empty(), "not without being named");
        assert!(
            stage.store.artifact(proxy.id).expect("lookup").is_some(),
            "the row is still there"
        );

        let resolved = [proxy.id];
        let collected =
            collect(&stage.store, &req(stage.project.id, &drop, &resolved)).expect("collect");
        assert_eq!(collected.removed, vec![proxy.id], "named, so it goes");
        assert_eq!(
            collected.plan.entries[0].action,
            Action::Resolved {
                by: vec![Reference::ReleasePackage(package.id.to_string())]
            },
            "and the plan says it was resolved by the caller, not found unreferenced"
        );
        assert!(
            stage.store.artifact(proxy.id).expect("lookup").is_none(),
            "the row is gone"
        );
    }

    /// Given two proxies, one referenced and one not
    /// When a plan is made
    /// Then each is reported with its own answer, so the unreferenced one is
    /// visible as such **before** anything is deleted.
    #[test]
    fn an_unreferenced_output_is_reported_as_such_before_deletion() {
        let stage = stage();
        let referenced = stage.proxy.clone();
        let package = stage.package_over(&referenced);
        let orphan = Artifact {
            id: ArtifactId::generate(),
            project_id: stage.project.id,
            variant_id: None,
            render_plan_id: Some(
                stage
                    .store
                    .artifact(referenced.id)
                    .expect("lookup")
                    .expect("present")
                    .render_plan_id
                    .expect("a plan"),
            ),
            kind: "proxy".into(),
            content_hash: "hash-orphan".into(),
            locator: Some(bytes_at(
                stage.dir.path(),
                "orphan.mp4",
                "orphan proxy bytes",
            )),
            bytes: Some(8),
            width: None,
            height: None,
            duration: None,
            created_at: AT.into(),
        };
        stage.store.insert_artifact(&orphan).expect("orphan");
        let drop = [referenced.id, orphan.id];

        let plan = plan(&stage.store, &req(stage.project.id, &drop, &[])).expect("plan");

        assert_eq!(plan.collectable, 1, "only the orphan is offered");
        assert_eq!(
            plan.entries[1].action,
            Action::Unreferenced,
            "as unreferenced"
        );
        assert!(
            orphan.locator.as_ref().expect("locator").is_file(),
            "and nothing has been removed yet — the report came first"
        );
        let explained = plan.entries[1].explain();
        assert!(
            explained.contains("referenced by nothing"),
            "the entry says so in words a UI can show: {explained}"
        );
        assert!(
            plan.entries[0].explain().contains(&package.id.to_string()),
            "and the protected one names its referrer"
        );
    }

    /// A plan is a reading, not a permit: `collect` re-derives it, so a caller
    /// cannot hand it a stale answer.
    #[test]
    fn collect_acts_on_the_current_graph_not_on_a_stale_plan() {
        let stage = stage();
        let proxy = stage.proxy.clone();
        let drop = [proxy.id];
        let before = plan(&stage.store, &req(stage.project.id, &drop, &[])).expect("plan");
        assert_eq!(before.collectable, 1, "it was collectable when asked about");
        stage.package_over(&proxy);

        let done = collect(&stage.store, &req(stage.project.id, &drop, &[])).expect("collect");

        assert!(
            done.removed.is_empty(),
            "a reference that appeared after the plan was taken protects it"
        );
        assert!(
            matches!(done.plan.entries[0].action, Action::Protected { .. }),
            "and the plan that was acted on says so"
        );
    }

    // ── bytes ────────────────────────────────────────────────────────────────

    /// A locator is a location, so two rows may name it. Collecting one must not
    /// take the other's file.
    #[test]
    fn collecting_one_output_leaves_bytes_another_row_still_names() {
        let stage = stage();
        let proxy = stage.proxy.clone();
        let locator = proxy.locator.clone().expect("locator");
        let twin = Artifact {
            id: ArtifactId::generate(),
            project_id: stage.project.id,
            variant_id: None,
            render_plan_id: Some(
                stage
                    .store
                    .artifact(proxy.id)
                    .expect("lookup")
                    .expect("present")
                    .render_plan_id
                    .expect("a plan"),
            ),
            kind: "proxy".into(),
            content_hash: "hash-twin".into(),
            locator: Some(locator.clone()),
            bytes: Some(24),
            width: None,
            height: None,
            duration: None,
            created_at: AT.into(),
        };
        stage.store.insert_artifact(&twin).expect("twin");
        let drop = [proxy.id];

        let done = collect(&stage.store, &req(stage.project.id, &drop, &[])).expect("collect");

        assert_eq!(done.removed, vec![proxy.id], "the row went");
        assert!(
            done.bytes_removed.is_empty(),
            "but the shared bytes stayed, because the twin still names them"
        );
        assert!(locator.is_file(), "and the file is really still there");
        assert!(
            stage.store.artifact(twin.id).expect("lookup").is_some(),
            "the twin row survived"
        );
    }

    #[test]
    fn collecting_the_last_named_row_removes_its_bytes() {
        let stage = stage();
        let proxy = stage.proxy.clone();
        let locator = proxy.locator.clone().expect("locator");
        let drop = [proxy.id];

        let done = collect(&stage.store, &req(stage.project.id, &drop, &[])).expect("collect");

        assert_eq!(
            done.bytes_removed,
            vec![locator.clone()],
            "the bytes went too"
        );
        assert!(!locator.exists(), "the file is really gone");
    }

    // ── the scan cannot silently miss a reference kind ───────────────────────

    /// Every foreign key in the schema that names `creator_artifacts` is a way
    /// for something to point at an output, and each needs a statement in
    /// [`REFERENCE_QUERIES`]. A new one added without a line there would fail
    /// *open* — the plan would call the output unreferenced — so this reads the
    /// schema rather than trusting the list.
    #[test]
    fn every_foreign_key_naming_an_artifact_is_scanned_for_references() {
        let store = Store::open_in_memory().expect("store opens");
        let mut stmt = store
            .conn
            .prepare(
                "SELECT m.name, f.\"table\", f.\"from\" FROM sqlite_master m
                 JOIN pragma_foreign_key_list(m.name) f
                 WHERE m.type='table' AND f.\"table\"='creator_artifacts'",
            )
            .expect("prepare");
        // Columns are (referencing table, referenced table, referencing column).
        let referencing: Vec<(String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(2)?)))
            .expect("query")
            .map(std::result::Result::unwrap)
            .collect();
        assert!(
            !referencing.is_empty(),
            "the fixture really does have foreign keys naming creator_artifacts, \
             so this is not a vacuous pass"
        );
        for (table, column) in referencing {
            let covered = REFERENCE_QUERIES
                .iter()
                .any(|(sql, _)| sql.contains(&table) && sql.contains(&column));
            assert!(
                covered,
                "{table}.{column} references creator_artifacts but no statement in \
                 REFERENCE_QUERIES looks at it, so an output it points at would be \
                 reported unreferenced and silently collected"
            );
        }
    }
}
