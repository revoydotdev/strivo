//! Fixtures the version 1 side of the time model needs: a database at
//! version 1, a way to seed the rows version 1 wrote, and a way to read the
//! column version 2 produced.

use rusqlite::Connection;

use super::super::schema::{self, MIGRATIONS};
/// A database carrying **only** version 1, which is what a real install at
/// version 1 looks like: the twelve tables and the millisecond columns.
pub fn at_version_one(path: &std::path::Path) -> Connection {
    let mut conn = Connection::open(path).expect("open");
    conn.execute_batch("PRAGMA foreign_keys=ON;")
        .expect("pragmas");
    schema::migrate(&mut conn, &MIGRATIONS[..1]).expect("version 1 applies");
    assert_eq!(
        schema::applied_version(&conn).expect("version"),
        1,
        "the fixture really is a version 1 database"
    );
    conn
}

pub fn seed_v1_source(conn: &Connection, id: &str, project: &str, duration_ms: Option<i64>) {
    conn.execute(
        "INSERT INTO creator_projects
         (id,name,description,channel,series,settings_json,created_at,updated_at)
         VALUES(?1,'p','','','','{}','2026-09-07T00:00:00Z','2026-09-07T00:00:00Z')",
        [project],
    )
    .expect("project");
    conn.execute(
        "INSERT INTO creator_source_assets
         (id,project_id,kind,title,media_fingerprint,locator,origin,rights_json,
          track_type,codec,duration_ms,created_at)
         VALUES(?1,?2,'recording','s','sha256:a',NULL,'capture','{}','video','h264',?3,
                 '2026-09-07T00:00:00Z')",
        rusqlite::params![id, project, duration_ms],
    )
    .expect("source");
}

pub fn source_duration_us(conn: &Connection, id: &str) -> Option<i64> {
    conn.query_row(
        "SELECT duration_us FROM creator_source_assets WHERE id=?1",
        [id],
        |r| r.get::<_, Option<i64>>(0),
    )
    .expect("query")
}
