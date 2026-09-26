//! The link itself: a transcript paired with the composition it indexes.

use serde::{Deserialize, Serialize};

use super::{edit::Edited, Timing, Transcript, TranscriptError, WordId};
use crate::time::{Entry, Interval, IntervalMap, MediaTime, Rate, SourceRef, SourceSpan};

/// The length between two media instants, exact or refused. `MediaTime` has
/// `checked_add` and no `checked_sub`; two non-negative `i64`s cannot overflow
/// on subtraction, and every call site has already established that the second
/// is at or after the first, so this is the one place that subtraction is
/// written rather than a gap in the type.
fn since(start: MediaTime, at: MediaTime) -> Result<MediaTime, TranscriptError> {
    Ok(MediaTime::from_micros(at.micros() - start.micros())?)
}

/// One source span of a composition, with the output arithmetic needed to
/// place a source instant on the output and read one back.
pub(crate) struct Placed<'a> {
    /// Where this span's source begins on the **output** timeline.
    output_start: MediaTime,
    /// Where its source begins on the **source** timeline.
    source_start: MediaTime,
    rate: Rate,
    span: &'a SourceSpan,
}

impl Placed<'_> {
    /// The output length this span occupies, at its own rate. Exact or
    /// refused: a map that rounds is not a map.
    fn output_len(&self) -> Result<MediaTime, TranscriptError> {
        Ok(self.rate.output_of(self.span.range.duration()?)?)
    }

    /// The output position a source instant lands on inside this span.
    fn project(&self, at: MediaTime) -> Result<MediaTime, TranscriptError> {
        let offset = since(self.source_start, at)?;
        let length = self.rate.output_of(offset)?;
        Ok(MediaTime::from_micros(
            self.output_start.micros() + length.micros(),
        )?)
    }

    /// The source instant an output position shows, inside this span.
    fn unproject(&self, at: MediaTime) -> Result<MediaTime, TranscriptError> {
        let offset = since(self.output_start, at)?;
        Ok(self
            .source_start
            .checked_add(self.rate.source_of(offset)?)?)
    }

    /// The output interval this span occupies.
    fn output(&self) -> Result<Interval, TranscriptError> {
        Interval::new(
            self.output_start,
            self.output_start.checked_add(self.output_len()?)?,
        )
        .map_err(TranscriptError::from)
    }

    /// How much **output** a source length occupies here.
    fn advance(&self, length: MediaTime) -> Result<MediaTime, TranscriptError> {
        Ok(self.rate.output_of(length)?)
    }
}

/// One removed source range, in the shape a render input wants it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceCut {
    /// The source identity the range was removed from. An id, never a path.
    pub source: SourceRef,
    /// The source range that is no longer in the composition.
    pub range: Interval,
    /// How much of the **output** this range occupied. The composition's new
    /// length is the old length minus this.
    pub output_removed: MediaTime,
}

/// A transcript paired with the composition it indexes, and the source
/// identity both name.
///
/// Built once and treated as immutable: every edit returns a **new** pair, so a
/// view holding a `Linked` holds a statement about the world at one revision,
/// and an edit that changed the composition cannot be half-applied to a link
/// somebody else is reading.
#[derive(Debug, Clone, PartialEq)]
pub struct Linked {
    transcript: Transcript,
    map: IntervalMap,
    source: SourceRef,
    /// The transcript revision this link is anchored to, when the caller knows
    /// it. **This is the transcript-revision selection** — held here, on the
    /// pair, rather than in a second store or a second column. See the module
    /// docs for why there is no column.
    revision: Option<String>,
    media: Option<MediaTime>,
}

impl Linked {
    /// Pair a transcript with a composition, checking the transcript against
    /// the media before it can be used.
    ///
    /// The media length is `Option` because a source whose length was never
    /// probed is a real state: the length check is skipped, and the transcript
    /// is not shortened to fit an assumption.
    pub fn new(
        transcript: Transcript,
        map: IntervalMap,
        source: SourceRef,
        revision: Option<String>,
        media: Option<MediaTime>,
    ) -> Result<Self, TranscriptError> {
        transcript.validate(media)?;
        Ok(Self {
            transcript,
            map,
            source,
            revision,
            media,
        })
    }

