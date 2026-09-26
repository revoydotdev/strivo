//! The unsaved window: what it holds, what it refuses, and what a kill leaves.
//!
//! Two failure modes are load-bearing. A draft that outlives the revision it
//! was made against is work aimed at the wrong thing. And a test asserting "the
//! draft did not change" against a store that never wrote one is free — so
//! every refusal here is paired with a positive control showing a legitimate
//! draft **is** accepted.

use tempfile::TempDir;

use strivo_editor::history::Autosave;
use strivo_editor::transcript::{Correction, TranscriptDraft, UnsavedWork, WordId};

use super::super::ids::{CompositionRevisionId, ProjectId};
use super::super::store::Store;
use super::super::HistoryStoreError;
use super::fixtures::{project, revision};

/// A store with one project, a workspace open, and two revisions to draft
/// against.
fn seeded() -> (Store, TempDir, ProjectId, CompositionRevisionId) {
    let dir = TempDir::new().expect("tempdir");
    let store = Store::open(dir.path().join("research.db")).expect("store opens");
    let p = project("streamer");
    store.insert_project(&p).expect("project inserted");
    let root = revision(p.id, 1, None);
    store
        .insert_composition_revision(&root)
        .expect("root edit inserted");
    let second = revision(p.id, 2, Some(root.id));
    store
        .insert_composition_revision(&second)
        .expect("second edit inserted");
    store.open_workspace(p.id, "t0").expect("workspace opened");
    (store, dir, p.id, root.id)
}

fn draft_against(base: &str) -> UnsavedWork {
    UnsavedWork {
        transcript: Some(TranscriptDraft {
            base_revision: base.into(),
            corrected: vec![Correction {
                word: WordId::new("w7").expect("a usable word id"),
                text: "roughly".into(),
            }],
            deleted: Vec::new(),
        }),
    }
}

fn stored_draft(store: &Store, project: ProjectId) -> String {
    store.workspace(project).expect("readable").draft
}

/// The bound the plan requires documented and measured, measured here rather
/// than asserted in prose. A keystroke restarts it, so this is the longest an
/// edit sits inside an unsaved window. Nothing *acknowledged* is inside one at
/// all: a commit is a history entry, and only uncommitted work is a draft.
#[test]
fn the_unsaved_window_is_measured_at_two_seconds_from_the_last_edit() {
    let mut a = Autosave::default();
    assert_eq!(Autosave::DEFAULT_INTERVAL_MS, 2_000);

    a.touched(10_000);
    a.touched(11_500);
    assert!(!a.due(12_499), "the window fired before the bound elapsed");
    assert_eq!(a.unsaved_window_ms(12_499), 999);
    assert!(a.due(13_500));
    assert_eq!(a.unsaved_window_ms(13_500), 2_000);
}

/// The positive control the two tests below depend on: a draft against the
/// revision the workspace *is* on is written, flagged, and readable back. If
/// this fails, "the draft did not change" below is satisfied by nothing.
#[test]
fn a_draft_against_the_selected_revision_is_accepted() {
    let (store, _d, project, root) = seeded();
    store
        .select_revision(project, root, "t1")
        .expect("selected");
    let work = draft_against(&root.to_string());

    store
        .save_draft(project, root, &work, "t2")
        .expect("a legitimate draft is accepted");

    let ws = store.workspace(project).expect("readable");
    assert!(ws.dirty, "the draft was written without the unsaved flag");
    let stored: UnsavedWork = serde_json::from_str(&ws.draft).expect("a readable draft");
    assert_eq!(stored, work);
}

/// Given a complete draft already stored
/// When a second write is refused
/// Then the stored draft is the *first* one, whole — never a blend. SQLite gives
/// this because the write is one statement in one transaction, so a kill during
/// it rolls the page back rather than committing half.
#[test]
fn a_refused_draft_leaves_the_previous_one_whole() {
    let (store, _d, project, root) = seeded();
    store
        .select_revision(project, root, "t1")
        .expect("selected");
    let first = draft_against(&root.to_string());
    store
        .save_draft(project, root, &first, "t2")
        .expect("saved");
    let before = stored_draft(&store, project);
    assert!(before.contains("w7"), "the positive control did not run");

    // A draft naming a revision the workspace is not on is refused, and its
    // body never reaches the column.
    let other = revision(project, 2, Some(root));
    store
        .insert_composition_revision(&other)
        .expect("second edit inserted");
    let second = draft_against(&other.id.to_string());
    assert!(matches!(
        store.save_draft(project, other.id, &second, "t3"),
        Err(HistoryStoreError::DraftSuperseded { .. })
    ));

    assert_eq!(
        stored_draft(&store, project),
        before,
        "a refused draft changed the stored one"
    );
}

/// The acceptance criterion's other half, in the store: the refusal names both
/// revisions, so a client can say which one it was editing against and which
/// one is open.
#[test]
fn a_draft_against_an_unselected_revision_names_both_revisions() {
    let (store, _d, project, root) = seeded();
    let other = revision(project, 2, Some(root));
    store
        .insert_composition_revision(&other)
        .expect("second edit inserted");
    store
        .select_revision(project, other.id, "t1")
        .expect("selected");

    match store.save_draft(project, root, &draft_against(&root.to_string()), "t2") {
        Err(HistoryStoreError::DraftSuperseded { base, current }) => {
            assert_eq!(base, root.to_string());
            assert_eq!(current, other.id.to_string());
        }
        other => panic!("expected a supersession, got {other:?}"),
    }
    assert!(!store.workspace(project).expect("readable").dirty);
}

#[test]
fn acknowledging_a_draft_clears_it_and_the_flag_together() {
    let (store, _d, project, root) = seeded();
    store
        .select_revision(project, root, "t1")
        .expect("selected");
    store
        .save_draft(project, root, &draft_against(&root.to_string()), "t2")
        .expect("saved");
    assert!(stored_draft(&store, project).contains("w7"));

    store
        .acknowledge_draft(project, "t3")
        .expect("acknowledged");

    let ws = store.workspace(project).expect("readable");
    assert!(!ws.dirty, "the flag outlived the draft");
    assert_eq!(ws.draft, "{}", "the draft outlived its acknowledgement");
}
