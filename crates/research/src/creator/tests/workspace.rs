//! The project workspace: one project, many recordings and many edits, and
//! **one** selected revision that every view reads.
//!
//! These are the todo's two load-bearing claims, written before the code
//! exists. Each test states the precondition it depends on before asserting
//! the many-ness or the sharing, because a workspace holding nothing satisfies
//! "many" for free — a vacuous pass is the failure mode this file is shaped
//! against.
//!
//! The two-views test opens **two** stores on one file rather than reading
//! twice through one handle. Two views are two independent readers; a shared
//! connection would make the agreement a property of the cache rather than of
//! the state.

use tempfile::TempDir;

use strivo_editor::time::Interval;

use super::super::store::Store;
use super::super::workspace::{View, WorkspaceError};
use super::fixtures::{at, project, revision, source};

/// Given a project that exists and holds two recordings and two edits
/// When it is read back as a workspace
/// Then it holds both recordings and both edits, and the workspace names that
/// project — the "one project holds many" half, asserted only after the
/// subject is known to be non-empty.
#[test]
fn one_project_holds_many_recordings_and_many_edits() {
    let dir = TempDir::new().expect("tempdir");
    let db = dir.path().join("research.db");
    let store = Store::open(&db).expect("store opens");
    let p = project("streamer");
    store.insert_project(&p).expect("project inserted");
    for _ in 0..2 {
        store
            .insert_source_asset(&source(p.id, None))
            .expect("recording inserted");
    }
    let root = revision(p.id, 1, None);
    store
        .insert_composition_revision(&root)
        .expect("root edit inserted");
    let child = revision(p.id, 2, Some(root.id));
    store
        .insert_composition_revision(&child)
        .expect("second edit inserted");

    // The subject is non-empty before anything is asserted about it.
    let sources = store.sources_of(p.id).expect("recordings listed");
    let revisions = store.composition_revisions_of(p.id).expect("edits listed");
    assert_eq!(sources.len(), 2, "the project holds two recordings");
    assert_eq!(revisions.len(), 2, "the project holds two edits");

    let ws = store
        .open_workspace(p.id, &at("2026-09-07T01:00:00Z"))
        .expect("workspace opens");
    assert_eq!(ws.project_id, p.id, "the workspace is this project");
    assert_eq!(
        store.sources_of(ws.project_id).expect("recordings").len(),
        2,
        "the workspace's project still holds both recordings"
    );
    assert_eq!(
        store
            .composition_revisions_of(ws.project_id)
            .expect("edits")
            .len(),
        2,
        "the workspace's project still holds both edits"
    );
}

/// Given two views reading the same project workspace concurrently, with one
/// revision selected
/// When both read, and then the selection is changed
/// Then both see the same revision before the change and the same new one
/// after it — one selected revision, not one per view.
#[test]
fn two_views_reading_concurrently_see_one_selected_revision() {
    let dir = TempDir::new().expect("tempdir");
    let db = dir.path().join("research.db");
    let seed = Store::open(&db).expect("store opens");
    let p = project("streamer");
    seed.insert_project(&p).expect("project inserted");
    let first = revision(p.id, 1, None);
    seed.insert_composition_revision(&first)
        .expect("first edit inserted");
    let second = revision(p.id, 2, Some(first.id));
    seed.insert_composition_revision(&second)
        .expect("second edit inserted");
    seed.open_workspace(p.id, &at("2026-09-07T01:00:00Z"))
        .expect("workspace");
    seed.select_revision(p.id, first.id, &at("2026-09-07T01:00:00Z"))
        .expect("first revision selected");

    // Two independent readers, as two browser tabs are.
    let view_a = Store::open(&db).expect("view A opens");
    let view_b = Store::open(&db).expect("view B opens");
    let a = view_a.resolve(p.id).expect("view A resolves");
    let b = view_b.resolve(p.id).expect("view B resolves");
    assert_eq!(
        a.selection.composition, b.selection.composition,
        "both views read one selected revision, not one each"
    );
    assert_eq!(
        a.composition.id, first.id,
        "and it is the one that was selected"
    );

    view_a
        .select_revision(p.id, second.id, &at("2026-09-07T02:00:00Z"))
        .expect("view A switches the selection");

    let a = view_a.resolve(p.id).expect("view A re-reads");
    let b = view_b.resolve(p.id).expect("view B re-reads");
    assert_eq!(
        a.selection.composition, b.selection.composition,
        "still one"
    );
    assert_eq!(b.composition.id, second.id, "view B sees the change");
    assert_eq!(a.composition.id, second.id, "view A sees it too");
}

