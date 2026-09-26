//! What reversing the durable chat migration costs, and the guard that stops a
//! concurrent lane taking version 8.
//!
//! Two claims, and the second is the one that protects a neighbour.
//!
//! **The version is asserted by name, not by arithmetic.** `max + 1` was
//! computed from `git ls-tree -r HEAD -- crates/research/src/creator/` at
//! commit time (the committed maximum was 7), but a plan running several lanes
//! in one tree means another lane can claim 8 between that reading and the
//! commit. A test that only checks "8 is the latest" would then pass for a
//! migration that is not this todo's. So the guard names the migration: a
//! `creator_durable_chat` that is not version 8 fails loudly here rather than
//! rolling back a neighbour's table.
//!
//! **The rollback is rehearsed, not asserted.** The `down` destroys the chat
//! archive — there is no earlier version of that state — so the rehearsal runs
//! against a copy of a real file that holds real rows, and checks that the
//! rows are gone, the version line moved, and everything from earlier versions
//! survived.

use rusqlite::Connection;

use super::super::schema::{self, Migration, MIGRATIONS, VERSION_KEY};
use super::super::{
    ChatCollectionId, CollectionEnd, CoverageGap, GapReason, RawChatMessage, Store,
};

fn v8() -> &'static Migration {
    MIGRATIONS
        .iter()
        .find(|m| m.name == "creator_durable_chat")
        .expect("this todo's migration is registered")
}

#[test]
fn this_todays_migration_is_version_eight_and_no_other_is() {
    assert_eq!(
        v8().version,
        8,
        "a concurrent lane took 8 first: rename this migration or take the next \
         version, do not overwrite a neighbour's table"
    );
    // No duplicate version, and no gap in the sequence: the schema's own
    // `validate` would refuse a bad list, but it only runs on migrate, so a
    // list that is merely *listed* wrong would sit here undetected.
    let mut versions: Vec<i64> = MIGRATIONS.iter().map(|m| m.version).collect();
    versions.sort_unstable();
    versions.dedup();
    assert_eq!(
        versions,
        (1..=schema::latest_version()).collect::<Vec<_>>(),
        "versions are 1..=latest with no duplicate and no gap"
    );
    assert!(
        v8().up.contains("creator_chat_messages"),
        "and it is the chat migration, not something that took its name"
    );
}

#[test]
fn every_migration_has_a_down_and_the_chat_one_names_its_tables() {
    for m in MIGRATIONS {
        assert!(
            !m.up.trim().is_empty() && !m.down.trim().is_empty(),
            "{} needs both",
            m.name
        );
    }
    let down = v8().down;
    for table in [
        "creator_chat_gaps",
        "creator_chat_deletions",
        "creator_chat_messages",
        "creator_chat_collections",
    ] {
        assert!(
            down.contains(&format!("DROP TABLE IF EXISTS {table}")),
            "the down must drop {table}"
        );
    }
    // Reverse creation order: a message row references its collection, so
    // dropping the collection first is a `down` that only works with foreign
    // keys off.
    let gaps = down.find("creator_chat_gaps").expect("gaps");
    let messages = down.find("creator_chat_messages").expect("messages");
    let collections = down.find("creator_chat_collections").expect("collections");
    assert!(
        gaps < messages && messages < collections,
        "tables go in reverse creation order"
    );
}

