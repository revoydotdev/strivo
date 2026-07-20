use serde_json::json;

use super::{NewSignal, SignalQuery, SignalStore, SignalStoreError};

fn temp_db_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "strivo_signal_store_test_{name}_{}.db",
        uuid::Uuid::new_v4()
    ))
}

fn sample(
    recording_id: &str,
    kind: &str,
    t_start: f64,
    t_end: f64,
    source_plugin: &str,
) -> NewSignal {
    NewSignal {
        recording_id: recording_id.to_string(),
        t_start,
        t_end,
        kind: kind.to_string(),
        label: format!("{kind} label"),
        payload: json!({"kind": kind, "t_start": t_start}),
        confidence: 0.75,
        source_plugin: source_plugin.to_string(),
    }
}

/// Independently inspects the on-disk schema without going through
/// `SignalStore` (which deliberately doesn't expose its raw connection),
/// so the assertion is a real check of what migration wrote, not just an
/// inference from round-tripping through the typed API.
fn assert_signals_table_present(path: &std::path::Path) {
    let conn = rusqlite::Connection::open(path).expect("path should be a valid sqlite db");
    let sql: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'signals'",
            [],
            |row| row.get(0),
        )
        .expect("signals table must exist after open");
    for column in [
        "recording_id",
        "t_start",
        "t_end",
        "kind",
        "label",
        "payload",
        "confidence",
        "source_plugin",
    ] {
        assert!(
            sql.contains(column),
            "signals table definition missing column {column}: {sql}"
        );
    }
    let user_version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("user_version pragma should be readable");
    assert!(user_version >= 1, "schema_version must be tracked and >= 1");
}

#[test]
fn signal_store_schema_creates_table_and_is_idempotent_on_reopen() {
    let path = temp_db_path("schema");

    {
        let _store = SignalStore::open(&path).expect("first open should create schema");
        assert_signals_table_present(&path);
    }

    // Re-open the same file. Migration must be idempotent: no error, the
    // schema is still present, and previously written data (once we write
    // some) must survive.
    let store = SignalStore::open(&path).expect("re-open must succeed");
    assert_signals_table_present(&path);
    let ids = store
        .write_signals(&[sample("rec-schema", "chapter", 0.0, 1.0, "chapters")])
        .expect("write after re-open should succeed");
    assert_eq!(ids.len(), 1);

    // Open a third time and confirm the row from the second open persisted
    // and the schema is still usable — proves migration didn't reset state.
    let store = SignalStore::open(&path).expect("third open must succeed");
    let rows = store
        .query_signals(&SignalQuery::new().recording_id("rec-schema"))
        .expect("query after multiple opens should succeed");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].kind, "chapter");

    std::fs::remove_file(&path).ok();
}

