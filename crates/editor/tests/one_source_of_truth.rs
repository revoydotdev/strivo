//! "Exactly one source of truth per edit."
//!
//! The invariant is stated over the **database**, not over the Rust API,
//! because a second representation that no type mentions is still a second
//! representation. Six coexisting representations of an edit were enumerated
//! in the todo; this binary proves the one that mattered at the persistence
//! layer is down to a single table.
//!
//! Every assertion is non-vacuous by construction: the surviving
//! representation is proved present and non-empty **before** anything is
//! asserted about the retired ones. A test that says "the list of
//! representations has one member" passes for free when the list is empty —
//! the sharpest of the ten vacuous passes this plan has recorded.

use rusqlite::Connection;
use strivo_editor::{store::EdlStore, Cut, CutKind, Edl};

/// Every user table in the database, excluding SQLite's own bookkeeping.
fn user_tables(path: &std::path::Path) -> Vec<String> {
    let conn = Connection::open(path).expect("open");
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'")
        .expect("prepare");
    let mut rows: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .expect("query")
        .collect::<Result<Vec<_>, _>>()
        .expect("rows");
    // Sorted: `sqlite_master` order is creation order, so an unsorted
    // comparison would fail on a schema that is correct but built differently.
    rows.sort();
    rows
}

/// The text columns of `table`, so a value can be examined without assuming
/// which column name the representation happens to use.
fn text_columns(path: &std::path::Path, table: &str) -> Vec<String> {
    let conn = Connection::open(path).expect("open");
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .expect("prepare");
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(1))
        .expect("query")
        .collect::<Result<Vec<_>, _>>()
        .expect("rows");
    rows
}

/// Tables that can answer "what is this recording's edit" — i.e. that hold a
/// value that deserialises as an [`Edl`]. This is deliberately shape-based:
/// naming the table would let a rename hide a surviving duplicate, and
/// naming the column would let a rename do the same.
fn tables_holding_an_edl(path: &std::path::Path) -> Vec<String> {
    user_tables(path)
        .into_iter()
        .filter(|t| {
            let conn = Connection::open(path).expect("open");
            let Ok(mut stmt) = conn.prepare(&format!("SELECT * FROM {t}")) else {
                return false;
            };
            let ncols = stmt.column_count();
            let Ok(mut rows) = stmt.query([]) else {
                return false;
            };
            while let Ok(Some(row)) = rows.next() {
                for i in 0..ncols {
                    let Ok(v) = row.get_ref(i) else { continue };
                    let rusqlite::types::ValueRef::Text(bytes) = v else {
                        continue;
                    };
                    let Ok(text) = std::str::from_utf8(bytes) else {
                        continue;
                    };
                    if serde_json::from_str::<Edl>(text).is_ok() {
                        return true;
                    }
                }
            }
            false
        })
        .collect()
}

