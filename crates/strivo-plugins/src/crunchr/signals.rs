//! Mirrors a recording's crunchr.db analytics into the canonical
//! `SignalStore` (VISION AX-6) so the Insights plugin + webui can read
//! `word_frequency`, `speaker_segment`, `sentiment`, and `topic` signals
//! through one typed store instead of reaching into `crunchr.db` directly.
//!
//! Called from [`super::runner::run_inner`] once segments/transcript are
//! persisted. Best-effort by design — a signal-store hiccup must never lose
//! a transcript, so callers should log a warning on `Err` rather than fail
//! the whole job.

use anyhow::{Context, Result};
use rusqlite::Connection;
use strivo_core::signal_store::{NewSignal, SignalStore};

use super::db;

const SOURCE_PLUGIN: &str = "crunchr";

/// How many top words to mirror as `word_frequency` signals per recording.
const TOP_WORDS_LIMIT: usize = 50;

/// Read `recording_id`'s analytics from the open crunchr.db `conn` and write
/// them into `store`. Returns the number of signals written (0 when the
/// recording is unknown or has no analytics yet).
pub fn write_recording_signals(
    conn: &Connection,
    store: &SignalStore,
    recording_id: &str,
) -> Result<usize> {
    let Some(video_id) =
        db::get_video_id_by_recording(conn, recording_id).context("looking up video id")?
    else {
        return Ok(0);
    };

    let mut signals = Vec::new();

    // ── word_frequency ──────────────────────────────────────────────────
    let top_words = db::get_top_words_for_video(conn, video_id, TOP_WORDS_LIMIT)
        .context("loading word frequencies")?;
    for (word, count) in &top_words {
        signals.push(NewSignal {
            recording_id: recording_id.to_string(),
            t_start: 0.0,
            t_end: 0.0,
            kind: "word_frequency".to_string(),
            label: word.clone(),
            payload: serde_json::json!({ "word": word, "count": count }),
            confidence: 1.0,
            source_plugin: SOURCE_PLUGIN.to_string(),
        });
    }

    // ── speaker_segment ──────────────────────────────────────────────────
    let segments = db::load_full_segments(conn, video_id).context("loading segments")?;
    for seg in &segments {
        let Some(speaker) = seg.speaker.as_deref() else {
            continue;
        };
        // Clamp instead of dropping the segment: `write_signals` rejects
        // out-of-range confidence outright, and a stray backend value
        // outside [0.0, 1.0] shouldn't cost the segment its signal.
        let confidence = seg.confidence.map(|c| c.clamp(0.0, 1.0)).unwrap_or(1.0);
        signals.push(NewSignal {
            recording_id: recording_id.to_string(),
            t_start: seg.start_sec,
            t_end: seg.end_sec,
            kind: "speaker_segment".to_string(),
            label: speaker.to_string(),
            payload: serde_json::json!({
                "text": seg.text,
                "speaker": speaker,
                "confidence": confidence,
            }),
            confidence,
            source_plugin: SOURCE_PLUGIN.to_string(),
        });
    }

    // ── sentiment + topic (from video_analysis, via recording_detail) ──────
    if let Some(detail) =
        db::recording_detail(conn, recording_id).context("loading recording detail")?
    {
        if let Some(sentiment) = detail.sentiment.as_deref() {
            signals.push(NewSignal {
                recording_id: recording_id.to_string(),
                t_start: 0.0,
                t_end: 0.0,
                kind: "sentiment".to_string(),
                label: sentiment.to_string(),
                payload: serde_json::json!({
                    "sentiment": sentiment,
                    "summary": detail.summary,
                }),
                confidence: 1.0,
                source_plugin: SOURCE_PLUGIN.to_string(),
            });
        }

        for topic in &detail.topics {
            signals.push(NewSignal {
                recording_id: recording_id.to_string(),
                t_start: 0.0,
                t_end: 0.0,
                kind: "topic".to_string(),
                label: topic.clone(),
                payload: serde_json::json!({ "topic": topic }),
                confidence: 1.0,
                source_plugin: SOURCE_PLUGIN.to_string(),
            });
        }
    }

    if signals.is_empty() {
        return Ok(0);
    }

    let ids = store
        .write_signals(&signals)
        .context("writing signals to store")?;
    Ok(ids.len())
}