    #[must_use]
    pub fn transcript(&self) -> &Transcript {
        &self.transcript
    }

    #[must_use]
    pub fn map(&self) -> &IntervalMap {
        &self.map
    }

    #[must_use]
    pub fn source(&self) -> &SourceRef {
        &self.source
    }

    /// The transcript revision this link is anchored to.
    #[must_use]
    pub fn revision(&self) -> Option<&str> {
        self.revision.as_deref()
    }

    #[must_use]
    pub fn media_duration(&self) -> Option<MediaTime> {
        self.media
    }

    /// The composition instant at which a word is spoken.
    ///
    /// Exact to the microsecond, or refused. A word with no provider timing is
    /// [`TranscriptError::NoWordTiming`]; a word whose media is not in the
    /// composition is [`TranscriptError::UnmappedSource`]; a word shown more
    /// than once is [`TranscriptError::Repeated`], because it has no *single*
    /// instant and taking the first would drop the repeat.
    pub fn seek_target(&self, id: &WordId) -> Result<MediaTime, TranscriptError> {
        let at = self.transcript.timed(id)?.start();
        let mut targets = self.seek_targets(id)?;
        match targets.len() {
            0 => Err(TranscriptError::UnmappedSource {
                word: id.as_str().to_string(),
                at: at.micros(),
            }),
            1 => Ok(targets.remove(0)),
            positions => Err(TranscriptError::Repeated {
                word: id.as_str().to_string(),
                positions,
            }),
        }
    }

    /// **Every** composition instant at which a word is spoken. The plural
    /// exists because the singular is sometimes wrong, and a caller that
    /// cannot tell the difference should be handed the whole answer.
    pub fn seek_targets(&self, id: &WordId) -> Result<Vec<MediaTime>, TranscriptError> {
        let at = self.transcript.timed(id)?.start();
        let mut out = Vec::new();
        for placed in self.placements() {
            if placed.span.range.contains(at) {
                out.push(placed.project(at)?);
            }
        }
        Ok(out)
    }

    /// The words spoken at a composition instant, in document order — the
    /// reverse of [`Linked::seek_target`], and exact in the same way. An
    /// instant no word covers is [`TranscriptError::NoWordAt`], never the
    /// nearest word.
    pub fn words_at(&self, at: MediaTime) -> Result<Vec<WordId>, TranscriptError> {
        let mut ids = Vec::new();
        for placed in self.placements() {
            if !placed.output()?.contains(at) {
                continue;
            }
            let source = placed.unproject(at)?;
            for word in self.transcript.words() {
                if word.at.is_some_and(|w| w.contains(source)) {
                    ids.push(word.id.clone());
                }
            }
        }
        if ids.is_empty() {
            return Err(TranscriptError::NoWordAt {
                at: at.micros(),
                timing: self.transcript.timing(),
            });
        }
        Ok(ids)
    }

    /// The composition interval a forward word range occupies — measured in
    /// output time, which is what a viewer seeking across the range needs and
    /// what a caption has to match. A range that runs backwards in document
    /// order has no interval and is [`TranscriptError::NotForward`], not
    /// reordered.
    pub fn composition_span(
        &self,
        from: &WordId,
        to: &WordId,
    ) -> Result<Interval, TranscriptError> {
        let last = self.transcript.timed(to)?;
        let (start, end) = self.range_bounds(from, to)?;
        // The tail is the last word's own length, advanced at the rate of the
        // span it sits in. It is added to the range's **end**, not to its
        // start: the range covers every word from `from` to `to`, so its end is
        // where `to` is spoken, and only `to`'s own length is still missing.
        let tail = self
            .placements()
            .into_iter()
            .find(|p| p.span.range.contains(last.start()))
            .ok_or(TranscriptError::UnmappedSource {
                word: to.as_str().to_string(),
                at: last.start().micros(),
            })?
            .advance(last.duration()?)?;
        Ok(Interval::new(start, end.checked_add(tail)?)?)
    }

