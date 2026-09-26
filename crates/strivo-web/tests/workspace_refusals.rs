//! What the workspace refuses, over the real router.
//!
//! Split from `workspace_routes` because a refusal and a success are different
//! claims: every coexistence test there also passes against a route that
//! answers everything, and only the tests here notice that. Keeping them apart
//! means neither group is a decoration on the other.
//!
//! `seed()` is deliberately duplicated from `workspace_routes` rather than
//! shared through `tests/common`: a shared helper here would mean editing a
//! file every other test binary includes, for ~40 lines. That is the same
//! trade `tests/media_input_spawns.rs` made against `tests/media_pool.rs`.

#![cfg(feature = "creator")]

mod common;

use axum::body::{to_bytes, Body};
use axum::http::{Method, Request, StatusCode};
use tower::ServiceExt;

use common::API_KEY;
use strivo_research::creator::{
    CompositionRevision, CompositionRevisionId, Project, ProjectId, Store,
};
use strivo_web::server::{build_router, AppState};

const NOW: &str = "2026-09-07T00:00:00Z";

fn state() -> AppState {
    common::shared_xdg_roots();
    common::ensure_creator_db();
    AppState::test_state(API_KEY)
}

fn store() -> Store {
    Store::open(strivo_web::creator_schema::creator_db_path()).expect("store opens")
}

