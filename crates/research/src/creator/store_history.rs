//! Persistence for the edit history: five verbs, and the one insert they share.
//!
//! Undo, redo, branch and snapshot are **not four stores**. Each is an argument
//! set over [`Store::commit_entry`], and each plans itself through
//! [`strivo_editor::history::History`] first, so the walk — what a parent is,
//! when a redo has diverged, where a cycle is — is implemented exactly once, in
//! the pure algebra, and this file never re-derives it in SQL.
//!
//! Two properties the read side cannot get on its own, and which therefore live
//! in the writes:
//!
//! * **A branch appends.** Nothing here issues an `UPDATE` or a `DELETE` against
//!   `creator_history`. Every verb is an `INSERT` through one function, so the
//!   entry a branch forked from is still there, byte for byte, with the child it
//!   had before the branch. Branch divergence is structural here, not a rule
//!   someone has to remember.
//! * **A commit is one transaction.** The membership checks and the insert
//!   commit together or not at all, so a kill mid-commit leaves the last
//!   complete entry as the tip and never a half-written one.

use rusqlite::{params, OptionalExtension, Transaction};
use strivo_editor::history::{Commit, History};

use super::history::{Edit, HistoryEntry, HistoryStoreError, TranscriptEdit, TranscriptPatch};
use super::ids::{HistoryId, ProjectId};
use super::store::Store;

const COLUMNS: &str = "history_id,project_id,parent_id,replaced_id,composition_revision_id,\
     transcript_revision_id,transcript_patch_json,label,created_at";

mod draft;
mod rows;

use rows::{json_error, row, to_algebra};

impl Store {
    /// Every entry of a project's history, oldest first. The order is the commit
    /// order the algebra walks, and the last row is the tip.
    pub fn history(&self, project: ProjectId) -> Result<Vec<HistoryEntry>, HistoryStoreError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM creator_history WHERE project_id=?1 \
             ORDER BY created_at, history_id"
        ))?;
        let rows = stmt.query_map([project.to_string()], row)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// The project's history as the pure algebra. Every verb below goes through
    /// here, so this conversion is the only place the two shapes meet.
    pub fn history_algebra(&self, project: ProjectId) -> Result<History, HistoryStoreError> {
        Ok(History::from_entries(
            self.history(project)?.iter().map(to_algebra).collect(),
        ))
    }

    /// The tip: the newest entry, which is the state the editor is on. `None`
    /// for a project nobody has edited, not a placeholder entry.
    pub fn history_tip(
        &self,
        project: ProjectId,
    ) -> Result<Option<HistoryEntry>, HistoryStoreError> {
        Ok(self.history(project)?.pop())
    }

    /// Commit one action against the current tip.
    ///
    /// `base` is the entry the edit was made against. Naming the tip is
    /// accepted; naming anything else means somebody else committed in between,
    /// and the save is refused as [`HistoryStoreError::Conflict`] rather than
    /// overwriting their work. `None` is "I did not read a history first", which
    /// is what a first save sends.
    pub fn record(
        &self,
        project: ProjectId,
        base: Option<HistoryId>,
        edit: Edit,
        label: &str,
        now: &str,
    ) -> Result<HistoryEntry, HistoryStoreError> {
        let entries = self.history(project)?;
        self.check_base(&entries, base.as_ref())?;
        let tip = entries.last().map(|e| e.id);
        self.commit_entry(project, tip, None, &edit, label, now)
    }

    /// Undo: commit an entry against the tip's parent, carrying the tip's own
    /// revisions, and recording the tip as what it put back. The state after an
    /// undo *is* the parent entry's state — the same immutable revision ids, not
    /// a recomputation.
    pub fn undo(
        &self,
        project: ProjectId,
        label: &str,
        now: &str,
    ) -> Result<HistoryEntry, HistoryStoreError> {
        let entries = self.history(project)?;
        let mut algebra = History::from_entries(entries.iter().map(to_algebra).collect());
        let id = HistoryId::generate();
        let planned = algebra.undo(Commit::new(&id.to_string(), label, now))?;
        let parent = planned
            .parent
            .as_deref()
            .and_then(|key| entries.iter().find(|e| e.id.to_string() == key))
            .ok_or_else(|| HistoryStoreError::UnknownEntry {
                key: planned.parent.clone().unwrap_or_default(),
            })?;
        // The algebra decides; the typed rows supply the identity. Reading the
        // replaced id back out of a `String` would re-parse an id this store
        // just wrote, and a parse failure there would be a bug dressed as data.
        let replaced = planned
            .replaced
            .as_deref()
            .and_then(|key| entries.iter().find(|e| e.id.to_string() == key))
            .map(|e| e.id);
        let restored = restored_edit(parent);
        self.commit_entry(project, Some(parent.id), replaced, &restored, label, now)
    }

    /// Redo: commit an entry against the current tip, carrying the revisions the
    /// tip's own undo put back. Refused once the history has branched, so a redo
    /// can never silently drop newer work.
    pub fn redo(
        &self,
        project: ProjectId,
        label: &str,
        now: &str,
    ) -> Result<HistoryEntry, HistoryStoreError> {
        let entries = self.history(project)?;
        let algebra = History::from_entries(entries.iter().map(to_algebra).collect());
        let target = algebra.redo_target()?;
        let tip = entries
            .last()
            .ok_or_else(|| HistoryStoreError::UnknownEntry {
                key: "(empty)".into(),
            })?;
        let source = entries
            .iter()
            .find(|e| e.id.to_string() == target.key)
            .ok_or_else(|| HistoryStoreError::UnknownEntry {
                key: target.key.clone(),
            })?;
        let restored = restored_edit(source);
        self.commit_entry(project, Some(tip.id), None, &restored, label, now)
    }

    /// Branch: commit an action against an earlier entry. **Nothing about that
    /// entry changes** — the branch is a second child, and the first child is
    /// still there.
    pub fn branch(
        &self,
        project: ProjectId,
        from: HistoryId,
        edit: Edit,
        label: &str,
        now: &str,
    ) -> Result<HistoryEntry, HistoryStoreError> {
        let entries = self.history(project)?;
        let mut algebra = History::from_entries(entries.iter().map(to_algebra).collect());
        let id = HistoryId::generate();
        algebra.branch(&from.to_string(), Commit::new(&id.to_string(), label, now))?;
        self.commit_entry(project, Some(from), None, &edit, label, now)
    }

    /// A snapshot: an entry against the tip, carrying a name, restoring nothing.
    /// Pointing the history at one later is the same commit every other restore
    /// is.
    pub fn snapshot(
        &self,
        project: ProjectId,
        label: &str,
        now: &str,
    ) -> Result<HistoryEntry, HistoryStoreError> {
        let entries = self.history(project)?;
        let mut algebra = History::from_entries(entries.iter().map(to_algebra).collect());
        let id = HistoryId::generate();
        algebra.snapshot(Commit::new(&id.to_string(), label, now))?;
        let tip = entries
            .last()
            .ok_or_else(|| HistoryStoreError::UnknownEntry {
                key: "(empty)".into(),
            })?;
        let restored = restored_edit(tip);
        self.commit_entry(project, Some(tip.id), None, &restored, label, now)
    }
}