    /// The output start of `from` and of `to`, with the document order checked
    /// once, here, rather than by each caller.
    fn range_bounds(
        &self,
        from: &WordId,
        to: &WordId,
    ) -> Result<(MediaTime, MediaTime), TranscriptError> {
        let a = self.transcript.position_of(from)?;
        let b = self.transcript.position_of(to)?;
        if b < a {
            return Err(TranscriptError::NotForward {
                from: from.as_str().to_string(),
                to: to.as_str().to_string(),
            });
        }
        Ok((self.seek_target(from)?, self.seek_target(to)?))
    }

    /// **Correct-text mode.** A new transcript with one word's text replaced;
    /// the composition is not reachable from this signature, so a correction
    /// cannot change the picture even in principle.
    ///
    /// The word keeps its [`WordId`] and its timing: correcting what somebody
    /// said must not move when they said it, nor renumber anything downstream.
    /// Empty text is refused, because an empty word is not a correction — a
    /// deletion has to go through the composition, or it is a deletion that
    /// quietly cuts nothing.
    pub fn correct_text(&self, id: &WordId, text: &str) -> Result<Transcript, TranscriptError> {
        if text.trim().is_empty() {
            return Err(TranscriptError::EmptyText {
                word: id.as_str().to_string(),
            });
        }
        let mut out = self.transcript.clone();
        let word =
            out.words_mut()
                .find(|w| w.id == *id)
                .ok_or_else(|| TranscriptError::WordNotFound {
                    word: id.as_str().to_string(),
                })?;
        word.text = text.to_string();
        Ok(out)
    }

    /// **Delete a word range**, which edits the composition and no media.
    ///
    /// The words leave the text and the source time they cover leaves the
    /// composition, so every later word seeks to a new — and exactly computed —
    /// instant. On a transcript whose provider reported no word timings this
    /// is [`TranscriptError::NotExactEnough`]: a cut at a resolution the
    /// provider never reported would be a guess about where the media is, and
    /// a cut that guesses is a cut on the wrong frame.
    pub fn delete(&self, from: &WordId, to: &WordId) -> Result<Edited, TranscriptError> {
        let timing = self.transcript.timing();
        if timing != Timing::WordLevel {
            return Err(TranscriptError::NotExactEnough { timing });
        }
        let (start, end) = self.source_range_of(from, to)?;
        let cut = self.cut_for(start, end)?;
        Ok(Edited::new(
            self.transcript.without(&[super::edit::WordRange {
                from_word: from.clone(),
                to_word: to.clone(),
            }]),
            self.without(start, end)?,
            vec![cut],
            self.transcript.clone(),
            self.map.clone(),
        ))
    }

    /// Apply a whole unsaved draft: the corrections, then the deletions, as one
    /// new pair. A draft names the revision it applies to, so a draft built
    /// against a superseded revision is reported rather than quietly applied to
    /// whatever happens to be loaded.
    ///
    /// The whole patch is validated **before** any of it is applied, so a draft
    /// with one bad word cannot leave the document half-edited.
    pub fn apply(&self, draft: &super::edit::TranscriptDraft) -> Result<Edited, TranscriptError> {
        if let Some(current) = &self.revision {
            if &draft.base_revision != current {
                return Err(TranscriptError::RevisionSuperseded {
                    base: draft.base_revision.clone(),
                    current: current.clone(),
                });
            }
        }
        for correction in &draft.corrected {
            self.transcript.word(&correction.word)?;
            if correction.text.trim().is_empty() {
                return Err(TranscriptError::EmptyText {
                    word: correction.word.as_str().to_string(),
                });
            }
        }
        for range in &draft.deleted {
            self.source_range_of(&range.from_word, &range.to_word)?;
        }

        let mut transcript = self.transcript.clone();
        for correction in &draft.corrected {
            let word = transcript
                .words_mut()
                .find(|w| w.id == correction.word)
                .expect("every correction word was validated above");
            word.text = correction.text.clone();
        }
        let mut cuts = Vec::with_capacity(draft.deleted.len());
        let mut map = self.map.clone();
        for range in &draft.deleted {
            let (start, end) = self.source_range_of(&range.from_word, &range.to_word)?;
            let cut = self.cut_for(start, end)?;
            map = without_range(&map, &cut.source, cut.range)?;
            cuts.push(cut);
        }
        Ok(Edited::new(
            transcript.without(&draft.deleted),
            map,
            cuts,
            self.transcript.clone(),
            self.map.clone(),
        ))
    }

