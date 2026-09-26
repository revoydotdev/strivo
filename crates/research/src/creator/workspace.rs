//! The project workspace: one project, many recordings and many edits, and
//! **one** selected revision that every view reads.
//!
//! This is a *container*, not a model of its own. The recordings are
//! [`SourceAsset`](super::entities::SourceAsset) rows, the edits are
//! [`CompositionRevision`](super::entities::CompositionRevision) rows, and the
//! delivery chain behind them is todo 24's lifecycle. What this module adds is
//! the one thing none of them had: a single answer to "what is open right now",
//! stored once so that the player, the transcript, the timeline, the inspector
//! and the task status cannot disagree about it.
//!
//! **Why one row and not one per view.** `docs/CREATOR-EDITION-REPORT-2026-09-07.md:114-116`
//! records Studio, Analytics and Publish as *organisational shells* — panes
//! that name a capability rather than showing its work. A shell is what you get
//! when each view keeps its own idea of what is open: the panes then have to be
//! reconciled, and the reconciliation is the product. So the selection lives in
//! exactly one row keyed by the project, and [`crate::creator::Store::resolve`]
//! is the single function every view calls to read it. A second cache in a view
//! is not a performance choice, it is a second answer.
//!
//! **A selected revision is a reference, never a copy.** The workspace holds
//! [`CompositionRevisionId`] and a content fingerprint taken at selection time;
//! the revision's own bytes stay in `creator_composition_revisions` where
//! todo 28's single revision store keeps them. Storing a second copy here is
//! how a workspace becomes a second revision store, which is the defect this
//! plan has now removed twice.
//!
//! **Staleness is detected, never assumed away.** A composition revision is
//! immutable by construction, so the only way one changes is a statement
//! outside this crate — a hand-edited or restored database. The fingerprint
//! turns that into a reported condition ([`WorkspaceError::Stale`]) rather than
//! a view rendering a selection that is not what it says it is.

use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use strivo_editor::time::{Interval, MediaTime, TimeError};

use super::entities::CompositionRevision;
use super::ids::{CompositionRevisionId, ProjectId};
use crate::ResearchError;

/// The five views that coexist over one project. Not an extension point: a
/// sixth view is a new variant here and a new column nowhere, because a view
/// has no state of its own beyond where it is looking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum View {
    Player,
    Transcript,
    Timeline,
    Inspector,
    Tasks,
}

impl View {
    /// The stored spelling. Also the wire spelling, so a client and the column
    /// cannot disagree about what "transcript" is called.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Player => "player",
            Self::Transcript => "transcript",
            Self::Timeline => "timeline",
            Self::Inspector => "inspector",
            Self::Tasks => "tasks",
        }
    }

    /// Every view, in workspace order. The tab strip's order is the same list,
    /// so the UI cannot invent a sequence the model does not have.
    #[must_use]
    pub const fn all() -> [Self; 5] {
        [
            Self::Player,
            Self::Transcript,
            Self::Timeline,
            Self::Inspector,
            Self::Tasks,
        ]
    }
}

impl fmt::Display for View {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for View {
    type Err = WorkspaceError;

    /// An unknown name is an error, never a default. A row read as `Player`
    /// because its own value was unrecognised is a view that silently opened
    /// somewhere the operator did not choose.
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Self::all()
            .into_iter()
            .find(|v| v.as_str() == s)
            .ok_or_else(|| WorkspaceError::UnknownView {
                found: s.to_string(),
            })
    }
}

/// What the project currently has selected.
///
/// `fingerprint` is present exactly when `composition` is — the schema's `CHECK`
/// enforces it, and [`revision_fingerprint`] is its only producer. It exists so
/// [`WorkspaceError::Stale`] can be reported; nothing reads it to decide what
/// the revision is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selection {
    pub composition: Option<CompositionRevisionId>,
    pub fingerprint: Option<String>,
}

impl Selection {
    /// Nothing selected yet. A project that has never been opened onto an edit
    /// is a real state, and it is `None` rather than a sentinel revision.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            composition: None,
            fingerprint: None,
        }
    }
}

/// The workspace row, as stored. One per project, ever.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    pub project_id: ProjectId,
    pub selection: Selection,
    pub playhead: Option<MediaTime>,
    /// The selected span. Half a range is a selection whose other end is
    /// unknown, so the type cannot hold one.
    pub range: Option<Interval>,
    pub active_view: View,
    /// Unsaved work, as an opaque JSON document. Its shape belongs to whoever
    /// edits it — todo 42 for text, todo 43 for the timeline — and the
    /// workspace's only obligation is to require an object so "unsaved" is
    /// never a bare string.
    pub draft: String,
    pub dirty: bool,
    pub updated_at: String,
}

impl Workspace {
    /// The selected revision, or a typed refusal. Never a default: a view
    /// handed a revision nobody chose would show work as if it were the
    /// project's.
    pub fn require_selection(&self) -> std::result::Result<&CompositionRevisionId, WorkspaceError> {
        self.selection
            .composition
            .as_ref()
            .ok_or(WorkspaceError::NoSelection {
                project: self.project_id,
            })
    }
}

