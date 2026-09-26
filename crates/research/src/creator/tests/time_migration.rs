//! The version 1 -> version 2 data migration and its reverse, asserted on the
//! **values** rather than on the version number.
//!
//! A migration can apply cleanly, move the version, create every table and
//! still leave every value at zero. A test that only asserts the version moved
//! cannot tell that from success, so each test here reads the rows back.

use rusqlite::Connection;
use strivo_editor::time::MediaTime;
use tempfile::TempDir;

use super::super::schema::{self, MIGRATIONS};
use super::super::store::Store;
use super::time_shared::{at_version_one, seed_v1_source, source_duration_us};

#[test]
fn a_version_one_database_migrates_its_durations_to_microseconds() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("research.db");
    // Real UUIDs, because the last assertion reads a row back through the
    // typed store, which parses ids rather than comparing strings.
    let (a, b, c) = (
        uuid::Uuid::new_v4().to_string(),
        uuid::Uuid::new_v4().to_string(),
        uuid::Uuid::new_v4().to_string(),
    );
    let project_a = uuid::Uuid::new_v4().to_string();
    {
        let conn = at_version_one(&path);
        // Two hours, one hour, and an unknown duration.
        seed_v1_source(&conn, &a, &project_a, Some(7_200_000));
        seed_v1_source(
            &conn,
            &b,
            &uuid::Uuid::new_v4().to_string(),
            Some(3_600_000),
        );
        seed_v1_source(&conn, &c, &uuid::Uuid::new_v4().to_string(), None);
        conn.execute(
            "INSERT INTO creator_artifacts
             (id,project_id,variant_id,render_plan_id,kind,content_hash,locator,bytes,
              width,height,duration_ms,created_at)
             VALUES(?1,?2,NULL,NULL,'render','sha256:b',NULL,10,1080,1920,
                     90000,'2026-09-07T00:00:00Z')",
            rusqlite::params![uuid::Uuid::new_v4().to_string(), project_a],
        )
        .expect("artifact");
    }

    let mut conn = Connection::open(&path).expect("reopen");
    let art: String = conn
        .query_row("SELECT id FROM creator_artifacts LIMIT 1", [], |r| r.get(0))
        .expect("the artifact id");
    let before: i64 = conn
        .query_row(
            "SELECT duration_ms FROM creator_source_assets WHERE id=?1",
            [&a],
            |r| r.get(0),
        )
        .expect("the v1 value is readable");
    assert_eq!(before, 7_200_000, "the fixture starts in milliseconds");

    let version = schema::migrate(&mut conn, MIGRATIONS).expect("every version applies");
    assert_eq!(
        version,
        schema::latest_version(),
        "the whole list applied, not just the time model"
    );

    assert_eq!(
        source_duration_us(&conn, &a),
        Some(7_200_000_000),
        "two hours is 7 200 000 000 microseconds, not 7 200 000 and not 0"
    );
    assert_eq!(
        source_duration_us(&conn, &b),
        Some(3_600_000_000),
        "one hour migrates to the value it denotes"
    );
    assert_eq!(
        source_duration_us(&conn, &c),
        None,
        "an unknown duration stays unknown rather than becoming a zero"
    );
    let artifact: Option<i64> = conn
        .query_row(
            "SELECT duration_us FROM creator_artifacts WHERE id=?1",
            [art],
            |r| r.get(0),
        )
        .expect("query");
    assert_eq!(
        artifact,
        Some(90_000_000),
        "an artifact's duration migrates on the same terms"
    );
    // And the value the typed store reads back is the migrated one.
    let store = Store::from_connection(conn);
    let source = store
        .source_asset(a.parse().expect("a typed id"))
        .expect("lookup ok")
        .expect("the row is still there");
    assert_eq!(
        source.duration,
        Some(MediaTime::from_millis(7_200_000).expect("a valid duration")),
        "the typed read agrees with the raw column"
    );
}

#[test]
fn the_version_two_down_reverses_the_conversion_and_its_loss() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("research.db");
    let mut conn = at_version_one(&path);
    let project = uuid::Uuid::new_v4().to_string();
    let asset = uuid::Uuid::new_v4().to_string();
    let session = uuid::Uuid::new_v4().to_string();
    seed_v1_source(&conn, &asset, &project, Some(1_000));
    // Only the first two migrations, so the fixture really is the version 2
    // database this test reverses. Migrating the whole list would put it at
    // the latest version and the `down` under test would never run.
    schema::migrate(&mut conn, &MIGRATIONS[..2]).expect("version 2 applies");
    // 1 500 us is not a whole number of milliseconds, so the reverse
    // conversion cannot be exact.
    conn.execute(
        "UPDATE creator_source_assets SET duration_us=1500 WHERE id=?1",
        [&asset],
    )
    .expect("sub-millisecond value");
    conn.execute(
        "INSERT INTO creator_capture_sessions
         (id,project_id,source_asset_id,started_at,ended_at,finalized,attributes_json,created_at)
         VALUES(?1,?2,?3,'2026-09-07T00:00:00Z',NULL,0,'{}','2026-09-07T00:00:00Z')",
        rusqlite::params![session, project, asset],
    )
    .expect("capture session");
    conn.execute(
        "INSERT INTO creator_clock_segments
         (id,capture_session_id,project_id,seq,wall_start_ms,media_start_us,media_len_us,
          rate_num,rate_den)
         VALUES(?1,?2,?3,0,1700000000000,0,3600000000,1,1)",
        rusqlite::params![uuid::Uuid::new_v4().to_string(), session, project],
    )
    .expect("clock segment");
    assert_eq!(
        schema::applied_version(&conn).expect("version"),
        2,
        "the fixture is at version 2 with data in it"
    );

    let rolled = schema::rollback(&mut conn, 1, MIGRATIONS).expect("the down applies");
    assert_eq!(rolled, 1, "and the version line says so");

    let left: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type='table' AND name='creator_clock_segments'",
            [],
            |r| r.get(0),
        )
        .expect("query");
    assert_eq!(left, 0, "the clock table is gone after the down");

    let back: Option<i64> = conn
        .query_row(
            "SELECT duration_ms FROM creator_source_assets WHERE id=?1",
            [&asset],
            |r| r.get(0),
        )
        .expect("the millisecond column is back");
    assert_eq!(
        back,
        Some(1),
        "1 500 us comes back as 1 ms, and that loss is the documented cost of \
         reversing a unit change rather than a bug in the statement"
    );
    // Re-migrating forward recovers the whole-millisecond values exactly,
    // which is why the roadmap only permits a destructive down from a backup.
    schema::migrate(&mut conn, MIGRATIONS).expect("forward again");
    assert_eq!(
        source_duration_us(&conn, &asset),
        Some(1_000),
        "and a whole-millisecond value survives the round trip exactly"
    );
}