/// An [`Edit`] that changes nothing but points at `source`'s revisions. This is
/// what an undo, a redo and a snapshot all commit: the same immutable revision
/// ids, reached by naming rather than by re-running anything.
pub(super) fn restored_edit(source: &HistoryEntry) -> Edit {
    Edit {
        composition: source.composition_revision_id,
        transcript: source.transcript.as_ref().map(|t| TranscriptPatch {
            revision: t.revision,
            draft: t.patch.clone(),
        }),
    }
}

impl Store {
    /// The one insert. The membership checks, the `INSERT` and the commit are
    /// one transaction, so a refusal leaves the history exactly as it was and a
    /// kill leaves the last complete entry as the tip.
    fn commit_entry(
        &self,
        project: ProjectId,
        parent: Option<HistoryId>,
        replaced: Option<HistoryId>,
        edit: &Edit,
        label: &str,
        now: &str,
    ) -> Result<HistoryEntry, HistoryStoreError> {
        let tx = self.conn.unchecked_transaction()?;
        self.check_membership(&tx, project, edit)?;
        let entry = HistoryEntry {
            id: HistoryId::generate(),
            project_id: project,
            parent,
            replaced,
            composition_revision_id: edit.composition,
            transcript: edit.transcript.as_ref().map(|t| TranscriptEdit {
                revision: t.revision,
                patch: t.draft.clone(),
            }),
            label: label.to_string(),
            created_at: now.to_string(),
        };
        let (transcript_id, patch) = match &entry.transcript {
            Some(t) => (
                Some(t.revision.to_string()),
                serde_json::to_string(&t.patch).map_err(json_error)?,
            ),
            None => (None, String::from("null")),
        };
        tx.execute(
            "INSERT INTO creator_history (history_id,project_id,parent_id,replaced_id,\
             composition_revision_id,transcript_revision_id,transcript_patch_json,label,created_at)\
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                entry.id.to_string(),
                entry.project_id.to_string(),
                entry.parent.map(|p| p.to_string()),
                entry.replaced.map(|r| r.to_string()),
                entry.composition_revision_id.to_string(),
                transcript_id,
                patch,
                entry.label,
                entry.created_at,
            ],
        )?;
        tx.commit()?;
        Ok(entry)
    }

    /// An action may only name revisions this project holds. A reference that
    /// resolves to another project's row is a data problem, and retargeting it
    /// would move an edit between projects.
    fn check_membership(
        &self,
        tx: &Transaction<'_>,
        project: ProjectId,
        edit: &Edit,
    ) -> Result<(), HistoryStoreError> {
        let composition: Option<String> = tx
            .query_row(
                "SELECT project_id FROM creator_composition_revisions WHERE id=?1",
                [edit.composition.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        if composition.as_deref() != Some(project.to_string().as_str()) {
            return Err(HistoryStoreError::ForeignComposition {
                revision: edit.composition.to_string(),
            });
        }
        let Some(patch) = &edit.transcript else {
            return Ok(());
        };
        let transcript: Option<String> = tx
            .query_row(
                "SELECT project_id FROM creator_transcript_revisions WHERE id=?1",
                [patch.revision.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        if transcript.as_deref() != Some(project.to_string().as_str()) {
            return Err(HistoryStoreError::ForeignTranscript {
                revision: patch.revision.to_string(),
            });
        }
        Ok(())
    }

    fn check_base(
        &self,
        entries: &[HistoryEntry],
        base: Option<&HistoryId>,
    ) -> Result<(), HistoryStoreError> {
        let Some(base) = base else { return Ok(()) };
        let current = entries
            .last()
            .map_or_else(|| "(empty)".to_string(), |e| e.id.to_string());
        if current == base.to_string() {
            return Ok(());
        }
        Err(HistoryStoreError::Conflict {
            base: base.to_string(),
            current,
        })
    }
}
