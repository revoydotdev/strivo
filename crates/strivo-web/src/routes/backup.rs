//! Config/DB backup + restore (roadmap item 16), plus the durable recording
//! history endpoint (roadmap item 17). Split out of `routes::api`.
//!
//! Dep-free: a backup is a directory `data_dir/backups/<timestamp>/`
//! holding copies of config.toml + jobs.db.
//!
//! ## A restore is checked, not asserted
//!
//! The old restore copied two files and answered `{"restored": [...]}` — a
//! claim about bytes on disk, and a green success for a project whose sources
//! did not come back. Todo 40 (report item O01) makes that the one thing this
//! module must not do, so a backup now also carries a **census**: the identity
//! of every project and source it held, taken when it was made
//! (`strivo_research::creator::Census`). A restore reconciles the restored store
//! against that census and reports per source, so the answer to "did it work?"
//! is `report.complete` and never an assumption.
//!
//! The census is deliberately **identity, not paths** — ids and content
//! fingerprints, todo 33's rule — so a restore onto a machine where every
//! recording has moved still reconciles correctly, and a row that came back
//! carrying different bytes is its own outcome rather than a success.
//!
//! Two SQLite details that are load-bearing rather than incidental:
//!
//! * The Creator database is **checkpointed before it is copied**. In WAL mode
//!   its newest commits live in `research.db-wal`, so a byte copy without a
//!   checkpoint captures a database missing the projects written moments ago.
//! * On restore the stale `-wal`/`-shm` are removed and the file is renamed into
//!   place, because a `-wal` belonging to the *previous* file at the same path
//!   would be replayed over the restored one.
//!
//! Retention (`strivo_research::creator::retention`) is Creator-only and mounted
//! by [`creator_router`], so a PVR build has no statement from it and answers
//! those paths with a bare 404 — `content-length: 0`, no `content-type`, which
//! is how the edition boundary is told apart from a Creator `problem+json`
//! refusal.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;

use crate::routes::settings::{check_key, page_bounds, walk_dir_bytes, PageQuery};
use crate::server::AppState;

fn backups_dir() -> std::path::PathBuf {
    strivo_core::config::AppConfig::data_dir().join("backups")
}

/// Validate a user-supplied backup name: plain filename, no path parts.
/// Prevents traversal on the restore/download path.
fn safe_backup_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

async fn backup_create(headers: HeaderMap, State(state): State<AppState>) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    let name = chrono::Utc::now().format("%Y-%m-%dT%H-%M-%SZ").to_string();
    let dest = backups_dir().join(&name);
    if let Err(e) = std::fs::create_dir_all(&dest) {
        return crate::problem::Problem::internal(format!("create backup dir: {e}"))
            .into_response();
    }
    let cfg = state
        .config_path()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(strivo_core::config::AppConfig::config_path);
    let db = strivo_core::config::AppConfig::data_dir().join("jobs.db");
    let mut copied = Vec::new();
    if cfg.exists() {
        if let Err(e) = std::fs::copy(&cfg, dest.join("config.toml")) {
            return crate::problem::Problem::internal(format!("copy config: {e}")).into_response();
        }
        copied.push("config.toml");
    }
    if db.exists() {
        if let Err(e) = std::fs::copy(&db, dest.join("jobs.db")) {
            return crate::problem::Problem::internal(format!("copy jobs.db: {e}")).into_response();
        }
        copied.push("jobs.db");
    }

    // Creator-only: the project database and the census of what it held. A PVR
    // build compiles none of this, so a PVR backup is exactly the two files it
    // always was and its restore reports `report: null`. The listing is where
    // the counts live, so this response does not repeat them.
    #[cfg(feature = "creator")]
    match take_creator_census(&dest) {
        Ok(taken) => {
            if taken {
                // Both, because the census is the half that makes the restore
                // checkable: naming the database alone would leave a reader
                // thinking the backup can be verified when it cannot.
                copied.push("research.db");
                copied.push(strivo_research::creator::CENSUS_FILE);
            }
        }
        Err(problem) => return problem.into_response(),
    }
    (
        StatusCode::CREATED,
        Json(json!({ "name": name, "files": copied, "bytes": walk_dir_bytes(&dest).unwrap_or(0) })),
    )
        .into_response()
}

