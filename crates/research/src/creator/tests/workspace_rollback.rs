//! The version-4 rollback rehearsal, on its own.
//!
//! Split from `workspace` because the rehearsal is a different class of thing:
//! `workspace` asserts what the workspace *is*, this asserts what reversing it
//! costs. It lives separately so the file that owns the model is not also the
//! file that owns its destruction, and so a change to either does not have to
//! reason about the other.

use tempfile::TempDir;

use super::super::store::Store;
use super::fixtures::{at, project, revision};
/// Given a Creator database at version 4, holding a project with a workspace,
/// a selected revision and unsaved work
/// When the version-4 `down` is rehearsed on a copy
/// Then the workspace table and its row are gone, the version line says 3, and
/// version 3's job-lifecycle tables are untouched — a rehearsal of *this*
/// version's reverse step, not only of the all-the-way-to-zero path.
///
/// The all-the-way-to-zero rehearsal in `rollback.rs` caught this migration's
/// missing `DROP` once already; this one names the step, so a future version-4
/// edit cannot pass by being covered incidentally.
#[test]
fn the_version_four_rollback_destroys_the_workspace_and_nothing_else() {
    let dir = TempDir::new().expect("tempdir");
    let db = dir.path().join("research.db");
    let copy = dir.path().join("rehearsal.db");

    {
        let store = Store::open(&db).expect("store opens");
        // At least 4, not exactly 4: this rehearses *version 4's* reverse
        // step, and later migrations stack on top of it. Asserting `== 4`
        // made this the first thing any future migration broke, which is a
        // test about the build's version rather than about this down file.
        assert!(
            store.schema_version().expect("version") >= 4,
            "a version-4 rehearsal needs a database at version 4 or later"
        );
        let p = project("streamer");
        store.insert_project(&p).expect("project inserted");
        let edit = revision(p.id, 1, None);
        store
            .insert_composition_revision(&edit)
            .expect("edit inserted");
        store
            .open_workspace(p.id, &at("2026-09-07T01:00:00Z"))
            .expect("workspace");
        store
            .select_revision(p.id, edit.id, &at("2026-09-07T01:00:00Z"))
            .expect("selected");
        let state = store
            .view_state(p.id)
            .expect("view state")
            .with_draft(r#"{"wip":1}"#, true);
        store
            .save_view_state(p.id, &state, &at("2026-09-07T01:05:00Z"))
            .expect("unsaved work stored");
    }
    super::migration::fs_copy(&db, &copy);

    let mut conn = rusqlite::Connection::open(&copy).expect("reopen the copy");
    assert!(
        super::migration::creator_tables(&conn).contains(&"creator_workspace".to_string()),
        "the copy really does hold the workspace table before the rehearsal"
    );
    let rolled = super::super::rollback(&mut conn, 3, super::super::MIGRATIONS)
        .expect("the version-4 down applies");
    assert_eq!(rolled, 3, "the version line moved down with the schema");
    assert_eq!(
        super::migration::version_of(&conn),
        3,
        "and it was stamped inside the same transaction as the down"
    );
    let tables = super::migration::creator_tables(&conn);
    assert!(
        !tables.contains(&"creator_workspace".to_string()),
        "the workspace table is gone, left {tables:?}"
    );
    assert!(
        tables.contains(&"creator_jobs".to_string()),
        "and version 3's job lifecycle is untouched, left {tables:?}"
    );
    assert!(
        tables.contains(&"creator_composition_revisions".to_string()),
        "as is the revision store the workspace referenced"
    );
}
