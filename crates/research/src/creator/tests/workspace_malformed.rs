//! What the workspace refuses, and what it reports rather than repairing.
//!
//! Every case here is a *typed* refusal or a *reported* condition. The three
//! that matter most:
//!
//! * a selection naming a revision that is not in the project — type confusion
//!   is how one entity's data reaches another's surface, and the typed ids only
//!   stop it at compile time;
//! * a selected revision whose row is gone — the selection must be reported
//!   dangling, never quietly cleared, or the next read would look like a
//!   project that was never selected;
//! * a selected revision whose content changed after selection — detectably
//!   stale, because a view rendering it would be showing work that is not the
//!   selection.

use tempfile::TempDir;

use super::super::ids::*;
use super::super::store::Store;
use super::super::workspace::{View, WorkspaceError};
use super::fixtures::{at, project, revision, source};

/// A store on a temp file with one project, two revisions and an open
/// workspace. Shared by every case below, so no test states setup it does not
/// assert against.
fn seeded() -> (
    TempDir,
    Store,
    ProjectId,
    CompositionRevisionId,
    CompositionRevisionId,
) {
    let dir = TempDir::new().expect("tempdir");
    let store = Store::open(dir.path().join("research.db")).expect("store opens");
    let p = project("streamer");
    store.insert_project(&p).expect("project inserted");
    let first = revision(p.id, 1, None);
    store
        .insert_composition_revision(&first)
        .expect("first edit inserted");
    let second = revision(p.id, 2, Some(first.id));
    store
        .insert_composition_revision(&second)
        .expect("second edit inserted");
    store
        .open_workspace(p.id, &at("2026-09-07T01:00:00Z"))
        .expect("workspace");
    (dir, store, p.id, first.id, second.id)
}

/// Given a workspace open on one project
/// When a revision belonging to a *different* project is selected
/// Then the selection is refused with a typed error naming both projects —
/// the id types stop this at compile time, and the membership check stops it
/// at runtime.
#[test]
fn selecting_a_revision_from_another_project_is_refused() {
    let (_dir, store, p, _first, _second) = seeded();
    let other = project("other-streamer");
    store
        .insert_project(&other)
        .expect("other project inserted");
    let foreign = revision(other.id, 1, None);
    store
        .insert_composition_revision(&foreign)
        .expect("foreign edit inserted");

    let refusal = store
        .select_revision(p, foreign.id, &at("2026-09-07T02:00:00Z"))
        .expect_err("a foreign revision cannot be selected");
    assert!(
        matches!(refusal, WorkspaceError::RevisionNotInProject { .. }),
        "typed refusal, got {refusal:?}"
    );
    assert_eq!(
        store.workspace(p).expect("read back").selection.composition,
        None,
        "and the selection was not written"
    );
}

/// Given a workspace with a revision selected
/// When that revision's row is deleted out from under it
/// Then reading the workspace reports the selection dangling rather than
/// silently falling back to nothing or to another revision.
#[test]
fn a_selection_naming_a_deleted_revision_is_reported_not_cleared() {
    let (_dir, store, p, first, _second) = seeded();
    store
        .select_revision(p, first, &at("2026-09-07T01:00:00Z"))
        .expect("selected");

    // No public method deletes a revision — an immutable edit is not deleted
    // through the workspace. A hand-run statement is how a corrupt or
    // externally-edited database holds it, which is exactly the case that must
    // be reported rather than repaired.
    store
        .raw_execute(
            "DELETE FROM creator_composition_revisions WHERE id=?1",
            &[first.to_string()],
        )
        .expect("row removed underneath the selection");

    let ws = store.workspace(p).expect("the row itself still reads");
    assert_eq!(
        ws.selection.composition,
        Some(first),
        "the selection is still recorded, not quietly cleared"
    );
    let refusal = store
        .resolve(p)
        .expect_err("resolving a dangling selection fails");
    assert!(
        matches!(refusal, WorkspaceError::Dangling { .. }),
        "and it is reported, got {refusal:?}"
    );
}

