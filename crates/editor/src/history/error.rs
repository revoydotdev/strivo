//! What the edit history refuses, and why each refusal is a distinct value.
//!
//! A caller has to branch on these: "there is nothing to redo" and "redoing
//! would discard newer work" are different answers, and only one of them is a
//! normal state. They are variants rather than messages for the same reason.

use std::fmt;

/// Longest accepted label, in bytes. A label arrives from a user interface and
/// is rendered back out of the history list, so an unbounded one is a denial of
/// service wearing a caption.
pub const MAX_LABEL_BYTES: usize = 65_536;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryError {
    UnknownEntry {
        key: String,
    },
    NothingToUndo {
        tip: String,
    },
    NothingToRedo {
        tip: String,
    },
    /// The history forked since the undo, so redoing would drop the fork.
    RedoDiverged {
        tip: String,
        would_restore: String,
    },
    /// The parent chain does not terminate. Unbounded by construction, so
    /// refusing is the only thing that keeps a walk finite.
    Cycle {
        key: String,
    },
    /// A branch off the tip is a plain commit; two spellings of one thing is
    /// how "did that branch?" becomes unanswerable.
    BranchOffTip {
        key: String,
    },
    /// Two entries may not share a key, or "the entry named X" is a guess.
    DuplicateKey {
        key: String,
    },
    LabelInvalid {
        detail: &'static str,
    },
}

impl HistoryError {
    /// The stable token a route reports.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::UnknownEntry { .. } | Self::DuplicateKey { .. } => "history/unknown-entry",
            Self::NothingToUndo { .. } | Self::NothingToRedo { .. } => "history/empty",
            Self::RedoDiverged { .. } => "history/diverged",
            Self::Cycle { .. } => "history/malformed",
            Self::BranchOffTip { .. } => "history/branch-off-tip",
            Self::LabelInvalid { .. } => "history/label-invalid",
        }
    }
}

impl fmt::Display for HistoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownEntry { key } => write!(f, "no history entry {key}"),
            Self::NothingToUndo { tip } => {
                write!(
                    f,
                    "{tip} is the first entry; there is nothing before it to undo to"
                )
            }
            Self::NothingToRedo { tip } => {
                write!(f, "{tip} was not an undo, so there is nothing to redo")
            }
            Self::RedoDiverged { tip, would_restore } => write!(
                f,
                "redoing would restore {would_restore}, but the history forked since \
                 ({tip} is now current)"
            ),
            Self::Cycle { key } => write!(f, "the history from {key} never reaches the beginning"),
            Self::BranchOffTip { key } => {
                write!(f, "{key} is already the tip; commit instead of branching")
            }
            Self::DuplicateKey { key } => write!(f, "{key} is already a history entry"),
            Self::LabelInvalid { detail } => write!(f, "not a usable label: {detail}"),
        }
    }
}

impl std::error::Error for HistoryError {}

pub(super) fn check_label(raw: &str) -> Result<(), HistoryError> {
    let bad = |detail: &'static str| HistoryError::LabelInvalid { detail };
    if raw.trim().is_empty() {
        return Err(bad("empty or whitespace only"));
    }
    if raw.len() > MAX_LABEL_BYTES {
        return Err(bad("longer than 65536 bytes"));
    }
    if raw.chars().any(char::is_control) {
        return Err(bad("contains a control character"));
    }
    Ok(())
}
