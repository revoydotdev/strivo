//! The history's claims, each one able to fail.
//!
//! Two matter most: [`branch_does_not_mutate_its_parent`] — the claim most
//! likely to be quietly false — and
//! [`redo_is_refused_once_a_branch_diverges`], because a redo that silently
//! discards newer work makes the whole history a lie.

use super::{Autosave, Commit, Entry, History, HistoryError, MAX_LABEL_BYTES};

/// A three-deep history: `a` -> `b` -> `c`.
fn chain() -> History {
    let mut h = History::new();
    h.commit(Commit::new("a", "open", "t0")).unwrap();
    h.commit(Commit::new("b", "trim clip 3", "t1").on("a"))
        .unwrap();
    h.commit(Commit::new("c", "correct word w7", "t2").on("b"))
        .unwrap();
    h
}

fn keys(h: &History) -> Vec<&str> {
    h.entries().iter().map(|e| e.key.as_str()).collect()
}

fn entry(key: &str, parent: Option<&str>) -> Entry {
    Entry {
        key: key.into(),
        parent: parent.map(str::to_string),
        replaced: None,
        label: key.into(),
        created_at: "t0".into(),
    }
}

// ── the tip, and what "current" means ────────────────────────────────────

#[test]
fn the_tip_is_the_newest_commit() {
    assert_eq!(chain().tip().map(|e| e.key.as_str()), Some("c"));
}

#[test]
fn an_empty_history_has_no_tip_rather_than_a_placeholder_one() {
    assert!(History::new().tip().is_none());
}

#[test]
fn every_commit_appears_exactly_once_in_commit_order() {
    assert_eq!(keys(&chain()), vec!["a", "b", "c"]);
}

#[test]
fn the_chain_runs_from_the_tip_back_to_the_root() {
    let h = chain();
    let walked: Vec<&str> = h
        .chain("c")
        .unwrap()
        .iter()
        .map(|e| e.key.as_str())
        .collect();
    assert_eq!(walked, vec!["c", "b", "a"]);
}

// ── branch divergence: the claim most likely to be quietly false ──────────

#[test]
fn branch_does_not_mutate_its_parent() {
    let mut h = chain();
    let before = h.get("a").unwrap().clone();
    let children_before = h.children("a").unwrap().len();

    h.branch("a", Commit::new("d", "alt-open", "t3")).unwrap();

    assert_eq!(
        *h.get("a").unwrap(),
        before,
        "branching rewrote the entry it forked from"
    );
    assert_eq!(
        h.children("a").unwrap().len(),
        children_before + 1,
        "the fork did not add a child to its parent"
    );
    assert_eq!(h.get("d").unwrap().parent.as_deref(), Some("a"));
    assert_eq!(
        keys(&h).first().copied(),
        Some("a"),
        "branching reordered or removed history"
    );
}

/// The positive control for the test above. Without it, "a branch changed
/// nothing" is satisfied by an implementation that changes nothing at all — a
/// refusal-free stub passes every not-mutated assertion for free.
#[test]
fn a_plain_commit_does_change_the_tip() {
    let mut h = chain();
    h.commit(Commit::new("d", "ripple 4s", "t3").on("c"))
        .unwrap();
    assert_eq!(h.tip().map(|e| e.key.as_str()), Some("d"));
    assert_ne!(keys(&h), vec!["a", "b", "c"]);
}

#[test]
fn the_original_line_survives_a_branch() {
    let mut h = chain();
    h.branch("a", Commit::new("d", "alt-open", "t3")).unwrap();
    h.commit(Commit::new("e", "alt-trim", "t4").on("d"))
        .unwrap();

    let alt: Vec<&str> = h
        .chain("e")
        .unwrap()
        .iter()
        .map(|x| x.key.as_str())
        .collect();
    assert_eq!(alt, vec!["e", "d", "a"]);
    assert_eq!(h.get("c").unwrap().parent.as_deref(), Some("b"));
    assert_eq!(h.children("a").unwrap().len(), 2);
}

// ── undo restores a revision, not an inverse edit ────────────────────────

/// Undo appends an entry whose parent is the tip's parent: the state after an
/// undo *is* the entry the undo was restoring, named rather than recomputed.
/// The entry it replaced is recorded, which is the only reason redo is
/// answerable without a session stack.
#[test]
fn undo_restores_the_parent_entry_rather_than_recomputing_it() {
    let mut h = chain();
    h.undo(Commit::new("d", "undo the correction", "t3"))
        .unwrap();

    let tip = h.tip().unwrap();
    assert_eq!(tip.parent.as_deref(), Some("b"));
    assert_eq!(tip.replaced.as_deref(), Some("c"));
    assert_eq!(
        keys(&h),
        vec!["a", "b", "c", "d"],
        "undo rewound the history"
    );
}

