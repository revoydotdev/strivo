//! The output-to-source interval map, and the declaration that says which
//! clock a coordinate is on.
//!
//! A composition is a sequence of segments on one output timeline. Each
//! segment either reads a span of a source (possibly sped up, possibly the
//! same span more than once) or is a **gap**: an output range with no source
//! at all, which is what a hold, a still or a track gap is. Deleted regions
//! need no representation — they are the absence of a segment.
//!
//! Both directions are integer arithmetic over [`MediaTime`], and both are
//! total or refused:
//!
//! * forward — an output position finds its source position, or reports
//!   [`TimeError::Unmapped`] when it falls in a gap. It never clamps to the
//!   gap edge, because a caption at the edge of a gap is a caption on the
//!   wrong frame;
//! * reverse — a source position finds **every** output position that shows
//!   it, because a repeated clip is many-to-one and picking the first match
//!   would silently drop the repeat.
//!
//! [`Coordinate`] is the declaration §7.2 asks for: a timestamped thing says
//! whether its coordinate is source time or composition time, and
//! [`IntervalMap::resolve`] dispatches on it. One entry point, both
//! directions, no way to ask the wrong question and get an answer.

use serde::{Deserialize, Serialize};

use super::{Interval, MediaTime, Rate, TimeBase, TimeError, TimeError::Unmapped};

/// A source's identity, as the owning store spells it.
///
/// A newtype over a string rather than a bare `String`, because the invariant
/// worth enforcing is that a filesystem path cannot be passed where a source
/// identity belongs: a path is a resolvable location that changes, the id
/// does not. The store that owns the id supplies it; this model never
/// resolves one and never looks a source up.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SourceRef(String);

impl SourceRef {
    #[must_use]
    pub fn new(id: &str) -> Self {
        Self(id.to_string())
    }

    /// A stand-in for a source identity, derived from a filesystem locator.
    ///
    /// For a caller that has only a path — the legacy edit decision list does
    /// — and a deliberate weakening: a locator is a resolvable location, so
    /// two locators can name one asset and one locator can move. A store that
    /// knows the asset ids should use [`SourceRef::new`] and overwrite these,
    /// and until it does, the reverse map distinguishes two locators rather
    /// than joining them.
    #[must_use]
    pub fn from_locator(locator: &str) -> Self {
        Self(format!("locator:{locator}"))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SourceRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One span of one source, placed on the output timeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSpan {
    /// Which source is read.
    pub source: SourceRef,
    /// That source's own timebase, recorded rather than assumed, so a frame
    /// index from a probe can be converted at the boundary instead of being
    /// read as microseconds.
    pub timebase: TimeBase,
    /// The source range this segment reads, in source media time. Must be
    /// non-empty: its length is the divisor of the reverse map.
    pub range: Interval,
    /// How much output one unit of this source produces.
    pub rate: Rate,
}

impl SourceSpan {
    /// Build a span, refusing an empty source range.
    pub fn new(
        source: SourceRef,
        timebase: TimeBase,
        range: Interval,
        rate: Rate,
    ) -> Result<Self, TimeError> {
        if range.duration()?.micros() == 0 {
            return Err(TimeError::Empty {
                at: range.start().micros(),
            });
        }
        Ok(Self {
            source,
            timebase,
            range,
            rate,
        })
    }
}

/// One position on the output timeline, with the uncertainty in its value.
///
/// Uncertainty is zero for every map operation — the arithmetic is exact —
/// and non-zero only for a wall-clock conversion, where the capture clock
/// and the media clock genuinely disagree. One type for both, so a caller
/// cannot read an exact map result as if it were a guess or vice versa.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MappedTime {
    pub at: MediaTime,
    /// The bound on how far `at` may be from the true coordinate, in
    /// microseconds. Never negative; zero means exact.
    pub uncertainty_micros: i64,
}

impl MappedTime {
    /// An exact position: the value is the coordinate.
    pub const fn exact(at: MediaTime) -> Self {
        Self {
            at,
            uncertainty_micros: 0,
        }
    }
}

/// Which clock a coordinate is measured on.
///
/// This is the declaration `docs/CREATOR-EDITION-REPORT-2026-09-07.md:196`
/// requires of every timestamped thing — captions, chat overlays, waveform
/// selections, detections, annotations and outcome charts. It is an enum
/// rather than a newtype pair because it is a *stored* fact that has to
/// round-trip through JSON, and a newtype pair would carry the same
/// information without making the choice explicit at the call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Coordinate {
    /// Media time of the source, before any edit.
    Source,
    /// Media time of the composition, after every edit.
    Composition,
}

