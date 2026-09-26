//! Edits, the undo that restores both halves, and the unsaved document.
//!
//! The two things todo 41 left open for this todo are **here**: the draft
//! shape ([`UnsavedWork`]) and, by way of the draft's `base_revision`, the
//! transcript-revision selection. Both are stated as types rather than as
//! prose, because a downstream todo reads them.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use super::{Transcript, WordId};
use crate::time::IntervalMap;

/// One corrected word: an id, and the text a human gave it.
///
/// Keyed by [`WordId`] rather than by a position, because a position is a
/// place in a document the edit is in the middle of changing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Correction {
    pub word: WordId,
    pub text: String,
}

/// One deleted word range, in document order and inclusive of both ends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WordRange {
    pub from_word: WordId,
    pub to_word: WordId,
}

/// The `transcript` member of the workspace's `unsaved_json` document.
///
/// **A patch against a named immutable base, never a copy of the document.** A
/// copy would be a second source of truth that can disagree with the revision
/// it claims to edit, and it would grow with the transcript. `base_revision` is
/// therefore mandatory: unsaved work with no base to apply it to is not unsaved
/// work, it is an orphan, and the store will report it as superseded.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TranscriptDraft {
    pub base_revision: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub corrected: Vec<Correction>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deleted: Vec<WordRange>,
}

/// The workspace's `unsaved_json` document.
///
/// One JSON **object** with one member per editing surface, so the workspace's
/// "must be an object" check is enough to keep the shape honest, and a surface
/// with nothing unsaved is *absent* rather than an empty value. The
/// `transcript` member is this todo's; a timeline member is the timeline todo's
/// to add, and neither has to know about the other.
///
/// ```json
/// {"transcript":{"base_revision":"<uuid>",
///                "corrected":[{"word":"w3","text":"roughly"}],
///                "deleted":[{"from_word":"w2","to_word":"w3"}]}}
/// ```
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UnsavedWork {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript: Option<TranscriptDraft>,
}

/// A transcript and a composition after an edit, with enough of the before to
/// put it back.
#[derive(Debug, Clone, PartialEq)]
pub struct Edited {
    transcript: Transcript,
    map: IntervalMap,
    cuts: Vec<super::link::SourceCut>,
    before: Unedited,
}

impl Edited {
    pub(crate) fn new(
        transcript: Transcript,
        map: IntervalMap,
        cuts: Vec<super::link::SourceCut>,
        before_transcript: super::Transcript,
        before_map: IntervalMap,
    ) -> Self {
        Self {
            transcript,
            map,
            cuts,
            before: Unedited {
                transcript: before_transcript,
                map: before_map,
            },
        }
    }

    #[must_use]
    pub fn transcript(&self) -> &Transcript {
        &self.transcript
    }

    #[must_use]
    pub fn map(&self) -> &IntervalMap {
        &self.map
    }

    /// What the edit removed from the composition, if it removed anything. A
    /// correction has no cut, and saying so differs from reporting a
    /// zero-length one.
    #[must_use]
    pub fn cut(&self) -> Option<&super::link::SourceCut> {
        self.cuts.first()
    }

    /// Every removed range, for a draft that struck more than one. One entry
    /// per range rather than a merged one, because a render input wants the
    /// ranges and merging two deletions that happen to be adjacent would lose
    /// the fact that they were two.
    #[must_use]
    pub fn cuts(&self) -> &[super::link::SourceCut] {
        &self.cuts
    }

    /// **Undo, restoring both halves.** Text and composition move together
    /// because they were changed together; returning one without the other is
    /// the misalignment the report calls out, and carrying the before is
    /// cheaper here than reconstructing it. A store-backed undo — real
    /// revisions, a history, conflict detection — belongs to the todo that owns
    /// a base-revision column, and this is the value it will hand out.
    #[must_use]
    pub fn undo(self) -> Unedited {
        self.before
    }
}

/// The pair an edit started from.
#[derive(Debug, Clone, PartialEq)]
pub struct Unedited {
    pub transcript: Transcript,
    pub map: IntervalMap,
}

impl super::Transcript {
    /// The text with a set of word ranges struck out.
    ///
    /// Segments a provider produced are otherwise left alone — including one
    /// that becomes empty, which is a real state rather than something to tidy
    /// away. Removing the segment would also renumber the transcript, and
    /// renumbering is what a stable [`WordId`] exists to prevent.
    pub(crate) fn without(&self, ranges: &[WordRange]) -> Transcript {
        let strike: HashSet<WordId> = ranges
            .iter()
            .flat_map(|r| self.ids_between(&r.from_word, &r.to_word))
            .cloned()
            .collect();
        let mut segments = self.segments.clone();
        for segment in &mut segments {
            segment.words.retain(|w| !strike.contains(&w.id));
        }
        Transcript { segments }
    }

    /// The ids a forward range covers. An endpoint that is not in the document
    /// yields nothing, because validation happens before this is called and a
    /// silently-empty strike would be a deletion that deleted nothing.
    fn ids_between<'a>(&'a self, from: &WordId, to: &WordId) -> Vec<&'a WordId> {
        let all: Vec<&WordId> = self.words().map(|w| &w.id).collect();
        let (Some(a), Some(b)) = (
            all.iter().position(|i| *i == from),
            all.iter().position(|i| *i == to),
        ) else {
            return Vec::new();
        };
        let (lo, hi) = (a.min(b), a.max(b));
        all.into_iter().skip(lo).take(hi - lo + 1).collect()
    }
}
