//! OBS-style multitrack import: identify every track, and keep them
//! synchronised.
//!
//! Report item C02 (`docs/CREATOR-EDITION-REPORT-2026-09-07.md:281`) and the
//! seam `docs/STRATEGY-NVIVO-RIVERSIDE.md:110-115` left open. A streamer
//! recording in OBS's multitrack mode ends up with one container holding a
//! picture and four to six audio stems — mic, game, voice chat, music and the
//! downmixed fallback — and this module turns that into a track set a person can
//! mute, solo and export without re-deriving anything. The vocabulary is
//! [`super::multitrack_model`]; the arithmetic is [`super::multitrack_align`];
//! the storage is [`super::multitrack_session`].
//!
//! ## It extends the import surface; it is not a second one
//!
//! Registration goes through [`super::import::import_files`] on an
//! [`AdmittedFile`], so there is one admission gate (todo 25), one content
//! identity and one deduplication rule (todo 33). A multitrack session is
//! imported as what it is — one more file — and everything this module adds sits
//! on top of the rows that import wrote. There is no `PathBuf` entry point here
//! either, so no client-named path can skip the gate.
//!
//! One container is **one** source. The stems are not four recordings, and
//! writing four rows would either mean rows outside the import path or a second
//! content identity for bytes that are the same file. A track is addressed by
//! `(source, stream_index)`, which is what a container stream already is.
//!
//! ## Bounds that are refusals, not adjectives
//!
//! [`MAX_TRACKS`] is checked before anything is registered. An unbounded track
//! count is not a missing feature, it is a way to make a server enumerate a
//! client's file; and OBS sessions are not always well-formed, so a truncated
//! session, a track with no content, a hole past the end of a track and a
//! duplicated stream index are each a typed [`MultitrackRejection`] rather than
//! a panic in a loader that walks someone else's file.

use std::collections::BTreeSet;

use strivo_editor::time::MediaTime;
use strivo_multitrack::{infer_kind, TrackKind};

use super::ids::CaptureSessionId;
use super::import::{import_files, ImportRequest};
use super::multitrack_align::{self, Aligned};
use super::multitrack_model::{
    IdentifiedTrack, MultitrackError, MultitrackRejection, MultitrackRequest, MultitrackSession,
    ReferenceTrack, TrackMeasurement, MAX_TRACKS,
};
use super::multitrack_session;
use super::store::Store;
use crate::{ResearchError, Result};

/// The role a track's label resolves to, and whether it was determined.
///
/// A name that matches nothing resolves to `Other` and is *not* identified, so
/// `Other` is never presented as knowledge. A user label always wins, and an
/// empty one falls through to the container's own name rather than overriding
/// it with nothing.
fn resolve_role(measured: &str, user: Option<&str>) -> (TrackKind, bool, String, String) {
    match user {
        Some(label) if !label.trim().is_empty() => {
            let role = infer_kind(label);
            (
                role,
                role != TrackKind::Other,
                label.into(),
                measured.into(),
            )
        }
        _ => {
            let role = infer_kind(measured);
            (
                role,
                role != TrackKind::Other,
                measured.into(),
                measured.into(),
            )
        }
    }
}

/// Check the measurements before anything is registered, so a refusal leaves no
/// state at all rather than half a session.
fn validate(request: &MultitrackRequest<'_>) -> std::result::Result<(), MultitrackRejection> {
    if request.tracks.is_empty() {
        return Err(MultitrackRejection::NoTracks);
    }
    if request.tracks.len() > MAX_TRACKS {
        return Err(MultitrackRejection::TooManyTracks {
            found: request.tracks.len(),
            limit: MAX_TRACKS,
        });
    }
    if let Some(labels) = request.labels {
        if labels.len() != request.tracks.len() {
            return Err(MultitrackRejection::LabelCount {
                labels: labels.len(),
                tracks: request.tracks.len(),
            });
        }
    }
    if let Some(picture) = request.picture {
        check_stream(picture)?;
    }
    let mut seen = BTreeSet::new();
    for track in request.tracks {
        if !seen.insert(track.stream_index) {
            return Err(MultitrackRejection::DuplicateStream {
                stream: track.stream_index,
            });
        }
        check_stream(track)?;
    }
    Ok(())
}

/// One stream's own invariants: some content, and holes that fall inside it
/// without overlapping. The timebase needs no check — `TimeBase::from_rate`
/// refuses a non-positive rational at construction, so a
/// [`TrackMeasurement`] cannot carry one.
fn check_stream(track: &TrackMeasurement) -> std::result::Result<(), MultitrackRejection> {
    if track.coverage == MediaTime::ZERO {
        return Err(MultitrackRejection::EmptyTrack {
            stream: track.stream_index,
        });
    }
    let end = track.start.micros() + track.coverage.micros();
    let mut cursor = track.start.micros();
    for gap in &track.gaps {
        let at = gap.at.micros();
        let finish = at + gap.len.micros();
        if at < cursor || finish > end {
            return Err(MultitrackRejection::GapOutsideTrack {
                stream: track.stream_index,
                at_micros: at,
            });
        }
        cursor = finish;
    }
    Ok(())
}

