//! Persistence for the project workspace — one row per project, read and
//! written by every view.
//!
//! The reads and the writes are both few and both total, which is the point: a
//! view's entire relationship to project state is [`Store::resolve`] and
//! [`Store::save_view_state`], and there is no third way in. Every read
//! re-validates the selection against the revisions table rather than trusting
//! the workspace's own claim, so a selection that has gone stale or dangling is
//! *reported* on the read that would have used it.

use rusqlite::{params, OptionalExtension};
use strivo_editor::time::{Interval, MediaTime};

use super::entities::CompositionRevision;
use super::ids::{CompositionRevisionId, ProjectId};
use super::store::Store;
use super::workspace::{
    revision_fingerprint, Resolved, Selection, ViewState, Workspace, WorkspaceError,
    WorkspaceResult,
};

const COLUMNS: &str = "project_id,selected_composition_revision_id,selected_fingerprint,\
     playhead_us,selection_start_us,selection_end_us,active_view,unsaved_json,dirty,updated_at";

impl Store {
    /// The workspace row for a project, or [`WorkspaceError::NoWorkspace`].
    /// A read never creates one: a `GET` that wrote would make a first page
    /// view indistinguishable from a project that has state.
    pub fn workspace(&self, project: ProjectId) -> WorkspaceResult<Workspace> {
        self.conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM creator_workspace WHERE project_id=?1"),
                [project.to_string()],
                row_to_workspace,
            )
            .optional()?
            .ok_or(WorkspaceError::NoWorkspace { project })
    }

    /// Open — creating if absent — the one workspace a project has, and refuse
    /// a project that does not exist rather than minting a workspace for an id
    /// nothing else can reach.
    pub fn open_workspace(&self, project: ProjectId, now: &str) -> WorkspaceResult<Workspace> {
        if self.project(project)?.is_none() {
            return Err(WorkspaceError::ProjectNotFound { project });
        }
        self.conn.execute(
            "INSERT INTO creator_workspace (project_id,unsaved_json,updated_at)
             VALUES(?1,'{}',?2) ON CONFLICT(project_id) DO NOTHING",
            params![project.to_string(), now],
        )?;
        self.workspace(project)
    }

    /// The workspace **and** the revision it selects, validated. The single
    /// read every view makes: the player, the transcript, the timeline, the
    /// inspector and the task status all come through here, which is why they
    /// cannot hold different ideas of what is open.
    ///
    /// Fails rather than falling back when the selection names a revision that
    /// is not there or no longer says what it said. A view handed the wrong
    /// revision is worse than a view told the truth.
    pub fn resolve(&self, project: ProjectId) -> WorkspaceResult<Resolved> {
        let workspace = self.workspace(project)?;
        let selected = *workspace.require_selection()?;
        let composition = self
            .composition_revision(selected)?
            .ok_or(WorkspaceError::Dangling { selected })?;
        let found = revision_fingerprint(&composition);
        match workspace.selection.fingerprint.as_deref() {
            Some(recorded) if recorded == found => {}
            recorded => {
                return Err(WorkspaceError::Stale {
                    selected,
                    recorded: recorded.unwrap_or("none").to_string(),
                    found,
                })
            }
        }
        Ok(Resolved {
            workspace,
            composition,
        })
    }

    /// Point the project's workspace at one of its own edits, recording the
    /// revision's content fingerprint so a later change is detectable.
    ///
    /// Membership is checked against `creator_composition_revisions`, not
    /// against the id: a well-formed [`CompositionRevisionId`] belonging to
    /// another project is the type confusion the typed ids cannot catch, and it
    /// is how one project's edit reaches another's surface.
    pub fn select_revision(
        &self,
        project: ProjectId,
        revision: CompositionRevisionId,
        now: &str,
    ) -> WorkspaceResult<()> {
        let row = self
            .composition_revision(revision)?
            .ok_or(WorkspaceError::Dangling { selected: revision })?;
        if row.project_id != project {
            return Err(WorkspaceError::RevisionNotInProject { project, revision });
        }
        self.workspace(project)?;
        self.conn.execute(
            "UPDATE creator_workspace
             SET selected_composition_revision_id=?2, selected_fingerprint=?3, updated_at=?4
             WHERE project_id=?1",
            params![
                project.to_string(),
                revision.to_string(),
                revision_fingerprint(&row),
                now
            ],
        )?;
        Ok(())
    }

    /// The mutable part of the workspace, for a caller about to change it.
    pub fn view_state(&self, project: ProjectId) -> WorkspaceResult<ViewState> {
        let ws = self.workspace(project)?;
        Ok(ViewState {
            playhead: ws.playhead,
            range: ws.range,
            active_view: ws.active_view,
            draft: ws.draft,
            dirty: ws.dirty,
        })
    }

    /// Write the playhead, the range, the active view and the draft in **one**
    /// statement, with one timestamp. One statement is the cancel-safety
    /// argument: there is no interleaving in which the draft moved and the
    /// unsaved flag did not, or a view switch landed without the work it was
    /// switching away from.
    pub fn save_view_state(
        &self,
        project: ProjectId,
        state: &ViewState,
        now: &str,
    ) -> WorkspaceResult<()> {
        check_draft(&state.draft)?;
        self.workspace(project)?;
        let (start, end) = range_columns(state.range);
        self.conn.execute(
            "UPDATE creator_workspace
             SET playhead_us=?2, selection_start_us=?3, selection_end_us=?4,
                 active_view=?5, unsaved_json=?6, dirty=?7, updated_at=?8
             WHERE project_id=?1",
            params![
                project.to_string(),
                state.playhead.map(MediaTime::micros),
                start,
                end,
                state.active_view.as_str(),
                state.draft,
                i64::from(state.dirty),
                now,
            ],
        )?;
        Ok(())
    }

    /// Throw unsaved work away. The flag goes in the same statement, so a
    /// discarded draft cannot leave a project permanently reporting work that
    /// no longer exists.
    pub fn discard_draft(&self, project: ProjectId) -> WorkspaceResult<()> {
        self.workspace(project)?;
        self.conn.execute(
            "UPDATE creator_workspace SET unsaved_json='{}', dirty=0 WHERE project_id=?1",
            [project.to_string()],
        )?;
        Ok(())
    }

    /// One composition revision by identity, or `None`.
    fn composition_revision(
        &self,
        id: CompositionRevisionId,
    ) -> WorkspaceResult<Option<CompositionRevision>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id,project_id,parent_revision_id,revision,author,timeline_json,\
                        output_profile,created_at
                 FROM creator_composition_revisions WHERE id=?1",
                [id.to_string()],
                row_to_revision,
            )
            .optional()?)
    }
}

