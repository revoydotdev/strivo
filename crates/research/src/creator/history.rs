//! The edit history: one chain of actions, and undo, redo, branch and snapshot
//! as that one chain read four ways.
//!
//! ## The whole model
//!
//! A [`HistoryEntry`] is a row: an action a person took, the entry it was made
//! against, the entry it put back if it was an undo, and the immutable
//! revisions that action produced or restored. The chain itself is
//! [`strivo_editor::history::History`], which is where the algebra lives and
//! where a walk is bounded against a cycle. Undo, redo, branch and snapshot are
//! four argument sets over one insert — see [`super::store_history`].
//!
//! ## What undo restores
//!
//! **A revision, not a re-executed inverse edit.** An undo names its parent
//! entry's `composition_revision_id` again. Nothing is computed backwards:
//! there is no "un-trim" that could disagree with what a trim did, and so no
//! second path to a composition that todo 23's compiler is no longer the only
//! way to reach. The undo is itself a row, so it is undoable.
//!
//! ## One history, transcript and timeline alike
//!
//! An entry is an *action*, not a surface. A transcript correction commits an
//! entry carrying a [`TranscriptEdit`]; a timeline trim commits one carrying
//! only a composition revision. They are the same list, in the order they
//! happened, and one undo moves across both — which is the whole of "one undo
//! history across transcript, timeline and inspector". There is no second stack
//! to reconcile because there is only one.
//!
//! The transcript half is a **patch against a named base**, in todo 42's own
//! [`TranscriptDraft`] vocabulary. A copy of the document would be a second
//! source of truth that can disagree with the revision it claims to edit.
//!
//! ## What an entry deliberately does not store
//!
//! Composition bytes. They stay in `creator_composition_revisions` where todo
//! 28's single revision store keeps them, and an entry *references* them. A
//! history row carrying a copy would be a second revision store wearing a
//! history's name.

use serde::{Deserialize, Serialize};
use strivo_editor::history::HistoryError;
use strivo_editor::transcript::TranscriptDraft;

use super::ids::{CompositionRevisionId, HistoryId, ProjectId, TranscriptRevisionId};

/// One committed action in a project's edit history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: HistoryId,
    pub project_id: ProjectId,
    /// The entry this was made against. `None` only at the root of a history.
    pub parent: Option<HistoryId>,
    /// The entry this one puts back, when it is an undo or a redo. `None` for
    /// an ordinary commit, and that `None` is what makes "nothing to redo" a
    /// state rather than a guess.
    pub replaced: Option<HistoryId>,
    /// The immutable revision this action produced or restored. **Never**
    /// `Option`: an entry naming no composition is not an action.
    pub composition_revision_id: CompositionRevisionId,
    /// The transcript half, when this action touched the text.
    pub transcript: Option<TranscriptEdit>,
    pub label: String,
    pub created_at: String,
}

/// A transcript correction as this action recorded it: which immutable
/// revision the patch applies to, and the patch itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptEdit {
    pub revision: TranscriptRevisionId,
    pub patch: TranscriptDraft,
}

/// Everything a project refuses to do to its history, each carrying the fact
/// that caused it.
///
/// Not `Clone`: a [`Storage`](HistoryStoreError::Storage) refusal owns a
/// `rusqlite::Error`, and cloning a driver error to hand it to two callers
/// would be copying something the driver owns.
#[derive(Debug, PartialEq)]
pub enum HistoryStoreError {
    /// The save named a base that is no longer the tip, so somebody else
    /// committed in between. **Reported, never resolved last-write-wins** — the
    /// two clients hold different edits and only the caller can say which
    /// survives.
    Conflict {
        base: String,
        current: String,
    },
    UnknownEntry {
        key: String,
    },
    /// A draft named a composition revision the workspace is not selected on.
    ///
    /// The store-side spelling of todo 42's `TranscriptError::RevisionSuperseded`,
    /// carrying the same two facts: unsaved work built on a revision nobody has
    /// open is **reported**, never silently re-pointed at the newer one. The
    /// transcript half of a draft gets that same refusal from todo 42's own
    /// `apply`, on the draft's `base_revision`, when the draft is applied rather
    /// than when it is stored — storing a patch is not applying it.
    DraftSuperseded {
        base: String,
        current: String,
    },
    /// The composition revision an action names is not in this project.
    ForeignComposition {
        revision: String,
    },
    /// The transcript revision a patch applies to is not in this project.
    ForeignTranscript {
        revision: String,
    },
    History(HistoryError),
    Storage(rusqlite::Error),
}

impl HistoryStoreError {
    /// The stable token a route reports, so a client branches on the class
    /// without matching on prose.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Conflict { .. } => "history/conflict",
            Self::UnknownEntry { .. } => "history/unknown-entry",
            Self::DraftSuperseded { .. } => "transcript/revision-superseded",
            Self::ForeignComposition { .. } | Self::ForeignTranscript { .. } => {
                "history/foreign-revision"
            }
            Self::History(inner) => inner.code(),
            Self::Storage(_) => "history/storage",
        }
    }
}

impl std::fmt::Display for HistoryStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Conflict { base, current } => write!(
                f,
                "this edit was made against history entry {base}, but {current} is now current"
            ),
            Self::UnknownEntry { key } => write!(f, "no history entry {key}"),
            Self::DraftSuperseded { base, current } => write!(
                f,
                "this draft is against revision {base}, and the open one is {current}"
            ),
            Self::ForeignComposition { revision } => write!(
                f,
                "this action names composition revision {revision}, which this project does not hold"
            ),
            Self::ForeignTranscript { revision } => write!(
                f,
                "this action names transcript revision {revision}, which this project does not hold"
            ),
            Self::History(e) => write!(f, "{e}"),
            Self::Storage(e) => write!(f, "history store: {e}"),
        }
    }
}

impl std::error::Error for HistoryStoreError {}

impl From<rusqlite::Error> for HistoryStoreError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Storage(e)
    }
}

impl From<HistoryError> for HistoryStoreError {
    fn from(e: HistoryError) -> Self {
        Self::History(e)
    }
}

/// What one action changed. Both halves are optional and neither is defaulted:
/// a trim commits a composition revision, a correction commits a transcript
/// patch, and an action that did both names both.
#[derive(Debug, Clone, PartialEq)]
pub struct Edit {
    pub composition: CompositionRevisionId,
    pub transcript: Option<TranscriptPatch>,
}

/// A transcript correction as one action recorded it. Carries the revision the
/// patch applies to, because a patch against no base is an orphan.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptPatch {
    pub revision: TranscriptRevisionId,
    pub draft: TranscriptDraft,
}

impl Edit {
    /// An action that changed only the picture.
    #[must_use]
    pub const fn composition(revision: CompositionRevisionId) -> Self {
        Self {
            composition: revision,
            transcript: None,
        }
    }

    /// The same action with a transcript half added. A commit that moved both
    /// halves at once — which is what todo 42's linked delete is — names the
    /// composition state it was made against *and* the text patch, so one undo
    /// restores both. There is deliberately no text-only constructor: a
    /// correction changes no media, so it names the composition revision already
    /// open rather than inventing one.
    #[must_use]
    pub fn and_transcript(mut self, patch: TranscriptPatch) -> Self {
        self.transcript = Some(patch);
        self
    }
}
