//! Local file and folder import: the web surface, and the one place a
//! client-supplied path is allowed to become a source.
//!
//! Report item C01 (`docs/CREATOR-EDITION-REPORT-2026-09-07.md:280`). The
//! bookkeeping lives in `strivo_research::creator::import`; this module is the
//! bridge, and it exists for exactly one reason: **import is a second way in**,
//! so it has to pass the same gate as the first.
//!
//! ## The parity is structural, not a promise
//!
//! [`admit_all`] calls [`media_input::admit_path`] — the same function
//! `admit_edl_at` calls for every cut of every edit — and returns
//! [`PathRejection`] unchanged. There is no second path grammar here to drift: a
//! `file://` source, a `..` hop out of the root, a directory where a file was
//! expected, a zero-byte file and a fifo all produce the codes todo 25 defined,
//! because the code that produced them is the same code. A test drives both
//! surfaces over the same hostile inputs and asserts the codes agree, so the
//! claim is checked rather than asserted.
//!
//! ## All-or-nothing
//!
//! The whole batch is admitted before any of it is imported, and one refusal
//! refuses the request. A folder import that registered three files and then
//! stopped would leave the user guessing which three, and would make the
//! library's contents depend on the order the filesystem happened to yield.
//!
//! ## Why the root is the caller's, not the recording directory
//!
//! Import exists to bring files in from *outside* the library, so requiring them
//! to already sit in the recording directory would make the feature useless.
//! The root is therefore the folder the operator picked, and what containment
//! buys is the rule that matters: a `..` hop cannot leave it. The API key is the
//! trust boundary above that, exactly as it is for every other mutating route —
//! this is not a second authorisation scheme.
//!
//! ## The edition boundary
//!
//! `#[cfg(feature = "creator")]` at the module declaration, so a PVR build does
//! not compile this file, does not mount either route, and does not link
//! `strivo-research` at all.

use std::path::Path;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use strivo_research::creator::{
    import_files, relink, unresolved_sources, AdmittedFile, ImportOutcome, ImportRequest,
    ProjectId, SourceAssetId, Store,
};

use crate::problem::Problem;
use crate::routes::media_input::{admit_path, PathRejection};
use crate::routes::settings::check_key;
use crate::server::AppState;

/// Admit a whole batch of client-named paths against the chosen root.
///
/// Every path, or none: the first refusal is returned as the gate's own
/// rejection, and the caller has nothing to undo because nothing was written
/// yet. Only the *first* refusal is reported — a batch of two hundred bad paths
/// should not produce a two-hundred-entry error body, and the caller fixes them
/// one request at a time.
pub fn admit_all(root: &Path, raw: &[String]) -> Result<Vec<AdmittedFile>, PathRejection> {
    raw.iter()
        .map(|path| admit_path(Some(root), path).map(AdmittedFile::admitted))
        .collect()
}

/// The creator store, or the problem to answer with. Opened per request, as
/// `creator_schema` opens it once per process: SQLite in WAL mode, and the
/// migration check is a no-op on an already-current file.
fn store() -> Result<Store, Problem> {
    Store::open(crate::creator_schema::creator_db_path())
        .map_err(|e| Problem::internal(format!("creator store: {e}")))
}

/// `POST /api/v1/creator/import` — bring local files into a project's source
/// collection, the same collection a capture writes to.
#[derive(Debug, Deserialize)]
struct ImportBody {
    project: uuid::Uuid,
    /// The folder the paths are resolved within. Each path must stay inside it.
    root: String,
    /// The files, named by the client as ffmpeg would see them — which is the
    /// whole reason [`admit_all`] exists.
    paths: Vec<String>,
}

/// `POST /api/v1/creator/relink` — point one source at where its bytes are now.
#[derive(Debug, Deserialize)]
struct RelinkBody {
    project: uuid::Uuid,
    source: uuid::Uuid,
    root: String,
    path: String,
}

