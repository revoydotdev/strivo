//! Stage 3 of the media-input gate: the per-cut range check.
//!
//! Split out of [`super`] so this file is the whole of the range logic and can
//! be unit-tested against an in-memory edit decision list with no server, no
//! filesystem and no child process. The input side ([`super::admit_path`] and
//! [`super::probe_admitted`]) is what produces the [`AdmittedMedia`] this
//! checks against; keeping them apart is what makes the check testable at all.

use std::collections::HashMap;
use std::path::Path;

use strivo_editor::{time::MediaTime, Cut, CutKind, Edl};

use super::{admit_path, AdmittedMedia, MediaRejection};

/// An edit decision list that passed admission: the sources it was allowed to
/// read, and its output length in whole microseconds.
#[derive(Debug, Clone, PartialEq)]
pub struct AdmittedEdl {
    /// The distinct canonical sources, in first-reference order.
    pub sources: Vec<std::path::PathBuf>,
    /// The output length, from [`IntervalMap::output_duration`] — the integer
    /// answer, which is the one that must not disagree with the floating-point
    /// `Edl::total_duration` the wire format still carries.
    ///
    /// [`IntervalMap::output_duration`]: strivo_editor::time::IntervalMap::output_duration
    pub output: MediaTime,
}

/// Check every cut of an already-path-admitted edit decision list against the
/// sources it was admitted with.
///
/// Pure: no clock, no filesystem, no child. `admitted` must hold every source
/// the EDL names — [`admit_path`] resolved the paths and [`super::probe_admitted`]
/// read their durations; this function is the decision.
///
/// Two refusals per cut, and they are different decisions. A cut whose own
/// coordinates are not a media coordinate is a corrupt document: NaN, an
/// infinity, a negative, or an end before its start, each carried as todo 22's
/// own [`TimeError`](strivo_editor::time::TimeError) rather than re-derived.
/// A cut that reads as a coordinate but reaches past the end of the source it
/// names is an out-of-range edit: the document is well-formed and the edit is
/// simply not possible. Refusing the second is the plan's "out-of-range
/// `start_sec`/`end_sec`", and it needs the source's real duration — which is
/// why it cannot live in the path stage.
pub fn admit_edl(
    root: &Path,
    edl: &Edl,
    admitted: &HashMap<std::path::PathBuf, AdmittedMedia>,
) -> Result<AdmittedEdl, MediaRejection> {
    if edl.cuts.is_empty() {
        return Err(MediaRejection::NoCuts);
    }
    let mut sources: Vec<std::path::PathBuf> = Vec::new();
    for (index, cut) in edl.cuts.iter().enumerate() {
        let real = admit_path(Some(root), cut_path(cut))
            .map_err(|source| MediaRejection::CutPath { cut: index, source })?;
        if !sources.contains(&real) {
            sources.push(real.clone());
        }
        // `Cut::span` is the one place the legacy `f32` seconds become media
        // time, and it already refuses non-finite, negative, inverted and
        // zero-length cuts by name. Its error is the reason; this adds the
        // cut's position.
        let span = cut
            .span()
            .map_err(|error| MediaRejection::CutRange { cut: index, error })?;
        let range = span.range;
        let end_us = range.end().micros();
        let source_us = admitted.get(&real).map(|m| m.duration.micros()).ok_or(
            MediaRejection::Undecodable {
                detail: format!("{}: no admitted duration for this source", real.display()),
            },
        )?;
        if end_us > source_us {
            return Err(MediaRejection::CutOutOfRange {
                cut: index,
                start_us: range.start().micros(),
                end_us,
                source_us,
            });
        }
    }
    // Integer, not the `f32` sum the wire format carries: the two are allowed
    // to differ by rounding, and this is the one a caller must compare against.
    let map = edl
        .interval_map()
        .map_err(|e| MediaRejection::CutRange { cut: 0, error: e })?;
    Ok(AdmittedEdl {
        sources,
        output: map.output_duration(),
    })
}

/// The path a cut names. Matched here rather than added to `Cut` so the
/// editor crate keeps no knowledge of admission.
pub(super) fn cut_path(cut: &Cut) -> &str {
    match &cut.kind {
        CutKind::Source { source_path } => source_path,
        CutKind::Broll { broll_path } => broll_path,
    }
}

#[cfg(test)]
mod tests;