impl std::fmt::Display for Coordinate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Source => "source",
            Self::Composition => "composition",
        })
    }
}

/// A timestamped thing that says which clock it is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timed {
    pub at: MediaTime,
    pub coordinate: Coordinate,
}

impl Timed {
    #[must_use]
    pub const fn source(at: MediaTime) -> Self {
        Self {
            at,
            coordinate: Coordinate::Source,
        }
    }

    #[must_use]
    pub const fn composition(at: MediaTime) -> Self {
        Self {
            at,
            coordinate: Coordinate::Composition,
        }
    }
}

/// The answer to [`IntervalMap::resolve`]: the other clock, with a
/// declaration of which one it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Mapped {
    /// Asked in composition time, answered in source time.
    Source(MappedTime),
    /// Asked in source time, answered in every composition time that shows
    /// it. One entry for an ordinary span; several for a repeated clip.
    Composition(Vec<MappedTime>),
}

/// One segment of the output timeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Entry {
    /// Reads a source span.
    Source { span: SourceSpan, output: Interval },
    /// An output range with no source behind it.
    Gap { output: Interval },
}

/// A composition's output-to-source map.
///
/// Built by appending, so the output timeline is contiguous by construction
/// and a composition cannot quietly contain an overlap. Use
/// [`IntervalMap::push_source_at`] to place a segment at an explicit output
/// position; it refuses anything that is not exactly where the timeline is,
/// which is where an overlap or a hole gets reported instead of absorbed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntervalMap {
    entries: Vec<Entry>,
}

impl IntervalMap {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Where the output timeline currently ends: the position the next
    /// appended segment must start at.
    #[must_use]
    pub fn output_duration(&self) -> MediaTime {
        self.entries.last().map_or(MediaTime::ZERO, |e| match e {
            Entry::Source { output, .. } | Entry::Gap { output } => output.end(),
        })
    }

    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Append a source span at the end of the timeline.
    pub fn push_source(&mut self, span: SourceSpan) -> Result<Interval, TimeError> {
        let at = self.output_duration();
        self.push_source_at(at, span)
    }

    /// Append a source span at an explicit output position. The position must
    /// be exactly the end of the timeline: a segment that starts earlier is
    /// an overlap and one that starts later is a hole, and neither is a
    /// thing this map can represent.
    pub fn push_source_at(
        &mut self,
        output_start: MediaTime,
        span: SourceSpan,
    ) -> Result<Interval, TimeError> {
        let expected = self.output_duration();
        if output_start != expected {
            return Err(TimeError::OutputDiscontinuity {
                expected: expected.micros(),
                got: output_start.micros(),
            });
        }
        let length = span.rate.output_of(span.range.duration()?)?;
        let output = Interval::new(
            output_start,
            MediaTime::from_micros(output_start.micros() + length.micros())?,
        )?;
        self.entries.push(Entry::Source { span, output });
        Ok(output)
    }

    /// Append a gap of `length` at the end of the timeline: output that has
    /// no source behind it. A zero-length gap is accepted and recorded as
    /// nothing, because refusing it would make a caller special-case an edit
    /// that trims to nothing.
    pub fn push_gap(&mut self, length: MediaTime) -> Result<Interval, TimeError> {
        let start = self.output_duration();
        let end = MediaTime::from_micros(start.micros() + length.micros())?;
        let output = Interval::new(start, end)?;
        self.entries.push(Entry::Gap { output });
        Ok(output)
    }

