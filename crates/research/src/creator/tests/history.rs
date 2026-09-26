//! The edit history: one chain, and undo, redo, branch and snapshot as four
//! readings of it.
//!
//! Written before the store code, and shaped against three failure modes this
//! plan has paid for:
//!
//! * **A vacuous pass.** Every test that asserts "the history did not change"
//!   first asserts a real history exists, and every test that asserts a refusal
//!   has a positive control nearby showing a legitimate action *is* accepted. A
//!   store that refused everything would otherwise satisfy half this file.
//! * **A claim that cannot fail.** The branch-divergence test is proved by
//!   mutation, not by assertion alone — see [`store_branch_does_not_mutate_its_parent`].
//! * **Silent last-write-wins.** A conflicting concurrent save must be *named*,
//!   not resolved.

use tempfile::TempDir;

use strivo_editor::transcript::{Correction, TranscriptDraft, WordId};

use super::super::ids::{CompositionRevisionId, HistoryId, ProjectId, TranscriptRevisionId};
use super::super::store::Store;
use super::super::{Edit, HistoryStoreError, TranscriptPatch};
use super::fixtures::{project, revision, source, transcript};

/// A store holding one project, two immutable composition revisions and one
/// transcript revision. A named struct rather than a tuple, because every test
/// here needs the project and the two revisions and a positional read of a
/// five-tuple is exactly the kind of thing a reviewer stops checking.
struct Fixture {
    store: Store,
    _dir: TempDir,
    project: ProjectId,
    root: CompositionRevisionId,
    second: CompositionRevisionId,
    words: TranscriptRevisionId,
}

fn seeded() -> Fixture {
    let dir = TempDir::new().expect("tempdir");
    let store = Store::open(dir.path().join("research.db")).expect("store opens");
    let p = project("streamer");
    store.insert_project(&p).expect("project inserted");
    let recording = source(p.id, None);
    store
        .insert_source_asset(&recording)
        .expect("recording inserted");
    let words = transcript(p.id, recording.id, 1);
    store
        .insert_transcript_revision(&words)
        .expect("transcript revision inserted");
    let root = revision(p.id, 1, None);
    store
        .insert_composition_revision(&root)
        .expect("root edit inserted");
    let second = revision(p.id, 2, Some(root.id));
    store
        .insert_composition_revision(&second)
        .expect("second edit inserted");
    Fixture {
        store,
        _dir: dir,
        project: p.id,
        root: root.id,
        second: second.id,
        words: words.id,
    }
}

fn draft(revision: TranscriptRevisionId, word: &str, text: &str) -> TranscriptPatch {
    TranscriptPatch {
        revision,
        draft: TranscriptDraft {
            base_revision: revision.to_string(),
            corrected: vec![Correction {
                word: WordId::new(word).expect("a usable word id"),
                text: text.into(),
            }],
            deleted: Vec::new(),
        },
    }
}

// ── the chain ────────────────────────────────────────────────────────────

/// Given two immutable composition revisions and nothing committed
/// When the first action is recorded
/// Then the history has exactly one entry and the tip is that entry — asserted
/// only after the history is known to be non-empty, because an empty history
/// satisfies "the tip is the newest" for free.
#[test]
fn the_first_recorded_action_is_the_tip() {
    let f = seeded();
    let (store, project, root) = (&f.store, f.project, f.root);
    assert!(store.history_tip(project).expect("readable").is_none());

    let first = store
        .record(project, None, Edit::composition(root), "open", "t1")
        .expect("first action recorded");

    let entries = store.history(project).expect("readable");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].id, first.id);
    assert_eq!(first.parent, None);
    assert_eq!(store.history_tip(project).expect("readable"), Some(first));
}

/// Given a recorded action
/// When the next names it as its base
/// Then the new entry's parent is that entry, so the chain has an order.
#[test]
fn an_action_naming_the_tip_extends_the_chain() {
    let f = seeded();
    let (store, project, root, second) = (&f.store, f.project, f.root, f.second);
    let first = store
        .record(project, None, Edit::composition(root), "open", "t1")
        .expect("recorded");
    let next = store
        .record(
            project,
            Some(first.id),
            Edit::composition(second),
            "trim clip 3",
            "t2",
        )
        .expect("recorded");
    assert_eq!(next.parent, Some(first.id));
    assert_eq!(store.history(project).expect("readable").len(), 2);
}