async fn call(
    state: &AppState,
    method: Method,
    path: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let builder = Request::builder()
        .method(method)
        .uri(path)
        .header("x-api-key", API_KEY);
    let req = match body {
        Some(v) => builder
            .header("content-type", "application/json")
            .body(Body::from(v.to_string()))
            .expect("request builds"),
        None => builder.body(Body::empty()).expect("request builds"),
    };
    let response = build_router(state.clone())
        .oneshot(req)
        .await
        .expect("router responds");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .expect("body reads");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

fn workspace_path(project: ProjectId) -> String {
    format!("/api/v1/creator/projects/{project}/workspace")
}

/// A project with one edit, an open workspace and that edit selected. A
/// composition revision has no foreign key to a source, so no recording is
/// planted: this file is about refusals, and a row it does not need is a row
/// whose absence could confuse a later reader.
fn seed() -> (ProjectId, CompositionRevisionId) {
    let store = store();
    let project = Project {
        id: ProjectId::generate(),
        name: "streamer".into(),
        description: String::new(),
        channel: "twitch".into(),
        series: String::new(),
        settings_json: "{}".into(),
        created_at: NOW.into(),
        updated_at: NOW.into(),
    };
    store.insert_project(&project).expect("project inserted");
    let edit = CompositionRevision {
        id: CompositionRevisionId::generate(),
        project_id: project.id,
        parent_revision_id: None,
        revision: 1,
        author: "operator".into(),
        timeline_json: "{\"rev\":1}".into(),
        output_profile: "long-form".into(),
        created_at: "2026-09-07T00:00:01Z".into(),
    };
    store
        .insert_composition_revision(&edit)
        .expect("edit inserted");
    store
        .open_workspace(project.id, NOW)
        .expect("workspace open");
    store
        .select_revision(project.id, edit.id, NOW)
        .expect("edit selected");
    (project.id, edit.id)
}

/// Given a project whose workspace has nothing selected
/// When the client asks for the context
/// Then the refusal carries a stable code rather than an invented revision.
#[tokio::test]
async fn a_project_with_no_selection_is_refused_with_a_code() {
    let state = state();
    let (project, _selected) = seed();
    let other = ProjectId::generate();
    store()
        .insert_project(&Project {
            id: other,
            name: "empty".into(),
            description: String::new(),
            channel: "twitch".into(),
            series: String::new(),
            settings_json: "{}".into(),
            created_at: NOW.into(),
            updated_at: NOW.into(),
        })
        .expect("other project inserted");
    store()
        .open_workspace(other, NOW)
        .expect("its workspace is open and empty");

    let (status, body) = call(&state, Method::GET, &workspace_path(other), None).await;
    assert_eq!(status, StatusCode::CONFLICT, "refused, not invented");
    let detail = body["detail"].as_str().unwrap_or_default();
    assert!(
        detail.starts_with("workspace/no-selection"),
        "the code is the first token, got {detail:?}"
    );
    // The seeded project still answers, so the refusal is about the empty one.
    let (status, _) = call(&state, Method::GET, &workspace_path(project), None).await;
    assert_eq!(status, StatusCode::OK, "the selected project is unaffected");
}

/// Given a revision belonging to a different project
/// When it is selected through the route
/// Then the selection is refused and this project's own selection is unchanged.
#[tokio::test]
async fn selecting_another_projects_revision_is_refused_over_http() {
    let state = state();
    let (project, selected) = seed();
    let other = ProjectId::generate();
    store()
        .insert_project(&Project {
            id: other,
            name: "other".into(),
            description: String::new(),
            channel: "twitch".into(),
            series: String::new(),
            settings_json: "{}".into(),
            created_at: NOW.into(),
            updated_at: NOW.into(),
        })
        .expect("other project inserted");
    let foreign = CompositionRevision {
        id: CompositionRevisionId::generate(),
        project_id: other,
        parent_revision_id: None,
        revision: 9,
        author: "someone-else".into(),
        timeline_json: "{}".into(),
        output_profile: "long-form".into(),
        created_at: "2026-09-07T00:00:09Z".into(),
    };
    store()
        .insert_composition_revision(&foreign)
        .expect("foreign edit inserted");

    let path = workspace_path(project);
    let (status, body) = call(
        &state,
        Method::POST,
        &format!("{path}/selection"),
        Some(serde_json::json!({ "revision_id": foreign.id.to_string() })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a foreign revision is refused"
    );
    let detail = body["detail"].as_str().unwrap_or_default();
    assert!(
        detail.starts_with("workspace/revision"),
        "with the stable code, got {detail:?}"
    );
    let (_, read) = call(&state, Method::GET, &path, None).await;
    assert_eq!(
        read["selected_revision"],
        selected.to_string(),
        "and this project's own selection is untouched"
    );
}

/// Given an id that is not a UUID
/// When it is used as a project id
/// Then it is a 400, not a 404 and not a panic.
#[tokio::test]
async fn a_malformed_project_id_is_a_bad_request() {
    let state = state();
    let (status, body) = call(
        &state,
        Method::GET,
        "/api/v1/creator/projects/not-a-uuid/workspace",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("ProjectId"),
        "and the detail names the id type, got {body}"
    );
}

/// Given a draft that is not a JSON object
/// When it is PUT as unsaved work
/// Then the whole write is refused and the previous state survives — the draft
/// and its unsaved flag are one statement, so they cannot be split.
#[tokio::test]
async fn a_non_object_draft_is_refused_and_nothing_moves() {
    let state = state();
    let (project, _selected) = seed();
    let path = workspace_path(project);
    let (status, _) = call(
        &state,
        Method::PUT,
        &path,
        Some(serde_json::json!({ "unsaved_json": { "cut": 1 } })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "an object draft is accepted"
    );

    let (status, body) = call(
        &state,
        Method::PUT,
        &path,
        Some(serde_json::json!({ "unsaved_json": ["not", "an", "object"] })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a list is not unsaved work"
    );
    let detail = body["detail"].as_str().unwrap_or_default();
    assert!(
        detail.starts_with("workspace/malformed"),
        "with the stable code, got {detail:?}"
    );
    let (_, read) = call(&state, Method::GET, &path, None).await;
    assert_eq!(
        read["unsaved_work"]["dirty"], true,
        "the earlier draft is intact"
    );
}

/// Given a request without credentials
/// When it asks for a project context
/// Then it is refused: the workspace holds a creator's unsaved work.
#[tokio::test]
async fn the_workspace_requires_authentication() {
    let state = state();
    let request = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/creator/projects/00000000-0000-0000-0000-000000000000/workspace")
        .body(Body::empty())
        .expect("request builds");
    let response = build_router(state)
        .oneshot(request)
        .await
        .expect("router responds");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