/// Given a workspace with work in progress — a playhead, a selection range,
/// a selected revision and an unsaved draft
/// When the view switches from the timeline to the transcript and back to the
/// inspector
/// Then the project, the playhead, the range, the selected revision and the
/// draft are all still there.
#[test]
fn switching_view_preserves_the_project_playhead_and_selection() {
    let dir = TempDir::new().expect("tempdir");
    let db = dir.path().join("research.db");
    let store = Store::open(&db).expect("store opens");
    let p = project("streamer");
    store.insert_project(&p).expect("project inserted");
    let edit = revision(p.id, 1, None);
    store
        .insert_composition_revision(&edit)
        .expect("edit inserted");
    store
        .open_workspace(p.id, &at("2026-09-07T01:00:00Z"))
        .expect("workspace");
    store
        .select_revision(p.id, edit.id, &at("2026-09-07T01:00:00Z"))
        .expect("selected");

    let range = Interval::new(
        strivo_editor::time::MediaTime::from_millis(1_000).expect("positive"),
        strivo_editor::time::MediaTime::from_millis(4_000).expect("positive"),
    )
    .expect("a forward range");
    let mut state = store
        .view_state(p.id)
        .expect("view state starts")
        .with_playhead(strivo_editor::time::MediaTime::from_millis(1_200).expect("positive"))
        .with_range(range)
        .with_active_view(View::Timeline)
        .with_draft(r#"{"trim":"a"}"#, true);
    store
        .save_view_state(p.id, &state, &at("2026-09-07T01:05:00Z"))
        .expect("work in progress saved");

    for view in [View::Transcript, View::Inspector] {
        state = state.with_active_view(view);
        store
            .save_view_state(p.id, &state, &at("2026-09-07T01:06:00Z"))
            .expect("view switch persists");
        let read = store
            .resolve(p.id)
            .expect("the workspace still resolves after a view switch");
        assert_eq!(read.workspace.project_id, p.id, "the project survived");
        assert_eq!(
            read.workspace.playhead.map(|t| t.micros()),
            Some(1_200_000),
            "the playhead survived the switch to {view:?}"
        );
        assert_eq!(read.workspace.range, Some(range), "the selection survived");
        assert_eq!(
            read.selection.composition,
            Some(edit.id),
            "the edit survived"
        );
        assert_eq!(
            read.workspace.draft, r#"{"trim":"a"}"#,
            "the draft survived"
        );
        assert!(read.workspace.dirty, "the unsaved work is still unsaved");
    }
}

/// Given a workspace holding unsaved work
/// When the whole store is closed and reopened — a reload
/// Then the unsaved work, the playhead, the range, the selected revision and
/// the active view all come back.
#[test]
fn unsaved_work_survives_a_reload() {
    let dir = TempDir::new().expect("tempdir");
    let db = dir.path().join("research.db");
    let (project_id, edit_id) = {
        let store = Store::open(&db).expect("store opens");
        let p = project("streamer");
        store.insert_project(&p).expect("project inserted");
        let edit = revision(p.id, 1, None);
        store
            .insert_composition_revision(&edit)
            .expect("edit inserted");
        store
            .open_workspace(p.id, &at("2026-09-07T01:00:00Z"))
            .expect("workspace");
        store
            .select_revision(p.id, edit.id, &at("2026-09-07T01:00:00Z"))
            .expect("selected");
        let state = store
            .view_state(p.id)
            .expect("view state")
            .with_playhead(strivo_editor::time::MediaTime::from_millis(9_000).expect("positive"))
            .with_active_view(View::Transcript)
            .with_draft(r#"{"unsaved":"keep me"}"#, true);
        store
            .save_view_state(p.id, &state, &at("2026-09-07T01:05:00Z"))
            .expect("saved");
        (p.id, edit.id)
    };

    // The store above is gone; this is a fresh process's open of the same file.
    let reopened = Store::open(&db).expect("reopened after the reload");
    let read = reopened
        .resolve(project_id)
        .expect("resolves after a reload");
    assert_eq!(
        read.workspace.draft, r#"{"unsaved":"keep me"}"#,
        "draft survived"
    );
    assert!(read.workspace.dirty, "still marked unsaved");
    assert_eq!(read.workspace.playhead.map(|t| t.micros()), Some(9_000_000));
    assert_eq!(read.workspace.active_view, View::Transcript);
    assert_eq!(read.selection.composition, Some(edit_id));
}

/// Given a workspace whose draft was saved and then discarded
/// When the workspace is read
/// Then the draft is empty and no longer dirty — discarding is one action that
/// cannot leave the flag behind the content.
#[test]
fn discarding_the_draft_clears_the_unsaved_flag_with_it() {
    let dir = TempDir::new().expect("tempdir");
    let db = dir.path().join("research.db");
    let store = Store::open(&db).expect("store opens");
    let p = project("streamer");
    store.insert_project(&p).expect("project inserted");
    store
        .open_workspace(p.id, &at("2026-09-07T01:00:00Z"))
        .expect("workspace");
    let state = store
        .view_state(p.id)
        .expect("view state")
        .with_draft(r#"{"wip":1}"#, true);
    store
        .save_view_state(p.id, &state, &at("2026-09-07T01:05:00Z"))
        .expect("saved");
    store.discard_draft(p.id).expect("discarded");

    let ws = store.workspace(p.id).expect("read back");
    assert_eq!(ws.draft, "{}", "the draft is empty after a discard");
    assert!(!ws.dirty, "and the unsaved flag went with it");
}

/// Given a project that has never had a workspace
/// When one is opened
/// Then it starts with no selection and no playhead — reported as empty, not
/// defaulted to some revision.
#[test]
fn a_fresh_workspace_selects_nothing_rather_than_defaulting() {
    let dir = TempDir::new().expect("tempdir");
    let db = dir.path().join("research.db");
    let store = Store::open(&db).expect("store opens");
    let p = project("streamer");
    store.insert_project(&p).expect("project inserted");

    let ws = store
        .open_workspace(p.id, &at("2026-09-07T01:00:00Z"))
        .expect("workspace opens");
    assert_eq!(ws.selection.composition, None, "nothing is selected yet");
    assert_eq!(ws.playhead, None, "no playhead yet");
    assert_eq!(ws.active_view, View::Player, "and it opens on the player");

    let refusal = ws
        .require_selection()
        .expect_err("a project with no selected revision is refused");
    assert!(
        matches!(refusal, WorkspaceError::NoSelection { .. }),
        "and the refusal is typed, got {refusal:?}"
    );
}
