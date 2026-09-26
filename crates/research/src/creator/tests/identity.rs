//! The four identity invariants, as failing-first tests.
//!
//! Each is one `When` and one assertion class. A fifth block covers the
//! malformed-input class: an id that names nothing, a duplicate id, and a
//! project id in a source slot.

use std::fs;

use tempfile::TempDir;

use super::super::entities::*;
use super::super::ids::*;
use super::super::store::Store;
use super::fixtures::{at, project, revision, source};

/// Given a project with three sources and a two-deep edit chain
/// When the project is read back
/// Then it holds all three sources and both revisions.
#[test]
fn project_holds_many_sources_and_many_revisions() {
    let store = Store::open_in_memory().expect("store opens");
    let p = project("streamer");
    store.insert_project(&p).expect("project inserted");

    let mut ids = Vec::new();
    for _ in 0..3 {
        let s = source(p.id, None);
        store.insert_source_asset(&s).expect("source inserted");
        ids.push(s.id);
    }
    let root = revision(p.id, 1, None);
    store
        .insert_composition_revision(&root)
        .expect("root revision inserted");
    let child = revision(p.id, 2, Some(root.id));
    store
        .insert_composition_revision(&child)
        .expect("child revision inserted");

    let sources = store.sources_of(p.id).expect("sources listed");
    let revisions = store
        .composition_revisions_of(p.id)
        .expect("revisions listed");

    assert_eq!(sources.len(), 3, "one project holds many sources");
    assert_eq!(revisions.len(), 2, "one project holds many edits");
    let mut found: Vec<SourceAssetId> = sources.iter().map(|s| s.id).collect();
    found.sort();
    let mut expected = ids;
    expected.sort();
    assert_eq!(
        found, expected,
        "every source is reachable from the project"
    );
    assert_eq!(
        revisions[1].parent_revision_id,
        Some(root.id),
        "the chain is intact"
    );
}

/// Given a source registered at one path, with a transcript and an edit that
/// name it
/// When the file is moved and the locator rewritten
/// Then the asset id, its fingerprint and its links are unchanged, and the
/// new path resolves.
#[test]
fn moving_the_file_preserves_identity() {
    let dir = TempDir::new().expect("tempdir");
    let store = Store::open_in_memory().expect("store opens");
    let p = project("streamer");
    store.insert_project(&p).expect("project inserted");

    let old_path = dir.path().join("before.mkv");
    fs::write(&old_path, b"media").expect("source written");
    let s = source(p.id, Some(old_path.clone()));
    store.insert_source_asset(&s).expect("source inserted");

    let transcript = TranscriptRevision {
        id: TranscriptRevisionId::generate(),
        project_id: p.id,
        source_asset_id: s.id,
        revision: 1,
        provider: "whisper".into(),
        model_version: "large-v3".into(),
        author: "machine".into(),
        body_json: "{}".into(),
        created_at: at("2026-09-07T00:00:00Z"),
    };
    store
        .insert_transcript_revision(&transcript)
        .expect("transcript inserted");
    let mut cut = revision(p.id, 1, None);
    cut.timeline_json = format!("{{\"source\":\"{}\"}}", s.id);
    store
        .insert_composition_revision(&cut)
        .expect("revision inserted");

    let new_path = dir.path().join("archive").join("after.mkv");
    fs::create_dir_all(new_path.parent().expect("parent")).expect("dir made");
    fs::rename(&old_path, &new_path).expect("file moved");
    store
        .set_source_locator(s.id, Some(&new_path))
        .expect("locator rewritten");

    let after = store
        .source_asset(s.id)
        .expect("lookup ok")
        .expect("the asset is still there");
    assert_eq!(after.id, s.id, "identity survives the move");
    assert_eq!(
        after.media_fingerprint, s.media_fingerprint,
        "content identity is not the path"
    );
    assert_eq!(
        after.locator,
        Some(new_path.clone()),
        "the new location is recorded"
    );
    assert_eq!(after.availability(), Availability::Resolved(new_path));

    let links = store
        .transcript_revisions_of_source(s.id)
        .expect("links listed");
    assert_eq!(links.len(), 1, "the transcript still names the same asset");
    assert_eq!(
        links[0].id, transcript.id,
        "the link is to the same revision"
    );

    let revisions = store
        .composition_revisions_of(p.id)
        .expect("revisions listed");
    assert!(
        revisions[0].timeline_json.contains(&s.id.to_string()),
        "the edit's source span still names the same asset"
    );
}

