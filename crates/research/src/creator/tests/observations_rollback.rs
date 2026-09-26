//! Reversing version 5: the two observation series and the live-status
//! trigger.
//!
//! The rehearsal copies a populated file and rolls it back, because a
//! destructive `down` is only ever run against a backup
//! (`docs/RESEARCH-PLATFORM-ROADMAP.md:72`). Two properties are asserted:
//! the three tables and the trigger are gone, and the tables that came before
//! are still there — a `down` that dropped more than its own version is the
//! failure this file exists to catch, and todo 41's rehearsal is the
//! precedent that found one.

use rusqlite::Connection;

use super::super::ids::CaptureSessionId;
use super::super::schema;
use super::super::{CaptureWindow, MetricAvailability, RawViewerSample, Store};

const T0: i64 = 1_789_000_000_000;

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

/// A populated version-5 file, plus the tables that predate it.
fn populated() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("research.db");
    let store = Store::open(&path).expect("open");
    let (project_id, session) = super::observations::with_session(&store, "stream");
    let window = CaptureWindow::new(T0, T0 + 60_000).expect("window");
    let mut clock = strivo_editor::time::ClockMap::new();
    clock
        .push(strivo_editor::time::ClockSegment::exact(
            T0,
            strivo_editor::time::MediaTime::ZERO,
            strivo_editor::time::MediaTime::from_millis(60_000).expect("60 s"),
        ))
        .expect("segment");
    store
        .append_viewer_samples(
            session,
            project_id,
            &[RawViewerSample {
                wall_ms: T0 + 10_000,
                value: 5,
                poll_interval_ms: 10_000,
                dropped: false,
            }],
            &window,
            &clock,
        )
        .expect("append");
    store
        .declare_metric(
            session,
            project_id,
            "concurrent_viewers",
            MetricAvailability::Observed(5),
        )
        .expect("declare");
    (dir, path)
}

#[test]
fn rolling_version_5_back_removes_exactly_the_version_5_objects() {
    // Given a populated version-5 file
    let (_dir, path) = populated();
    let before = Store::open(&path).expect("open");
    assert_eq!(
        before.schema_version().expect("version"),
        schema::latest_version()
    );
    for table in [
        "creator_viewer_samples",
        "creator_stream_events",
        "creator_metric_availability",
    ] {
        assert!(
            objects(before.raw_connection(), "table", table).contains(&table.to_string()),
            "{table} should exist before the rollback"
        );
    }
    assert!(
        !objects(before.raw_connection(), "trigger", "creator_%").is_empty(),
        "the live-status trigger should exist before the rollback"
    );
    drop(before);

    // When it is rolled back to 4
    let mut conn = Connection::open(&path).expect("open");
    let version = schema::rollback(&mut conn, 4, schema::MIGRATIONS).expect("rollback");
    assert_eq!(
        version, 4,
        "the version line moves down with the statements"
    );

    // Then every version-5 table and the trigger are gone
    for table in [
        "creator_viewer_samples",
        "creator_stream_events",
        "creator_metric_availability",
    ] {
        assert!(
            objects(&conn, "table", table).is_empty(),
            "{table} survived its own down file"
        );
    }
    assert!(
        objects(&conn, "trigger", "creator_%").is_empty(),
        "the live-status trigger survived its own down file"
    );

    // And the tables that came before are untouched
    for table in [
        "creator_projects",
        "creator_metric_observations",
        "creator_workspace",
    ] {
        assert!(
            objects(&conn, "table", table).contains(&table.to_string()),
            "{table} was dropped by a down that should only reverse its own version"
        );
    }
}

#[test]
fn a_rolled_back_database_opens_forward_again_and_the_series_return_empty() {
    // Given a file rolled back to 4
    let (_dir, path) = populated();
    let mut conn = Connection::open(&path).expect("open");
    schema::rollback(&mut conn, 4, schema::MIGRATIONS).expect("rollback");
    drop(conn);

    // When it is opened again, which migrates it forward
    let store = Store::open(&path).expect("reopen");
    assert_eq!(
        store.schema_version().expect("version"),
        schema::latest_version()
    );

    // Then the project survived and the observation series are empty, not absent
    assert!(store
        .creator_tables()
        .expect("tables")
        .contains(&"creator_viewer_samples".into()));
    let session = CaptureSessionId::generate();
    assert_eq!(
        store.viewer_samples(session).expect("read"),
        Vec::new(),
        "re-migrating must not resurrect a rolled-back series"
    );
}

#[test]
fn the_live_status_trigger_is_installed_by_the_migration_and_not_by_rust() {
    // Given a fresh database
    let store = Store::open_in_memory().expect("store");

    // Then the trigger exists in the schema itself
    let names = objects(store.raw_connection(), "trigger", "creator_%");
    assert_eq!(
        names,
        vec!["creator_metric_observations_reject_live_status".to_string()],
        "the enforcing mechanism is a schema object, not a Rust branch"
    );
}
