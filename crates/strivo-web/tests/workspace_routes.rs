//! The project workspace over the **real** router: one project, one selected
//! revision, and the same answer for every view.
//!
//! ## Why an integration test and not a `#[cfg(test)]` module
//!
//! Two reasons, and the second is the load-bearing one.
//!
//! 1. It drives `server::build_router` — the router the binary serves — rather
//!    than a hand-assembled one, so "the route is mounted" is asserted rather
//!    than assumed. A workspace module nothing merged would pass a unit test.
//! 2. `strivo_web::routes::plugins`' own tests mutate `XDG_DATA_HOME` under a
//!    private `ENV_GUARD` **in the `--lib` binary**. A second module mutating
//!    the same variable under a *different* mutex interleaves with it, and the
//!    symptom is a test that passes alone and fails in the full run — which is
//!    what happened the first time this file's suite was written inside
//!    `routes/workspace.rs`. Each integration test file is its own process, so
//!    `common::shared_xdg_roots()` is uncontended, and `common::ensure_creator_db()`
//!    reproduces the one invariant that first-writer race needs.
//!
//! ## Non-emptiness first
//!
//! The capability-reachability test asserts the served list is non-empty and
//! equal to the descriptor's before it asserts anything about reachability. An
//! empty list satisfies "every capability is reachable from the project" for
//! free.

#![cfg(feature = "creator")]

mod common;

use std::collections::BTreeSet;

use axum::body::{to_bytes, Body};
use axum::http::{Method, Request, StatusCode};
use tower::ServiceExt;

use common::{shared_xdg_roots, API_KEY};
use strivo_research::creator::{
    CompositionRevision, CompositionRevisionId, Project, ProjectId, SourceAsset, SourceAssetId,
    Store,
};
use strivo_web::server::{build_router, AppState};

const NOW: &str = "2026-09-07T00:00:00Z";