/// Unsaved work must be a JSON **object**, checked before any write. The
/// workspace's shape obligations stop at "an object"; what is inside it belongs
/// to whoever edits it (todo 42 for text, todo 43 for the timeline).
fn check_draft(draft: &str) -> WorkspaceResult<()> {
    match serde_json::from_str::<serde_json::Value>(draft) {
        Ok(serde_json::Value::Object(_)) => Ok(()),
        Ok(_) => Err(WorkspaceError::MalformedDraft {
            detail: "expected a JSON object".to_string(),
        }),
        Err(e) => Err(WorkspaceError::MalformedDraft {
            detail: e.to_string(),
        }),
    }
}

fn range_columns(range: Option<Interval>) -> (Option<i64>, Option<i64>) {
    match range {
        Some(r) => (Some(r.start().micros()), Some(r.end().micros())),
        None => (None, None),
    }
}

fn row_to_revision(r: &rusqlite::Row<'_>) -> rusqlite::Result<CompositionRevision> {
    Ok(CompositionRevision {
        id: CompositionRevisionId::from_uuid(uuid_at(r, 0)?),
        project_id: ProjectId::from_uuid(uuid_at(r, 1)?),
        parent_revision_id: r
            .get::<_, Option<String>>(2)?
            .as_deref()
            .map(str::parse)
            .transpose()
            .map_err(|e| bad_column(2, e))?,
        revision: r.get::<_, i64>(3)? as u32,
        author: r.get(4)?,
        timeline_json: r.get(5)?,
        output_profile: r.get(6)?,
        created_at: r.get(7)?,
    })
}

/// The column order `COLUMNS` declares, by index. Named rather than inlined
/// because the reader below reads nine positional columns and a single
/// reordering in the `SELECT` would otherwise be a silent misread.
mod col {
    pub const PROJECT: usize = 0;
    pub const COMPOSITION: usize = 1;
    pub const FINGERPRINT: usize = 2;
    pub const PLAYHEAD: usize = 3;
    pub const RANGE_START: usize = 4;
    pub const RANGE_END: usize = 5;
    pub const VIEW: usize = 6;
    pub const DRAFT: usize = 7;
    pub const DIRTY: usize = 8;
    pub const UPDATED: usize = 9;
}

fn row_to_workspace(r: &rusqlite::Row<'_>) -> rusqlite::Result<Workspace> {
    use col::{
        COMPOSITION, DIRTY, DRAFT, FINGERPRINT, PLAYHEAD, PROJECT, RANGE_END, RANGE_START, UPDATED,
        VIEW,
    };
    // The schema's CHECK makes a half-range unrepresentable, so a lone endpoint
    // reaching here is a corrupt row: read as no range rather than invented.
    let range = match (media_time_at(r, RANGE_START)?, media_time_at(r, RANGE_END)?) {
        (Some(start), Some(end)) => {
            Some(Interval::new(start, end).map_err(|e| bad_column(RANGE_START, e.to_string()))?)
        }
        _ => None,
    };
    let view: String = r.get(VIEW)?;
    Ok(Workspace {
        project_id: ProjectId::from_uuid(uuid_at(r, PROJECT)?),
        selection: Selection {
            composition: r
                .get::<_, Option<String>>(COMPOSITION)?
                .as_deref()
                .map(str::parse)
                .transpose()
                .map_err(|e| bad_column(COMPOSITION, e))?,
            fingerprint: r.get(FINGERPRINT)?,
        },
        playhead: media_time_at(r, PLAYHEAD)?,
        range,
        active_view: view
            .parse()
            .map_err(|e: WorkspaceError| bad_column(VIEW, e.to_string()))?,
        draft: r.get(DRAFT)?,
        dirty: r.get::<_, i64>(DIRTY)? != 0,
        updated_at: r.get(UPDATED)?,
    })
}

fn uuid_at(r: &rusqlite::Row<'_>, i: usize) -> rusqlite::Result<uuid::Uuid> {
    uuid::Uuid::parse_str(&r.get::<_, String>(i)?).map_err(|e| bad_column(i, e.to_string()))
}

fn media_time_at(r: &rusqlite::Row<'_>, i: usize) -> rusqlite::Result<Option<MediaTime>> {
    r.get::<_, Option<i64>>(i)?
        .map(MediaTime::from_micros)
        .transpose()
        .map_err(|e| bad_column(i, e.to_string()))
}

fn bad_column(i: usize, e: impl ToString) -> rusqlite::Error {
    rusqlite::Error::InvalidColumnType(i, e.to_string(), rusqlite::types::Type::Text)
}