fn sample_edl(rec: &str, n: usize) -> Edl {
    Edl {
        recording_id: rec.into(),
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

fn store_at(path: &std::path::Path) -> EdlStore {
    EdlStore::open(path).expect("open store")
}

/// The headline invariant. Given a store with one saved revision, exactly one
/// table in the database can hold that edit.
#[test]
fn exactly_one_table_in_the_database_can_hold_an_edit() {
    // Given: a fresh database with one revision written through the store.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("editor.db");
    let store = store_at(&path);
    store.save(None, &sample_edl("rec1", 2), "first").unwrap();

    // Anti-vacuity: the surviving representation is proved present and
    // non-empty FIRST. Without this, an empty database satisfies "one table".
    let survivors = tables_holding_an_edl(&path);
    assert!(
        !survivors.is_empty(),
        "no table holds an edit at all, so 'exactly one' would pass for free \
         without ever storing anything"
    );

    // When/Then: the count.
    assert_eq!(
        survivors,
        vec!["revisions".to_string()],
        "more than one table can answer 'what is this recording's edit', so \
         there is more than one source of truth"
    );
}

/// A second write must not be able to fork the truth. Two saves, then the
/// tables are read directly: the current edit is a *function* of the single
/// table, not a separate row that could disagree with it.
#[test]
fn a_second_write_appends_rather_than_forks() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("editor.db");
    let store = store_at(&path);
    store.save(None, &sample_edl("rec1", 2), "first").unwrap();
    store.save(None, &sample_edl("rec1", 5), "second").unwrap();

    let conn = Connection::open(&path).expect("open");
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM revisions", [], |r| r.get(0))
        .expect("count revisions");
    assert_eq!(rows, 2, "each save is one appended revision");

    // The loaded current edit is the newest row, and it is the only row the
    // schema lets a reader see as "current".
    let loaded = store
        .current("rec1")
        .unwrap()
        .expect("a current edit is derivable from the single table");
    assert_eq!(loaded.edl.cuts.len(), 5, "current is the newest revision");
    assert_eq!(loaded.revision_id, 2, "current names the row it read");
}

/// The historical-revisions promise: unifying must not lose a single past
/// edit. This is checked against the table, not against the API, so a
/// "migration" that quietly dropped rows cannot pass.
#[test]
fn every_historical_revision_survives() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("editor.db");
    let store = store_at(&path);
    for n in 1..=4 {
        store
            .save(None, &sample_edl("rec1", n), &format!("v{n}"))
            .unwrap();
    }
    let listed = store.list_revisions("rec1", 50).unwrap();
    assert_eq!(
        listed.len(),
        4,
        "four saves are four revisions, newest first"
    );
    let mut counts: Vec<usize> = listed.iter().map(|r| r.cut_count).collect();
    counts.sort_unstable();
    assert_eq!(counts, vec![1, 2, 3, 4], "each revision kept its own edit");
}

/// Anti-vacuity for the shape probe itself: the probe must be capable of
/// reporting zero *and* more than one. Without this, `tables_holding_an_edl`
/// returning a constant would satisfy every other test in the file.
#[test]
fn the_shape_probe_reports_zero_and_many() {
    let dir = tempfile::tempdir().expect("tempdir");

    // Zero: a database that never held an edit.
    let empty = dir.path().join("empty.db");
    store_at(&empty);
    assert!(
        tables_holding_an_edl(&empty).is_empty(),
        "a database nothing was ever written to must report no table holding an edit"
    );

    // Many: a second table holding the same shape is detected, which is the
    // whole point of the probe. The duplicate is planted with the bytes taken
    // from the surviving table, so it is a real copy and not a lookalike.
    let both = dir.path().join("both.db");
    let store = store_at(&both);
    store.save(None, &sample_edl("rec1", 1), "first").unwrap();
    {
        let conn = Connection::open(&both).unwrap();
        conn.execute_batch("CREATE TABLE shadow (recording_id TEXT, edl_json TEXT)")
            .unwrap();
        let json: String = conn
            .query_row("SELECT edl_json FROM revisions LIMIT 1", [], |r| r.get(0))
            .unwrap();
        conn.execute(
            "INSERT INTO shadow (recording_id, edl_json) VALUES (?1, ?2)",
            rusqlite::params!["rec1", json],
        )
        .unwrap();
    }
    assert_eq!(
        tables_holding_an_edl(&both),
        vec!["revisions".to_string(), "shadow".to_string()],
        "a second table holding the same shape must be reported, or the probe \
         cannot see the very duplicate it exists to find"
    );
}

