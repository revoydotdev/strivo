//! Reversing the version that added source rights notes, rehearsed from a
//! copy.
//!
//! A destructive `down` is only ever run against a backup
//! (`docs/RESEARCH-PLATFORM-ROADMAP.md:72`), so the rehearsal copies. Two
//! properties are asserted: the notes table is gone, and the tables that came
//! before are still there — a `down` that dropped more than its own version is
//! the failure this file exists to catch. Todo 41's rehearsal is the precedent
//! that found one: a `schema_v4.down.sql` carrying the explanatory comment and
//! no `DROP` statement at all.

use rusqlite::Connection;

use super::super::schema;
use super::super::store::Store;
use super::super::{Consent, Licence, Ownership, SourceNote};

/// The version this todo's migration is expected to be. Written as a named
/// constant rather than `latest - 1` so a *missing* migration fails loudly
/// instead of rolling back a neighbour's version by accident.
const RIGHTS_VERSION: i64 = 6;

fn objects(conn: &Connection, kind: &str, pattern: &str) -> Vec<String> {
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type=?1 AND name LIKE ?2 ORDER BY name")
        .expect("prepare");
    let rows = stmt
        .query_map([kind, pattern], |r| r.get::<_, String>(0))
        .expect("query");
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .expect("rows")
}

/// A populated file carrying a rights note, at the version this build
/// understands.
fn populated() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("research.db");
    let store = Store::open(&path).expect("open");
    let project = super::fixtures::project("streamer");
    store.insert_project(&project).expect("project");
    let mut source = super::fixtures::source(project.id, None);
    source.media_fingerprint = "sha256:rollback".into();
    store.insert_source_asset(&source).expect("source");
    store
        .record_note(
            &SourceNote {
                source: source.id,
                ownership: Ownership::CreatorOwned,
                licence: Licence::Granted {
                    grant: "my own capture".into(),
                },
                consent: Consent::NotApplicable,
                disclosure: "kept in the archive".into(),
                recorded_at: "2026-09-07T00:00:00Z".into(),
                recorded_by: "operator".into(),
            }
            .validated()
            .expect("validates"),
        )
        .expect("note recorded");
    (dir, path)
}

#[test]
fn the_rights_migration_is_the_version_this_test_reverses() {
    // Guard: a concurrent lane that took v6 would otherwise make this file
    // roll back somebody else's table and pass for having rehearsed its own.
    let found = schema::MIGRATIONS
        .iter()
        .find(|m| m.name == "creator_source_rights")
        .unwrap_or_else(|| {
            panic!(
                "no migration named creator_source_rights; this build has {:?}",
                schema::MIGRATIONS
                    .iter()
                    .map(|m| m.name)
                    .collect::<Vec<_>>()
            )
        });
    assert_eq!(
        found.version, RIGHTS_VERSION,
        "creator_source_rights is not v{RIGHTS_VERSION}; re-read the committed \
         schema version and renumber"
    );
    assert!(
        !found.up.trim().is_empty() && !found.down.trim().is_empty(),
        "a migration needs both directions"
    );
}

#[test]
fn rolling_the_rights_version_back_removes_exactly_its_own_table() {
    // Given a populated file at the current version
    let (_dir, path) = populated();
    let before = Store::open(&path).expect("open");
    assert_eq!(
        before.schema_version().expect("version"),
        schema::latest_version()
    );
    assert!(
        objects(before.raw_connection(), "table", "creator_source_notes")
            .contains(&"creator_source_notes".to_string()),
        "the notes table exists before the rollback"
    );
    let notes_before: i64 = before
        .raw_connection()
        .query_row("SELECT COUNT(*) FROM creator_source_notes", [], |r| {
            r.get(0)
        })
        .expect("count");
    assert_eq!(notes_before, 1, "the rehearsal is on a populated table");
    drop(before);

    // When it is rolled back to the version before
    let mut conn = Connection::open(&path).expect("open");
    let version = schema::rollback(&mut conn, RIGHTS_VERSION - 1, schema::MIGRATIONS)
        .expect("rollback applies");

    // Then the version line moved with the statements
    assert_eq!(
        version,
        RIGHTS_VERSION - 1,
        "the version line moves down in the same transaction as the down"
    );
    // And the notes table is gone
    assert!(
        objects(&conn, "table", "creator_source_notes").is_empty(),
        "creator_source_notes survived its own down file"
    );
    // And every table that came before is still there
    for table in [
        "creator_projects",
        "creator_source_assets",
        "creator_metric_observations",
        "creator_viewer_samples",
        "creator_workspace",
    ] {
        assert!(
            objects(&conn, "table", table).contains(&table.to_string()),
            "{table} was dropped by a down that should only reverse its own version"
        );
    }
}

#[test]
fn a_rolled_back_database_reopens_forward_and_the_notes_read_empty_not_absent() {
    // Given a file rolled back one version
    let (_dir, path) = populated();
    let mut conn = Connection::open(&path).expect("open");
    schema::rollback(&mut conn, RIGHTS_VERSION - 1, schema::MIGRATIONS).expect("rollback");
    drop(conn);

    // When it is opened again, which migrates it forward
    let store = Store::open(&path).expect("reopen");
    assert_eq!(
        store.schema_version().expect("version"),
        schema::latest_version()
    );

    // Then the table is back and reads empty rather than absent
    assert!(store
        .creator_tables()
        .expect("tables")
        .contains(&"creator_source_notes".to_string()));
    let remaining: i64 = store
        .raw_connection()
        .query_row("SELECT COUNT(*) FROM creator_source_notes", [], |r| {
            r.get(0)
        })
        .expect("count");
    assert_eq!(
        remaining, 0,
        "re-migrating must not resurrect a rolled-back note"
    );
    // And a source that exists is simply unannotated again — undetermined,
    // not publishable, which is the fail-closed direction.
    let project = super::fixtures::project("streamer");
    store.insert_project(&project).expect("project");
    let mut source = super::fixtures::source(project.id, None);
    source.media_fingerprint = "sha256:after-rollback".into();
    store.insert_source_asset(&source).expect("source");
    let verdict = super::super::rights::eligibility_of(
        store.source_note(source.id).expect("read").as_ref(),
        &super::super::CalendarDate::parse("2026-09-07").expect("a date"),
    );
    assert!(!verdict.is_publishable(), "{verdict}");
}