    /// Forward: the source coordinate shown at an output position. A
    /// position inside a gap has no source and is reported as unmapped.
    pub fn source_at(&self, at: MediaTime) -> Result<MappedTime, TimeError> {
        for entry in &self.entries {
            match entry {
                // A gap is skipped rather than matched: no entry's output
                // contains a position inside one, so a gap position falls
                // through the loop to the same refusal a position past the
                // end gets. An arm here returning the same error would be a
                // second copy of it.
                Entry::Gap { .. } => {}
                Entry::Source { span, output } if output.contains(at) => {
                    let offset = MediaTime::from_micros(at.micros() - output.start().micros())?;
                    return Ok(MappedTime::exact(
                        span.range
                            .start()
                            .checked_add(span.rate.source_of(offset)?)?,
                    ));
                }
                Entry::Source { .. } => {}
            }
        }
        Err(Unmapped {
            coordinate: Coordinate::Composition,
            at: at.micros(),
        })
    }

    /// Reverse: every output position that shows a source coordinate.
    ///
    /// Many results are the normal case, not an edge case: the same source
    /// span used twice in one composition maps to two output positions, and
    /// a caller that took only the first would lose the repeat.
    pub fn composition_times_of(&self, at: MediaTime) -> Result<Vec<MappedTime>, TimeError> {
        let mut hits = Vec::new();
        for entry in &self.entries {
            let Entry::Source { span, output } = entry else {
                continue;
            };
            if !span.range.contains(at) {
                continue;
            }
            let offset = span.rate.output_of(MediaTime::from_micros(
                at.micros() - span.range.start().micros(),
            )?)?;
            hits.push(MappedTime::exact(MediaTime::from_micros(
                output.start().micros() + offset.micros(),
            )?));
        }
        if hits.is_empty() {
            return Err(Unmapped {
                coordinate: Coordinate::Source,
                at: at.micros(),
            });
        }
        Ok(hits)
    }

    /// Every output interval a whole source range occupies — one per entry that
    /// contains all of it.
    ///
    /// [`Self::composition_times_of`] cannot answer this. It takes one instant,
    /// so two instants answered separately may come from *different* entries,
    /// and pairing them would draw one continuous thing across the cut between
    /// them. This takes the range itself and only reports entries that contain
    /// all of it, so a range running out of a kept segment has no answer here
    /// and is refused rather than joined up.
    ///
    /// More than one answer is the repeated-clip case, exactly as it is above.
    pub fn output_spans_of(&self, range: Interval) -> Result<Vec<Interval>, TimeError> {
        let mut out = Vec::new();
        for entry in &self.entries {
            let Entry::Source { span, output } = entry else {
                continue;
            };
            if range.start() < span.range.start() || range.end() > span.range.end() {
                continue;
            }
            let offset = span.rate.output_of(MediaTime::from_micros(
                range.start().micros() - span.range.start().micros(),
            )?)?;
            let start = MediaTime::from_micros(output.start().micros() + offset.micros())?;
            let length = span.rate.output_of(range.duration()?)?;
            out.push(Interval::new(
                start,
                MediaTime::from_micros(start.micros() + length.micros())?,
            )?);
        }
        if out.is_empty() {
            return Err(Unmapped {
                coordinate: Coordinate::Source,
                at: range.start().micros(),
            });
        }
        Ok(out)
    }

    /// The declared-coordinate entry point. A source-time coordinate is
    /// answered with composition positions, a composition-time coordinate
    /// with a source position, and the declaration is the only thing that
    /// selects the direction.
    pub fn resolve(&self, timed: Timed) -> Result<Mapped, TimeError> {
        match timed.coordinate {
            Coordinate::Composition => self.source_at(timed.at).map(Mapped::Source),
            Coordinate::Source => self.composition_times_of(timed.at).map(Mapped::Composition),
        }
    }
}