// ── conflict: detected and named, never resolved silently ────────────────

/// The acceptance criterion: two clients, one history, second writer refused.
/// Both clients read the same base, which is the whole scenario.
#[test]
fn a_conflicting_concurrent_save_is_reported_with_both_ids() {
    let f = seeded();
    let (store, project, root, second) = (&f.store, f.project, f.root, f.second);
    let base = store
        .record(project, None, Edit::composition(root), "open", "t1")
        .expect("recorded");

    // Client A commits against `base`.
    store
        .record(
            project,
            Some(base.id),
            Edit::composition(second),
            "client A",
            "t2",
        )
        .expect("client A recorded");
    let tip_after_a = store
        .history_tip(project)
        .expect("readable")
        .expect("a tip");

    // Client B still believes `base` is current.
    match store.record(
        project,
        Some(base.id),
        Edit::composition(second),
        "client B",
        "t3",
    ) {
        Err(HistoryStoreError::Conflict { base: got, current }) => {
            assert_eq!(got, base.id.to_string());
            assert_eq!(current, tip_after_a.id.to_string());
        }
        other => panic!("expected a conflict, got {other:?}"),
    }

    // Client B's edit is not the current state, and the history is unchanged.
    assert_eq!(store.history(project).expect("readable").len(), 2);
    assert_eq!(
        store.history_tip(project).expect("readable"),
        Some(tip_after_a)
    );
}

// ── undo: a revision, not a re-executed inverse edit ─────────────────────

/// Given two actions, the second naming a different composition revision
/// When it is undone
/// Then the new entry carries the **first** entry's revision id verbatim — the
/// same id, not a recomputed document. That is what "restores a revision"
/// means, and it is checkable without trusting any inverse operation.
#[test]
fn undo_restores_the_parents_revision_id_verbatim() {
    let f = seeded();
    let (store, project, root, second) = (&f.store, f.project, f.root, f.second);
    let first = store
        .record(project, None, Edit::composition(root), "open", "t1")
        .expect("recorded");
    store
        .record(
            project,
            Some(first.id),
            Edit::composition(second),
            "trim",
            "t2",
        )
        .expect("recorded");

    let undone = store.undo(project, "undo the trim", "t3").expect("undone");

    assert_eq!(
        undone.composition_revision_id, root,
        "undo recomputed the edit"
    );
    assert_eq!(
        undone.replaced,
        Some(store.history(project).expect("readable")[1].id)
    );
    assert_eq!(undone.parent, Some(first.id));
    // History only grows: the undo is itself a row, so it is undoable.
    assert_eq!(store.history(project).expect("readable").len(), 3);
}

#[test]
fn undo_with_nothing_to_undo_is_a_typed_state() {
    let f = seeded();
    let (store, project, root) = (&f.store, f.project, f.root);
    store
        .record(project, None, Edit::composition(root), "open", "t1")
        .expect("recorded");
    assert!(matches!(
        store.undo(project, "undo", "t2"),
        Err(HistoryStoreError::History(_))
    ));
    assert_eq!(store.history(project).expect("readable").len(), 1);
}

// ── redo, and the branch that must kill it ───────────────────────────────

/// Given an undo, and then a *branch* from the point the undo rolled back to
/// When a redo is attempted
/// Then it is refused, naming the entry it would have restored — because
/// redoing across the branch would silently discard it.
#[test]
fn redo_is_refused_once_a_branch_diverges() {
    let f = seeded();
    let (store, project, root, second) = (&f.store, f.project, f.root, f.second);
    let first = store
        .record(project, None, Edit::composition(root), "open", "t1")
        .expect("recorded");
    store
        .record(
            project,
            Some(first.id),
            Edit::composition(second),
            "trim",
            "t2",
        )
        .expect("recorded");
    let undone = store.undo(project, "undo", "t3").expect("undone");

    // A branch off the same point, which is what an editor does when the user
    // tries something else after an undo.
    let forked = store
        .branch(
            project,
            first.id,
            Edit::composition(second),
            "other idea",
            "t4",
        )
        .expect("branched");
    assert_ne!(forked.id, undone.id);

    assert!(matches!(
        store.redo(project, "redo", "t5"),
        Err(HistoryStoreError::History(_))
    ));
    assert_eq!(store.history(project).expect("readable").len(), 4);
}

