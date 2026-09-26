//! The rollback rehearsal from a backup, and the explicit reverse migration.

use rusqlite::Connection;
use tempfile::TempDir;

use super::super::entities::Project;
use super::super::ids::*;
use super::super::schema::{self, MIGRATIONS, TABLES, VERSION_KEY};
use super::super::store::Store;
use super::migration::{creator_tables, fs_copy, kernel_rows, seed_kernel, version_of};

/// Given a Creator database at the current version, with a backup taken
/// before the version bump
/// When the database is restored from the backup
/// Then the Creator tables are gone and the recorded version is 0, and the
/// kernel rows the backup carried are still readable.
#[test]
fn rollback_rehearsal_from_a_backup_restores_the_prior_state() {
    let dir = TempDir::new().expect("tempdir");
    let db = dir.path().join("research.db");
    let backup = dir.path().join("research.db.pre-creator");

    // Version 0: the research kernel, with an edit and its provenance in it.
    seed_kernel(&db);
    fs_copy(&db, &backup);
    let before = {
        let conn = Connection::open(&backup).expect("reopen backup");
        (version_of(&conn), kernel_rows(&conn), creator_tables(&conn))
    };
    assert_eq!(before.0, 0, "the backup is a version 0 database");
    assert!(before.2.is_empty(), "with no Creator tables");

    // The Creator schema, and a row written through it.
    {
        let store = Store::open(&db).expect("creator store opens");
        store
            .insert_project(&Project {
                id: ProjectId::generate(),
                name: "streamer".into(),
                description: String::new(),
                channel: "twitch".into(),
                series: String::new(),
                settings_json: "{}".into(),
                created_at: "2026-09-07T00:00:00Z".into(),
                updated_at: "2026-09-07T00:00:00Z".into(),
            })
            .expect("project inserted");
        assert_eq!(store.creator_tables().expect("tables").len(), TABLES.len());
    }
    let after_migrate = {
        let conn = Connection::open(&db).expect("reopen");
        (version_of(&conn), creator_tables(&conn))
    };
    assert_eq!(
        after_migrate.0, 2,
        "the live database is at the current version"
    );
    assert!(!after_migrate.1.is_empty(), "and has Creator tables");

    // The rollback rehearsal: restore the backup over the migrated file.
    fs_copy(&backup, &db);
    let restored = {
        let conn = Connection::open(&db).expect("reopen");
        (version_of(&conn), kernel_rows(&conn), creator_tables(&conn))
    };

    assert_eq!(restored.0, 0, "the restored file is back at version 0");
    assert!(
        restored.2.is_empty(),
        "the restored file has no Creator tables, found {:?}",
        restored.2
    );
    assert_eq!(restored.1, before.1, "and the prior rows are intact");

    // And the same restore through the schema's own down path, on a copy, so
    // the explicit reverse migration is exercised rather than assumed.
    let copy = dir.path().join("rollback.db");
    fs_copy(&db, &copy);
    Store::open(&copy).expect("copy migrates forward");
    let mut conn = Connection::open(&copy).expect("reopen");
    let rolled = schema::rollback(&mut conn, 0, MIGRATIONS).expect("rollback applies");
    assert_eq!(rolled, 0, "the explicit down path returns to version 0");
    assert!(
        creator_tables(&conn).is_empty(),
        "the explicit down path removed every Creator table"
    );
    assert_eq!(kernel_rows(&conn), before.1, "and kept the kernel rows");
    let stamped: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM schema_meta WHERE key=?1",
            [VERSION_KEY],
            |r| r.get(0),
        )
        .expect("query");
    assert_eq!(stamped, 0, "the version line is removed, not left dangling");
}