/// Copy the Creator database into `dest` and write the census beside it.
///
/// Checkpoints first — see the module doc for why a byte copy of the Creator
/// database without one is not a backup.
#[cfg(feature = "creator")]
fn take_creator_census(dest: &std::path::Path) -> Result<bool, crate::problem::Problem> {
    use strivo_research::creator::{Census, Store};

    let live = crate::creator_schema::creator_db_path();
    if !live.exists() {
        // A PVR-era install, or one that has never opened a project. Nothing to
        // record, and saying so is better than writing an empty census that
        // would later reconcile as "this backup held nothing".
        return Ok(false);
    }
    let store = Store::open(&live)
        .map_err(|e| crate::problem::Problem::internal(format!("open creator store: {e}")))?;
    let census = Census::snapshot(&store, &chrono::Utc::now().to_rfc3339())
        .map_err(|e| crate::problem::Problem::internal(format!("census: {e}")))?;
    store
        .checkpoint()
        .map_err(|e| crate::problem::Problem::internal(format!("checkpoint: {e}")))?;
    std::fs::copy(&live, dest.join("research.db"))
        .map_err(|e| crate::problem::Problem::internal(format!("copy research.db: {e}")))?;
    census
        .write(dest)
        .map_err(|e| crate::problem::Problem::internal(format!("write census: {e}")))?;
    Ok(true)
}

/// Project and source counts a backup's census recorded, for the listing.
///
/// A backup with no census reads as zero, which is what a PVR backup genuinely
/// is — and the restore path is the one that must not mistake "no record" for
/// "nothing wrong".
#[cfg(feature = "creator")]
fn census_counts(dir: &std::path::Path) -> (usize, usize) {
    use strivo_research::creator::Census;
    match Census::read(dir) {
        Ok(Some(census)) => (census.projects.len(), census.source_count()),
        _ => (0, 0),
    }
}

async fn backups_list(headers: HeaderMap, State(state): State<AppState>) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    let dir = backups_dir();
    let mut sets: Vec<serde_json::Value> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| {
            let p = e.path();
            let name = p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            #[cfg(feature = "creator")]
            let (projects, sources) = census_counts(&p);
            #[cfg(not(feature = "creator"))]
            let (projects, sources) = (0usize, 0usize);
            json!({
                "name": name,
                "bytes": walk_dir_bytes(&p).unwrap_or(0),
                "files": std::fs::read_dir(&p)
                    .into_iter().flatten().flatten()
                    .filter_map(|f| f.file_name().to_str().map(String::from))
                    .collect::<Vec<_>>(),
                "projects": projects,
                "sources": sources,
            })
        })
        .collect();
    // Newest first (names are sortable timestamps).
    sets.sort_by(|a, b| b["name"].as_str().cmp(&a["name"].as_str()));
    Json(json!({ "backups": sets })).into_response()
}

/// `GET /api/v1/backups/<name>/download` — stream the backup as a
/// tarball so the user can pull a copy off-box. Same name/safety rules
/// as restore; no auth bypass.
async fn backup_download(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    if !safe_backup_name(&name) {
        return crate::problem::Problem::bad_request("invalid backup name").into_response();
    }
    let dir = backups_dir().join(&name);
    if !dir.is_dir() {
        return crate::problem::Problem::not_found("backup not found").into_response();
    }
    // Archive work is blocking, so keep it off Tokio's request workers.
    // Conservative content-type makes browsers use "Save As".
    let archive_name = name.clone();
    let archive = tokio::task::spawn_blocking(move || {
        let child = std::process::Command::new("tar")
            .args([
                "-C",
                &dir.parent().unwrap_or(&dir).to_string_lossy(),
                "-czf",
                "-",
                &archive_name,
            ])
            .stdout(std::process::Stdio::piped())
            .spawn();
        let mut child = match child {
            Ok(child) => child,
            Err(error) => return Err(format!("tar: {error}")),
        };
        let mut out = Vec::new();
        use std::io::Read;
        if let Some(mut stdout) = child.stdout.take() {
            stdout
                .read_to_end(&mut out)
                .map_err(|error| error.to_string())?;
        }
        let status = child.wait().map_err(|error| error.to_string())?;
        if !status.success() {
            return Err(format!("tar exited with {status}"));
        }
        Ok::<_, String>(out)
    })
    .await;
    let out = match archive {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => return crate::problem::Problem::internal(e).into_response(),
        Err(e) => {
            return crate::problem::Problem::internal(format!("archive worker crashed: {e}"))
                .into_response()
        }
    };
    (
        [
            (axum::http::header::CONTENT_TYPE, "application/gzip"),
            (
                axum::http::header::CONTENT_DISPOSITION,
                Box::leak(
                    format!("attachment; filename=\"strivo-backup-{name}.tar.gz\"")
                        .into_boxed_str(),
                ),
            ),
        ],
        out,
    )
        .into_response()
}

