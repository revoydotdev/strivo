//! OBS-style multitrack import: the web surface.
//!
//! Report item C02 (`docs/CREATOR-EDITION-REPORT-2026-09-07.md:281`). The
//! bookkeeping is `strivo_research::creator::multitrack`; the measurement is
//! [`multitrack_probe`]. This module is the bridge, and it is short on purpose —
//! the three things it must get right are all inherited rather than written here.
//!
//! ## The same gate, because import is already a second way in
//!
//! [`admit_all`] is todo 33's function, calling [`media_input::admit_path`] —
//! the same gate `admit_edl_at` calls for every cut of every edit — and it
//! returns [`PathRejection`] unchanged. A multitrack import is a *third* way in,
//! so the same rules have to hold for it: a `..` hop out of the chosen root, a
//! `file://` source, a directory, a zero-byte file and a fifo produce todo 25's
//! codes, because the code that produced them is the same code.
//! `tests/multitrack_boundary.rs` drives both surfaces over the same hostile
//! inputs and asserts the codes agree, so the claim is checked rather than
//! asserted — and it opens with a control that admits a real four-track session,
//! because a gate that refuses everything satisfies every "nothing unsafe was
//! imported" assertion for free.
//!
//! ## What the response states, and what it does not
//!
//! Per track: the **named** role, the **named** timebase, the offset, the clock
//! segments, and every gap with the bound that follows it. Session-wide:
//! `worst_drift_micros`, the largest bound any answer in the session carries.
//!
//! What it does not claim is a decoded-sample position. Alignment is exact in
//! **container-timeline** terms; a `-c copy` seek lands on the nearest keyframe,
//! so each gap carries the keyframe distance the probe measured. A caller that
//! needs sample accuracy has the number.
//!
//! Muting, soloing, gain and the render itself are todo 44's: they go through
//! the single `RenderPlan` from todo 23, and a second gain path written here
//! would be exactly the preview/render divergence that todo exists to end. What
//! this surface delivers is the track set that plan mutes — each track its own
//! `(source, stream_index)`, addressable and independently extractable with the
//! existing `-c copy` path.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use strivo_research::creator::{
    import_multitrack, MultitrackRequest, MultitrackSession, ProjectId,
};

use crate::problem::Problem;
use crate::routes::import::admit_all;
use crate::routes::multitrack_probe::probe_session;
use crate::routes::settings::check_key;
use crate::server::AppState;

#[derive(Debug, Deserialize)]
struct Body {
    project: uuid::Uuid,
    /// The folder `path` is resolved within. It must stay inside it.
    root: String,
    /// The session container, as ffmpeg would see it — which is the whole
    /// reason the body carries one string and not a path.
    path: String,
    /// The user's labels, one per audio stream in container order, overriding
    /// what the container's own track names were read to mean.
    #[serde(default)]
    labels: Option<Vec<String>>,
}

async fn creator_multitrack_import(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<Body>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    let project = ProjectId::from_uuid(body.project);
    let admitted = match admit_all(std::path::Path::new(&body.root), &[body.path]) {
        Ok(files) => files,
        Err(rejection) => return rejection.to_problem().into_response(),
    };
    let store = match strivo_research::creator::Store::open(crate::creator_schema::creator_db_path())
    {
        Ok(store) => store,
        Err(e) => {
            return Problem::internal(format!("creator store: {e}")).into_response();
        }
    };
    // Checked here for the same reason todo 33's route checks it: the refusal is
    // the caller's mistake and names the project, where a foreign-key failure
    // from the first insert would name neither.
    match store.project(project) {
        Ok(Some(_)) => {}
        Ok(None) => {
            return Problem::not_found(format!("multitrack/no-such-project: {project}"))
                .into_response();
        }
        Err(e) => return Problem::internal(format!("multitrack import: {e}")).into_response(),
    }
    // The path refusal already answered. Only a media *fact* is left, and it is
    // a client decision: this file is not a multitrack session, or is too long.
    // The path was already admitted; what is left is a statement about the
    // file's contents, which is a decision the caller made wrongly.
    let probe = match probe_session(&admitted[0]).await {
        Ok(probe) => probe,
        Err(rejection) => {
            return Problem::bad_request(format!("{}: {}", rejection.code(), rejection))
                .into_response();
        }
    };
    let scratch = strivo_core::config::AppConfig::data_dir().join("scratch");
    let at = chrono::Utc::now().to_rfc3339();
    let session = import_multitrack(
        &store,
        &MultitrackRequest {
            project,
            container: &admitted[0],
            tracks: &probe.tracks,
            picture: probe.picture.as_ref(),
            labels: body.labels.as_deref(),
            scratch_root: &scratch,
            at: &at,
        },
    );
    match session {
        Ok(session) => (
            StatusCode::CREATED,
            Json(render(&session, &probe.duration_us, &body.root)),
        )
            .into_response(),
        // A refusal is a decision the caller made wrongly, so it is a 400 with
        // the stable code the research module defines. A store failure is not.
        Err(e) if e.code() != "multitrack/internal" => {
            Problem::bad_request(format!("{}: {}", e.code(), e)).into_response()
        }
        Err(e) => Problem::internal(format!("multitrack import: {e}")).into_response(),
    }
}