fn state() -> AppState {
    shared_xdg_roots();
    common::ensure_creator_db();
    AppState::test_state(API_KEY)
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

/// A project holding two recordings and two edits, its workspace open and the
/// first edit selected. Seeded through the store because these tests are about
/// what the *route* reports, not about how rows are made.
fn seed() -> (ProjectId, CompositionRevisionId, CompositionRevisionId) {
    let store = Store::open(strivo_web::creator_schema::creator_db_path()).expect("store opens");
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
    for n in 0..2 {
        store
            .insert_source_asset(&SourceAsset {
                id: SourceAssetId::generate(),
                project_id: project.id,
                kind: "recording".into(),
                title: format!("stream {n}"),
                media_fingerprint: format!("sha256:{n}"),
                locator: None,
                origin: "capture".into(),
                rights_json: "{}".into(),
                track_type: "video".into(),
                codec: "h264".into(),
                duration: None,
                created_at: NOW.into(),
            })
            .expect("recording inserted");
    }
    let edit = |n: u32, parent: Option<CompositionRevisionId>| CompositionRevision {
        id: CompositionRevisionId::generate(),
        project_id: project.id,
        parent_revision_id: parent,
        revision: n,
        author: "operator".into(),
        timeline_json: format!("{{\"rev\":{n}}}"),
        output_profile: "long-form".into(),
        created_at: format!("2026-09-07T00:00:0{n}Z"),
    };
    let first = edit(1, None);
    let second = edit(2, Some(first.id));
    store
        .insert_composition_revision(&first)
        .expect("first edit inserted");
    store
        .insert_composition_revision(&second)
        .expect("second edit inserted");
    store
        .open_workspace(project.id, NOW)
        .expect("workspace open");
    store
        .select_revision(project.id, first.id, NOW)
        .expect("first edit selected");
    (project.id, first.id, second.id)
}

fn workspace_path(project: ProjectId) -> String {
    format!("/api/v1/creator/projects/{project}/workspace")
}

/// Given two views reading the same project over HTTP
/// When one changes the selected revision
/// Then the other sees the change — the property the signpost panes could not
/// have, because each held its own idea of what was open.
#[tokio::test]
async fn two_views_over_http_see_one_selected_revision() {
    let state = state();
    let (project, first, second) = seed();
    let path = workspace_path(project);

    let (status, view_a) = call(&state, Method::GET, &path, None).await;
    assert_eq!(status, StatusCode::OK, "view A reads the context");
    let (status, view_b) = call(&state, Method::GET, &path, None).await;
    assert_eq!(status, StatusCode::OK, "view B reads the context");
    assert_eq!(
        view_a["selected_revision"], view_b["selected_revision"],
        "both views report the same selected revision"
    );
    assert_eq!(view_a["selected_revision"], first.to_string());

    let (status, _) = call(
        &state,
        Method::POST,
        &format!("{path}/selection"),
        Some(serde_json::json!({ "revision_id": second.to_string() })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "view A switches");

    let (_, view_a) = call(&state, Method::GET, &path, None).await;
    let (_, view_b) = call(&state, Method::GET, &path, None).await;
    assert_eq!(
        view_a["selected_revision"], view_b["selected_revision"],
        "still one selection, not one per view"
    );
    assert_eq!(
        view_b["selected_revision"],
        second.to_string(),
        "and it is the new one"
    );
}

/// Given a view with work in progress
/// When it switches panes and then reads back
/// Then the project, the playhead, the selection, the active view and the
/// unsaved work all survived, and a second reader sees the same values —
/// because they are the same row.
#[tokio::test]
async fn work_in_progress_survives_a_pane_switch_and_a_second_reader() {
    let state = state();
    let (project, _first, _second) = seed();
    let path = workspace_path(project);

    let (status, _) = call(
        &state,
        Method::PUT,
        &path,
        Some(serde_json::json!({
            "playhead_us": 1_200_000i64,
            "selection_us": { "start": 1_000_000i64, "end": 4_000_000i64 },
            "active_view": "timeline",
            "unsaved_json": { "trim": "keep me" },
        })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "work in progress saved");

    for view in ["transcript", "inspector"] {
        let (status, _) = call(
            &state,
            Method::PUT,
            &path,
            Some(serde_json::json!({ "active_view": view })),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "switched to {view}");
    }

    let (status, read) = call(&state, Method::GET, &path, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        read["project_id"],
        project.to_string(),
        "the project survived"
    );
    assert_eq!(read["playhead_us"], 1_200_000i64, "the playhead survived");
    assert_eq!(
        read["selection_us"]["start"], 1_000_000i64,
        "the selection survived"
    );
    assert_eq!(read["active_view"], "inspector", "the last pane is current");
    assert_eq!(read["unsaved_work"]["dirty"], true, "still unsaved");
    assert!(
        read["unsaved_work"]["bytes"].as_u64().unwrap_or(0) > 0,
        "and the work is still there"
    );

    // A field the switch did not mention is left alone, not reset — which is
    // the difference between a patch and a per-view cache in patch's clothes.
    let (_, again) = call(&state, Method::GET, &path, None).await;
    assert_eq!(
        again["playhead_us"], read["playhead_us"],
        "a second reader agrees"
    );
    assert_eq!(
        again["selection_us"], read["selection_us"],
        "and about the range"
    );
}

/// Given a project context
/// When it is read
/// Then every capability the descriptor publishes is reachable from it, and the
/// served set is derived from that one table rather than a second list — the
/// machine-checkable form of "reachable from the project context rather than
/// from a signpost tab".
#[tokio::test]
async fn every_creator_capability_is_reachable_from_the_project_context() {
    let state = state();
    let (project, _first, _second) = seed();
    let (status, body) = call(&state, Method::GET, &workspace_path(project), None).await;
    assert_eq!(status, StatusCode::OK);

    let rows = body["capabilities"]
        .as_array()
        .expect("the context carries a capability list");
    assert!(!rows.is_empty(), "a Creator build publishes capabilities");
    let served: BTreeSet<&str> = rows.iter().filter_map(|r| r["key"].as_str()).collect();
    let descriptor: BTreeSet<&str> = strivo_web::capabilities::CAPABILITIES
        .iter()
        .map(|c| c.key.as_str())
        .collect();
    assert_eq!(served, descriptor, "one source of capabilities, not two");
    assert!(
        rows.iter()
            .all(|r| r["reachable_from"] == serde_json::json!("project")),
        "every capability is reachable from the project, not from a pane"
    );
    // Every Pro capability the entitlement gate consults is in there too, so a
    // Pro-only capability cannot be pane-only by omission.
    for pro in strivo_web::capabilities::pro_slugs() {
        assert!(served.contains(pro), "{pro} is reachable from the project");
    }
}