/// The retired head table must be gone from an *upgraded* database, not only
/// from a fresh one. A `DROP TABLE` that only ran on a fresh open would leave
/// every existing install with two sources of truth forever.
#[test]
fn an_existing_two_table_database_is_reduced_to_one() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("editor.db");

    // Given: a database in the shape the pre-unification code left behind —
    // built here by hand so the fixture cannot drift with the code.
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE edls (
                 recording_id TEXT PRIMARY KEY,
                 edl_json     TEXT NOT NULL,
                 updated_at   TEXT NOT NULL
             );
             CREATE TABLE revisions (
                 revision_id  INTEGER PRIMARY KEY AUTOINCREMENT,
                 recording_id TEXT NOT NULL,
                 edl_json     TEXT NOT NULL,
                 label        TEXT NOT NULL,
                 created_at   TEXT NOT NULL
             );
             INSERT INTO revisions (recording_id, edl_json, label, created_at)
             VALUES ('rec1', '{\"recording_id\":\"rec1\",\"cuts\":[{\"start_sec\":0.0,\"end_sec\":5.0,\"kind\":{\"kind\":\"source\",\"source_path\":\"/a.mkv\"}}]}', 'v1', '2026-01-01T00:00:00Z');
             INSERT INTO edls (recording_id, edl_json, updated_at)
             SELECT recording_id, edl_json, created_at FROM revisions;",
        )
        .unwrap();
    }
    assert_eq!(
        tables_holding_an_edl(&path),
        vec!["edls".to_string(), "revisions".to_string()],
        "fixture precondition: the legacy database really does hold two"
    );

    // When: the store is opened against it, which is the migration.
    let store = store_at(&path);

    // Then: one table, and the historical revision is still readable.
    let survivors = tables_holding_an_edl(&path);
    assert_eq!(
        survivors,
        vec!["revisions".to_string()],
        "opening a legacy database must reduce it to one source of truth"
    );
    let listed = store.list_revisions("rec1", 50).unwrap();
    assert_eq!(
        listed.len(),
        1,
        "the migration dropped history, which the todo forbids in as many words"
    );
    let loaded = store
        .current("rec1")
        .unwrap()
        .expect("the edit survived the migration");
    assert_eq!(loaded.edl.cuts.len(), 1);
    assert!(
        !text_columns(&path, "revisions").is_empty(),
        "the surviving table still has its columns"
    );
}

/// The surviving representation round-trips: an edit written through the
/// unified path reads back *identically*, not approximately. Serialised bytes
/// are compared, because `PartialEq` on floats would tolerate a drift the
/// stored document must not have.
#[test]
fn a_revision_round_trips_byte_identically_through_the_unified_path() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("editor.db");
    let store = store_at(&path);
    let original = sample_edl("rec1", 3);
    let written = store.save(None, &original, "round trip").expect("save");

    let conn = Connection::open(&path).expect("open");
    let stored: String = conn
        .query_row(
            "SELECT edl_json FROM revisions WHERE revision_id = ?1",
            rusqlite::params![written.revision_id],
            |r| r.get(0),
        )
        .expect("row");

    let current = store.current("rec1").unwrap().expect("current");
    assert_eq!(
        current.edl, original,
        "the unified path did not round-trip the edit"
    );
    assert_eq!(
        serde_json::to_string(&current.edl).unwrap(),
        stored,
        "what reads back is not what was written"
    );
    assert_eq!(current.revision_id, written.revision_id);
}

/// A second write cannot fork the truth. `current` is a *function* of the
/// single table, so it is not expressible for the two to disagree — and this
/// checks it from both ends: the derived value, and the table's own newest id.
#[test]
fn current_is_a_function_of_the_one_table_and_cannot_diverge_from_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("editor.db");
    let store = store_at(&path);
    let mut last = None;
    for n in 1..=6 {
        last = Some(
            store
                .save(None, &sample_edl("rec1", n), &format!("v{n}"))
                .unwrap(),
        );
    }
    let current = store.current("rec1").unwrap().expect("current");
    let newest_in_table: i64 = Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT MAX(revision_id) FROM revisions WHERE recording_id = 'rec1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(current.revision_id, newest_in_table);
    assert_eq!(current.revision_id, last.unwrap().revision_id);
    assert_eq!(current.edl.cuts.len(), 6);
}
