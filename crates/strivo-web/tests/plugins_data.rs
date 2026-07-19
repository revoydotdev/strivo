// Creator Edition only: exercises the first-party plugin read path.
#![cfg(feature = "creator")]
//! Integration coverage for the read path behind `/api/v1/plugins/*`.
//!
//! The route handlers are thin: open the plugin's SQLite DB read-only and call
//! the plugin crate's query functions. These tests exercise those functions
//! through their real public API against temp fixtures — the same calls the
//! handlers make — so the data-shaping the SPA depends on is verified without
//! standing up a daemon + auth.

use rusqlite::Connection;
use strivo_core::config::AppConfig;
use strivo_core::plugin::{Plugin, PluginContext};
use strivo_plugins::archiver::db as adb;
use strivo_plugins::crunchr::db as cdb;
use strivo_plugins::insights::frequency;
use strivo_plugins::viewguard::store::{self, VerdictRow, ViewguardStore};
use strivo_plugins::viewguard::ViewguardPlugin;

#[test]
fn crunchr_list_and_detail() {
    let dir = tempfile::tempdir().unwrap();
    let conn = cdb::open_and_init(&dir.path().join("crunchr.db")).unwrap();

    let vid = cdb::insert_video(&conn, "rec-1", "Chan", "Title", "/tmp/a.mkv").unwrap();
    // Types inferred from `insert_segments`' signature; only `None` needs a hint.
    let segs = vec![(0, 0.0, 2.0, "hello there", Some("Alice"), None::<f64>)];
    cdb::insert_segments(&conn, vid, &segs).unwrap();
    conn.execute(
        "INSERT INTO video_analysis (video_id, summary, topics, sentiment) \
         VALUES (?1, 'a summary', '[\"news\"]', 'positive')",
        [vid],
    )
    .unwrap();

    let list = cdb::list_videos(&conn).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].segment_count, 1);
    assert!(list[0].has_analysis);

    let detail = cdb::recording_detail(&conn, "rec-1").unwrap().unwrap();
    assert_eq!(detail.segments.len(), 1);
    assert_eq!(detail.segments[0].speaker.as_deref(), Some("Alice"));
    assert_eq!(detail.topics, vec!["news".to_string()]);
    assert_eq!(detail.sentiment.as_deref(), Some("positive"));
    assert!(cdb::recording_detail(&conn, "nope").unwrap().is_none());
}

#[test]
fn insights_word_frequency_filters_stopwords() {
    let dir = tempfile::tempdir().unwrap();
    let conn = cdb::open_and_init(&dir.path().join("crunchr.db")).unwrap();
    let vid = cdb::insert_video(&conn, "rec-1", "Chan", "Title", "/tmp/a.mkv").unwrap();
    for (word, count) in [("the", 100), ("stream", 40), ("recording", 25)] {
        conn.execute(
            "INSERT INTO word_frequency (video_id, word, count) VALUES (?1, ?2, ?3)",
            rusqlite::params![vid, word, count],
        )
        .unwrap();
    }

    let with = frequency::top_words_global(&conn, 10, true).unwrap();
    assert!(with.iter().any(|r| r.word == "the"));

    let without = frequency::top_words_global(&conn, 10, false).unwrap();
    assert!(!without.iter().any(|r| r.word == "the"));
    assert!(without.iter().any(|r| r.word == "stream"));
}

#[test]
fn archiver_channels_and_videos() {
    let dir = tempfile::tempdir().unwrap();
    let conn = adb::open_and_init(&dir.path().join("archiver.db")).unwrap();

    let cid =
        adb::upsert_channel(&conn, "Alpha", "https://t/alpha", "Twitch", "/arc/alpha").unwrap();
    let vids = vec![
        (
            "v1".to_string(),
            "One".to_string(),
            "20260101".to_string(),
            Some(60.0),
            None,
        ),
        (
            "v2".to_string(),
            "Two".to_string(),
            "20260102".to_string(),
            None,
            None,
        ),
    ];
    adb::insert_videos(&conn, cid, &vids).unwrap();
    adb::mark_downloaded(&conn, cid, "v1").unwrap();

    let chans = adb::list_channels(&conn).unwrap();
    assert_eq!(chans.len(), 1);
    assert_eq!(chans[0].video_count, 2);
    assert_eq!(chans[0].downloaded_count, 1);

    let listed = adb::list_videos(&conn, cid).unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].video_id, "v2"); // newest upload first
}

#[test]
fn viewguard_verdicts_and_samples_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("viewguard.db");
    {
        let s = ViewguardStore::open(&path).unwrap();
        let now = chrono::Utc::now();
        s.upsert_verdict(&VerdictRow {
            channel_id: "c1".into(),
            stream_started_at: now,
            stream_ended_at: None,
            final_score: 0.85,
            band: "fraudulent".into(),
            contributors_json: "[]".into(),
        })
        .unwrap();
        for i in 0..4 {
            s.insert_sample(
                "c1",
                "twitch",
                now + chrono::Duration::minutes(i),
                (i * 5) as u32,
            )
            .unwrap();
        }
    }

    // The web layer opens the same file read-only.
    let conn =
        Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let verdicts = store::all_verdicts(&conn).unwrap();
    assert_eq!(verdicts.len(), 1);
    assert_eq!(verdicts[0].band, "fraudulent");

    let samples = store::samples_for(&conn, "c1", 10).unwrap();
    assert_eq!(samples.len(), 4);
    assert!(samples[0].viewers <= samples[3].viewers); // oldest-first
}

/// Guards against the double-nesting regression (AX-6): the plugin registry's
/// `init_all` already scopes each plugin's `data_dir` to
/// `<base>/plugins/<name>` before calling `init`, so the plugin must use that
/// path as-is rather than re-joining `plugins/<name>` a second time. This
/// mirrors exactly how `registry.rs::init_all` builds the `PluginContext`, and
/// asserts the resulting DB lands at the same flat path the web layer's
/// `plugins_root()`-style resolution computes: `<base>/plugins/viewguard/viewguard.db`.
#[test]
fn viewguard_data_path() {
    let base = tempfile::tempdir().unwrap();
    let base_data = base.path().to_path_buf();

    let config = AppConfig::default();
    let ctx = PluginContext {
        config: &config,
        data_dir: base_data.join("plugins").join("viewguard"),
        cache_dir: base_data.join("cache").join("plugins").join("viewguard"),
    };

    let mut plugin = ViewguardPlugin::new();
    plugin.init(&ctx).unwrap();

    let expected_db = base_data
        .join("plugins")
        .join("viewguard")
        .join("viewguard.db");
    assert!(
        expected_db.exists(),
        "expected viewguard db at {}",
        expected_db.display()
    );

    let double_nested = base_data
        .join("plugins")
        .join("viewguard")
        .join("plugins")
        .join("viewguard")
        .join("viewguard.db");
    assert!(
        !double_nested.exists(),
        "viewguard db must not double-nest under {}",
        double_nested.display()
    );
}