#[test]
fn undo_past_the_beginning_is_a_typed_state() {
    let mut h = History::new();
    h.commit(Commit::new("a", "open", "t0")).unwrap();
    match h.undo(Commit::new("d", "undo", "t1")) {
        Err(HistoryError::NothingToUndo { tip }) => assert_eq!(tip, "a"),
        other => panic!("expected NothingToUndo, got {other:?}"),
    }
    assert_eq!(keys(&h), vec!["a"]);
}

// ── redo, and the branch that must invalidate it ─────────────────────────

#[test]
fn redo_names_the_entry_the_undo_replaced() {
    let mut h = chain();
    h.undo(Commit::new("d", "undo", "t3")).unwrap();
    assert_eq!(h.redo_target().unwrap().key, "c");

    h.redo(Commit::new("e", "redo", "t4")).unwrap();
    let tip = h.tip().unwrap();
    assert_eq!(tip.parent.as_deref(), Some("d"));
    assert!(tip.replaced.is_none(), "a redo replaced nothing");
    assert_eq!(keys(&h), vec!["a", "b", "c", "d", "e"]);
}

/// The one that makes the history trustworthy. A branch taken after an undo
/// would be silently discarded by a redo, so redo is refused and says which
/// entry it wanted.
#[test]
fn redo_is_refused_once_a_branch_diverges() {
    let mut h = chain();
    h.undo(Commit::new("d", "undo", "t3")).unwrap();
    h.branch("a", Commit::new("e", "a different idea", "t4"))
        .unwrap();

    match h.redo(Commit::new("f", "redo", "t5")) {
        Err(HistoryError::RedoDiverged { tip, would_restore }) => {
            assert_eq!(tip, "e");
            assert_eq!(would_restore, "c");
        }
        other => panic!("expected RedoDiverged, got {other:?}"),
    }
    assert_eq!(h.tip().map(|e| e.key.as_str()), Some("e"));
    assert_eq!(keys(&h).len(), 5, "a refused redo wrote a row");
}

#[test]
fn redo_with_no_prior_undo_is_a_typed_state() {
    match chain().redo_target() {
        Err(HistoryError::NothingToRedo { tip }) => assert_eq!(tip, "c"),
        other => panic!("expected NothingToRedo, got {other:?}"),
    }
}

/// The same rule by the route a real editor takes: an undo, then a *new edit*
/// rather than an explicit branch. That edit is a fork of the point the undo
/// rolled back to, and redoing across it would throw it away.
#[test]
fn a_new_edit_after_an_undo_also_kills_redo() {
    let mut h = chain();
    h.undo(Commit::new("d", "undo", "t3")).unwrap();
    h.commit(Commit::new("e", "a different idea", "t4").on("d"))
        .unwrap();

    match h.redo(Commit::new("f", "redo", "t5")) {
        Err(HistoryError::RedoDiverged { tip, would_restore }) => {
            assert_eq!(tip, "e");
            assert_eq!(would_restore, "c");
        }
        other => panic!("expected RedoDiverged, got {other:?}"),
    }
}

#[test]
fn redo_on_an_empty_history_is_a_typed_state() {
    assert!(matches!(
        History::new().redo_target(),
        Err(HistoryError::NothingToUndo { .. })
    ));
}

// ── snapshots ────────────────────────────────────────────────────────────

/// A snapshot is an ordinary commit that points at the tip and carries a name.
/// No second table: it is a row the history can be pointed at.
#[test]
fn a_snapshot_is_a_named_commit_on_the_tip() {
    let mut h = chain();
    h.snapshot(Commit::new("d", "before-the-correction", "t3"))
        .unwrap();
    let tip = h.tip().unwrap();
    assert_eq!(tip.parent.as_deref(), Some("c"));
    assert!(tip.replaced.is_none(), "a snapshot undid nothing");
}

