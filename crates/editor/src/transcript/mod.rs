//! Transcript editing linked to media operations.
//!
//! The report's E02 and the plan's highest-leverage editor phase: a transcript
//! is not a caption track beside a video, it is **an index into the media**.
//! Editing a word and editing the picture are the same act, and they must be
//! separable on purpose — a text correction changes no media at all, and a
//! deletion changes the composition and no media at all.
//!
//! **What "linked" means here, exactly.** A transcript word carries a
//! [`WordId`] and, when its provider reported one, an exact source
//! [`Interval`]. [`Linked`] pairs a transcript with the composition's
//! [`IntervalMap`] and answers two questions exactly:
//!
//! * forward — the composition instant at which a given word is spoken, and
//! * reverse — the words spoken at a given composition instant.
//!
//! Both are integer microseconds ([`MediaTime`]) with **no tolerance**. A seek
//! that lands on the right word but one microsecond away is not "the same
//! instant", and a tolerance would hide precisely the drift this module exists
//! to prevent. Every refusal here is a value: a coordinate that cannot be
//! answered is an error, never an approximation and never a clamp to the
//! nearest edge.
//!
//! **What "linked" excludes, stated rather than left to be discovered.**
//!
//! * **No alignment algorithm.** A transcript whose provider reported no word
//!   timings is [`Timing::SegmentLevel`] or [`Timing::Untimed`], and a word's
//!   timing is never interpolated from its segment's. A word with no position
//!   has no seek target, and saying so is the answer.
//! * **No reordering.** [`IntervalMap`] is an append-only ordered sequence of
//!   source spans over one source, so a word *reorder* is not expressible
//!   here: placing a span at an output position that is not the end is
//!   `OutputDiscontinuity`. A reorder is a permutation of placed clips, which
//!   is what the compiled plan and the timeline own. A half-working reorder
//!   here would be the silent partial success this plan keeps refusing, so
//!   there is no API for one at all.
//! * **No media access.** This module reaches no `std::fs` API and spawns no
//!   process, so it cannot open a file, cannot overwrite one, and has no path
//!   by which it could route around `render`'s `OutputExists` refusal. The
//!   composition is the only input to a decode, so a correction that leaves it
//!   byte-identical leaves the decoded media bit-identical.
//! * **No second edit store, and no second selection store.** The text lives in
//!   todo 21's immutable transcript revisions and the picture in todo 28's
//!   single composition revision; this module is the *link* between them and
//!   holds neither.
//!
//! **Which transcript revision a project is editing — the first of the two
//! things todo 41 left open.** There is deliberately **no selection column**,
//! and the reason is todo 41's own: the workspace holds "one selected revision",
//! and a transcript is reached *through* the source its selected composition is
//! over. So the revision is a property of the [`Linked`] value, carried
//! explicitly in [`Linked::revision`], and a draft naming any other revision is
//! [`TranscriptError::RevisionSuperseded`] — reported, never silently
//! re-pointed at the newer revision. A re-transcription is an immutable *new*
//! revision (todo 21), so an edit built on the old one is then editing
//! something nobody is looking at; the honest response is to say so.
//!
//! **The unsaved document — the second thing todo 41 left open.** [`UnsavedWork`]
//! is the shape that goes into the workspace's `unsaved_json` column: one JSON
//! object, one member per editing surface. The `transcript` member is a *patch
//! against a named immutable base*, never a copy of the document — a copy would
//! be a second source of truth that can disagree with the revision it claims to
//! edit, and it would grow with the transcript. Corrections are keyed by
//! [`WordId`], so they survive a re-transcription's renumbering; deletions are
//! [`WordId`] ranges, which is this module's own vocabulary rather than a
//! timeline's, leaving the timeline member to the todo that owns it.

mod edit;
mod link;

pub use edit::{Correction, Edited, TranscriptDraft, Unedited, UnsavedWork, WordRange};
pub use link::{Linked, SourceCut};

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::time::{Interval, MediaTime, TimeError};

/// Longest accepted word id, in bytes. A word id arrives from a provider, so
/// it is a trust boundary; an unbounded one is a denial-of-service vector
/// wearing an identifier.
const MAX_WORD_ID_BYTES: usize = 256;

