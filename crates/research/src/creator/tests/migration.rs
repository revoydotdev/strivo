//! Forward migration, idempotent open, and the refusal paths.
//!
//! The interesting subject is not "does the SQL run" — it is "does the schema
//! actually change when it claims to, and is a bad migration list reported".

use rusqlite::Connection;
use tempfile::TempDir;

use super::super::schema::{self, Migration, MIGRATIONS, TABLES, VERSION_KEY};
use super::super::store::Store;
use crate::ResearchError;

pub fn creator_tables(conn: &Connection) -> Vec<String> {
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'creator_%' ORDER BY name")
        .expect("prepare");
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .expect("query");
    rows.map(std::result::Result::unwrap).collect()
}

/// A snapshot of the research kernel's own tables as plain text, so the
/// comparison after a migration is exact and does not lean on the kernel's
/// read API being in the same shape twice.
pub fn kernel_rows(conn: &Connection) -> Vec<String> {
    let queries = [
        "SELECT id||'|'||name||'|'||description FROM projects ORDER BY id",
        "SELECT id||'|'||project_id||'|'||kind||'|'||title FROM sources ORDER BY id",
        "SELECT id||'|'||kind||'|'||label||'|'||start_ms||'|'||end_ms||'|'||COALESCE(confidence,'')
         FROM signals ORDER BY id",
    ];
    let mut out = Vec::new();
    for sql in queries {
        let mut stmt = conn.prepare(sql).expect("prepare");
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .expect("query");
        for row in rows {
            out.push(row.expect("row"));
        }
    }
    out
}

/// A research-kernel database with an edit and its provenance in it: a
/// project, a source, and a confidence-scored signal.
pub fn seed_kernel(path: &std::path::Path) {
    use crate::{NewSignal, SourceKind};
    let store = crate::ResearchStore::open(path).expect("kernel store opens");
    let project = store
        .create_project("archive", "prior work")
        .expect("project");
    let source = crate::NewSource {
        id: uuid::Uuid::new_v4(),
        project_id: project.id,
        recording_id: None,
        kind: SourceKind::Recording,
        title: "2026-09-01".into(),
        uri: Some("file:///archive/2026-09-01.mkv".into()),
        duration_ms: Some(3_600_000),
        attributes: serde_json::json!({"why": "prior work"}),
    };
    store.upsert_source(&source).expect("source inserted");
    store
        .append_signal(&NewSignal {
            id: uuid::Uuid::new_v4(),
            project_id: project.id,
            source_id: source.id,
            start_ms: 0,
            end_ms: 1_000,
            kind: "chapter".into(),
            label: "intro".into(),
            payload: serde_json::json!({"why": "prior work"}),
            confidence: Some(0.9),
            provenance_id: None,
        })
        .expect("signal inserted");
    assert!(path.exists(), "the pre-creator database exists");
}

pub fn version_of(conn: &Connection) -> i64 {
    schema::applied_version(conn).expect("version readable")
}

/// Given a database carrying the research kernel's rows and no Creator schema
/// When the Creator schema is applied
/// Then the version moves 0 -> 2, every Creator table appears, and the
/// kernel's rows are exactly what they were.
#[test]
fn forward_migration_adds_the_schema_and_keeps_prior_state() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("research.db");
    seed_kernel(&path);
    let before = {
        let conn = Connection::open(&path).expect("reopen");
        assert_eq!(version_of(&conn), 0, "the fixture starts at version 0");
        assert!(creator_tables(&conn).is_empty(), "with no Creator tables");
        kernel_rows(&conn)
    };
    assert!(before.len() >= 3, "the fixture really had rows to lose");

    let mut conn = Connection::open(&path).expect("reopen");
    let version = schema::migrate(&mut conn, MIGRATIONS).expect("migration applies");

    // Tracked to the migration list rather than to a number, so appending a
    // version does not silently turn this into a test that asserts a stale
    // value and a migration that applies nothing.
    assert_eq!(
        version,
        schema::latest_version(),
        "the version moved to the latest this build knows"
    );
    assert_eq!(
        version_of(&conn),
        schema::latest_version(),
        "and it is what the file records"
    );
    assert!(
        version > 2,
        "a fresh file really did apply more than the two versions that predate \
         the job lifecycle, so this is not a vacuous pass"
    );
    assert_eq!(
        creator_tables(&conn).len(),
        TABLES.len(),
        "every Creator table exists after migrating"
    );
    assert_eq!(kernel_rows(&conn), before, "prior rows are intact");
}