    /// The source range a forward word range covers, as a whole.
    pub(crate) fn source_range_of(
        &self,
        from: &WordId,
        to: &WordId,
    ) -> Result<(MediaTime, MediaTime), TranscriptError> {
        let a = self.transcript.position_of(from)?;
        let b = self.transcript.position_of(to)?;
        if b < a {
            return Err(TranscriptError::NotForward {
                from: from.as_str().to_string(),
                to: to.as_str().to_string(),
            });
        }
        let last = self.transcript.timed(to)?;
        let first = self.transcript.timed(from)?;
        Ok((first.start(), last.end()))
    }

    /// The cut a source range makes, including how much output it removes, so
    /// a caller can build a render input without recomputing it.
    fn cut_for(&self, start: MediaTime, end: MediaTime) -> Result<SourceCut, TranscriptError> {
        let range = Interval::new(start, end)?;
        let mut removed = MediaTime::ZERO;
        for placed in self.placements() {
            let whole = placed.span.range;
            let lo = if whole.start() > start {
                whole.start()
            } else {
                start
            };
            let hi = if whole.end() < end { whole.end() } else { end };
            if hi <= lo {
                continue;
            }
            removed = MediaTime::from_micros(
                removed.micros() + placed.advance(since(lo, hi)?)?.micros(),
            )?;
        }
        Ok(SourceCut {
            source: self.source.clone(),
            range,
            output_removed: removed,
        })
    }

    /// Every source span of the composition, with its output arithmetic.
    pub(crate) fn placements(&self) -> Vec<Placed<'_>> {
        self.map
            .entries()
            .iter()
            .filter_map(|entry| match entry {
                Entry::Source { span, output } => Some(Placed {
                    output_start: output.start(),
                    source_start: span.range.start(),
                    rate: span.rate,
                    span,
                }),
                Entry::Gap { .. } => None,
            })
            .collect()
    }

    /// The composition with a source range removed. The map is rebuilt rather
    /// than edited, because it is append-only: an entry that would have to
    /// start before the timeline's end cannot be pushed onto it at all.
    fn without(&self, start: MediaTime, end: MediaTime) -> Result<IntervalMap, TranscriptError> {
        without_range(
            &self.map,
            &self.source,
            Interval::new(start, end).map_err(TranscriptError::from)?,
        )
    }
}

/// A composition with one source range removed, rebuilt in output order.
///
/// A range not in the map at all is not an error: it is already gone, and
/// rebuilding without it changes nothing. That is what makes an edit
/// idempotent against a map that has already been cut.
fn without_range(
    map: &IntervalMap,
    source: &SourceRef,
    range: Interval,
) -> Result<IntervalMap, TranscriptError> {
    let mut out = IntervalMap::new();
    for entry in map.entries() {
        match entry {
            Entry::Gap { output } => {
                out.push_gap(output.duration()?)?;
            }
            Entry::Source { span, .. } => {
                if &span.source != source {
                    out.push_source(span.clone())?;
                    continue;
                }
                for piece in subtract(span.range, range)? {
                    out.push_source(
                        SourceSpan::new(span.source.clone(), span.timebase, piece, span.rate)
                            .map_err(TranscriptError::from)?,
                    )?;
                }
            }
        }
    }
    Ok(out)
}

/// `whole` minus `cut`, as a list of pieces in order. Exact integer
/// arithmetic, and a piece is emitted only when it has length, because a
/// zero-length span is refused by `SourceSpan::new`.
fn subtract(whole: Interval, cut: Interval) -> Result<Vec<Interval>, TranscriptError> {
    if cut.end() <= whole.start() || cut.start() >= whole.end() {
        return Ok(vec![whole]);
    }
    let mut pieces = Vec::new();
    if whole.start() < cut.start() {
        pieces.push(Interval::new(whole.start(), cut.start())?);
    }
    if cut.end() < whole.end() {
        pieces.push(Interval::new(cut.end(), whole.end())?);
    }
    Ok(pieces)
}