/// A word's identity: assigned once by the revision that produced it, and
/// carried forward unchanged by every later correction.
///
/// **Never derived from the word's text.** A transcript is model-produced text
/// that reaches a user interface, so anything that matched on the text would be
/// a match on attacker-influenced content: two words saying the same thing
/// would collapse into one, and a word containing markup would be a different
/// word from the same text without it. Identity is an id, and the id is
/// validated where it is minted — including through deserialisation, which is
/// the path a stored revision takes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct WordId(String);

impl WordId {
    /// Adopt a provider-assigned id, refusing one that could not be a stable
    /// key: empty, whitespace only, containing a control character, or so long
    /// that storing it in a document is a problem rather than an id.
    pub fn new(raw: &str) -> Result<Self, TranscriptError> {
        let bad = |detail: &str| TranscriptError::WordIdInvalid {
            detail: detail.to_string(),
        };
        if raw.is_empty() {
            return Err(bad("empty"));
        }
        if raw.trim().is_empty() {
            return Err(bad("whitespace only"));
        }
        if raw.len() > MAX_WORD_ID_BYTES {
            return Err(bad("longer than 256 bytes"));
        }
        if raw.chars().any(char::is_control) {
            return Err(bad("contains a control character"));
        }
        Ok(Self(raw.to_string()))
    }

    /// The id as the provider wrote it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for WordId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::str::FromStr for WordId {
    type Err = TranscriptError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

/// Deserialised through [`WordId::new`], so an invalid id cannot reach a
/// document by way of stored bytes.
impl<'de> Deserialize<'de> for WordId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        Self::new(&raw).map_err(serde::de::Error::custom)
    }
}

/// One word, its exact position when the provider gave one, and its text.
///
/// `at` is `None` rather than a default, because a measured zero is a real
/// measurement: a zero-length word is somebody clicking, and reading it as "no
/// timing" is the `||`-guards-a-legitimate-zero defect this plan has already
/// paid for once.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Word {
    pub id: WordId,
    /// The word's own position on the source timeline, or `None` when the
    /// provider reported none. **Never** derived from the segment.
    #[serde(default)]
    pub at: Option<Interval>,
    /// The word as transcribed, or as a human corrected it. **Data**: nothing
    /// in this module interprets it, and it is never used to identify a word.
    pub text: String,
    #[serde(default)]
    pub speaker: Option<String>,
}

/// One segment: a provider's own unit of transcript, and the position it
/// reported for that unit as a whole.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    /// The segment's own position, or `None`. A segment with a position and
    /// words without them is the degraded case, and the two are not the same
    /// thing.
    #[serde(default)]
    pub at: Option<Interval>,
    #[serde(default)]
    pub words: Vec<Word>,
}

/// A transcript revision's body, as stored in `body_json`.
///
/// A **value**, not a model: a transcript is data that arrived from outside
/// and is checked against the media at [`Linked::new`], which is the one place
/// that knows both the timings and how long the media is.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Transcript {
    #[serde(default)]
    pub segments: Vec<Segment>,
}

/// What a transcript's timings can support, derived from the document and
/// never stored — a stored flag is a second claim about the same fact, and it
/// can disagree with the words it describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Timing {
    /// Every word carries its own position.
    WordLevel,
    /// Only segments carry positions. Word positions are **absent**, not
    /// estimated.
    SegmentLevel,
    /// Nothing carries a position.
    Untimed,
}

impl Timing {
    /// The short form a UI shows, and the `timing` field in the wire form.
    #[must_use]
    pub fn token(self) -> &'static str {
        match self {
            Self::WordLevel => "word",
            Self::SegmentLevel => "segment",
            Self::Untimed => "untimed",
        }
    }

    /// Whether this timing can answer a seek. A transcript that cannot is
    /// **degraded, explicitly** — the alternative is a guessed word position,
    /// and a guess that looks like a measurement is the failure this whole plan
    /// keeps refusing.
    #[must_use]
    pub fn is_degraded(self) -> bool {
        !matches!(self, Self::WordLevel)
    }

    /// A stable token naming *why* it is degraded, or `None`. A token rather
    /// than prose, so a UI can branch on it and a test can assert it, and the
    /// reason cannot drift away from the state.
    #[must_use]
    pub fn degraded_reason(self) -> Option<&'static str> {
        match self {
            Self::WordLevel => None,
            Self::SegmentLevel => Some("transcript/no-word-timing"),
            Self::Untimed => Some("transcript/no-timing"),
        }
    }

    /// The wire form: the state and its reason as one value, so "degraded" and
    /// "why" cannot be rendered from two things that are out of step.
    #[must_use]
    pub fn wire(self) -> serde_json::Value {
        serde_json::json!({
            "timing": self.token(),
            "degraded": self.is_degraded(),
            "degraded_reason": self.degraded_reason(),
        })
    }
}