#[test]
fn signal_store_write_persists_valid_and_rejects_invalid_provenance() {
    let store = SignalStore::open_in_memory().expect("open_in_memory should succeed");

    // Valid signal persists.
    let ids = store
        .write_signals(&[sample("rec-1", "cuepoint", 1.0, 2.0, "cuepoints")])
        .expect("valid signal should be accepted");
    assert_eq!(ids.len(), 1);
    let rows = store
        .query_signals(&SignalQuery::new().recording_id("rec-1"))
        .expect("query should succeed");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].source_plugin, "cuepoints");
    assert_eq!(rows[0].payload, json!({"kind": "cuepoint", "t_start": 1.0}));

    // Confidence above 1.0 is rejected, not clamped.
    let mut bad = sample("rec-1", "cuepoint", 3.0, 4.0, "cuepoints");
    bad.confidence = 1.5;
    match store.write_signals(&[bad]) {
        Err(SignalStoreError::InvalidConfidence(c)) => assert_eq!(c, 1.5),
        other => panic!("expected InvalidConfidence, got {other:?}"),
    }

    // Confidence below 0.0 is rejected.
    let mut bad = sample("rec-1", "cuepoint", 3.0, 4.0, "cuepoints");
    bad.confidence = -0.1;
    match store.write_signals(&[bad]) {
        Err(SignalStoreError::InvalidConfidence(c)) => assert_eq!(c, -0.1),
        other => panic!("expected InvalidConfidence, got {other:?}"),
    }

    // Empty source_plugin is rejected.
    let bad = sample("rec-1", "cuepoint", 3.0, 4.0, "");
    match store.write_signals(&[bad]) {
        Err(SignalStoreError::EmptySourcePlugin) => {}
        other => panic!("expected EmptySourcePlugin, got {other:?}"),
    }

    // Whitespace-only source_plugin is also rejected.
    let bad = sample("rec-1", "cuepoint", 3.0, 4.0, "   ");
    match store.write_signals(&[bad]) {
        Err(SignalStoreError::EmptySourcePlugin) => {}
        other => panic!("expected EmptySourcePlugin, got {other:?}"),
    }

    // The rejected writes must not have persisted any rows.
    let rows = store
        .query_signals(&SignalQuery::new().recording_id("rec-1"))
        .expect("query should succeed");
    assert_eq!(rows.len(), 1, "invalid batches must not partially persist");
}

#[test]
fn signal_store_query_filters_by_recording_kind_and_time_range() {
    let store = SignalStore::open_in_memory().expect("open_in_memory should succeed");

    store
        .write_signals(&[
            sample("rec-a", "chapter", 0.0, 10.0, "chapters"),
            sample("rec-a", "cuepoint", 5.0, 6.0, "cuepoints"),
            sample("rec-a", "cuepoint", 20.0, 21.0, "cuepoints"),
            sample("rec-b", "chapter", 0.0, 10.0, "chapters"),
        ])
        .expect("writing the spread of signals should succeed");

    // Filter by recording_id.
    let by_recording = store
        .query_signals(&SignalQuery::new().recording_id("rec-b"))
        .expect("query by recording should succeed");
    assert_eq!(by_recording.len(), 1);
    assert_eq!(by_recording[0].recording_id, "rec-b");

    // Filter by kind (scoped across recordings).
    let by_kind = store
        .query_signals(&SignalQuery::new().kind("cuepoint"))
        .expect("query by kind should succeed");
    assert_eq!(by_kind.len(), 2);
    assert!(by_kind.iter().all(|s| s.kind == "cuepoint"));

    // Filter by recording_id + kind together.
    let combined = store
        .query_signals(&SignalQuery::new().recording_id("rec-a").kind("chapter"))
        .expect("combined query should succeed");
    assert_eq!(combined.len(), 1);
    assert_eq!(combined[0].t_start, 0.0);

    // Filter by time-range overlap: [4.0, 7.0] overlaps the rec-a cuepoint
    // at [5.0, 6.0] and the rec-a/rec-b chapters at [0.0, 10.0], but not the
    // rec-a cuepoint at [20.0, 21.0].
    let by_range = store
        .query_signals(&SignalQuery::new().time_range(4.0, 7.0))
        .expect("query by time range should succeed");
    assert_eq!(by_range.len(), 3);
    assert!(by_range
        .iter()
        .all(|s| !(s.t_start == 20.0 && s.t_end == 21.0)));

    // Filter by time-range that only the far cuepoint overlaps.
    let by_range_far = store
        .query_signals(
            &SignalQuery::new()
                .recording_id("rec-a")
                .time_range(19.0, 22.0),
        )
        .expect("query by far time range should succeed");
    assert_eq!(by_range_far.len(), 1);
    assert_eq!(by_range_far[0].t_start, 20.0);

    // A range that overlaps nothing returns an empty result.
    let by_range_none = store
        .query_signals(&SignalQuery::new().time_range(100.0, 200.0))
        .expect("query by non-overlapping range should succeed");
    assert!(by_range_none.is_empty());
}