/// Import one OBS-style multitrack session: identify every track, align them to
/// one timeline, and register the container through the ordinary import path.
///
/// **Every refusal happens before any row is written**, so a session that is
/// refused leaves the project exactly as it found it — no source, no job, no
/// manifest. The write order after that is import's: the manifest is the
/// artifact a durable job completes with, and the identification is written onto
/// the rows import created.
pub fn import_multitrack(
    store: &Store,
    request: &MultitrackRequest<'_>,
) -> std::result::Result<MultitrackSession, MultitrackError> {
    validate(request)?;

    let files = std::slice::from_ref(request.container);
    let manifest = import_files(
        store,
        &ImportRequest {
            project: request.project,
            files,
            scratch_root: request.scratch_root,
            at: request.at,
        },
    )?;
    let container = manifest.entries[0].outcome.outcome_asset().id;

    // The session's origin is the earliest position any stream reaches, so no
    // track can end up at a negative session position — which `MediaTime`
    // refuses, and which a fixed origin of zero would have produced.
    let origin = request
        .picture
        .into_iter()
        .chain(request.tracks)
        .map(|m| m.start)
        .min()
        .unwrap_or(MediaTime::ZERO);

    let picture = request.picture.map(|m| place(m, origin)).transpose()?;
    let aligned: Vec<Aligned> = request
        .tracks
        .iter()
        .map(|m| place(m, origin))
        .collect::<Result<_>>()?;

    let reference_index = reference_of(request.tracks);
    let reference_measurement = &request.tracks[reference_index];
    let reference_aligned = &aligned[reference_index];
    let (role, _, label, _) = resolve_role(
        &reference_measurement.label,
        request.labels.map(|l| l[reference_index].as_str()),
    );

    let timeline = multitrack_align::timeline(origin, reference_aligned, picture.as_ref());
    let mut resolved: Vec<(String, bool, String, String)> = Vec::with_capacity(aligned.len());
    let tracks = request
        .tracks
        .iter()
        .zip(aligned)
        .enumerate()
        .map(|(i, (measured, aligned))| {
            let (role, identified, label, measured_label) =
                resolve_role(&measured.label, request.labels.map(|l| l[i].as_str()));
            resolved.push((
                role.as_str().to_string(),
                identified,
                label.clone(),
                measured_label.clone(),
            ));
            Ok(IdentifiedTrack {
                stream_index: measured.stream_index,
                label,
                measured_label,
                role,
                identified,
                time_base: measured.time_base,
                source: container,
                offset: aligned.track.offset,
                aligned: aligned.track.aligned,
                gaps: aligned.track.gaps,
                complete: measured.complete,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    // The source row carries what a *source* is: a multitrack recording, and how
    // long it runs. Not a role — a container has four tracks and a row has one
    // `track_type` column, so the first draft's per-track write had the fourth
    // track silently overwrite the first three. The per-track facts go in the
    // capture session, which has room for them.
    let length = timeline.length;
    store
        .annotate_source_media(container, "multitrack", length)
        .map_err(|e| ResearchError::Validation(e.to_string()))?;

    // Computed once, from the tracks and the timeline together, so the stored
    // figure and the returned one cannot be two numbers.
    let worst = multitrack_align::worst_bound(
        tracks
            .iter()
            .map(|t| (&t.aligned, t.gaps.as_slice()))
            .chain(std::iter::once((&timeline.clock, timeline.gaps.as_slice()))),
    );
    let attrs = multitrack_session::document(
        origin,
        length,
        reference_measurement.stream_index,
        role == TrackKind::Mixed,
        worst,
        request.tracks,
        &resolved,
    );
    let capture = CaptureSessionId::generate();
    store.insert_capture_session(&multitrack_session::session_row(
        capture,
        request.project,
        container,
        request.at,
        &attrs,
    ))?;

    Ok(MultitrackSession {
        capture,
        attributes: attrs,
        timeline,
        reference: ReferenceTrack {
            stream_index: reference_measurement.stream_index,
            label,
            mixed: role == TrackKind::Mixed,
        },
        manifest,
        tracks,
    })
}

/// Which track the session's timeline is read through: the downmixed fallback
/// when there is one, else the track covering the most timeline, ties going to
/// the lowest stream index so the choice is a function of the measurements
/// rather than of their order.
/// Place one stream. A [`TimeError`] here would be an overflow the validated
/// measurements cannot produce, so it is surfaced as a research error rather
/// than swallowed.
fn place(m: &TrackMeasurement, origin: MediaTime) -> Result<Aligned> {
    multitrack_align::align(m.start, m.coverage, &m.gaps, origin)
        .map_err(|e| ResearchError::Validation(e.to_string()))
}

fn reference_of(tracks: &[TrackMeasurement]) -> usize {
    tracks
        .iter()
        .position(|t| infer_kind(&t.label) == TrackKind::Mixed)
        .unwrap_or_else(|| {
            tracks
                .iter()
                .enumerate()
                .max_by_key(|(i, t)| (t.coverage.micros(), std::cmp::Reverse(*i)))
                .map_or(0, |(i, _)| i)
        })
}
