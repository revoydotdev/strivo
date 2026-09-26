//! Malformed and wrong-kind inputs, plus the end-to-end write of every
//! Creator type. The invariants themselves are in [`super::identity`].

use super::super::entities::*;
use super::super::ids::*;
use super::super::store::Store;
use super::fixtures::{at, dur, project, revision, source};
use crate::ResearchError;

/// Given a store with one project and one source
/// When an id that names no row is looked up
/// Then it resolves to nothing rather than to a different row.
#[test]
fn an_unknown_id_resolves_to_nothing() {
    let store = Store::open_in_memory().expect("store opens");
    let p = project("streamer");
    store.insert_project(&p).expect("project inserted");
    let s = source(p.id, None);
    store.insert_source_asset(&s).expect("source inserted");

    assert!(
        store
            .source_asset(SourceAssetId::generate())
            .expect("lookup ok")
            .is_none(),
        "an id that names no source is not a source"
    );
    assert!(
        store
            .project(ProjectId::generate())
            .expect("lookup ok")
            .is_none(),
        "an id that names no project is not a project"
    );
    assert!(
        store
            .sources_of(ProjectId::generate())
            .expect("listing ok")
            .is_empty(),
        "another project's id lists nothing"
    );
}

/// Given a source already stored under one id
/// When it is inserted again with the same id
/// Then the insert is refused; a duplicate id must not become a second row.
#[test]
fn a_duplicate_id_is_refused() {
    let store = Store::open_in_memory().expect("store opens");
    let p = project("streamer");
    store.insert_project(&p).expect("project inserted");
    let s = source(p.id, None);
    store.insert_source_asset(&s).expect("source inserted");

    let again = store.insert_source_asset(&s);

    assert!(
        again.is_err(),
        "a duplicate id is an error, not a second row"
    );
    assert_eq!(
        store.sources_of(p.id).expect("sources listed").len(),
        1,
        "still exactly one row"
    );
}

/// Given a source belonging to one project
/// When a row is inserted naming a project that does not exist
/// Then the foreign key refuses it, because foreign keys are enabled.
#[test]
fn a_source_cannot_name_a_project_that_does_not_exist() {
    let store = Store::open_in_memory().expect("store opens");
    let orphan = source(ProjectId::generate(), None);

    let err = store
        .insert_source_asset(&orphan)
        .expect_err("fk must refuse");

    assert!(
        matches!(err, ResearchError::Database(_)),
        "a dangling project reference is a database error, got {err:?}"
    );
    assert!(
        store
            .sources_of(orphan.project_id)
            .expect("listing ok")
            .is_empty(),
        "and nothing was written"
    );
}

/// Given a project that does not exist
/// When a research link names it
/// Then the link is refused rather than left dangling.
#[test]
fn a_research_link_needs_a_real_project() {
    let store = Store::open_in_memory().expect("store opens");

    let err = store
        .link_research_project(
            ProjectId::generate(),
            ResearchProjectId::generate(),
            "2026-09-07T00:00:00Z",
        )
        .expect_err("fk must refuse");

    assert!(
        matches!(err, ResearchError::Database(_)),
        "a link to no project is refused, got {err:?}"
    );
}

/// Given the full set of Creator types
/// When each is inserted in dependency order
/// Then all eleven have durable rows, and the chain from a composition
/// revision to a metric observation is walkable.
#[test]
fn every_creator_type_has_a_durable_row() {
    let store = Store::open_in_memory().expect("store opens");
    let p = project("streamer");
    store.insert_project(&p).expect("project inserted");
    let s = source(p.id, None);
    store.insert_source_asset(&s).expect("source inserted");
    store
        .insert_capture_session(&CaptureSession {
            id: CaptureSessionId::generate(),
            project_id: p.id,
            source_asset_id: Some(s.id),
            started_at: at("2026-09-07T00:00:00Z"),
            ended_at: Some(at("2026-09-07T02:00:00Z")),
            finalized: true,
            attributes_json: "{}".into(),
            created_at: at("2026-09-07T02:00:00Z"),
        })
        .expect("capture session inserted");
    let cut = revision(p.id, 1, None);
    store
        .insert_composition_revision(&cut)
        .expect("revision inserted");

    let variant = Variant {
        id: VariantId::generate(),
        project_id: p.id,
        composition_revision_id: cut.id,
        name: "vertical".into(),
        format_overrides_json: "{}".into(),
        created_at: at("2026-09-07T02:00:00Z"),
    };
    store.insert_variant(&variant).expect("variant inserted");
    let plan = RenderPlan {
        id: RenderPlanId::generate(),
        project_id: p.id,
        variant_id: variant.id,
        renderer: "ffmpeg".into(),
        renderer_version: "7.1".into(),
        resolved_inputs_json: "{}".into(),
        content_hash: "sha256:plan".into(),
        created_at: at("2026-09-07T02:00:00Z"),
    };
    store
        .insert_render_plan(&plan)
        .expect("render plan inserted");
    let artifact = Artifact {
        id: ArtifactId::generate(),
        project_id: p.id,
        variant_id: Some(variant.id),
        render_plan_id: Some(plan.id),
        kind: "render".into(),
        content_hash: "sha256:artifact".into(),
        locator: None,
        bytes: Some(1024),
        width: Some(1080),
        height: Some(1920),
        duration: Some(dur(60_000)),
        created_at: at("2026-09-07T02:00:00Z"),
    };
    store.insert_artifact(&artifact).expect("artifact inserted");
    let package = ReleasePackage {
        id: ReleasePackageId::generate(),
        project_id: p.id,
        artifact_id: artifact.id,
        metadata_json: "{}".into(),
        approved_by: "operator".into(),
        approved_at: Some(at("2026-09-07T02:05:00Z")),
        state: "approved".into(),
        created_at: at("2026-09-07T02:05:00Z"),
    };
    store
        .insert_release_package(&package)
        .expect("package inserted");
    let publication = Publication {
        id: PublicationId::generate(),
        project_id: p.id,
        release_package_id: package.id,
        destination: "youtube".into(),
        account: "channel".into(),
        state: "published".into(),
        scheduled_at: None,
        timezone: "UTC".into(),
        remote_id: Some("yt-1".into()),
        uploaded_at: Some(at("2026-09-07T02:06:00Z")),
        published_at: Some(at("2026-09-07T02:07:00Z")),
        last_error: None,
        created_at: at("2026-09-07T02:05:00Z"),
    };
    store
        .insert_publication(&publication)
        .expect("publication inserted");
    store
        .insert_metric_observation(&MetricObservation {
            id: MetricObservationId::generate(),
            project_id: p.id,
            publication_id: Some(publication.id),
            release_package_id: Some(package.id),
            artifact_id: Some(artifact.id),
            composition_revision_id: Some(cut.id),
            provider: "youtube".into(),
            metric: "views".into(),
            definition_version: "1".into(),
            scope: "lifetime".into(),
            unit: "count".into(),
            denominator: None,
            interval_start: at("2026-09-07T02:07:00Z"),
            interval_end: at("2026-09-08T02:07:00Z"),
            timezone: "UTC".into(),
            value: Some(1_024.0),
            coverage: Some(1.0),
            retrieved_at: at("2026-09-08T02:08:00Z"),
            source_query_json: "{}".into(),
            transform_json: "{}".into(),
            chart_json: "{}".into(),
        })
        .expect("metric inserted");

    assert_eq!(
        store.creator_tables().expect("tables listed").len(),
        super::super::schema::TABLES.len(),
        "all Creator tables exist in a Creator store"
    );
}
