// Creator Edition only: exercises the signal-store-backed corpus hydration.
#![cfg(feature = "creator")]
//! Coverage for [`strivo_web::corpus::hydrate_corpus`] — building a
//! `strivo_dataviz::Corpus` scoped by recording/playlist/channel + an
//! optional date range, straight off an in-memory `SignalStore`. No
//! PersistDb/crunchr.db IO here; `RecordingMeta` is supplied directly, the
//! same shape the real HTTP handler resolves from the crunchr `videos`
//! table.

use strivo_core::signal_store::{NewSignal, SignalStore};
use strivo_web::corpus::{hydrate_corpus, CorpusScope, RecordingMeta};

fn segment(recording_id: &str, speaker: &str, text: &str, start: f64, end: f64) -> NewSignal {
    NewSignal {
        recording_id: recording_id.to_string(),
        t_start: start,
        t_end: end,
        kind: "speaker_segment".to_string(),
        label: speaker.to_string(),
        payload: serde_json::json!({ "text": text, "speaker": speaker }),
        confidence: 1.0,
        source_plugin: "crunchr".to_string(),
    }
}

/// Three recordings across two channels, spanning three months.
fn recordings() -> Vec<RecordingMeta> {
    vec![
        RecordingMeta {
            id: "rec-1".into(),
            title: "Pilot".into(),
            date: "2026-01-15 00:00:00".into(),
            channel: "TalkShow".into(),
            playlist: None,
        },
        RecordingMeta {
            id: "rec-2".into(),
            title: "Sequel".into(),
            date: "2026-03-10 00:00:00".into(),
            channel: "TalkShow".into(),
            playlist: None,
        },
        RecordingMeta {
            id: "rec-3".into(),
            title: "Other Show".into(),
            date: "2026-02-01 00:00:00".into(),
            channel: "OtherChannel".into(),
            playlist: None,
        },
    ]
}

fn seeded_store() -> SignalStore {
    let store = SignalStore::open_in_memory().unwrap();
    store
        .write_signals(&[
            // rec-1: written out of time order to prove hydrate_corpus sorts.
            segment("rec-1", "Bob", "second line", 30.0, 60.0),
            segment("rec-1", "Alice", "first line", 0.0, 30.0),
            segment("rec-2", "Alice", "returns", 0.0, 45.0),
            segment("rec-3", "Carol", "hello from the other show", 0.0, 20.0),
        ])
        .unwrap();
    store
}

#[test]
fn corpus_hydrate_recording_scope_orders_utterances_by_start() {
    let store = seeded_store();
    let recs = recordings();

    let corpus = hydrate_corpus(
        &store,
        &recs,
        &CorpusScope::Recording {
            id: "rec-1".into(),
        },
        None,
    )
    .unwrap();

    assert_eq!(corpus.episodes.len(), 1, "only rec-1 should be in scope");
    let ep = &corpus.episodes[0];
    assert_eq!(ep.id, "rec-1");
    assert_eq!(ep.title, "Pilot");
    assert_eq!(ep.date, "2026-01-15 00:00:00");
    assert_eq!(ep.utterances.len(), 2);
    // Written Bob-then-Alice but spans put Alice first — proves the sort.
    assert_eq!(ep.utterances[0].speaker, "Alice");
    assert_eq!(ep.utterances[0].text, "first line");
    assert_eq!(ep.utterances[0].start_sec, 0.0);
    assert_eq!(ep.utterances[1].speaker, "Bob");
    assert_eq!(ep.utterances[1].text, "second line");
    assert_eq!(ep.utterances[1].start_sec, 30.0);
}

#[test]
fn corpus_hydrate_channel_scope_yields_all_matching_recordings() {
    let store = seeded_store();
    let recs = recordings();

    let corpus = hydrate_corpus(
        &store,
        &recs,
        &CorpusScope::Channel {
            name: "TalkShow".into(),
        },
        None,
    )
    .unwrap();

    // rec-1 + rec-2 (TalkShow), rec-3 (OtherChannel) excluded, chronological.
    let ids: Vec<&str> = corpus.episodes.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, vec!["rec-1", "rec-2"]);
    assert_eq!(corpus.episodes[1].utterances.len(), 1);
    assert_eq!(corpus.episodes[1].utterances[0].text, "returns");
}

#[test]
fn corpus_hydrate_date_range_filters_episodes_out() {
    let store = seeded_store();
    let recs = recordings();

    let corpus = hydrate_corpus(
        &store,
        &recs,
        &CorpusScope::Channel {
            name: "TalkShow".into(),
        },
        Some(("2026-02-01 00:00:00", "2026-12-31 23:59:59")),
    )
    .unwrap();

    // rec-1 (January) falls before date_from and is dropped; rec-2 remains.
    let ids: Vec<&str> = corpus.episodes.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, vec!["rec-2"]);
}

#[test]
fn corpus_hydrate_playlist_scope_is_empty_without_playlist_data() {
    // No RecordingMeta in this test fixture carries a playlist id, mirroring
    // today's real metadata source (crunchr `videos` has no playlist
    // column yet). The scope must still resolve cleanly to an empty corpus
    // rather than error.
    let store = seeded_store();
    let recs = recordings();

    let corpus = hydrate_corpus(
        &store,
        &recs,
        &CorpusScope::Playlist {
            id: "playlist-x".into(),
        },
        None,
    )
    .unwrap();

    assert!(corpus.episodes.is_empty());
    assert_eq!(corpus.label, "Playlist playlist-x");
}

#[test]
fn corpus_hydrate_includes_recording_with_no_segments_as_empty_episode() {
    let store = seeded_store();
    let mut recs = recordings();
    recs.push(RecordingMeta {
        id: "rec-4".into(),
        title: "Untranscribed".into(),
        date: "2026-04-01 00:00:00".into(),
        channel: "TalkShow".into(),
        playlist: None,
    });

    let corpus = hydrate_corpus(
        &store,
        &recs,
        &CorpusScope::Recording {
            id: "rec-4".into(),
        },
        None,
    )
    .unwrap();

    assert_eq!(corpus.episodes.len(), 1);
    assert!(corpus.episodes[0].utterances.is_empty());
}
