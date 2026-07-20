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
use strivo_core::signal_store::{NewSignal, SignalStore};
use strivo_plugins::archiver::db as adb;
use strivo_plugins::crunchr::db as cdb;
use strivo_plugins::insights::{frequency, speakers, topics};
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

/// Proves the migration onto the canonical signal store: seeds a
/// `SignalStore` (not `crunchr.db`) with the same kinds crunchr's
/// `write_recording_signals` mirrors — `word_frequency`, `speaker_segment`,
/// `sentiment`, `topic` — across two recordings, then exercises the
/// `*_from_signals` functions the web handlers now call. A handler still
/// reading `crunchr.db` would see none of this seeded data.
#[test]
fn insights_via_signal_store() {
    let store = SignalStore::open_in_memory().unwrap();

    let sig = |recording_id: &str,
               t_start: f64,
               t_end: f64,
               kind: &str,
               label: &str,
               payload: serde_json::Value,
               confidence: f64| NewSignal {
        recording_id: recording_id.to_string(),
        t_start,
        t_end,
        kind: kind.to_string(),
        label: label.to_string(),
        payload,
        confidence,
        source_plugin: "crunchr".to_string(),
    };

    store
        .write_signals(&[
            // word_frequency — rec-1 has a stopword and a content word;
            // rec-2 contributes more of the same content word plus a new
            // one, so the global aggregate must sum across recordings.
            sig(
                "rec-1",
                0.0,
                0.0,
                "word_frequency",
                "the",
                serde_json::json!({"word": "the", "count": 100}),
                1.0,
            ),
            sig(
                "rec-1",
                0.0,
                0.0,
                "word_frequency",
                "stream",
                serde_json::json!({"word": "stream", "count": 10}),
                1.0,
            ),
            sig(
                "rec-2",
                0.0,
                0.0,
                "word_frequency",
                "stream",
                serde_json::json!({"word": "stream", "count": 5}),
                1.0,
            ),
            sig(
                "rec-2",
                0.0,
                0.0,
                "word_frequency",
                "speedrun",
                serde_json::json!({"word": "speedrun", "count": 3}),
                1.0,
            ),
            // speaker_segment — Alice speaks across two segments, Bob one.
            sig(
                "rec-1",
                0.0,
                2.0,
                "speaker_segment",
                "Alice",
                serde_json::json!({"text": "hi", "speaker": "Alice", "confidence": 0.9}),
                0.9,
            ),
            sig(
                "rec-1",
                2.0,
                5.0,
                "speaker_segment",
                "Alice",
                serde_json::json!({"text": "there", "speaker": "Alice", "confidence": 0.8}),
                0.8,
            ),
            sig(
                "rec-1",
                5.0,
                6.0,
                "speaker_segment",
                "Bob",
                serde_json::json!({"text": "yo", "speaker": "Bob", "confidence": 1.0}),
                1.0,
            ),
            // sentiment — at most one per recording.
            sig(
                "rec-1",
                0.0,
                0.0,
                "sentiment",
                "positive",
                serde_json::json!({"sentiment": "positive", "summary": "A friendly chat"}),
                1.0,
            ),
            // topic — normalization must fold case/whitespace variants
            // together across recordings.
            sig(
                "rec-1",
                0.0,
                0.0,
                "topic",
                "Streaming",
                serde_json::json!({"topic": "Streaming"}),
                1.0,
            ),
            sig(
                "rec-1",
                0.0,
                0.0,
                "topic",
                "Highlights",
                serde_json::json!({"topic": "Highlights"}),
                1.0,
            ),
            sig(
                "rec-2",
                0.0,
                0.0,
                "topic",
                "streaming ",
                serde_json::json!({"topic": "streaming "}),
                1.0,
            ),
        ])
        .unwrap();

    // Global word frequency sums across both recordings and drops stopwords.
    let global = frequency::top_words_global_from_signals(&store, 10, false).unwrap();
    assert!(!global.iter().any(|r| r.word == "the"));
    let stream = global.iter().find(|r| r.word == "stream").unwrap();
    assert_eq!(stream.count, 15); // 10 (rec-1) + 5 (rec-2)
    let speedrun = global.iter().find(|r| r.word == "speedrun").unwrap();
    assert_eq!(speedrun.count, 3);

    let global_with_stop = frequency::top_words_global_from_signals(&store, 10, true).unwrap();
    assert!(global_with_stop.iter().any(|r| r.word == "the"));

    // Per-recording word frequency is scoped to rec-1 only.
    let per_rec =
        frequency::top_words_for_recording_from_signals(&store, "rec-1", 10, false).unwrap();
    assert_eq!(per_rec.len(), 1);
    assert_eq!(per_rec[0].word, "stream");
    assert_eq!(per_rec[0].count, 10);

    // Speaker airtime: Alice's two segments sum to 5s, Bob's one to 1s,
    // sorted descending by seconds.
    let airtime = speakers::airtime_for_recording_from_signals(&store, "rec-1").unwrap();
    assert_eq!(airtime.len(), 2);
    assert_eq!(airtime[0].speaker, "Alice");
    assert_eq!(airtime[0].seconds, 5.0);
    assert_eq!(airtime[0].segments, 2);
    assert_eq!(airtime[1].speaker, "Bob");
    assert_eq!(airtime[1].seconds, 1.0);
    assert_eq!(airtime[1].segments, 1);

    // Sentiment for rec-1 is present; rec-2 never got a sentiment signal.
    let sentiment = speakers::sentiment_for_recording_from_signals(&store, "rec-1")
        .unwrap()
        .expect("rec-1 sentiment signal should be present");
    assert_eq!(sentiment.label.label(), "positive");
    assert!(
        speakers::sentiment_for_recording_from_signals(&store, "rec-2")
            .unwrap()
            .is_none()
    );

    // Cross-recording topics: "Streaming" (rec-1) and "streaming " (rec-2)
    // normalize to the same topic and aggregate to count 2.
    let topics = topics::cross_recording_topics_from_signals(&store).unwrap();
    let streaming = topics.iter().find(|t| t.topic == "streaming").unwrap();
    assert_eq!(streaming.count, 2);
    assert!(!streaming.first_seen.is_empty());
    assert!(!streaming.last_seen.is_empty());
    let highlights = topics.iter().find(|t| t.topic == "highlights").unwrap();
    assert_eq!(highlights.count, 1);
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