async fn backup_restore(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    if !safe_backup_name(&name) {
        return crate::problem::Problem::bad_request("invalid backup name").into_response();
    }
    let src = backups_dir().join(&name);
    if !src.is_dir() {
        return crate::problem::Problem::not_found("backup not found").into_response();
    }
    let mut restored = Vec::new();
    let cfg_src = src.join("config.toml");
    if cfg_src.exists() {
        let cfg_dest = state
            .config_path()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(strivo_core::config::AppConfig::config_path);
        if let Err(e) = std::fs::copy(&cfg_src, cfg_dest) {
            return crate::problem::Problem::internal(format!("restore config: {e}"))
                .into_response();
        }
        restored.push("config.toml");
    }
    let db_src = src.join("jobs.db");
    if db_src.exists() {
        let db_dest = strivo_core::config::AppConfig::data_dir().join("jobs.db");
        if let Err(e) = std::fs::copy(&db_src, &db_dest) {
            return crate::problem::Problem::internal(format!("restore jobs.db: {e}"))
                .into_response();
        }
        restored.push("jobs.db");
    }

    // The project database, then the check. See the module doc for why the
    // write-ahead log is cleared and the file renamed rather than copied over.
    #[cfg(feature = "creator")]
    let report = {
        use strivo_research::creator::{reconcile_opt, Census, Store};
        let restored_db = match restore_creator_db(&src) {
            Ok(Some(path)) => {
                restored.push("research.db");
                Some(path)
            }
            // A backup with no project database is a PVR-era one. Not an error:
            // the census is what decides, and there is none.
            Ok(None) => None,
            Err(problem) => return problem.into_response(),
        };
        let Some(path) = restored_db else {
            return Json(json!({
                "restored": restored,
                "report": serde_json::Value::Null,
                "note": "This backup carries no project record, so nothing about projects was verified.",
            }))
            .into_response();
        };
        let census = match Census::read(&src) {
            Ok(census) => census,
            Err(e) => {
                return crate::problem::Problem::internal(format!("backup census: {e}"))
                    .into_response()
            }
        };
        let store = match Store::open(&path) {
            Ok(store) => store,
            Err(e) => {
                return crate::problem::Problem::internal(format!("restored store: {e}"))
                    .into_response()
            }
        };
        match reconcile_opt(&census, &store) {
            Ok(report) => serde_json::to_value(report).unwrap_or(serde_json::Value::Null),
            Err(e) => {
                return crate::problem::Problem::internal(format!("reconcile restore: {e}"))
                    .into_response()
            }
        }
    };
    #[cfg(not(feature = "creator"))]
    let report = serde_json::Value::Null;

    Json(json!({
        "restored": restored,
        "report": report,
        "note": "Restart the daemon for the restored config/DB to take effect.",
    }))
    .into_response()
}

/// Put the backup's project database back where the store opens it, returning
/// the live path — or `None` when the backup has none.
///
/// Staged copy, stale `-wal`/`-shm` removed, then renamed: a write-ahead log
/// left over from the *previous* file at this path would otherwise be replayed
/// over the restored one, which is a corruption the reconcile would then
/// report as missing projects.
#[cfg(feature = "creator")]
fn restore_creator_db(
    src: &std::path::Path,
) -> Result<Option<std::path::PathBuf>, crate::problem::Problem> {
    let from = src.join("research.db");
    if !from.exists() {
        return Ok(None);
    }
    let live = crate::creator_schema::creator_db_path();
    let staging = live.with_extension("db.restored");
    let io =
        |e: std::io::Error, what: &str| crate::problem::Problem::internal(format!("{what}: {e}"));
    std::fs::copy(&from, &staging).map_err(|e| io(e, "stage research.db"))?;
    for suffix in ["-wal", "-shm"] {
        let side = live.with_file_name(format!(
            "{}{suffix}",
            live.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("research.db")
        ));
        match std::fs::remove_file(&side) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(io(e, "clear research.db write-ahead log")),
        }
    }
    std::fs::rename(&staging, &live).map_err(|e| io(e, "put research.db in place"))?;
    Ok(Some(live))
}