async fn creator_import(
    headers: HeaderMap,
    State(_state): State<AppState>,
    Json(body): Json<ImportBody>,
) -> impl IntoResponse {
    if check_key(&headers, &_state).is_err() {
        return Problem::unauthorized().into_response();
    }
    // A `ProjectId` is a `ProjectId` from the wire uuid, never a `SourceAssetId`:
    // the typed ids make naming the wrong entity a compile error here rather
    // than a row read from the wrong table at runtime.
    let project = ProjectId::from_uuid(body.project);
    let admitted = match admit_all(Path::new(&body.root), &body.paths) {
        Ok(files) => files,
        Err(rejection) => return rejection.to_problem().into_response(),
    };
    let scratch = strivo_core::config::AppConfig::data_dir().join("scratch");
    let at = chrono::Utc::now().to_rfc3339();
    let store = match store() {
        Ok(store) => store,
        Err(problem) => return problem.into_response(),
    };
    // A project id that names no project is a client error, and it is caught
    // here rather than surfacing as a foreign-key failure from the first source
    // insert. Found by booting a Creator binary: `import_files` against a
    // non-existent project returned `500 database error: FOREIGN KEY
    // constraint failed`, which tells the caller nothing and leaves a submitted
    // job behind.
    match store.project(project) {
        Ok(Some(_)) => {}
        Ok(None) => {
            return Problem::not_found(format!("import/no-such-project: {project}"))
                .into_response();
        }
        Err(e) => return Problem::internal(format!("creator import: {e}")).into_response(),
    }
    match import_files(
        &store,
        &ImportRequest {
            project,
            files: &admitted,
            scratch_root: &scratch,
            at: &at,
        },
    ) {
        Ok(manifest) => (
            StatusCode::CREATED,
            Json(json!({
                "job": manifest.job.to_string(),
                "manifest": manifest.artifact.to_string(),
                "cache_key": manifest.cache_key.as_str(),
                "imported": manifest.imported(),
                "duplicates": manifest.duplicates(),
                "entries": manifest.entries.iter().map(|e| json!({
                    "name": e.name,
                    "fingerprint": e.fingerprint,
                    "source": e.outcome.outcome_asset().id.to_string(),
                    // Named, not implied: "this is the recording you already
                    // have, and here it is" is the whole answer to a duplicate.
                    "outcome": match e.outcome {
                        ImportOutcome::Imported { .. } => "imported",
                        ImportOutcome::Duplicate { .. } => "duplicate",
                    },
                    "title": e.outcome.outcome_asset().title,
                })).collect::<Vec<_>>(),
            })),
        )
            .into_response(),
        Err(e) => Problem::internal(format!("creator import: {e}")).into_response(),
    }
}

async fn creator_relink(
    headers: HeaderMap,
    State(_state): State<AppState>,
    Json(body): Json<RelinkBody>,
) -> impl IntoResponse {
    if check_key(&headers, &_state).is_err() {
        return Problem::unauthorized().into_response();
    }
    let project = ProjectId::from_uuid(body.project);
    let source = SourceAssetId::from_uuid(body.source);
    let admitted = match admit_all(Path::new(&body.root), &[body.path]) {
        Ok(files) => files,
        Err(rejection) => return rejection.to_problem().into_response(),
    };
    let store = match store() {
        Ok(store) => store,
        Err(problem) => return problem.into_response(),
    };
    match relink(&store, source, &admitted[0]) {
        Ok(asset) => {
            let missing = unresolved_sources(&store, project).unwrap_or_default();
            (
                StatusCode::OK,
                Json(json!({
                    "source": asset.id.to_string(),
                    "fingerprint": asset.media_fingerprint,
                    // The identity is the answer; the locator is not. Which is
                    // why "is it back" is answered by asking the row rather
                    // than by remembering where the file used to be.
                    "resolved": !missing.iter().any(|s| s.id == asset.id),
                    "still_unresolved": missing
                        .iter()
                        .map(|s| json!({
                            "id": s.id.to_string(),
                            "title": s.title,
                            "fingerprint": s.media_fingerprint,
                        }))
                        .collect::<Vec<_>>(),
                })),
            )
                .into_response()
        }
        // A relink refusal is a client decision — wrong id, wrong file — so it
        // is a 400 carrying the stable code the research module defines.
        Err(rejection) => {
            Problem::bad_request(format!("{}: {}", rejection.code(), rejection)).into_response()
        }
    }
}

/// The import routes. Mounted only by a Creator build; see `routes::api`.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/creator/import", post(creator_import))
        .route("/api/v1/creator/relink", post(creator_relink))
}