// `strivo-plugins` unconditionally pulls in `strivo-core`'s `creator`
// feature (see its Cargo.toml), so this module — and its tests — always
// build; there's no separate "creator off" state to gate behind within this
// crate. `cargo test --workspace --features creator` reaches it the same as
// plain `cargo test --workspace`, since `--features creator` just targets
// the workspace-root `strivo-core` package.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::crunchr::db::{insert_segments, insert_video, insert_word_frequencies};

    /// `db::open_and_init` sets `PRAGMA journal_mode=WAL`, which needs a
    /// file-backed database, so tests use a temp file rather than
    /// `:memory:`. The returned `TempDir` must stay alive alongside the
    /// connection — dropping it early would delete the db file out from
    /// under an open WAL-mode handle.
    fn fresh_conn() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("crunchr.db");
        let conn = db::open_and_init(&path).unwrap();
        (dir, conn)
    }

    #[test]
    fn crunchr_writes_signals() {
        let (_dir, conn) = fresh_conn();
        let recording_id = "rec-signals-1";
        let video_id = insert_video(&conn, recording_id, "Chan", "Talk", "/tmp/v.mkv").unwrap();

        // Segments with speakers, feeding both `speaker_segment` and the
        // word-frequency source text.
        let segs: Vec<(usize, f64, f64, &str, Option<&str>, Option<f64>)> = vec![
            (0, 0.0, 2.0, "hello world", Some("Alice"), Some(0.9)),
            (1, 2.0, 4.0, "goodbye world", Some("Bob"), None),
            (2, 4.0, 5.0, "narration only", None, None),
        ];
        insert_segments(&conn, video_id, &segs).unwrap();

        // word_frequency rows (normally populated by the runner's
        // `pipeline::word_frequencies` + `insert_word_frequencies` call).
        let freqs = vec![("world".to_string(), 2usize), ("hello".to_string(), 1usize)];
        insert_word_frequencies(&conn, video_id, &freqs).unwrap();

        // video_analysis row, for sentiment + topic.
        conn.execute(
            "INSERT INTO video_analysis (video_id, summary, topics, sentiment) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                video_id,
                "A friendly chat",
                "[\"greetings\",\"farewells\"]",
                "positive"
            ],
        )
        .unwrap();

        let store = SignalStore::open_in_memory().unwrap();
        let written = write_recording_signals(&conn, &store, recording_id).unwrap();
        // 2 word_frequency + 2 speaker_segment (segment w/o speaker is
        // skipped) + 1 sentiment + 2 topic = 7.
        assert_eq!(written, 7);

        let word_freq_signals = store
            .query_signals(
                &strivo_core::signal_store::SignalQuery::new()
                    .recording_id(recording_id)
                    .kind("word_frequency"),
            )
            .unwrap();
        assert_eq!(word_freq_signals.len(), 2);
        let top = word_freq_signals
            .iter()
            .find(|s| s.label == "world")
            .expect("world signal present");
        assert_eq!(top.payload["word"], "world");
        assert_eq!(top.payload["count"], 2);

        let speaker_signals = store
            .query_signals(
                &strivo_core::signal_store::SignalQuery::new()
                    .recording_id(recording_id)
                    .kind("speaker_segment"),
            )
            .unwrap();
        assert_eq!(speaker_signals.len(), 2);
        let alice = speaker_signals
            .iter()
            .find(|s| s.label == "Alice")
            .expect("alice signal present");
        assert_eq!(alice.payload["text"], "hello world");
        assert_eq!(alice.t_start, 0.0);
        assert_eq!(alice.t_end, 2.0);
        assert_eq!(alice.confidence, 0.9);

        let sentiment_signals = store
            .query_signals(
                &strivo_core::signal_store::SignalQuery::new()
                    .recording_id(recording_id)
                    .kind("sentiment"),
            )
            .unwrap();
        assert_eq!(sentiment_signals.len(), 1);
        assert_eq!(sentiment_signals[0].label, "positive");
        assert_eq!(sentiment_signals[0].payload["summary"], "A friendly chat");

        let topic_signals = store
            .query_signals(
                &strivo_core::signal_store::SignalQuery::new()
                    .recording_id(recording_id)
                    .kind("topic"),
            )
            .unwrap();
        assert_eq!(topic_signals.len(), 2);
        let labels: Vec<&str> = topic_signals.iter().map(|s| s.label.as_str()).collect();
        assert!(labels.contains(&"greetings"));
        assert!(labels.contains(&"farewells"));
    }

    #[test]
    fn unknown_recording_writes_nothing() {
        let (_dir, conn) = fresh_conn();
        let store = SignalStore::open_in_memory().unwrap();
        let written = write_recording_signals(&conn, &store, "no-such-recording").unwrap();
        assert_eq!(written, 0);
    }
}