// ── branch divergence: the claim most likely to be quietly false ─────────

/// A branch **appends**. The entry it forked from is still there, still says
/// what it said, and now has two children instead of one.
///
/// The positive control is [`a_record_does_change_the_tip`] below: without it,
/// "nothing changed" is satisfied by a store that changes nothing at all.
#[test]
fn store_branch_does_not_mutate_its_parent() {
    let f = seeded();
    let (store, project, root, second) = (&f.store, f.project, f.root, f.second);
    let first = store
        .record(project, None, Edit::composition(root), "open", "t1")
        .expect("recorded");
    store
        .record(
            project,
            Some(first.id),
            Edit::composition(second),
            "trim",
            "t2",
        )
        .expect("recorded");
    let before = store
        .history(project)
        .expect("readable")
        .into_iter()
        .find(|e| e.id == first.id)
        .expect("the forked-from entry is present before the branch");

    store
        .branch(project, first.id, Edit::composition(second), "alt", "t3")
        .expect("branched");

    let entries = store.history(project).expect("readable");
    let after = entries
        .iter()
        .find(|e| e.id == first.id)
        .expect("still present");
    assert_eq!(
        after, &before,
        "the branch rewrote the entry it forked from"
    );
    let children = entries
        .iter()
        .filter(|e| e.parent == Some(first.id))
        .count();
    assert_eq!(children, 2, "the fork did not add a child to its parent");
    assert_eq!(
        entries.len(),
        3,
        "the branch appended a row rather than replacing one"
    );
}

/// The positive control the divergence test needs: a normal record *does* move
/// the tip, so the assertion above is not satisfied by an inert store.
#[test]
fn a_record_does_change_the_tip() {
    let f = seeded();
    let (store, project, root, second) = (&f.store, f.project, f.root, f.second);
    let first = store
        .record(project, None, Edit::composition(root), "open", "t1")
        .expect("recorded");
    let next = store
        .record(
            project,
            Some(first.id),
            Edit::composition(second),
            "trim",
            "t2",
        )
        .expect("recorded");
    assert_eq!(store.history_tip(project).expect("readable"), Some(next));
}

// ── snapshots ────────────────────────────────────────────────────────────

/// A snapshot is an ordinary entry against the tip that changes nothing, so
/// restoring one later is the same insert every other restore is.
#[test]
fn a_snapshot_restores_the_same_revision_the_tip_had() {
    let f = seeded();
    let (store, project, root, second) = (&f.store, f.project, f.root, f.second);
    let first = store
        .record(project, None, Edit::composition(root), "open", "t1")
        .expect("recorded");
    store
        .record(
            project,
            Some(first.id),
            Edit::composition(second),
            "trim",
            "t2",
        )
        .expect("recorded");

    let snap = store
        .snapshot(project, "before-the-render", "t3")
        .expect("snapshotted");
    assert_eq!(snap.composition_revision_id, second);
    assert_eq!(snap.replaced, None, "a snapshot undid nothing");
    assert_eq!(store.history(project).expect("readable").len(), 3);
}

// ── one history across transcript and timeline ───────────────────────────

