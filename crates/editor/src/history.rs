//! The edit history: one append-only chain, and undo, redo, branch and
//! snapshot as that same append read four ways.
//!
//! A key is the writing store's own typed identity — a `Uuid` newtype that
//! validates where it is minted — so this module checks no key shape. Checking
//! it here would be the second validation of the same fact.
//!
//! ## The whole mechanism
//!
//! An [`Entry`] is a row: a key, the entry it was made against (`parent`), the
//! entry it put back if it was an undo (`replaced`), a label and a timestamp.
//! [`History::commit`] is the only thing that adds one. Undo, redo, branch and
//! snapshot are four argument sets over it — no second store, no inverse-edit
//! log, and no session stack whose contents a reload would lose.
//!
//! ## What undo restores
//!
//! **A revision, not a re-executed inverse edit.** Undo appends an entry whose
//! `parent` is the tip's parent, so the state after an undo *is* the entry the
//! undo was restoring. Nothing is computed backwards: there is no "un-trim"
//! that could disagree with what a trim did, and so no second path to a
//! composition that todo 23's compiler is no longer the only way to reach.
//!
//! ## Redo, and the branch that must kill it
//!
//! An undo records what it replaced, so a redo needs no memory of its own. It
//! is refused the moment the history has moved on, and the refusal names the
//! entry it wanted — a redo that silently discarded newer work would make the
//! whole history a lie.
//!
//! ## One history, transcript and timeline alike
//!
//! An entry is an *action*, not a surface. A transcript correction and a
//! timeline trim are two entries in this one chain, in the order they
//! happened, and one undo moves across both. What an entry carries is the
//! writing store's business; this module never assumes it is only a timeline.

use std::collections::BTreeSet;

/// One committed action, in commit order.
///
/// `parent` is the whole of "what this was made against". `replaced` is the
/// whole of "undo": an entry that put another one back names it, which is why
/// [`History::redo_target`] can answer without a session stack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub key: String,
    pub parent: Option<String>,
    pub replaced: Option<String>,
    pub label: String,
    pub created_at: String,
}

/// The arguments of one append. A value object rather than five positional
/// parameters, because they are read together and swapping two of them by
/// position is otherwise not a compile error.
#[derive(Debug, Clone, Copy)]
pub struct Commit<'a> {
    key: &'a str,
    parent: Option<&'a str>,
    replaced: Option<&'a str>,
    label: &'a str,
    at: &'a str,
}

impl<'a> Commit<'a> {
    /// A commit against no prior entry: the first action in a history.
    #[must_use]
    pub const fn new(key: &'a str, label: &'a str, at: &'a str) -> Self {
        Self {
            key,
            parent: None,
            replaced: None,
            label,
            at,
        }
    }

    /// The entry this was made against. A history of unrelated roots is a
    /// list, not a history.
    #[must_use]
    pub const fn on(mut self, parent: &'a str) -> Self {
        self.parent = Some(parent);
        self
    }

    /// The entry this one puts back.
    #[must_use]
    pub const fn replacing(mut self, replaced: &'a str) -> Self {
        self.replaced = Some(replaced);
        self
    }
}

/// Every entry of one history, in commit order. The last is the tip, and the
/// tip is what "current" means.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct History {
    entries: Vec<Entry>,
}

impl History {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Adopt rows read back from storage, in commit order. Nothing is validated
    /// here: a stored history may be corrupt, and the reads that walk it are
    /// where a cycle or a dangling parent is *reported* rather than discovered
    /// by hanging.
    #[must_use]
    pub fn from_entries(entries: Vec<Entry>) -> Self {
        Self { entries }
    }

    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The newest commit: the state the editor is on.
    #[must_use]
    pub fn tip(&self) -> Option<&Entry> {
        self.entries.last()
    }

    /// One entry by key.
    pub fn get(&self, key: &str) -> Result<&Entry, HistoryError> {
        self.entries
            .iter()
            .find(|e| e.key == key)
            .ok_or_else(|| HistoryError::UnknownEntry {
                key: key.to_string(),
            })
    }

    /// Every entry made against `key`. Two of them is a branch, which is why
    /// this is a public question and not an internal one.
    pub fn children(&self, key: &str) -> Result<Vec<&Entry>, HistoryError> {
        self.get(key)?;
        Ok(self
            .entries
            .iter()
            .filter(|e| e.parent.as_deref() == Some(key))
            .collect())
    }

    /// Every entry carrying `label`, newest first. A snapshot is a commit with
    /// a name, so this is a filter over the one table rather than a second one.
    #[must_use]
    pub fn snapshots(&self, label: &str) -> Vec<&Entry> {
        self.entries
            .iter()
            .rev()
            .filter(|e| e.label == label)
            .collect()
    }

    /// The entry names and every entry it descends from, ending at the root.
    ///
    /// Bounded by the number of entries, and a repeat is a
    /// [`HistoryError::Cycle`] rather than an infinite walk.
    pub fn chain(&self, from: &str) -> Result<Vec<&Entry>, HistoryError> {
        let mut seen = BTreeSet::new();
        let mut out = Vec::new();
        let mut cursor = Some(from.to_string());
        while let Some(key) = cursor {
            if !seen.insert(key.clone()) {
                return Err(HistoryError::Cycle { key });
            }
            let entry = self.get(&key)?;
            out.push(entry);
            cursor = entry.parent.clone();
        }
        Ok(out)
    }