#[test]
fn snapshots_are_found_by_name_newest_first() {
    let mut h = chain();
    h.snapshot(Commit::new("d", "first", "t3")).unwrap();
    h.commit(Commit::new("e", "more", "t4").on("d")).unwrap();
    h.snapshot(Commit::new("f", "second", "t5")).unwrap();

    let found: Vec<&str> = h
        .snapshots("second")
        .iter()
        .map(|e| e.label.as_str())
        .collect();
    assert_eq!(found, vec!["second"]);
    assert!(h.snapshots("no-such-snapshot").is_empty());
    // A snapshot names a revision, so restoring one is the same commit every
    // other restore is.
    assert_eq!(h.snapshots("first")[0].key, "d");
}

// ── malformed input: every refusal is a value, never a panic ─────────────

#[test]
fn a_commit_naming_an_unknown_parent_is_refused() {
    let mut h = chain();
    match h.commit(Commit::new("d", "x", "t3").on("nope")) {
        Err(HistoryError::UnknownEntry { key }) => assert_eq!(key, "nope"),
        other => panic!("expected UnknownEntry, got {other:?}"),
    }
    assert_eq!(keys(&h), vec!["a", "b", "c"]);
}

#[test]
fn a_branch_from_a_revision_that_does_not_exist_is_refused() {
    let mut h = chain();
    assert!(matches!(
        h.branch("nope", Commit::new("d", "x", "t3")),
        Err(HistoryError::UnknownEntry { .. })
    ));
}

#[test]
fn a_branch_off_the_tip_is_refused_as_a_commit_instead() {
    let mut h = chain();
    assert!(matches!(
        h.branch("c", Commit::new("d", "x", "t3")),
        Err(HistoryError::BranchOffTip { .. })
    ));
}

/// A cycle would make every walk unbounded, so it is refused at the door
/// rather than discovered by hanging.
#[test]
fn a_cycle_in_the_parent_chain_is_refused_rather_than_walked() {
    let h = History::from_entries(vec![entry("a", Some("b")), entry("b", Some("a"))]);
    match h.chain("a") {
        Err(HistoryError::Cycle { key }) => assert_eq!(key, "a"),
        other => panic!("expected Cycle, got {other:?}"),
    }
}

#[test]
fn a_dangling_parent_is_reported_rather_than_walked_past() {
    let h = History::from_entries(vec![entry("a", Some("ghost"))]);
    assert!(matches!(
        h.chain("a"),
        Err(HistoryError::UnknownEntry { .. })
    ));
}

/// A label is untrusted text that reaches a user interface. It is **data**:
/// stored and returned byte for byte, never interpreted, so markup is legal
/// content and refusing it would be a bug in the other direction. What is
/// refused is a label that cannot be *recorded* — empty, or carrying a control
/// character, which would break the log line it lands in.
#[test]
fn a_label_is_stored_verbatim_and_only_unrecordable_ones_are_refused() {
    let markup = "<script>alert(1)</script>";
    let mut h = History::new();
    h.commit(Commit::new("k", markup, "t0")).unwrap();
    assert_eq!(h.get("k").unwrap().label, markup, "the label was altered");
    assert_eq!(
        h.snapshots(markup).len(),
        1,
        "a label containing markup could not be found again by its own name"
    );

    for bad in ["", "   ", "a\nb", "\u{7}", &"x".repeat(MAX_LABEL_BYTES + 1)] {
        let mut fresh = History::new();
        assert!(
            matches!(
                fresh.commit(Commit::new("k", bad, "t0")),
                Err(HistoryError::LabelInvalid { .. })
            ),
            "label {bad:?} was accepted"
        );
        assert!(fresh.entries().is_empty());
    }
    // The positive control. Without it, "must refuse a bad label" is satisfied
    // by refusing everything and every other assertion here is free.
    assert!(h.commit(Commit::new("k2", "trim clip 3", "t0")).is_ok());
}

#[test]
fn a_duplicate_key_is_refused_rather_than_silently_shadowing() {
    let mut h = chain();
    match h.commit(Commit::new("c", "again", "t3").on("b")) {
        Err(HistoryError::DuplicateKey { key }) => assert_eq!(key, "c"),
        other => panic!("expected DuplicateKey, got {other:?}"),
    }
    assert_eq!(h.entries().iter().filter(|e| e.key == "c").count(), 1);
    assert_eq!(keys(&h), vec!["a", "b", "c"]);
}