/// Given a workspace whose selected revision is later changed on disk
/// When a view resolves the workspace
/// Then the change is reported as stale, naming both fingerprints, rather
/// than being rendered as the selection.
#[test]
fn a_selected_revision_edited_after_selection_is_detectably_stale() {
    let (_dir, store, p, first, _second) = seeded();
    store
        .select_revision(p, first, &at("2026-09-07T01:00:00Z"))
        .expect("selected");
    store
        .raw_execute(
            "UPDATE creator_composition_revisions SET timeline_json=?2 WHERE id=?1",
            &[first.to_string(), r#"{"rev":"rewritten"}"#.to_string()],
        )
        .expect("row rewritten underneath the selection");

    let refusal = store
        .resolve(p)
        .expect_err("a rewritten selection is stale");
    match refusal {
        WorkspaceError::Stale { selected, .. } => assert_eq!(selected, first, "names the revision"),
        other => panic!("expected a staleness report, got {other:?}"),
    }
}

/// Given a project with no edits at all
/// When its workspace is opened and a selection demanded
/// Then the demand is a typed refusal, not a default selection.
#[test]
fn a_project_with_no_revisions_refuses_to_select_one() {
    let dir = TempDir::new().expect("tempdir");
    let store = Store::open(dir.path().join("research.db")).expect("store opens");
    let p = project("empty");
    store.insert_project(&p).expect("project inserted");
    store
        .insert_source_asset(&source(p.id, None))
        .expect("a recording, but no edit");
    let ws = store
        .open_workspace(p.id, &at("2026-09-07T01:00:00Z"))
        .expect("workspace");

    assert!(
        store
            .composition_revisions_of(p.id)
            .expect("edits listed")
            .is_empty(),
        "the project really holds no edits"
    );
    assert!(
        matches!(
            ws.require_selection().expect_err("refused"),
            WorkspaceError::NoSelection { .. }
        ),
        "so requiring one is refused"
    );
    let refusal = store
        .resolve(p.id)
        .expect_err("and resolving reports it rather than inventing a revision");
    assert!(
        matches!(refusal, WorkspaceError::NoSelection { .. }),
        "typed, got {refusal:?}"
    );
}

/// Given a workspace holding one draft
/// When a draft that is not a JSON object is saved
/// Then the save is refused and the previous draft and flag survive — a write
/// interrupted mid-flight cannot leave a project with content but no record
/// that it is unsaved, because the two move in one statement.
#[test]
fn a_malformed_draft_is_refused_and_leaves_the_previous_one_intact() {
    let (_dir, store, p, _first, _second) = seeded();
    let good = store
        .view_state(p)
        .expect("view state")
        .with_draft(r#"{"cut":1}"#, true);
    store
        .save_view_state(p, &good, &at("2026-09-07T01:05:00Z"))
        .expect("good draft saved");

    let bad = store
        .view_state(p)
        .expect("view state")
        .with_draft("not json at all", true);
    let refusal = store
        .save_view_state(p, &bad, &at("2026-09-07T01:06:00Z"))
        .expect_err("a draft that is not a JSON object is refused");
    assert!(
        matches!(refusal, WorkspaceError::MalformedDraft { .. }),
        "typed, got {refusal:?}"
    );
    let ws = store.workspace(p).expect("read back");
    assert_eq!(ws.draft, r#"{"cut":1}"#, "the previous draft is intact");
    assert!(ws.dirty, "and it is still recorded as unsaved");
}

/// Given a workspace, whose range the Rust type already makes forward-only
/// When a raw statement stores an inverted range
/// Then the column's own CHECK refuses it — the storage boundary is a second
/// line of defence, not the only one, and both are tested.
#[test]
fn the_column_also_refuses_an_inverted_selection_range() {
    let (_dir, store, p, _first, _second) = seeded();
    store
        .raw_execute(
            "UPDATE creator_workspace SET selection_start_us=?2, selection_end_us=?3
             WHERE project_id=?1",
            &[p.to_string(), "2000000".to_string(), "1000000".to_string()],
        )
        .expect_err("an inverted range is refused by the schema itself");

    // A half-range is refused too: the other end of the selection would be a
    // guess, and a view would render the guess.
    store
        .raw_execute(
            "UPDATE creator_workspace SET selection_start_us=?2, selection_end_us=NULL
             WHERE project_id=?1",
            &[p.to_string(), "1000000".to_string()],
        )
        .expect_err("half a range is refused by the schema itself");

    assert_eq!(
        store.workspace(p).expect("read back").range,
        None,
        "so no range was written"
    );
}

/// Given a workspace whose row names a view this build does not have
/// When the column is written directly
/// Then the schema refuses it, and the Rust-side name parser refuses it too —
/// the storage boundary and the type are both tested, because the type is what
/// a caller meets first and the column is what a restored database meets.
#[test]
fn an_unknown_view_name_is_refused_by_both_the_column_and_the_type() {
    let (_dir, store, p, _first, _second) = seeded();
    store
        .raw_execute(
            "UPDATE creator_workspace SET active_view=?2 WHERE project_id=?1",
            &[p.to_string(), "colour-grading".to_string()],
        )
        .expect_err("the CHECK list refuses a view this build does not have");

    assert!(
        "colour-grading".parse::<View>().is_err(),
        "and the parser refuses it too, rather than defaulting to the player"
    );
    assert_eq!(
        "player".parse::<View>().expect("a real view parses"),
        View::Player,
        "while a real one still does"
    );
}

/// Given a workspace opened for a project that does not exist
/// When it is opened
/// Then the refusal is typed and names the project — not an empty workspace
/// that looks like a project nobody has worked in.
#[test]
fn a_workspace_for_an_unknown_project_is_refused() {
    let dir = TempDir::new().expect("tempdir");
    let store = Store::open(dir.path().join("research.db")).expect("store opens");
    let ghost = ProjectId::generate();
    let refusal = store
        .open_workspace(ghost, &at("2026-09-07T01:00:00Z"))
        .expect_err("an unknown project has no workspace");
    assert!(
        matches!(refusal, WorkspaceError::ProjectNotFound { .. }),
        "typed, got {refusal:?}"
    );
}
