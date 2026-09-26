//! The project workspace over HTTP: one project, one selected revision, and
//! the same answer for every view.
//!
//! `docs/CREATOR-EDITION-REPORT-2026-09-07.md:114-116` records Studio,
//! Analytics and Publish as *organisational shells* — panes that name a
//! capability instead of showing its work, several of them advertising a
//! `POST /api/v1/plugins/<slug>/<recording_id>` that does not exist. This module
//! is the other end of that: a project context every capability is reached
//! *from*, rather than a tab per capability.
//!
//! **Why the read is one endpoint and not one per view.** `GET …/workspace` is
//! the only read, and it answers with the workspace *and* the revision it
//! selects. Five per-view reads would be five chances to disagree, and the
//! disagreement is the defect the shells have. A view that wants a different
//! view's data asks this endpoint again; it does not keep a copy.
//!
//! **The response never carries a plugin slug to type, a filter string or a
//! JSON input to edit.** It carries ids, positions and the Creator capability
//! list the descriptor already publishes, so a creator sees their own work and
//! the advanced research tooling stays reachable as a perspective on the same
//! evidence rather than as a tab of its own.
//!
//! Creator-only, and compiled out of a PVR build rather than mounted inert: the
//! workspace table does not exist there, so a mounted route would advertise a
//! project context a PVR install cannot have.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use strivo_editor::time::{Interval, MediaTime};
use strivo_research::creator::{ProjectId, Store, View, WorkspaceError};

use crate::problem::Problem;
use crate::server::AppState;

/// Session cookie or API key. The workspace holds a creator's unsaved work, so
/// it is behind the same auth as every other Creator route; the `creator`
/// feature is what keeps it out of a PVR build, not a licence gate — the
/// workspace itself produces no capability's output, and gating a container on
/// one of the capabilities inside it is the conflation todo 20 removed.
fn authed(headers: &HeaderMap, state: &AppState) -> Result<(), StatusCode> {
    crate::routes::login::check_dual(headers, &state.api_key, &state.session_secret)
}

/// Every Creator capability, derived from the one descriptor table rather than
/// listed here. "A capability is reachable from the project context" is only
/// worth something if the set of capabilities has a single definition, and
/// `capabilities::CAPABILITIES` is it.
fn capabilities() -> Vec<serde_json::Value> {
    let mut rows = crate::capabilities::descriptor_json()["capabilities"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for row in &mut rows {
        if let Some(map) = row.as_object_mut() {
            map.insert("reachable_from".to_string(), json!("project"));
        }
    }
    rows
}

/// The workspace as a client sees it. A projection, not the row:
/// `selected_fingerprint` is a cache-validation field with no meaning to a
/// client, and the draft is reported as a byte count so a large transcript body
/// does not ride along with every playhead scrub.
fn project_context(
    workspace: &strivo_research::creator::Workspace,
    selected: Option<String>,
) -> serde_json::Value {
    json!({
        "project_id": workspace.project_id.to_string(),
        "active_view": workspace.active_view.as_str(),
        "views": View::all().iter().map(|v| v.as_str()).collect::<Vec<_>>(),
        "playhead_us": workspace.playhead.map(MediaTime::micros),
        "selection_us": workspace.range.map(|r| json!({
            "start": r.start().micros(),
            "end": r.end().micros(),
        })),
        "selected_revision": selected,
        "unsaved_work": { "bytes": workspace.draft.len(), "dirty": workspace.dirty },
        "updated_at": workspace.updated_at,
        "capabilities": capabilities(),
    })
}

/// Map a workspace refusal onto the status that names what went wrong.
///
/// The stable token is the first word of `detail`, not a new `Problem` field —
/// the same discipline `routes::media_input` uses, because `Problem` is shared
/// by every handler and its envelope is not this todo's to change. A test
/// asserts the code; the prose around it is free to change.
fn refuse(e: WorkspaceError) -> Response {
    let (status, code) = match &e {
        WorkspaceError::ProjectNotFound { .. } | WorkspaceError::NoWorkspace { .. } => {
            (StatusCode::NOT_FOUND, "workspace/project")
        }
        WorkspaceError::RevisionNotInProject { .. } | WorkspaceError::Dangling { .. } => {
            (StatusCode::NOT_FOUND, "workspace/revision")
        }
        WorkspaceError::NoSelection { .. } => (StatusCode::CONFLICT, "workspace/no-selection"),
        WorkspaceError::Stale { .. } => (StatusCode::CONFLICT, "workspace/stale"),
        WorkspaceError::MalformedDraft { .. } | WorkspaceError::UnknownView { .. } => {
            (StatusCode::BAD_REQUEST, "workspace/malformed")
        }
        WorkspaceError::Research(_) | WorkspaceError::Time(_) => {
            (StatusCode::INTERNAL_SERVER_ERROR, "workspace/internal")
        }
    };
    Problem::new(status, format!("{code}: {e}")).into_response()
}

/// Run one store operation off the async runtime, against the database the
/// Creator schema is actually created in.
///
/// Returns the [`WorkspaceError`] rather than a rendered `Response`: a
/// `Response` in the error position is 128 bytes of `Err`, and every handler
/// maps it through [`refuse`] anyway, so converting here would only mean
/// converting twice.
async fn blocking<T, F>(f: F) -> Result<T, WorkspaceError>
where
    F: FnOnce(&Store) -> Result<T, WorkspaceError> + Send + 'static,
    T: Send + 'static,
{
    let path = crate::creator_schema::creator_db_path();
    match tokio::task::spawn_blocking(move || {
        let store = Store::open(path).map_err(WorkspaceError::Research)?;
        f(&store)
    })
    .await
    {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(e)) => Err(e),
        Err(e) => Err(WorkspaceError::Research(
            strivo_research::ResearchError::Validation(format!("workspace worker: {e}")),
        )),
    }
}