/// Every commit parents to an entry that already existed and takes a key that
/// does not, so the API cannot build a cycle at all — and a walk from any tip
/// reaches the root in exactly one step per entry.
#[test]
fn the_api_cannot_construct_a_parent_cycle() {
    let mut h = chain();
    h.branch("a", Commit::new("d", "alt", "t3")).unwrap();
    h.commit(Commit::new("e", "more", "t4").on("d")).unwrap();

    // A self-parenting commit is the shortest possible cycle, and it is refused
    // because the key is taken.
    assert!(matches!(
        h.commit(Commit::new("e", "self", "t5").on("e")),
        Err(HistoryError::DuplicateKey { .. })
    ));
    // Every entry's walk terminates at a root, and visits nothing twice. The
    // forked tip's chain is shorter than the table — `b` and `c` are the
    // original line and are simply not on it.
    for entry in h.entries() {
        let walked = h.chain(&entry.key).unwrap();
        assert!(
            walked.last().unwrap().parent.is_none(),
            "{} did not reach a root",
            entry.key
        );
        let mut seen: Vec<&str> = Vec::new();
        for node in &walked {
            assert!(
                !seen.contains(&node.key.as_str()),
                "{} repeated {}",
                node.key,
                entry.key
            );
            seen.push(&node.key);
        }
    }
}

#[test]
fn an_undo_naming_an_entry_that_vanished_is_refused() {
    let mut h = History::from_entries(vec![entry("a", None), entry("b", Some("ghost"))]);
    assert!(matches!(
        h.undo(Commit::new("c", "undo", "t1")),
        Err(HistoryError::UnknownEntry { .. })
    ));
}

#[test]
fn every_refusal_reports_a_stable_code() {
    let mut h = chain();
    let cases: Vec<HistoryError> = vec![
        h.commit(Commit::new("d", "x", "t3").on("nope"))
            .unwrap_err(),
        h.branch("nope", Commit::new("d", "x", "t3")).unwrap_err(),
        h.branch("c", Commit::new("d", "x", "t3")).unwrap_err(),
        h.redo_target().unwrap_err(),
        h.commit(Commit::new("e", "x", "t3").on("c").replacing("nope"))
            .unwrap_err(),
        h.commit(Commit::new("f", "", "t3").on("c")).unwrap_err(),
    ];
    let codes: Vec<&str> = cases.iter().map(HistoryError::code).collect();
    assert_eq!(
        codes,
        vec![
            "history/unknown-entry",
            "history/unknown-entry",
            "history/branch-off-tip",
            "history/empty",
            "history/unknown-entry",
            "history/label-invalid",
        ]
    );
}

// ── the unsaved window ───────────────────────────────────────────────────

/// The window is a value the caller drives. It must be bounded, and it must
/// close the moment the edit is acknowledged.
#[test]
fn the_unsaved_window_is_bounded_and_closes_on_acknowledgement() {
    let mut a = Autosave::default();
    a.touched(1_000);
    assert!(!a.due(1_000), "an autosave fired with no elapsed time");
    assert!(!a.due(1_000 + Autosave::DEFAULT_INTERVAL_MS - 1));
    assert!(a.due(1_000 + Autosave::DEFAULT_INTERVAL_MS));
    assert_eq!(
        a.unsaved_window_ms(1_000 + Autosave::DEFAULT_INTERVAL_MS),
        Autosave::DEFAULT_INTERVAL_MS
    );

    a.acknowledge();
    assert!(!a.due(1_000 + Autosave::DEFAULT_INTERVAL_MS + 1));
    assert!(!a.due(1_000 + 10 * Autosave::DEFAULT_INTERVAL_MS));
    assert_eq!(
        a.unsaved_window_ms(1_000 + 10 * Autosave::DEFAULT_INTERVAL_MS),
        0
    );
}

/// Every keystroke restarts the window, so the measured bound is the bound from
/// the *last* edit — not the first plus the interval.
#[test]
fn a_keystroke_restarts_the_window_rather_than_extending_it() {
    let mut a = Autosave::default();
    a.touched(0);
    a.touched(Autosave::DEFAULT_INTERVAL_MS - 1);
    assert!(!a.due(Autosave::DEFAULT_INTERVAL_MS));
    assert!(a.due(2 * Autosave::DEFAULT_INTERVAL_MS - 1));
}

/// A measured zero is a real instant, and reading it as "no time yet" is the
/// `||`-guards-a-legitimate-zero defect this plan has paid for before.
#[test]
fn an_edit_made_at_instant_zero_still_opens_a_window() {
    let mut a = Autosave::default();
    a.touched(0);
    assert!(a.due(Autosave::DEFAULT_INTERVAL_MS));
    assert!(!a.due(0), "an autosave fired inside the same instant");
}
