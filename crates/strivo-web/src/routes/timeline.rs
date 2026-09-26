//! The timeline's one HTTP surface: apply an edit, get the compiled plan back.
//!
//! ## Why a route at all
//!
//! Because the timeline is a **view over the compiled plan**, and the only
//! thing that produces a plan is todo 23's compiler. An edit that the browser
//! applied to its own copy of the clip list would be a second description of
//! the composition — the exact preview/render split this crate was built to
//! end. So the surface sends an *intent* (`trim`, `split`, `ripple`, `slip`,
//! `link`) and this route answers with the plan the compiler made of it.
//!
//! That does not put a server round-trip in the preview path: playback decodes
//! in the browser (Mediabunny + WebCodecs), and a drag's ghost is computed
//! locally from the plan the surface already holds. Only the *commit* of an
//! edit comes here, and what comes back is the same shape the surface already
//! had, with the edit applied.
//!
//! ## Staleness
//!
//! The body carries the revision the client believes it is editing. A request
//! whose `base_revision` is not the request's own `revision` is refused with
//! `409` and the two names, rather than applied to a composition the client has
//! never seen. This is the same rule `routes::workspace` applies to a selected
//! revision, for the same reason: two views must not be looking at different
//! work.
//!
//! Creator-only, because the media this reads lives behind the Creator store
//! and a PVR install has no composition to edit.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use strivo_editor::render::{MediaTime, RenderError, RenderRequest};
use strivo_editor::timeline::{Clamp, Edit, SourceLibrary, Timeline, TimelineError};

use crate::problem::Problem;
use crate::server::AppState;

fn authed(headers: &HeaderMap, state: &AppState) -> Result<(), StatusCode> {
    crate::routes::login::check_dual(headers, &state.api_key, &state.session_secret)
}

/// One source's declared length, in the integer unit todo 22 defines. A `f64`
/// seconds field here would be the second time representation the whole model
/// exists to remove, so the wire unit is microseconds and is named.
#[derive(Deserialize)]
struct SourceBody {
    source: String,
    length_us: i64,
}

#[derive(Deserialize)]
struct ApplyBody {
    /// The request the client is editing.
    request: RenderRequest,
    /// The revision the client believes it holds. Refused when it is not the
    /// request's own revision.
    base_revision: String,
    /// Declared length per source, so an edit can be clamped to a real end.
    sources: Vec<SourceBody>,
    /// Absent for a plain read of the plan as it stands.
    edit: Option<Edit>,
}

/// Map a refusal onto the status that names what went wrong.
///
/// The stable token is the first word of `detail`, the same discipline
/// `routes::workspace` and `routes::media_input` use: `Problem` is shared by
/// every handler and its envelope is not this module's to change.
fn refuse(error: TimelineError) -> Response {
    let status = match &error {
        TimelineError::NoClips | TimelineError::UnknownSource { .. } | TimelineError::Time(_) => {
            StatusCode::BAD_REQUEST
        }
        TimelineError::OutOfSource { .. } => StatusCode::UNPROCESSABLE_ENTITY,
        TimelineError::NoClip { .. }
        | TimelineError::ZeroLength { .. }
        | TimelineError::SplitOutsideClip { .. }
        | TimelineError::RippleOutsideComposition { .. }
        | TimelineError::SelfLink { .. } => StatusCode::CONFLICT,
    };
    Problem::new(status, format!("{}: {error}", error.code())).into_response()
}

/// A plan the compiler would not produce is a `422`, not a `500`: the request
/// described a composition that cannot exist, which is the client's answer to
/// fix, and naming it as an internal error would send the triage the wrong way.
fn refuse_plan(error: RenderError) -> Response {
    Problem::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        format!("timeline/uncompilable: {error}"),
    )
    .into_response()
}

/// Build the declared-length library, refusing a negative or a source named
/// twice. Both are the client's arithmetic to fix and neither has a sensible
/// default, so neither gets one.
fn library(sources: &[SourceBody]) -> Result<SourceLibrary, Problem> {
    let mut library = SourceLibrary::new();
    for entry in sources {
        let length = MediaTime::from_micros(entry.length_us)
            .map_err(|e| Problem::bad_request(format!("timeline/source: {e}")))?;
        if library.length_of(&entry.source).is_some() {
            return Err(Problem::bad_request(format!(
                "timeline/source: {:?} is declared twice",
                entry.source
            )));
        }
        library = library.with(&entry.source, length);
    }
    Ok(library)
}

/// `POST /api/v1/creator/timeline/apply` — the one read and the one write.
///
/// One handler for both because they are the same operation with and without an
/// edit: "compile this request". A separate read route would be a second place
/// for the two to disagree about what a plan is.
async fn apply(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<ApplyBody>,
) -> Response {
    if let Err(status) = authed(&headers, &state) {
        return Problem::new(status, "authentication required").into_response();
    }
    if body.base_revision != body.request.revision {
        return Problem::conflict(format!(
            "timeline/stale: the client is editing revision {:?} but posted {:?}",
            body.request.revision, body.base_revision
        ))
        .into_response();
    }
    let library = match library(&body.sources) {
        Ok(library) => library,
        Err(problem) => return problem.into_response(),
    };
    let mut timeline = match Timeline::new(body.request, library) {
        Ok(timeline) => timeline,
        Err(e) => return refuse(e),
    };
    let clamps: Vec<Clamp> = match &body.edit {
        None => Vec::new(),
        Some(edit) => match timeline.apply(edit) {
            Ok(clamps) => clamps,
            Err(e) => return refuse(e),
        },
    };
    let plan = match timeline.plan() {
        Ok(plan) => plan,
        Err(e) => return refuse_plan(e),
    };
    let snap_points = timeline
        .snap_points()
        .unwrap_or_else(|_| vec![MediaTime::ZERO]);
    Json(json!({
        "plan": plan,
        // The request as the server holds it after the edit, so a second edit
        // builds on the composition the server decided rather than on the one
        // the browser remembers sending.
        "request": timeline.request(),
        "clamps": clamps,
        "snap_points_us": snap_points.iter().copied().map(MediaTime::micros).collect::<Vec<_>>(),
    }))
    .into_response()
}

pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/creator/timeline/apply", post(apply))
}

#[cfg(test)]
mod tests {
    use super::apply;
    use crate::server::AppState;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::Router;
    use serde_json::json;
    use strivo_editor::render::example;
    use tower::ServiceExt;

    fn state() -> AppState {
        crate::server::test_state("k")
    }

    fn app() -> Router {
        Router::new()
            .route("/api/v1/creator/timeline/apply", axum::routing::post(apply))
            .with_state(state())
    }

    async fn post(body: serde_json::Value) -> axum::response::Response {
        app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/creator/timeline/apply")
                    .header("content-type", "application/json")
                    .header("x-api-key", "k")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    /// A well-formed read: the example revision over a declared source.
    fn read() -> serde_json::Value {
        json!({
            "request": example::request(),
            "base_revision": "rev-7",
            "sources": [{
                "source": strivo_editor::time::SourceRef::from_locator(example::SOURCE).as_str(),
                "length_us": 10_000_000i64,
            }],
        })
    }

    #[tokio::test]
    async fn a_read_returns_the_compiled_plan_and_its_snap_points() {
        let response = post(read()).await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["plan"]["revision"], "rev-7");
        assert_eq!(body["request"]["revision"], "rev-7");
        assert_eq!(body["plan"]["output_duration"], 2_500_000);
        assert_eq!(body["clamps"], json!([]));
        let points = body["snap_points_us"].as_array().unwrap();
        assert!(
            points.contains(&json!(0)) && points.contains(&json!(1_000_000)),
            "the clip boundary is a snap point: {points:?}"
        );
    }

    /// The stale-state probe: an edit built on a revision the server did not
    /// compile is refused, not applied to a composition the client never saw.
    #[tokio::test]
    async fn an_edit_against_a_superseded_revision_is_refused_as_stale() {
        let mut body = read();
        body["base_revision"] = json!("rev-6");
        let response = post(body).await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let problem: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(
            problem["detail"]
                .as_str()
                .unwrap()
                .starts_with("timeline/stale"),
            "{problem}"
        );
    }

    #[tokio::test]
    async fn a_trim_past_the_source_end_is_applied_clamped_and_reported() {
        let mut body = read();
        body["edit"] = json!({"op": "trim", "clip": 0, "edge": "out", "to": 10_000_001});
        let response = post(body).await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            body["clamps"],
            json!([{"clip": 0, "edge": "out", "requested_micros": 10_000_001, "applied_micros": 10_000_000}]),
        );
        // And the plan that came back is the clamped one, not the asked-for one.
        assert_eq!(body["plan"]["output_duration"], 11_500_000);
        // The request comes back clamped too, so the next edit compounds on
        // what the server actually holds.
        assert_eq!(
            body["request"]["clips"][0]["span"]["range"]["end"],
            10_000_000
        );
    }

    #[tokio::test]
    async fn an_edit_naming_a_clip_that_does_not_exist_is_a_conflict() {
        let mut body = read();
        body["edit"] = json!({"op": "slip", "clip": 9, "by": 1000});
        let response = post(body).await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn a_request_whose_track_names_a_cut_that_does_not_exist_is_unprocessable() {
        let mut body = read();
        body["request"]["tracks"][0]["clips"] = json!([9]);
        let response = post(body).await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let problem: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(
            problem["detail"]
                .as_str()
                .unwrap()
                .starts_with("timeline/uncompilable"),
            "{problem}"
        );
    }

    #[tokio::test]
    async fn a_source_declared_twice_is_refused_rather_than_silently_replaced() {
        let mut body = read();
        let source = body["sources"][0]["source"].clone();
        body["sources"] = json!([
            {"source": source, "length_us": 10_000_000i64},
            {"source": source, "length_us": 20_000_000i64},
        ]);
        let response = post(body).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn a_negative_declared_length_is_refused_rather_than_clamped_to_zero() {
        let mut body = read();
        body["sources"][0]["length_us"] = json!(-1);
        let response = post(body).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    /// A body that is not the shape at all must be a client error, not a panic
    /// and not a 500: a timeline that 500s on a malformed drag is a timeline
    /// whose failure mode is invisible.
    #[tokio::test]
    async fn a_malformed_body_is_a_client_error() {
        let response = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/creator/timeline/apply")
                    .header("content-type", "application/json")
                    .header("x-api-key", "k")
                    .body(Body::from("{\"request\":42}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            response.status().is_client_error(),
            "got {}",
            response.status()
        );
    }
}
