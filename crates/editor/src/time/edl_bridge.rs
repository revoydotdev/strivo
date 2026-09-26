//! The one boundary where `f32` seconds become media time.
//!
//! [`Cut`] carries its timestamps as `f32` seconds because that is the
//! on-disk and SPA wire shape of the existing edit decision list, which
//! other work owns. It is a **boundary format, not a time model**: the floats
//! are read here, once, and everything downstream is an integer
//! [`MediaTime`].
//!
//! §7.2's instruction is "must not silently coerce the existing `f32` cuts",
//! and this is the reading of it that is safe in both directions. Nothing is
//! coerced: a NaN, an infinity, a negative and an inverted span are each
//! refused by name, so a corrupt EDL is reported at the boundary instead of
//! arriving downstream as a plausible wrong frame. Nothing is *lost* either
//! — the old representation is still what the store reads and the SPA still
//! writes, so an existing draft keeps loading, and todo 25 owns admission
//! validation of the request that produced it.
//!
//! Two standing-ins are declared here rather than left implicit. A cut
//! carries no frame index, so its timebase is recorded as the identity rate
//! (see [`SourceSpan::timebase`]). And a cut carries a *path*, not an asset
//! id, so the span's source is a locator-derived stand-in
//! ([`SourceRef::from_locator`]) — which is precisely why the same recording
//! reached through two different paths is two sources here, and a store that
//! knows the asset ids should overwrite them.

use super::interval_map::{IntervalMap, SourceRef, SourceSpan};
use super::{Interval, MediaTime, Rate, TimeBase, TimeError};
use crate::{Cut, CutKind, Edl};

/// Read one named `f32`-seconds field as a media coordinate, naming the field
/// in any refusal so a corrupt EDL says which coordinate was wrong. The
/// conversion itself is [`MediaTime::from_secs_f64`], shared with every other
/// caller that has seconds; this wrapper only attaches the field name.
fn micros_from_secs(field: &'static str, secs: f32) -> Result<MediaTime, TimeError> {
    MediaTime::from_secs_f64(f64::from(secs)).map_err(|e| match e {
        TimeError::NotFinite { .. } => TimeError::NotFinite { field },
        TimeError::NegativeSeconds { .. } => TimeError::NegativeSeconds { field },
        other => other,
    })
}

impl Cut {
    /// The output-to-source reading this cut states, as an integer span.
    pub fn span(&self) -> Result<SourceSpan, TimeError> {
        let source = Interval::new(
            micros_from_secs("start_sec", self.start_sec)?,
            micros_from_secs("end_sec", self.end_sec)?,
        )?;
        // An inserted B-roll asset is a distinct source for mapping
        // purposes, so the same source interval in two cuts is a
        // many-to-one reverse map rather than one entry overwriting the
        // other. A cut that trims to nothing is refused: an empty source
        // span has no reverse map.
        let locator = match &self.kind {
            CutKind::Source { source_path } => source_path,
            CutKind::Broll { broll_path } => broll_path,
        };
        SourceSpan::new(
            SourceRef::from_locator(locator),
            TimeBase::from_rate(1, 1)?,
            source,
            Rate::ONE,
        )
    }
}

impl Edl {
    /// This edit's output-to-source map, in integer media time.
    ///
    /// Deleted regions need no representation: they are the cuts that are
    /// not there. Every cut is placed in order, so the output timeline is the
    /// concatenation of the cuts' own durations — which is what
    /// [`Edl::total_duration`] computes in floating point, and is the one
    /// place the two representations could disagree. A caller that needs the
    /// exact output length reads [`IntervalMap::output_duration`].
    pub fn interval_map(&self) -> Result<IntervalMap, TimeError> {
        let mut map = IntervalMap::new();
        for cut in &self.cuts {
            map.push_source(cut.span()?)?;
        }
        Ok(map)
    }
}
