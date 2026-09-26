//! The unsaved window: one draft, written by one statement.
//!
//! Separate from the chain because it is a different lifetime. A history entry
//! is an acknowledged action and never disappears; a draft is work in progress
//! that a commit replaces, and the two must not be able to disagree about
//! whether something is unsaved.
//!
//! The draft shape is todo 42's own
//! [`strivo_editor::transcript::UnsavedWork`] and the column is todo 41's own
//! `unsaved_json`, so this file introduces neither a second draft store nor a
//! second shape. What it adds is the refusal: a draft naming a base the
//! workspace is not looking at is reported, reusing todo 42's
//! `RevisionSuperseded` answer rather than inventing a second staleness error.

use rusqlite::params;
use strivo_editor::transcript::UnsavedWork;

use super::super::history::HistoryStoreError;
use super::super::ids::{CompositionRevisionId, ProjectId};
use super::super::store::Store;
use super::rows::json_error;

impl Store {
    /// Persist one unsaved draft, or refuse it.
    ///
    /// The draft is todo 42's own [`strivo_editor::transcript::UnsavedWork`]
    /// written into todo 41's own `unsaved_json` — no second draft store, and
    /// no second shape. It is written by **one** statement, which is the same
    /// invariant version 4 established for this column: `unsaved_json` and
    /// `dirty` move together or not at all, so a draft can never be recorded
    /// without the flag that says it is unsaved.
    ///
    /// Crash-safety is that statement's atomicity, not a journal of its own. A
    /// kill during it leaves the previous complete draft; a kill cannot leave
    /// half of one, because SQLite will not have committed a partial page.
    ///
    /// The draft is refused when it names a base that is not the workspace's
    /// selected revision — [`HistoryStoreError::DraftSuperseded`], which is
    /// todo 42's `RevisionSuperseded` answer rather than a second staleness
    /// error wearing another name. Storing a patch is not applying it, so the
    /// *transcript* half's base is checked by todo 42's own `apply` at the
    /// moment the draft is used.
    ///
    /// The caller owns the clock and the bound — see
    /// [`strivo_editor::history::Autosave`]. Nothing here blocks a request path
    /// on a timer; a caller that autosaves on a request is autosaving
    /// synchronously, which is its own choice to make.
    pub fn save_draft(
        &self,
        project: ProjectId,
        base: CompositionRevisionId,
        work: &UnsavedWork,
        now: &str,
    ) -> Result<(), HistoryStoreError> {
        let body = serde_json::to_string(work).map_err(json_error)?;
        let changed = self.conn.execute(
            "UPDATE creator_workspace SET unsaved_json=?2, dirty=1, updated_at=?3
             WHERE project_id=?1 AND selected_composition_revision_id=?4",
            params![project.to_string(), body, now, base.to_string(),],
        )?;
        if changed == 0 {
            return Err(HistoryStoreError::DraftSuperseded {
                base: base.to_string(),
                current: self
                    .workspace(project)
                    .ok()
                    .and_then(|w| w.selection.composition.map(|c| c.to_string()))
                    .unwrap_or_else(|| "(nothing selected)".to_string()),
            });
        }
        Ok(())
    }

    /// Close the unsaved window: the draft is now a history entry, so the
    /// workspace carries no unsaved work and the flag goes with it. One
    /// statement again, so a workspace cannot read "clean" with a draft still
    /// in it.
    pub fn acknowledge_draft(
        &self,
        project: ProjectId,
        now: &str,
    ) -> Result<(), HistoryStoreError> {
        self.conn.execute(
            "UPDATE creator_workspace SET unsaved_json='{}', dirty=0, updated_at=?2
             WHERE project_id=?1",
            params![project.to_string(), now],
        )?;
        Ok(())
    }
}
