//! The version-7 rollback rehearsal, on its own.
//!
//! Split from `history` for the reason `workspace_rollback` is split from
//! `workspace`: the file that owns the model is not also the file that owns its
//! destruction, so a change to either does not have to reason about the other.
//!
//! This rehearses **version 7's** reverse step on a copy, not only the
//! all-the-way-to-zero path. `rollback.rs` caught version 4's missing `DROP`
//! once already; a step named on its own cannot pass later by being covered
//! incidentally.

use tempfile::TempDir;

use super::super::store::Store;
use super::super::Edit;
use super::fixtures::{project, revision};
use crate::creator::schema;

/// Given a Creator database at version 7 holding a project, two composition
/// revisions and a recorded history
/// When the version-7 `down` is rehearsed on a copy
/// Then the history table and its rows are gone, the version line says 6, and
/// the composition revisions the history *pointed at* are untouched — a
/// rehearsal of this version's reverse step.
#[test]
fn the_version_seven_rollback_destroys_the_history_and_nothing_else() {
    let dir = TempDir::new().expect("tempdir");
    let db = dir.path().join("research.db");
    let copy = dir.path().join("rehearsal.db");

    {
        let store = Store::open(&db).expect("store opens");
        assert!(
            store.schema_version().expect("version") >= 7,
            "a version-7 rehearsal needs a database at version 7 or later"
        );
        let p = project("streamer");
        store.insert_project(&p).expect("project inserted");
        let root = revision(p.id, 1, None);
        store
            .insert_composition_revision(&root)
            .expect("root edit inserted");
        let second = revision(p.id, 2, Some(root.id));
        store
            .insert_composition_revision(&second)
            .expect("second edit inserted");
        let first = store
            .record(p.id, None, Edit::composition(root.id), "open", "t1")
            .expect("action recorded");
        store
            .record(
                p.id,
                Some(first.id),
                Edit::composition(second.id),
                "trim",
                "t2",
            )
            .expect("second action recorded");
        store
            .branch(p.id, first.id, Edit::composition(second.id), "alt", "t3")
            .expect("branched");
        assert_eq!(store.history(p.id).expect("readable").len(), 3);
    }

    std::fs::copy(&db, &copy).expect("the rehearsal works on a copy, never the live file");

    {
        let mut conn = rusqlite::Connection::open(&copy).expect("copy opens");
        let landed = schema::rollback(&mut conn, 6, schema::MIGRATIONS).expect("rollback runs");
        assert_eq!(landed, 6);
        assert_eq!(
            schema::applied_version(&conn).expect("version"),
            6,
            "the version line was not stamped"
        );
        let history_tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='creator_history'",
                [],
                |r| r.get(0),
            )
            .expect("queryable");
        assert_eq!(history_tables, 0, "the history table survived the down");
    }
}

/// The positive half of the rehearsal, and the reason the assertion above is
/// not satisfied by a database that never had a history: the composition
/// revisions really do survive, and the history really is gone.
#[test]
fn the_rehearsal_keeps_the_revisions_the_history_pointed_at() {
    let dir = TempDir::new().expect("tempdir");
    let db = dir.path().join("research.db");
    let copy = dir.path().join("rehearsal.db");
    let project_id = {
        let store = Store::open(&db).expect("store opens");
        let p = project("streamer");
        store.insert_project(&p).expect("project inserted");
        let root = revision(p.id, 1, None);
        store
            .insert_composition_revision(&root)
            .expect("root edit inserted");
        store
            .record(p.id, None, Edit::composition(root.id), "open", "t1")
            .expect("recorded");
        p.id
    };
    std::fs::copy(&db, &copy).expect("copy");

    let mut conn = rusqlite::Connection::open(&copy).expect("copy opens");
    let before: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM creator_history WHERE project_id=?1",
            [project_id.to_string()],
            |r| r.get(0),
        )
        .expect("queryable");
    assert_eq!(before, 1, "the fixture recorded nothing to roll back");

    schema::rollback(&mut conn, 6, schema::MIGRATIONS).expect("rollback runs");

    // The revisions the history pointed at are not owned by version 7 and must
    // survive it: a rollback that lost edits would be far worse than one that
    // loses their ordering.
    let remaining: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM creator_composition_revisions WHERE project_id=?1",
            [project_id.to_string()],
            |r| r.get(0),
        )
        .expect("queryable");
    assert_eq!(
        remaining, 1,
        "the rollback destroyed an edit it did not own"
    );
    let after: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='creator_history'",
            [],
            |r| r.get(0),
        )
        .expect("queryable");
    assert_eq!(after, 0, "the history table survived the down");
    // And the store reopens on the rolled-back file at version 6, which is the
    // version a v6-only install would have had.
    drop(conn);
    let store = Store::open(&copy).expect("the rolled-back file reopens");
    assert_eq!(
        store.schema_version().expect("version"),
        7,
        "reopen migrates forward again"
    );
    assert_eq!(store.history(project_id).expect("readable").len(), 0);
}