    /// The one way an entry is created.
    pub fn commit(&mut self, c: Commit<'_>) -> Result<&Entry, HistoryError> {
        error::check_label(c.label)?;
        if self.entries.iter().any(|e| e.key == c.key) {
            return Err(HistoryError::DuplicateKey {
                key: c.key.to_string(),
            });
        }
        let Some(parent) = c.parent else {
            return self.push(c, None);
        };
        self.get(parent)?;
        if let Some(replaced) = c.replaced {
            self.get(replaced)?;
        }
        self.push(c, Some(parent))
    }

    fn push(&mut self, c: Commit<'_>, parent: Option<&str>) -> Result<&Entry, HistoryError> {
        self.entries.push(Entry {
            key: c.key.to_string(),
            parent: parent.map(str::to_string),
            replaced: c.replaced.map(str::to_string),
            label: c.label.to_string(),
            created_at: c.at.to_string(),
        });
        Ok(self.entries.last().expect("just pushed"))
    }

    fn tip_or_empty(&self) -> Result<&Entry, HistoryError> {
        self.tip().ok_or_else(|| HistoryError::NothingToUndo {
            tip: String::from("(empty)"),
        })
    }

    /// Undo: append an entry made against the tip's parent, recording the tip
    /// as what it put back. The restored state is that parent entry verbatim —
    /// nothing is computed backwards.
    pub fn undo(&mut self, c: Commit<'_>) -> Result<&Entry, HistoryError> {
        let tip = self.tip_or_empty()?;
        let Some(parent) = tip.parent.clone() else {
            return Err(HistoryError::NothingToUndo {
                tip: tip.key.clone(),
            });
        };
        let replaced = tip.key.clone();
        self.commit(c.on(&parent).replacing(&replaced))
    }

    /// The entry a redo would put back, without doing it.
    pub fn redo_target(&self) -> Result<&Entry, HistoryError> {
        let tip = self.tip_or_empty()?;
        if let Some(replaced) = &tip.replaced {
            return self.get(replaced);
        }
        // The tip is not itself an undo. Any undo still standing whose target
        // is not on the tip's ancestry is a redo the history has branched away
        // from, and taking it would silently drop that branch.
        let ancestry: BTreeSet<&str> = self
            .chain(&tip.key)?
            .iter()
            .map(|e| e.key.as_str())
            .collect();
        match self
            .entries
            .iter()
            .filter_map(|e| e.replaced.as_deref())
            .find(|target| !ancestry.contains(target))
        {
            Some(would_restore) => Err(HistoryError::RedoDiverged {
                tip: tip.key.clone(),
                would_restore: would_restore.to_string(),
            }),
            None => Err(HistoryError::NothingToRedo {
                tip: tip.key.clone(),
            }),
        }
    }

    /// Redo: append an entry made against the current tip, restoring what the
    /// tip's own undo restored. Nothing is recomputed.
    pub fn redo(&mut self, c: Commit<'_>) -> Result<&Entry, HistoryError> {
        self.redo_target()?;
        let parent = self.tip_or_empty()?.key.clone();
        self.commit(c.on(&parent))
    }

    /// Branch: append an entry made against an earlier entry. That earlier
    /// entry is not touched, which is the whole claim — a fork adds a second
    /// child and rewrites nothing.
    pub fn branch(&mut self, from: &str, c: Commit<'_>) -> Result<&Entry, HistoryError> {
        self.get(from)?;
        if self.tip().is_some_and(|t| t.key == from) {
            return Err(HistoryError::BranchOffTip {
                key: from.to_string(),
            });
        }
        self.commit(c.on(from))
    }

    /// A snapshot: an entry made against the tip and carrying a name. It undid
    /// nothing, so it is not a step in the undo chain — it is a row the history
    /// can be pointed at.
    pub fn snapshot(&mut self, c: Commit<'_>) -> Result<&Entry, HistoryError> {
        let parent = self.tip_or_empty()?.key.clone();
        self.commit(c.on(&parent))
    }
}

/// The unsaved window, as a value. No thread, no timer, no task: the caller
/// owns the clock and asks. A background thread here would be a second
/// definition of "now" that a test could not control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Autosave {
    /// The bound the plan requires to be documented. Long enough to coalesce a
    /// burst of keystrokes, short enough that a crash costs under two seconds
    /// of typing.
    pub interval_ms: u64,
    pending_since_ms: Option<u64>,
}

impl Default for Autosave {
    fn default() -> Self {
        Self {
            interval_ms: Self::DEFAULT_INTERVAL_MS,
            pending_since_ms: None,
        }
    }
}

impl Autosave {
    pub const DEFAULT_INTERVAL_MS: u64 = 2_000;

    /// An edit happened at `now_ms`. The window restarts, so the bound is
    /// measured from the *last* edit rather than the first.
    pub fn touched(&mut self, now_ms: u64) {
        self.pending_since_ms = Some(now_ms);
    }

    /// Whether the caller should persist. A measured zero is a real instant,
    /// so this is `Some(0)` and not "no time yet".
    #[must_use]
    pub fn due(&self, now_ms: u64) -> bool {
        self.pending_since_ms
            .is_some_and(|since| now_ms.saturating_sub(since) >= self.interval_ms)
    }

    /// The edit is durable; the window closes. Called after the store has
    /// committed, never before.
    pub fn acknowledge(&mut self) {
        self.pending_since_ms = None;
    }

    /// How long unacknowledged work has been waiting. `0` when there is none.
    #[must_use]
    pub fn unsaved_window_ms(&self, now_ms: u64) -> u64 {
        self.pending_since_ms
            .map_or(0, |since| now_ms.saturating_sub(since))
    }
}

mod error;

pub use error::{HistoryError, MAX_LABEL_BYTES};

#[cfg(test)]
mod tests;