impl Transcript {
    /// Every word, in reading order. The document order is the order the
    /// provider produced, and a word range is a range in **this** order.
    pub fn words(&self) -> impl Iterator<Item = &Word> {
        self.segments.iter().flat_map(|s| s.words.iter())
    }

    pub(crate) fn words_mut(&mut self) -> impl Iterator<Item = &mut Word> {
        self.segments.iter_mut().flat_map(|s| s.words.iter_mut())
    }

    /// One word by identity. An absent word is [`TranscriptError::WordNotFound`]
    /// naming what was asked for, never the nearest word that happens to
    /// exist.
    pub fn word(&self, id: &WordId) -> Result<&Word, TranscriptError> {
        self.words()
            .find(|w| &w.id == id)
            .ok_or_else(|| TranscriptError::WordNotFound {
                word: id.as_str().to_string(),
            })
    }

    /// The document's own statement about what its timings can support.
    ///
    /// A document with no words is [`Timing::Untimed`]: an empty transcript
    /// carries no positions, and reporting it as degraded is the honest answer
    /// rather than a vacuous pass.
    #[must_use]
    pub fn timing(&self) -> Timing {
        let words = self.words().count();
        if words == 0 {
            return Timing::Untimed;
        }
        if self.words().all(|w| w.at.is_some()) {
            Timing::WordLevel
        } else if self.segments.iter().any(|s| s.at.is_some()) {
            Timing::SegmentLevel
        } else {
            Timing::Untimed
        }
    }

    /// The position of a word, or [`TranscriptError::NoWordTiming`]. **Never**
    /// the segment's position: a segment's span covers a run of words, and
    /// placing a word at either end of it is an interpolation.
    pub(crate) fn timed(&self, id: &WordId) -> Result<Interval, TranscriptError> {
        self.word(id)?
            .at
            .ok_or_else(|| TranscriptError::NoWordTiming {
                word: id.as_str().to_string(),
            })
    }

    /// A word's position in document order, which is what makes a range
    /// forward or backward.
    pub(crate) fn position_of(&self, id: &WordId) -> Result<usize, TranscriptError> {
        self.words()
            .position(|w| &w.id == id)
            .ok_or_else(|| TranscriptError::WordNotFound {
                word: id.as_str().to_string(),
            })
    }

    /// Check the document against itself and against the media it describes.
    ///
    /// Every failure carries the two facts that disagreed, so a caller can say
    /// which words and which segments rather than reporting a number. A source
    /// of unknown length skips the length check rather than assuming it is
    /// short.
    pub fn validate(&self, media: Option<MediaTime>) -> Result<(), TranscriptError> {
        let mut previous: Option<(usize, Interval)> = None;
        for (index, segment) in self.segments.iter().enumerate() {
            if let Some(at) = segment.at {
                ordered(at, &format!("segment {index}"))?;
                self.end_within(at, media, &format!("segment {index}"))?;
                if let Some((first, before)) = previous {
                    if at.start() < before.end() {
                        return Err(TranscriptError::OverlappingSegments {
                            first,
                            second: index,
                        });
                    }
                }
                previous = Some((index, at));
            }
            for word in &segment.words {
                let Some(at) = word.at else { continue };
                ordered(at, word.id.as_str())?;
                self.end_within(at, media, word.id.as_str())?;
                if let Some(segment_at) = segment.at {
                    if at.start() < segment_at.start() || at.end() > segment_at.end() {
                        return Err(TranscriptError::WordOutsideSegment {
                            word: word.id.as_str().to_string(),
                            segment: index,
                        });
                    }
                }
            }
        }
        Ok(())
    }

    fn end_within(
        &self,
        at: Interval,
        media: Option<MediaTime>,
        what: &str,
    ) -> Result<(), TranscriptError> {
        let Some(duration) = media else { return Ok(()) };
        if at.end() > duration {
            return Err(TranscriptError::BeyondMedia {
                at: at.end().micros(),
                duration: duration.micros(),
                detail: what.to_string(),
            });
        }
        Ok(())
    }
}