// ── retention: which outputs a cleanup may drop ──────────────────────────
//
// Creator-only. Mounted by `creator_router` below, so a PVR build answers these
// paths with a bare 404 — no `creator` statement, no `retention` vocabulary.

/// Which outputs a plan covers.
#[cfg(feature = "creator")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum Scope {
    /// Exactly the ids the request named.
    Named,
    /// Every output the project holds. Explicit rather than inferred from an
    /// empty list, so an empty body can never mean "collect everything".
    Project,
}

#[cfg(feature = "creator")]
#[derive(Debug, serde::Deserialize)]
struct RetentionBody {
    project: uuid::Uuid,
    #[serde(default)]
    scope: Option<Scope>,
    #[serde(default)]
    drop: Vec<uuid::Uuid>,
    #[serde(default)]
    resolved: Vec<uuid::Uuid>,
}

#[cfg(feature = "creator")]
#[derive(Debug, serde::Deserialize)]
struct RetentionQuery {
    project: uuid::Uuid,
    #[serde(default)]
    scope: Option<Scope>,
    #[serde(default)]
    drop: Vec<uuid::Uuid>,
    #[serde(default)]
    resolved: Vec<uuid::Uuid>,
}

/// The candidates a request covers: the ids it named, or the project's whole
/// output set when it asked for the project scope.
///
/// `Scope` exists so an omitted scope cannot widen into "everything" — a
/// cleanup is the one request where a defaulted parameter is data loss.
#[cfg(feature = "creator")]
fn candidates(
    store: &strivo_research::creator::Store,
    project: strivo_research::creator::ProjectId,
    scope: Option<Scope>,
    named: Vec<uuid::Uuid>,
) -> Result<Vec<strivo_research::creator::ArtifactId>, crate::problem::Problem> {
    use strivo_research::creator::{retention, ArtifactId};
    let named: Vec<ArtifactId> = named.into_iter().map(ArtifactId::from_uuid).collect();
    if scope != Some(Scope::Project) {
        return Ok(named);
    }
    let ids = retention::outputs(store, project)
        .map_err(|e| crate::problem::Problem::internal(format!("list outputs: {e}")))?;
    Ok(if ids.is_empty() { named } else { ids })
}

#[cfg(feature = "creator")]
fn plan_json(plan: &strivo_research::creator::Plan) -> serde_json::Value {
    use strivo_research::creator::Action;
    json!({
        "project": plan.project.to_string(),
        "collectable": plan.collectable,
        "entries": plan.entries.iter().map(|e| json!({
            "id": e.id.to_string(),
            "kind": e.kind,
            "class": format!("{:?}", e.class).to_lowercase(),
            "reason": e.class.reason(),
            "action": match &e.action {
                Action::Unknown => "unknown",
                Action::Refused { .. } => "refused",
                Action::Unreferenced => "unreferenced",
                Action::Protected { .. } => "protected",
                Action::Resolved { .. } => "resolved",
            },
            "collected": e.action.is_collected(),
            "explain": e.explain(),
            "locator": e.locator.as_ref().map(|p| p.display().to_string()),
            "bytes": e.bytes,
        })).collect::<Vec<_>>(),
    })
}

/// `GET /api/v1/creator/retention/plan` — report, change nothing.
#[cfg(feature = "creator")]
async fn retention_plan(
    headers: HeaderMap,
    State(state): State<AppState>,
    Query(query): Query<RetentionQuery>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    use strivo_research::creator::{plan, PlanRequest, ProjectId, Store};
    let project = ProjectId::from_uuid(query.project);
    let store = match Store::open(crate::creator_schema::creator_db_path()) {
        Ok(store) => store,
        Err(e) => {
            return crate::problem::Problem::internal(format!("creator store: {e}")).into_response()
        }
    };
    let drop = match candidates(&store, project, query.scope, query.drop) {
        Ok(ids) => ids,
        Err(problem) => return problem.into_response(),
    };
    let resolved: Vec<strivo_research::creator::ArtifactId> =
        query.resolved.into_iter().map(Into::into).collect();
    match plan(
        &store,
        &PlanRequest {
            project,
            drop: &drop,
            resolved: &resolved,
        },
    ) {
        Ok(found) => Json(plan_json(&found)).into_response(),
        Err(e) => crate::problem::Problem::internal(format!("retention plan: {e}")).into_response(),
    }
}

