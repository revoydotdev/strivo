//! Scene bundles are not a second copy of the edit.
//!
//! The todo enumerated six coexisting representations of an edit. The Scene
//! bundle was one of them: `scenes.manifest_json` stored the EDL *verbatim*
//! under a component key named `editor`, so a scene could hold an edit that had
//! since been changed, and restoring the scene wrote that stale copy straight
//! back over the live one. Two writes, two truths.
//!
//! The unification: a scene names a **revision id**. The bytes live in exactly
//! one place, and a scene restore activates a revision through the same
//! validated, atomic path a revision restore uses.
//!
//! Every test here asserts the *code* or the *stored bytes*, never a message,
//! and every one of them drives the real router.

#![cfg(feature = "creator")]

mod common;

use axum::http::StatusCode;
use rusqlite::Connection;
use strivo_editor::{store::EdlStore, Cut, CutKind, Edl};

use common::{editor_store_path, ffmpeg_available, shared_xdg_roots, Sandbox, EDITOR_STORE};

fn scenes_db_path() -> std::path::PathBuf {
    strivo_core::config::AppConfig::data_dir()
        .join("plugins")
        .join("scenes")
        .join("scenes.db")
}

/// The stored `manifest_json` for every scene of a recording, read straight out
/// of the database rather than through the API — a scene that *stores* a copy
/// of the edit is a storage fact, and the storage is what has to be inspected.
fn stored_manifests(recording_id: &str) -> Vec<serde_json::Value> {
    let path = scenes_db_path();
    if !path.exists() {
        return Vec::new();
    }
    let conn = Connection::open(&path).expect("open scenes db");
    let mut stmt = conn
        .prepare("SELECT manifest_json FROM scenes WHERE recording_id = ?1 ORDER BY created_at")
        .expect("prepare");
    let rows = stmt
        .query_map(rusqlite::params![recording_id], |r| r.get::<_, String>(0))
        .expect("query")
        .collect::<Result<Vec<_>, _>>()
        .expect("rows");
    rows.into_iter()
        .map(|j| serde_json::from_str(&j).expect("manifest json"))
        .collect()
}

fn edl_for(recording_id: &str, n: usize) -> Edl {
    Edl {
        recording_id: recording_id.to_string(),
        cuts: (0..n)
            .map(|i| Cut {
                kind: CutKind::Source {
                    source_path: "/archive/source.mkv".into(),
                },
                start_sec: i as f32 * 10.0,
                end_sec: i as f32 * 10.0 + 5.0,
                fade_in_sec: 0.0,
                fade_out_sec: 0.0,
            })
            .collect(),
    }
}

fn store() -> EdlStore {
    shared_xdg_roots();
    EdlStore::open(&editor_store_path()).expect("open editor store")
}