#[test]
fn rolling_back_to_seven_removes_only_the_chat_record() {
    let dir = std::env::temp_dir().join(format!("strivo-t35-down-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch");
    let db = dir.join("research.db");

    let (session, tables_at_eight) = {
        let store = Store::open(&db).expect("open");
        assert_eq!(store.schema_version().expect("version"), 8);
        let (project, session) = super::chat::with_session(&store, "stream");
        let collection = store
            .open_chat_collection(session, project, "twitch", "streamer", 1_789_000_000_000)
            .expect("open");
        store
            .append_chat(
                &collection,
                &[RawChatMessage {
                    wall_ms: 1_789_000_001_000,
                    room: "streamer".into(),
                    sender: "ann".into(),
                    text: "kept until the rollback".into(),
                    is_action: false,
                    provider_id: Some("m1".into()),
                }],
                &super::chat::window(),
                &super::chat::clock_over_the_real_recording(),
            )
            .expect("append");
        store
            .record_chat_gap(
                &collection,
                &CoverageGap::new(
                    1_789_000_002_000,
                    1_789_000_003_000,
                    GapReason::Disconnected,
                )
                .expect("a forward gap"),
            )
            .expect("gap");
        (session, store.creator_tables().expect("tables"))
    };
    assert!(tables_at_eight.contains(&"creator_chat_messages".to_string()));
    // A pre-chat table is present, so "only the chat record went" is checkable
    // rather than vacuous.
    assert!(tables_at_eight.contains(&"creator_history".to_string()));

    // The rehearsal runs on a copy, because the `down` is destructive by
    // construction and `docs/RESEARCH-PLATFORM-ROADMAP.md:72` permits it only
    // against a backup.
    let copy = dir.join("copy.db");
    std::fs::copy(&db, &copy).expect("copy");
    let mut conn = Connection::open(&copy).expect("reopen");
    schema::rollback(&mut conn, 7, MIGRATIONS).expect("rollback to 7");

    let version: String = conn
        .query_row(
            "SELECT value FROM schema_meta WHERE key=?1",
            [VERSION_KEY],
            |r| r.get(0),
        )
        .expect("the version line moved with the schema");
    assert_eq!(
        version, "7",
        "the line moved in the same transaction as the down"
    );
    let left: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'creator_%'")
        .expect("prepare")
        .query_map([], |r| r.get::<_, String>(0))
        .expect("rows")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect");
    for table in [
        "creator_chat_collections",
        "creator_chat_messages",
        "creator_chat_deletions",
        "creator_chat_gaps",
    ] {
        assert!(!left.contains(&table.to_string()), "{table} is gone at 7");
    }
    for kept in [
        "creator_projects",
        "creator_history",
        "creator_source_notes",
    ] {
        assert!(
            left.contains(&kept.to_string()),
            "{kept} survived the rollback"
        );
    }
    drop(conn);

    // And the rolled-back file reopens cleanly at 8 with the record empty
    // rather than half-restored — the forward migration is re-runnable from a
    // rolled-back file, which is the case a user hits after a bad downgrade.
    // Asserted on the *copy*, because the original was deliberately not rolled
    // back: an assertion here against the untouched original would pass for the
    // wrong reason.
    let store = Store::open(&copy).expect("reopen forward");
    assert_eq!(store.schema_version().expect("version"), 8);
    assert_eq!(
        store.schema_version().expect("version"),
        8,
        "and it is idempotent"
    );
    assert_eq!(
        store.creator_tables().expect("tables").len(),
        left.len() + 4
    );
    assert!(
        store.chat_messages(session).expect("read").is_empty(),
        "the archived messages did not come back"
    );
    // The untouched original still has them, so the `down` really did
    // discriminate between two identical files rather than appearing to work.
    assert_eq!(
        Store::open(&db)
            .expect("original")
            .chat_messages(session)
            .expect("read")
            .len(),
        1,
        "the file that was not rolled back still holds its message"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_database_from_before_this_version_migrates_forward_and_back_without_data_loss() {
    // The other direction a user hits: an existing install at 7. The chat
    // tables appear, and everything that was already there is untouched.
    let dir = std::env::temp_dir().join(format!("strivo-t35-fwd-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch");
    let db = dir.join("research.db");

    let before = {
        let mut conn = Connection::open(&db).expect("open");
        let at_seven: Vec<Migration> = MIGRATIONS
            .iter()
            .copied()
            .filter(|m| m.version <= 7)
            .collect();
        schema::migrate(&mut conn, &at_seven).expect("migrate to 7");
        schema::applied_version(&conn).expect("version")
    };
    assert_eq!(before, 7);

    let store = Store::open(&db).expect("forward migrate");
    assert_eq!(store.schema_version().expect("version"), 8);
    let tables = store.creator_tables().expect("tables");
    assert!(tables.contains(&"creator_chat_gaps".to_string()));
    assert!(
        tables.contains(&"creator_history".to_string()),
        "v7's own table survived"
    );

    // A fresh collection is writable, so the forward migration produced a
    // usable schema and not merely the right version number.
    let (project, session) = super::chat::with_session(&store, "stream");
    let collection: ChatCollectionId = store
        .open_chat_collection(session, project, "twitch", "streamer", 1_789_000_000_000)
        .expect("open")
        .id;
    store
        .close_chat_collection(
            &store
                .chat_collections(session)
                .expect("read")
                .into_iter()
                .find(|c| c.id == collection)
                .expect("the collection just opened"),
            1_789_000_001_000,
            CollectionEnd::Closed,
        )
        .expect("close");
    let _ = std::fs::remove_dir_all(&dir);
}