/// `GET /api/v1/creator/projects/{id}/workspace` — the project context. The one
/// read every view makes, so two views cannot be looking at different work.
async fn get_workspace(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    if let Err(status) = authed(&headers, &state) {
        return Problem::new(status, "authentication required").into_response();
    }
    let project = match id.parse::<ProjectId>() {
        Ok(p) => p,
        Err(e) => return Problem::bad_request(e.to_string()).into_response(),
    };
    match blocking(move |store| store.resolve(project))
        .await
        .map_err(refuse)
    {
        Ok(resolved) => Json(project_context(
            &resolved.workspace,
            Some(resolved.composition.id.to_string()),
        ))
        .into_response(),
        Err(r) => r,
    }
}

/// `POST /api/v1/creator/projects/{id}/workspace` — open a project's workspace,
/// creating it the first time. A `GET` never creates one, so a first page view
/// cannot be confused with a project that has state.
async fn open_workspace(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    if let Err(status) = authed(&headers, &state) {
        return Problem::new(status, "authentication required").into_response();
    }
    let project = match id.parse::<ProjectId>() {
        Ok(p) => p,
        Err(e) => return Problem::bad_request(e.to_string()).into_response(),
    };
    match blocking(move |store| store.open_workspace(project, &chrono::Utc::now().to_rfc3339()))
        .await
        .map_err(refuse)
    {
        Ok(ws) => Json(project_context(&ws, None)).into_response(),
        Err(r) => r,
    }
}

/// `POST /api/v1/creator/projects/{id}/workspace/selection` — point the project
/// at one of its own edits. The only path that can change the selection, and it
/// is the one that checks the revision belongs to this project and records its
/// fingerprint.
async fn select_revision(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<SelectBody>,
) -> Response {
    if let Err(status) = authed(&headers, &state) {
        return Problem::new(status, "authentication required").into_response();
    }
    let (project, revision) = match (id.parse::<ProjectId>(), body.revision_id.parse()) {
        (Ok(p), Ok(r)) => (p, r),
        (Err(e), _) | (_, Err(e)) => return Problem::bad_request(e.to_string()).into_response(),
    };
    match blocking(move |store| {
        store.select_revision(project, revision, &chrono::Utc::now().to_rfc3339())
    })
    .await
    .map_err(refuse)
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(r) => r,
    }
}

/// `PUT /api/v1/creator/projects/{id}/workspace` — patch the view state.
///
/// Every field is optional and an omitted field is **left as it is**, not
/// reset: a view that has not loaded the selection range yet must not clear it
/// for every other view. The write itself is one statement, so the playhead,
/// the range, the active view and the unsaved draft cannot be observed
/// disagreeing with each other.
async fn put_workspace(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<ViewStateBody>,
) -> Response {
    if let Err(status) = authed(&headers, &state) {
        return Problem::new(status, "authentication required").into_response();
    }
    let project = match id.parse::<ProjectId>() {
        Ok(p) => p,
        Err(e) => return Problem::bad_request(e.to_string()).into_response(),
    };
    match blocking(move |store| {
        let now = chrono::Utc::now().to_rfc3339();
        let mut state = store.view_state(project)?;
        if let Some(us) = body.playhead_us {
            state.playhead = Some(MediaTime::from_micros(us)?);
        }
        if let Some(sel) = body.selection_us {
            state.range = Some(Interval::new(
                MediaTime::from_micros(sel.start)?,
                MediaTime::from_micros(sel.end)?,
            )?);
        }
        if let Some(view) = body.active_view.as_deref() {
            state.active_view = view.parse()?;
        }
        if let Some(draft) = body.unsaved_json {
            // An empty object is a discard, so the unsaved flag is a
            // consequence of the content rather than a second claim about it.
            if draft.as_object().is_some_and(serde_json::Map::is_empty) {
                return store.discard_draft(project);
            }
            state.draft = draft.to_string();
            state.dirty = true;
        }
        store.save_view_state(project, &state, &now)
    })
    .await
    .map_err(refuse)
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(r) => r,
    }
}

#[derive(Deserialize)]
struct SelectBody {
    revision_id: String,
}

#[derive(Deserialize)]
struct ViewStateBody {
    playhead_us: Option<i64>,
    selection_us: Option<SelectionBody>,
    active_view: Option<String>,
    unsaved_json: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct SelectionBody {
    start: i64,
    end: i64,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/creator/projects/{id}/workspace",
            get(get_workspace).post(open_workspace).put(put_workspace),
        )
        .route(
            "/api/v1/creator/projects/{id}/workspace/selection",
            axum::routing::post(select_revision),
        )
}