/// `POST /api/v1/creator/retention/collect` — remove what the plan says is
/// collectable, and answer with the plan it acted on rather than the request.
#[cfg(feature = "creator")]
async fn retention_collect(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<RetentionBody>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    use strivo_research::creator::{collect, ArtifactId, PlanRequest, ProjectId, Store};
    let project = ProjectId::from_uuid(body.project);
    let store = match Store::open(crate::creator_schema::creator_db_path()) {
        Ok(store) => store,
        Err(e) => {
            return crate::problem::Problem::internal(format!("creator store: {e}")).into_response()
        }
    };
    let drop = match candidates(&store, project, body.scope, body.drop) {
        Ok(ids) => ids,
        Err(problem) => return problem.into_response(),
    };
    let resolved: Vec<ArtifactId> = body
        .resolved
        .into_iter()
        .map(ArtifactId::from_uuid)
        .collect();
    match collect(
        &store,
        &PlanRequest {
            project,
            drop: &drop,
            resolved: &resolved,
        },
    ) {
        Ok(done) => Json(json!({
            "plan": plan_json(&done.plan),
            "removed": done.removed.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "bytes_removed": done
                .bytes_removed
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>(),
        }))
        .into_response(),
        Err(e) => {
            crate::problem::Problem::internal(format!("retention collect: {e}")).into_response()
        }
    }
}

/// The retention routes, mounted only in a Creator build.
#[cfg(feature = "creator")]
pub fn creator_router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/creator/retention/plan", get(retention_plan))
        .route("/api/v1/creator/retention/collect", post(retention_collect))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/history", get(history))
        .route("/api/v1/backup", post(backup_create))
        .route("/api/v1/backups", get(backups_list))
        .route("/api/v1/backups/{name}/restore", post(backup_restore))
        .route("/api/v1/backups/{name}/download", get(backup_download))
}

/// `GET /api/v1/history` — durable recording history from the jobs DB
/// (roadmap item 17). Unlike `/recordings` (the in-memory, bounded daemon
/// snapshot), this survives restarts and includes completed/failed jobs.
async fn history(
    headers: HeaderMap,
    State(state): State<AppState>,
    Query(query): Query<PageQuery>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    let db = match state.jobs_db().await {
        Ok(d) => d,
        Err(e) => return crate::problem::Problem::internal(e).into_response(),
    };
    if let Some(limit) = query.limit {
        let start = query.cursor.unwrap_or(0);
        return match db
            .load_recording_jobs_page(start, limit.clamp(1, 500))
            .await
        {
            Ok((jobs, total)) => {
                let end = start.saturating_add(jobs.len());
                Json(json!({
                    "history": jobs,
                    "total": total,
                    "next_cursor": (end < total).then_some(end),
                }))
                .into_response()
            }
            Err(e) => crate::problem::Problem::internal(e.to_string()).into_response(),
        };
    }
    match db.load_recording_jobs().await {
        Ok(mut jobs) => {
            jobs.sort_by_key(|j| std::cmp::Reverse(j.started_at));
            let total = jobs.len();
            let (start, end, next_cursor) = page_bounds(total, &query);
            Json(json!({
                "history": &jobs[start..end],
                "total": total,
                "next_cursor": next_cursor,
            }))
            .into_response()
        }
        Err(e) => crate::problem::Problem::internal(e.to_string()).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_name_rejects_traversal() {
        assert!(safe_backup_name("2026-05-26T22-30-00Z"));
        assert!(safe_backup_name("snapshot_1.bak"));
        assert!(!safe_backup_name(""));
        assert!(!safe_backup_name(".."));
        assert!(!safe_backup_name("../etc"));
        assert!(!safe_backup_name("a/b"));
        assert!(!safe_backup_name("with space"));
    }

    /// `Scope` exists so an omitted scope cannot widen into "collect
    /// everything" — a cleanup is the one request where a defaulted parameter is
    /// data loss. Pinned in both spellings so a default added later is a test
    /// failure rather than a silent scope change.
    #[cfg(feature = "creator")]
    #[test]
    fn an_absent_scope_is_named_never_the_whole_project() {
        let named: Scope = serde_json::from_str("\"named\"").expect("named parses");
        let project: Scope = serde_json::from_str("\"project\"").expect("project parses");
        assert_ne!(named, project, "the two scopes are different answers");
        assert_eq!(
            serde_json::from_str::<Option<Scope>>("null").expect("null parses"),
            None,
            "and an omitted scope is absent, which the caller reads as Named"
        );
    }
}