/// Given a database at the current version
/// When it is opened again
/// Then nothing is applied and the version does not move.
#[test]
fn opening_twice_applies_nothing() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("research.db");

    let first = Store::open(&path).expect("first open");
    let version = first.schema_version().expect("version");
    let tables = first.creator_tables().expect("tables");
    drop(first);

    let second = Store::open(&path).expect("second open");

    assert_eq!(second.schema_version().expect("version"), version);
    assert_eq!(second.creator_tables().expect("tables"), tables);
}

/// Given a migration list with a gap in the numbering
/// When it is applied
/// Then the runner refuses it, before touching the database.
#[test]
fn a_corrupt_migration_list_is_reported() {
    let dir = TempDir::new().expect("tempdir");
    let mut conn = Connection::open(dir.path().join("x.db")).expect("open");
    let corrupt = [
        Migration {
            version: 1,
            name: "good",
            up: "CREATE TABLE a(x);",
            down: "DROP TABLE a;",
        },
        Migration {
            version: 3,
            name: "skips-two",
            up: "CREATE TABLE c(x);",
            down: "DROP TABLE c;",
        },
    ];

    let err = schema::migrate(&mut conn, &corrupt).expect_err("must refuse");

    assert!(
        matches!(err, ResearchError::Validation(_)),
        "a gap is a validation error, got {err:?}"
    );
    assert_eq!(
        version_of(&conn),
        0,
        "and nothing was applied before the refusal"
    );
}

/// Given a migration with no down step
/// When it is validated
/// Then it is refused, because a migration that cannot be reversed is the
/// unrecoverable state the roadmap forbids shipping.
#[test]
fn a_migration_without_a_down_is_refused() {
    let dir = TempDir::new().expect("tempdir");
    let mut conn = Connection::open(dir.path().join("x.db")).expect("open");
    let one_way = [Migration {
        version: 1,
        name: "one-way",
        up: "CREATE TABLE a(x);",
        down: "   ",
    }];

    let err = schema::migrate(&mut conn, &one_way).expect_err("must refuse");

    assert!(
        matches!(err, ResearchError::Validation(_)),
        "a missing down is a validation error, got {err:?}"
    );
    assert_eq!(version_of(&conn), 0, "nothing applied");
}

/// Given a migration whose SQL fails half-way through
/// When it is applied
/// Then the transaction rolls back: no table survives, the version does not
/// move, and the next open converges by re-running it.
#[test]
fn an_interrupted_migration_leaves_the_previous_version_intact() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("x.db");
    let broken = [Migration {
        version: 1,
        name: "half-written",
        up: "CREATE TABLE first(x); CREATE TABLE second(NOT VALID SQL);",
        down: "DROP TABLE IF EXISTS first;",
    }];

    let mut conn = Connection::open(&path).expect("open");
    let err = schema::migrate(&mut conn, &broken).expect_err("must fail");
    assert!(
        matches!(err, ResearchError::Database(_)),
        "a bad statement is a database error, got {err:?}"
    );
    assert_eq!(version_of(&conn), 0, "the version did not move");
    assert!(
        creator_tables(&conn).is_empty(),
        "the half-applied table was rolled back, not left behind"
    );
    drop(conn);

    // The next open converges rather than reporting a stuck migration.
    let store = Store::open(&path).expect("the real migration applies on the next run");
    assert_eq!(
        store.schema_version().expect("version"),
        schema::latest_version(),
        "the interrupted run converges to the latest version, not to the one it died in"
    );
    assert_eq!(store.creator_tables().expect("tables").len(), TABLES.len());
}

/// Given a database written by a newer build
/// When this build opens it
/// Then it is reported as unsupported rather than opened as if understood.
#[test]
fn a_newer_database_is_reported_not_opened() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("x.db");
    {
        let conn = Connection::open(&path).expect("open");
        conn.execute_batch("CREATE TABLE schema_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);")
            .expect("meta");
        conn.execute(
            "INSERT INTO schema_meta(key,value) VALUES(?1,'99')",
            [VERSION_KEY],
        )
        .expect("stamp");
    }

    let err = match Store::open(&path) {
        Ok(_) => panic!("a version 99 database must not open"),
        Err(e) => e,
    };

    match err {
        ResearchError::UnsupportedSchema { found, supported } => {
            assert_eq!(found, 99, "the found version is reported");
            assert_eq!(supported, schema::latest_version(), "and what we support");
        }
        other => panic!("expected UnsupportedSchema, got {other:?}"),
    }
    let conn = Connection::open(&path).expect("reopen");
    assert_eq!(version_of(&conn), 99, "and the database was left untouched");
}

pub fn fs_copy(from: &std::path::Path, to: &std::path::Path) {
    std::fs::copy(from, to)
        .unwrap_or_else(|e| panic!("copy {} -> {}: {e}", from.display(), to.display()));
}