/// The mutable part of a workspace, read out and written back as one value.
///
/// Deliberately a value with builders rather than four setters: the playhead,
/// the range, the active view and the draft all move together in one
/// `UPDATE`, and four setters would be four chances to write three of them.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewState {
    pub playhead: Option<MediaTime>,
    pub range: Option<Interval>,
    pub active_view: View,
    pub draft: String,
    pub dirty: bool,
}

impl ViewState {
    #[must_use]
    pub const fn with_playhead(mut self, at: MediaTime) -> Self {
        self.playhead = Some(at);
        self
    }

    #[must_use]
    pub const fn with_range(mut self, range: Interval) -> Self {
        self.range = Some(range);
        self
    }

    #[must_use]
    pub const fn with_active_view(mut self, view: View) -> Self {
        self.active_view = view;
        self
    }

    /// Attach unsaved work. `dirty` is a *consequence* of there being a draft
    /// rather than a separate claim, so a caller cannot record work and forget
    /// to say it is unsaved.
    #[must_use]
    pub fn with_draft(mut self, draft: &str, dirty: bool) -> Self {
        self.draft = draft.to_string();
        self.dirty = dirty;
        self
    }
}

/// A workspace together with the revision it selects, validated. What every
/// view actually renders, and the reason a view cannot show a selection the
/// project does not hold.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    pub workspace: Workspace,
    pub composition: CompositionRevision,
}

impl std::ops::Deref for Resolved {
    type Target = Workspace;

    fn deref(&self) -> &Self::Target {
        &self.workspace
    }
}

/// Everything the workspace refuses, each with the fact that caused it. A bare
/// string would leave a caller unable to tell "you asked for the wrong thing"
/// from "the database says something impossible".
#[derive(Debug)]
pub enum WorkspaceError {
    ProjectNotFound {
        project: ProjectId,
    },
    NoWorkspace {
        project: ProjectId,
    },
    RevisionNotInProject {
        project: ProjectId,
        revision: CompositionRevisionId,
    },
    NoSelection {
        project: ProjectId,
    },
    /// The selected revision's row is gone. Reported, never cleared — see the
    /// migration's note on why the column is not a foreign key.
    Dangling {
        selected: CompositionRevisionId,
    },
    Stale {
        selected: CompositionRevisionId,
        recorded: String,
        found: String,
    },
    MalformedDraft {
        detail: String,
    },
    UnknownView {
        found: String,
    },
    Research(ResearchError),
    Time(TimeError),
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProjectNotFound { project } => {
                write!(f, "no creator project {project}")
            }
            Self::NoWorkspace { project } => {
                write!(f, "project {project} has no workspace yet")
            }
            Self::RevisionNotInProject { project, revision } => write!(
                f,
                "revision {revision} is not an edit of project {project}"
            ),
            Self::NoSelection { project } => {
                write!(f, "project {project} has no selected revision")
            }
            Self::Dangling { selected } => write!(
                f,
                "the selected revision {selected} is no longer in the project"
            ),
            Self::Stale {
                selected,
                recorded,
                found,
            } => write!(
                f,
                "revision {selected} changed after it was selected ({recorded} recorded, {found} on disk)"
            ),
            Self::MalformedDraft { detail } => {
                write!(f, "unsaved work is not a JSON object: {detail}")
            }
            Self::UnknownView { found } => write!(f, "not a workspace view: {found}"),
            Self::Research(e) => write!(f, "{e}"),
            Self::Time(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for WorkspaceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Research(e) => Some(e),
            Self::Time(e) => Some(e),
            _ => None,
        }
    }
}

impl From<ResearchError> for WorkspaceError {
    fn from(e: ResearchError) -> Self {
        Self::Research(e)
    }
}

impl From<rusqlite::Error> for WorkspaceError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Research(e.into())
    }
}

impl From<TimeError> for WorkspaceError {
    fn from(e: TimeError) -> Self {
        Self::Time(e)
    }
}

/// A workspace-scoped result, so a caller does not have to know that some of
/// these refusals are about time and some are about rows.
pub type WorkspaceResult<T> = std::result::Result<T, WorkspaceError>;

/// The content hash a selection records: everything about a revision that a
/// change would alter, and nothing about when it was written.
///
/// `created_at` and `author` are excluded on purpose. Re-recording the same
/// edit must not make a selection stale, and an operator re-saving an unchanged
/// revision is not a change to it. This is the same rule as
/// [`super::render_plan::content_hash`] and for the same reason — never a
/// timestamp, and never a file's size or mtime.
#[must_use]
pub fn revision_fingerprint(revision: &CompositionRevision) -> String {
    let mut digest = Sha256::new();
    for part in [
        revision.revision.to_string(),
        revision
            .parent_revision_id
            .map_or(String::new(), |p| p.to_string()),
        revision.timeline_json.clone(),
        revision.output_profile.clone(),
    ] {
        digest.update(part.as_bytes());
        // A separator, so ("ab","c") and ("a","bc") cannot share a digest.
        digest.update(b"\x1f");
    }
    digest
        .finalize()
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            hex.push_str(&format!("{byte:02x}"));
            hex
        })
}