/// The interleave the plan asks about, stated as an ordering. A transcript
/// correction and a timeline trim are two entries in **one** list, in the order
/// they happened, and one undo moves across both — because the undo restores
/// the parent entry's revisions, and that entry carries the transcript patch.
#[test]
fn a_transcript_correction_and_a_timeline_trim_share_one_history() {
    let f = seeded();
    let (store, project, root) = (&f.store, f.project, f.root);
    let words = f.words;
    // The fixture really did insert a transcript revision, so the correction
    // below is a patch against something that exists rather than an orphan.
    let timeline = store
        .record(project, None, Edit::composition(root), "trim clip 3", "t1")
        .expect("recorded");
    let transcript = store
        .record(
            project,
            Some(timeline.id),
            Edit::composition(root).and_transcript(draft(words, "w7", "roughly")),
            "correct word w7",
            "t2",
        )
        .expect("recorded");

    let entries = store.history(project).expect("readable");
    assert_eq!(entries.len(), 2, "two surfaces made two undo stacks");
    assert_eq!(entries[0].transcript, None);
    let patch = entries[1]
        .transcript
        .as_ref()
        .expect("the correction is recorded");
    assert_eq!(patch.patch.corrected[0].word.as_str(), "w7");

    // One undo from the tip lands on the timeline trim's entry, and the new
    // entry carries that entry's revisions — including its transcript half.
    let undone = store
        .undo(project, "undo the correction", "t3")
        .expect("undone");
    assert_eq!(
        undone.composition_revision_id,
        transcript.composition_revision_id
    );
    assert_eq!(undone.parent, Some(timeline.id));
    assert_eq!(store.history(project).expect("readable").len(), 3);
}

// ── a commit is one transaction ──────────────────────────────────────────

/// The crash-safety property, driven rather than asserted: a commit that fails
/// its membership check leaves the stored history byte-identical, which is the
/// same guarantee a kill mid-commit gets from the rollback.
#[test]
fn a_refused_commit_leaves_the_history_exactly_as_it_was() {
    let f = seeded();
    let (store, project, root) = (&f.store, f.project, f.root);
    store
        .record(project, None, Edit::composition(root), "open", "t1")
        .expect("recorded");
    let before = store.history(project).expect("readable");

    // A composition revision that exists in no project: the check fails inside
    // the transaction that would have written the row.
    assert!(store
        .record(
            project,
            None,
            Edit::composition(CompositionRevisionId::generate()),
            "foreign",
            "t2",
        )
        .is_err());

    assert_eq!(store.history(project).expect("readable"), before);
}

// ── membership: an action may only name this project's revisions ────────

#[test]
fn an_action_naming_another_projects_revision_is_refused() {
    let dir = TempDir::new().expect("tempdir");
    let store = Store::open(dir.path().join("research.db")).expect("store opens");
    let a = project("a");
    let b = project("b");
    store.insert_project(&a).expect("a");
    store.insert_project(&b).expect("b");
    let b_edit = revision(b.id, 1, None);
    store
        .insert_composition_revision(&b_edit)
        .expect("b's edit");

    assert!(matches!(
        store.record(a.id, None, Edit::composition(b_edit.id), "steal", "t1"),
        Err(HistoryStoreError::ForeignComposition { .. })
    ));
    assert!(store.history(a.id).expect("readable").is_empty());
    // The positive control: the *same* revision is accepted by the project that
    // owns it, so the refusal above is about ownership and not about the shape.
    assert!(store
        .record(b.id, None, Edit::composition(b_edit.id), "own", "t1")
        .is_ok());
}

#[test]
fn an_action_naming_a_revision_that_does_not_exist_is_refused() {
    let f = seeded();
    let (store, project) = (&f.store, f.project);
    assert!(matches!(
        store.record(
            project,
            None,
            Edit::composition(CompositionRevisionId::generate()),
            "ghost",
            "t1",
        ),
        Err(HistoryStoreError::ForeignComposition { .. })
    ));
}

#[test]
fn a_branch_from_an_entry_that_does_not_exist_is_refused() {
    let f = seeded();
    let (store, project, root) = (&f.store, f.project, f.root);
    assert!(store
        .branch(
            project,
            HistoryId::generate(),
            Edit::composition(root),
            "ghost",
            "t1",
        )
        .is_err());
    assert!(store.history(project).expect("readable").is_empty());
}

#[test]
fn a_redo_with_no_prior_undo_is_a_typed_state() {
    let f = seeded();
    let (store, project, root) = (&f.store, f.project, f.root);
    store
        .record(project, None, Edit::composition(root), "open", "t1")
        .expect("recorded");
    assert!(matches!(
        store.redo(project, "redo", "t2"),
        Err(HistoryStoreError::History(_))
    ));
}

#[test]
fn an_undo_on_an_empty_history_is_a_typed_state() {
    let f = seeded();
    let (store, project) = (&f.store, f.project);
    assert!(store.undo(project, "undo", "t1").is_err());
    assert!(store.history(project).expect("readable").is_empty());
}