/// The response body. Every figure is a measurement or a derived bound; nothing
/// here is a promise the probe did not make.
fn render(session: &MultitrackSession, duration_us: &i64, root: &str) -> serde_json::Value {
    json!({
        "job": session.manifest.job.to_string(),
        "manifest": session.manifest.artifact.to_string(),
        "capture_session": session.capture.to_string(),
        "source": session.manifest.entries[0].outcome.outcome_asset().id.to_string(),
        "fingerprint": session.manifest.entries[0].fingerprint,
        "imported": session.manifest.imported(),
        "duplicates": session.manifest.duplicates(),
        "root": root,
        "timeline": {
            "origin_us": session.timeline.origin.micros(),
            "length_us": session.timeline.length.micros(),
            "container_duration_us": duration_us,
            "gaps": session.timeline.gaps.iter().map(gap).collect::<Vec<_>>(),
        },
        "reference": {
            "stream": session.reference.stream_index,
            "label": session.reference.label,
            "mixed": session.reference.mixed,
        },
        // The figure a UI shows next to the tracks. It is the largest bound any
        // answer in this session carries, not a tolerance somebody chose.
        "worst_drift_micros": session.worst_drift_micros(),
        "tracks": session.tracks.iter().map(|t| json!({
            "stream": t.stream_index,
            "label": t.label,
            "measured_label": t.measured_label,
            "role": t.role.as_str(),
            // Whether the role was determined or merely defaulted to `other`.
            "identified": t.identified,
            // Ticks per second, exact: the track's own clock, not a description.
            "time_base": {"num": t.time_base.rate().0, "den": t.time_base.rate().1},
            "source": t.source.to_string(),
            "offset_us": t.offset.micros(),
            "gaps": t.gaps.iter().map(gap).collect::<Vec<_>>(),
            "segments": t.aligned.segments().iter().map(|s| json!({
                "wall_start_ms": s.wall_start_ms,
                "media_start_us": s.media_start.micros(),
                "media_len_us": s.media_len.micros(),
                "rate": {"num": s.rate_num, "den": s.rate_den},
            })).collect::<Vec<_>>(),
            // `false` means the probe did not read this stream to its end, so a
            // hole past that point was never looked for.
            "complete": t.complete,
        })).collect::<Vec<_>>(),
    })
}

fn gap(g: &strivo_research::creator::MediaGap) -> serde_json::Value {
    json!({
        "at_us": g.at.micros(),
        "len_us": g.len.micros(),
        // Measured, not assumed: how far a `-c copy` seek can land from the far
        // side. This is the number that makes the alignment claim precise.
        "keyframe_distance_us": g.keyframe_distance.micros(),
        "drift_after_micros": g.drift_after_micros,
    })
}

/// The multitrack routes. Mounted only by a Creator build; see `routes::api`.
pub fn router() -> Router<AppState> {
    Router::new().route(
        "/api/v1/creator/multitrack/import",
        post(creator_multitrack_import),
    )
}