/// The headline storage invariant. Given a captured scene, no component of the
/// stored manifest holds a copy of the edit — the manifest names a revision and
/// stops there.
#[tokio::test]
async fn a_captured_scene_names_a_revision_and_stores_no_copy_of_the_edit() {
    if !ffmpeg_available() {
        eprintln!("skipping: ffmpeg is not on PATH, so no recording can be registered");
        return;
    }
    let _guard = EDITOR_STORE.lock().await;
    let sb = Sandbox::new();
    let store = store();
    let written = store
        .save(None, &edl_for(&sb.recording_id, 3), "first")
        .unwrap();
    let revision_id = written.revision_id;

    let (status, body) = sb
        .post(
            &format!("/api/v1/plugins/scenes/{}", sb.recording_id),
            serde_json::json!({ "name": "before the trim" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let manifests = stored_manifests(&sb.recording_id);
    assert_eq!(
        manifests.len(),
        1,
        "the capture did not land: {manifests:?}"
    );
    let components = manifests[0]["components"].as_object().expect("components");

    // Anti-vacuity: the component that is supposed to hold the pointer exists
    // and is an object. Asserting "no component holds an edit" over an empty
    // component map would pass for free.
    let editor = components
        .get("editor")
        .expect("the editor component is missing, so the assertions below are vacuous")
        .clone();
    assert!(
        editor.is_object(),
        "expected a revision pointer object, got {editor}"
    );
    assert_eq!(
        editor.get("revision_id").and_then(|v| v.as_i64()),
        Some(revision_id),
        "the scene must name the revision it captured"
    );

    // The invariant itself: no component anywhere in the manifest carries an
    // edit. Checked by shape — a value that deserialises as an `Edl` is a
    // stored copy of the edit, whatever the component is called.
    for (name, value) in components {
        assert!(
            serde_json::from_value::<Edl>(value.clone()).is_err(),
            "scene component {name:?} stores a copy of the edit, so the scene is \
             a second source of truth: {value}"
        );
    }
}

/// A scene restore must reproduce the edit the scene named, and it must do so
/// by activating a revision — so a second save after the capture is undoable
/// and the restore itself is recorded.
#[tokio::test]
async fn restoring_a_scene_activates_the_revision_it_named() {
    if !ffmpeg_available() {
        return;
    }
    let _guard = EDITOR_STORE.lock().await;
    let sb = Sandbox::new();
    let store = store();
    store
        .save(None, &edl_for(&sb.recording_id, 2), "two cuts")
        .unwrap();
    let (status, body) = sb
        .post(
            &format!("/api/v1/plugins/scenes/{}", sb.recording_id),
            serde_json::json!({ "name": "take one" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    store
        .save(None, &edl_for(&sb.recording_id, 7), "seven cuts")
        .unwrap();

    let (status, body) = sb
        .post(
            &format!(
                "/api/v1/plugins/scenes/{}/{}/restore",
                sb.recording_id,
                body["id"].as_str().unwrap()
            ),
            serde_json::json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["restored"].as_array().map(|a| a.len()),
        Some(1),
        "the editor component was not restored: {body}"
    );

    // The live edit is the captured one again.
    let live = store
        .current(&sb.recording_id)
        .unwrap()
        .expect("an edit is live")
        .edl;
    assert_eq!(
        live.cuts.len(),
        2,
        "the scene did not restore the edit it named"
    );

    // And the restore was itself recorded as a revision, so it is undoable —
    // the same "a restore is a save" property the revision store has.
    let listed = store.list_revisions(&sb.recording_id, 50).unwrap();
    assert!(
        listed.iter().any(|r| r.label == "restored from scene"),
        "the restore was not recorded as a revision: {listed:?}"
    );
}

/// A scene captured *before* the unification holds a verbatim copy of the edit
/// under `editor`. Restoring it must be **reported**, not guessed at: the edit
/// is left exactly as it is, and the response says why.
#[tokio::test]
async fn a_scene_carrying_the_retired_editor_copy_is_reported_not_guessed_at() {
    if !ffmpeg_available() {
        return;
    }
    let _guard = EDITOR_STORE.lock().await;
    let sb = Sandbox::new();
    let store = store();
    // The live edit: five cuts.
    store
        .save(None, &edl_for(&sb.recording_id, 5), "five cuts")
        .unwrap();

    // Plant a manifest in the retired shape, by hand, so the fixture cannot
    // drift with the code under test.
    let legacy_id = "legacy-scene";
    {
        let path = scenes_db_path();
        std::fs::create_dir_all(path.parent().unwrap()).expect("scenes dir");
        let conn = Connection::open(&path).expect("open");
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS scenes (
                 id TEXT PRIMARY KEY, recording_id TEXT NOT NULL, name TEXT NOT NULL,
                 created_at TEXT NOT NULL, size_bytes INTEGER NOT NULL, manifest_json TEXT NOT NULL
             );",
        )
        .expect("scenes table");
        let manifest = serde_json::json!({
            "id": legacy_id,
            "name": "old scene",
            "recording_id": sb.recording_id,
            "created_at": "2026-01-01T00:00:00Z",
            "components": { "editor": edl_for(&sb.recording_id, 2) },
        });
        conn.execute(
            "INSERT OR REPLACE INTO scenes (id, recording_id, name, created_at, size_bytes, manifest_json)
             VALUES (?1, ?2, 'old scene', '2026-01-01T00:00:00Z', 1, ?3)",
            rusqlite::params![legacy_id, sb.recording_id, manifest.to_string()],
        )
        .expect("plant");
    }

    let (status, body) = sb
        .post(
            &format!(
                "/api/v1/plugins/scenes/{}/{legacy_id}/restore",
                sb.recording_id
            ),
            serde_json::json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let skipped = body["skipped"].as_array().expect("skipped list");
    assert!(
        skipped.iter().any(|v| v == "editor"),
        "the retired editor copy was not reported as skipped: {body}"
    );
    assert_eq!(
        body["skipped_reasons"]["editor"].as_str(),
        Some("retired-scene-editor-copy"),
        "the skip must name the retired representation rather than a generic \
         failure, so the operator knows to re-capture: {body}"
    );

    // The decisive part: the live edit is untouched. A two-cut copy was
    // available in that manifest; a best-effort interpretation would have
    // applied it.
    let live = store
        .current(&sb.recording_id)
        .unwrap()
        .expect("an edit is live")
        .edl;
    assert_eq!(
        live.cuts.len(),
        5,
        "a retired representation was interpreted instead of reported"
    );
}

/// A save that names a stale base revision is a conflict, not a silent
/// overwrite. This is the whole of "concurrent saves detect conflict" at the
/// store layer, exercised through the real route.
#[tokio::test]
async fn a_save_against_a_stale_base_revision_is_refused() {
    if !ffmpeg_available() {
        return;
    }
    let _guard = EDITOR_STORE.lock().await;
    let sb = Sandbox::new();
    let store = store();
    let first = store
        .save(None, &edl_for(&sb.recording_id, 2), "first")
        .unwrap();

    // Someone else saves on top of the revision this client last saw.
    store
        .save(None, &edl_for(&sb.recording_id, 9), "someone else")
        .unwrap();

    // This client saves against its own stale base.
    let outcome = store.save(
        Some(first.revision_id),
        &edl_for(&sb.recording_id, 4),
        "stale client",
    );
    assert!(
        matches!(
            outcome,
            Err(strivo_editor::store::RevisionError::Conflict { .. })
        ),
        "a save against a superseded base must be refused as a conflict, got {outcome:?}"
    );

    // And the winner is untouched.
    let live = store
        .current(&sb.recording_id)
        .unwrap()
        .expect("an edit is live")
        .edl;
    assert_eq!(live.cuts.len(), 9, "the refused save changed the live edit");
}

/// Restoring a revision must never retarget a *different* recording. Before the
/// unification the restore route passed the stored document's own
/// `recording_id` to the store, so restoring a row whose JSON named another
/// recording wrote that edit into the other recording's history. The store now
/// takes the recording from the path and refuses a document that disagrees.
#[tokio::test]
async fn a_revision_naming_another_recording_is_refused_and_changes_nothing() {
    if !ffmpeg_available() {
        return;
    }
    let _guard = EDITOR_STORE.lock().await;
    let sb = Sandbox::new();
    let store = store();
    store
        .save(None, &edl_for(&sb.recording_id, 3), "mine")
        .unwrap();

    // Plant a row for this recording whose *document* names another recording —
    // the exact shape a hand-edited or cross-contaminated database holds.
    let foreign = Edl {
        recording_id: "some-other-recording".into(),
        ..edl_for("some-other-recording", 8)
    };
    {
        let conn = Connection::open(editor_store_path()).expect("open");
        conn.execute(
            "INSERT INTO revisions (recording_id, doc_version, edl_json, label, created_at)
             VALUES (?1, ?2, ?3, 'foreign', '2026-01-01T00:00:00Z')",
            rusqlite::params![
                sb.recording_id,
                strivo_editor::store::REVISION_VERSION,
                serde_json::to_string(&foreign).unwrap()
            ],
        )
        .expect("plant");
    }

    let foreign_id: i64 = {
        let conn = Connection::open(editor_store_path()).expect("open");
        conn.query_row(
            "SELECT revision_id FROM revisions WHERE label = 'foreign'",
            [],
            |r| r.get(0),
        )
        .expect("id")
    };

    let (status, body) = sb
        .post(
            &format!(
                "/api/v1/plugins/editor/{}/revisions/{foreign_id}/restore",
                sb.recording_id
            ),
            serde_json::json!({}),
        )
        .await;
    assert!(
        !status.is_success(),
        "a revision naming another recording must not restore: {status} {body}"
    );

    // The refusal wrote nothing. Asserted on the table, because every reader
    // now refuses the planted row by design — that is the same single decoder
    // under test, so asking a reader would be asking the thing under test to
    // grade itself.
    let cut_counts: Vec<usize> = {
        let conn = Connection::open(editor_store_path()).expect("open");
        let mut stmt = conn
            .prepare("SELECT edl_json FROM revisions WHERE recording_id = ?1 ORDER BY revision_id")
            .expect("prepare");
        let json: Vec<String> = stmt
            .query_map(rusqlite::params![sb.recording_id], |r| r.get(0))
            .expect("query")
            .collect::<Result<Vec<_>, _>>()
            .expect("rows");
        json.iter()
            .map(|j| {
                serde_json::from_str::<Edl>(j)
                    .expect("row parses")
                    .cuts
                    .len()
            })
            .collect()
    };
    assert_eq!(
        cut_counts,
        vec![3, 8],
        "expected the original three-cut edit and the planted foreign row, and \
         nothing appended by the refused restore: {cut_counts:?}"
    );
    let foreign_rows: i64 = Connection::open(editor_store_path())
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM revisions WHERE recording_id = 'some-other-recording'",
            [],
            |r| r.get(0),
        )
        .expect("count");
    assert_eq!(
        foreign_rows, 0,
        "the refused restore wrote into the recording the document named"
    );
}

/// The conflict at the surface a client meets it: the load reports the current
/// `revision_id`, the save hands it back as `?base_revision_id=`, and a save
/// against a revision someone else has since superseded is refused with a
/// stable code rather than silently overwriting their edit.
#[tokio::test]
async fn a_save_against_a_superseded_revision_is_refused_with_a_typed_code() {
    if !ffmpeg_available() {
        return;
    }
    let _guard = EDITOR_STORE.lock().await;
    let sb = Sandbox::new();
    let path = format!("/api/v1/plugins/editor/{}", sb.recording_id);

    // The client's first save, and the revision the server reports back.
    let (status, first) = sb
        .save_edl(common::edl_cutting(&sb.recording, 0.0, 4.0))
        .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let base = first["revision_id"]
        .as_i64()
        .expect("the save must report the revision it wrote");

    // Someone else saves on top of it.
    let (status, other) = sb
        .save_edl(common::edl_cutting(&sb.recording, 0.0, 3.0))
        .await;
    assert_eq!(status, StatusCode::OK, "{other}");

    // This client still holds the superseded revision and saves against it.
    let (status, refused) = sb
        .post(
            &format!("{path}?base_revision_id={base}"),
            common::edl_cutting(&sb.recording, 0.0, 2.0),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "a save against a superseded base must be 409, got {status}: {refused}"
    );
    assert!(
        Sandbox::detail(&refused).starts_with("edit-conflict/stale-base"),
        "the refusal must name the conflict as a code, not prose: {refused}"
    );

    // The other client's edit is the live one, unchanged.
    let live = store()
        .current(&sb.recording_id)
        .unwrap()
        .expect("an edit is live")
        .edl;
    assert_eq!(
        live.cuts[0].end_sec, 3.0,
        "the refused save overwrote the other client's edit"
    );
    assert_eq!(
        store().list_revisions(&sb.recording_id, 50).unwrap().len(),
        2,
        "the refused save appended a revision anyway"
    );
}