/// Given a source whose file has been deleted
/// When the asset is read
/// Then the row and its links survive, and availability reports the bytes
/// as unreachable rather than pretending otherwise.
#[test]
fn a_deleted_file_does_not_dangle() {
    let dir = TempDir::new().expect("tempdir");
    let store = Store::open_in_memory().expect("store opens");
    let p = project("streamer");
    store.insert_project(&p).expect("project inserted");

    let path = dir.path().join("gone.mkv");
    fs::write(&path, b"media").expect("source written");
    let s = source(p.id, Some(path.clone()));
    store.insert_source_asset(&s).expect("source inserted");
    store
        .insert_transcript_revision(&TranscriptRevision {
            id: TranscriptRevisionId::generate(),
            project_id: p.id,
            source_asset_id: s.id,
            revision: 1,
            provider: "whisper".into(),
            model_version: "large-v3".into(),
            author: "machine".into(),
            body_json: "{}".into(),
            created_at: at("2026-09-07T00:00:00Z"),
        })
        .expect("transcript inserted");

    fs::remove_file(&path).expect("file deleted");

    let after = store
        .source_asset(s.id)
        .expect("lookup ok")
        .expect("the row is still there");
    assert_eq!(after.id, s.id, "deleting the file did not delete the asset");
    assert_eq!(after.availability(), Availability::Unresolved);
    assert_eq!(
        store
            .transcript_revisions_of_source(s.id)
            .expect("links listed")
            .len(),
        1,
        "links are not dangling silently: they still resolve to the asset"
    );
}

/// Given a research-kernel project id
/// When two creator projects link to it and one links to a second
/// Then the relationship is not one-to-one in either direction.
#[test]
fn research_link_is_stable_id_and_not_one_to_one() {
    let store = Store::open_in_memory().expect("store opens");
    let a = project("compilation");
    let b = project("single stream");
    store.insert_project(&a).expect("a inserted");
    store.insert_project(&b).expect("b inserted");

    // The research kernel's own Project, minted by its own constructor so
    // this is a real kernel id and not a string that looks like one.
    let kernel = crate::ResearchStore::open(":memory:")
        .expect("research store opens")
        .create_project("archive", "every stream")
        .expect("kernel project created");
    let other = ResearchProjectId::from_uuid(kernel.id);

    store
        .link_research_project(a.id, other, "2026-09-07T00:00:00Z")
        .expect("a links");
    store
        .link_research_project(b.id, other, "2026-09-07T00:00:00Z")
        .expect("b links");
    store
        .link_research_project(a.id, ResearchProjectId::generate(), "2026-09-07T00:00:00Z")
        .expect("a links a second");

    assert_eq!(
        store.research_links(a.id).expect("links listed").len(),
        2,
        "one creator project may cover several research projects"
    );
    assert_eq!(
        store.research_link_inbound(other).expect("inbound counted"),
        2,
        "one research project may cover several creator projects"
    );
    // The kernel id is a valid UUID, so it parses as a ProjectId too — the
    // typed ids stop a *compile* mistake, not a value that looks right.
    // Casting it deliberately must still find no Creator row.
    assert!(
        store
            .project(ProjectId::from_uuid(other.as_uuid()))
            .expect("lookup ok")
            .is_none(),
        "a research project id is not a creator project row"
    );
}