/// A span whose end is before its start.
///
/// [`Interval::new`] refuses one, and no Rust code can build one — but
/// `Interval` derives `Deserialize`, so a document read back from stored bytes
/// can arrive inverted without passing the constructor. The check is here
/// rather than in the time model because the time model's file is not this
/// todo's, and an inverted interval reaching `Interval::contains` makes a word
/// silently unseekable instead of loudly malformed.
fn ordered(at: Interval, what: &str) -> Result<(), TranscriptError> {
    if at.end() < at.start() {
        return Err(TranscriptError::Time(TimeError::Inverted {
            start: at.start().micros(),
            end: at.end().micros(),
        }));
    }
    let _ = what;
    Ok(())
}

/// Everything this module refuses, each carrying the fact that caused it. A
/// bare string would leave a caller unable to tell "you asked for the wrong
/// thing" from "the provider never told us".
#[derive(Debug, Clone, PartialEq)]
pub enum TranscriptError {
    WordIdInvalid {
        detail: String,
    },
    WordNotFound {
        word: String,
    },
    /// The provider reported no position for this word, and one will not be
    /// invented from its segment.
    NoWordTiming {
        word: String,
    },
    /// The composition does not show this instant, so there is nothing to seek
    /// to. Reported rather than clamped to the nearest edge.
    UnmappedSource {
        word: String,
        at: i64,
    },
    /// The word is shown more than once, so it has no single seek target.
    Repeated {
        word: String,
        positions: usize,
    },
    NoWordAt {
        at: i64,
        timing: Timing,
    },
    NotForward {
        from: String,
        to: String,
    },
    EmptyText {
        word: String,
    },
    /// The transcript's timings cannot place a cut exactly, so one is refused
    /// rather than approximated.
    NotExactEnough {
        timing: Timing,
    },
    OverlappingSegments {
        first: usize,
        second: usize,
    },
    WordOutsideSegment {
        word: String,
        segment: usize,
    },
    BeyondMedia {
        at: i64,
        duration: i64,
        detail: String,
    },
    RevisionSuperseded {
        base: String,
        current: String,
    },
    Time(TimeError),
}

impl fmt::Display for TranscriptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WordIdInvalid { detail } => write!(f, "not a usable word id: {detail}"),
            Self::WordNotFound { word } => write!(f, "no word {word} in this transcript"),
            Self::NoWordTiming { word } => {
                write!(f, "word {word} has no position; its provider reported none")
            }
            Self::UnmappedSource { word, at } => {
                let who = if word.is_empty() {
                    "that instant"
                } else {
                    word.as_str()
                };
                write!(f, "the composition does not show {who} at {at}us")
            }
            Self::Repeated { word, positions } => write!(
                f,
                "word {word} is shown {positions} times, so it has no single seek target"
            ),
            Self::NoWordAt { at, timing } => {
                write!(
                    f,
                    "no word is spoken at {at}us (timing: {})",
                    timing.token()
                )
            }
            Self::NotForward { from, to } => write!(f, "{to} does not follow {from}"),
            Self::EmptyText { word } => write!(f, "word {word} cannot be corrected to nothing"),
            Self::NotExactEnough { timing } => write!(
                f,
                "this transcript is {} timed, so the media cannot be cut exactly; \
                 correct the text instead",
                timing.token()
            ),
            Self::OverlappingSegments { first, second } => {
                write!(f, "segments {first} and {second} overlap")
            }
            Self::WordOutsideSegment { word, segment } => {
                write!(f, "word {word} is timed outside segment {segment}")
            }
            Self::BeyondMedia {
                at,
                duration,
                detail,
            } => {
                write!(f, "{detail} ends at {at}us, past the media's {duration}us")
            }
            Self::RevisionSuperseded { base, current } => write!(
                f,
                "this edit is against transcript revision {base}, and the open one is {current}"
            ),
            Self::Time(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for TranscriptError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Time(e) => Some(e),
            _ => None,
        }
    }
}

impl From<TimeError> for TranscriptError {
    fn from(e: TimeError) -> Self {
        Self::Time(e)
    }
}

/// A transcript-scoped result, so a caller does not have to know that some of
/// these refusals are about the time model and some are about the document.
pub type TranscriptResult<T> = Result<T, TranscriptError>;
