//! Speaker airtime + sentiment-trend queries over the Crunchr DB. (I2.)
//!
//! Read-only — never writes back. Airtime is sum(end_sec - start_sec)
//! grouped by speaker for one recording. Sentiment-trend buckets the
//! `video_analysis.sentiment` JSON if present; today Crunchr stores a
//! single per-video sentiment classification (positive / neutral /
//! negative + a confidence score) so the "trend" is a degenerate
//! single point per recording. When per-window sentiment lands the
//! query swaps in place.

use std::collections::HashMap;

use anyhow::Result;
use rusqlite::Connection;

use strivo_core::signal_store::{SignalQuery, SignalStore};

/// One row in the speakers airtime view.
#[derive(Debug, Clone, PartialEq)]
pub struct SpeakerAirtime {
    pub speaker: String,
    pub seconds: f64,
    /// Number of distinct segments. Useful for "Alice spoke a lot but
    /// in tiny chunks" vs "monologue" inferences.
    pub segments: i64,
}

/// Sum each speaker's total time-on-mic for one recording. Sorted
/// descending so the largest air-time speaker renders first.
pub fn airtime_for_recording(conn: &Connection, recording_id: &str) -> Result<Vec<SpeakerAirtime>> {
    let mut stmt = conn.prepare(
        "SELECT COALESCE(s.speaker, '(unlabeled)') AS sp, \
                SUM(s.end_sec - s.start_sec) AS secs, \
                COUNT(*) AS segs \
         FROM segments s \
         JOIN videos v ON v.id = s.video_id \
         WHERE v.recording_id = ?1 \
         GROUP BY sp \
         ORDER BY secs DESC",
    )?;
    let rows = stmt.query_map([recording_id], |row| {
        Ok(SpeakerAirtime {
            speaker: row.get(0)?,
            seconds: row.get(1)?,
            segments: row.get(2)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// Sum each speaker's total time-on-mic for one recording, sourced from the
/// canonical signal store's `speaker_segment` signals instead of
/// the crunchr database directly. Sorted descending by seconds.
///
/// Unlike [`airtime_for_recording`] there is no `(unlabeled)` bucket: the
/// producer (`crunchr::signals::write_recording_signals`) never mirrors
/// segments without a speaker label, so those minutes are simply absent
/// here — an accepted gap, not a bug.
pub fn airtime_for_recording_from_signals(
    store: &SignalStore,
    recording_id: &str,
) -> Result<Vec<SpeakerAirtime>> {
    let rows = store.query_signals(
        &SignalQuery::new()
            .recording_id(recording_id)
            .kind("speaker_segment"),
    )?;
    let mut by_speaker: HashMap<String, (f64, i64)> = HashMap::new();
    for row in &rows {
        let entry = by_speaker.entry(row.label.clone()).or_insert((0.0, 0));
        entry.0 += row.t_end - row.t_start;
        entry.1 += 1;
    }
    let mut out: Vec<SpeakerAirtime> = by_speaker
        .into_iter()
        .map(|(speaker, (seconds, segments))| SpeakerAirtime {
            speaker,
            seconds,
            segments,
        })
        .collect();
    out.sort_by(|a, b| {
        b.seconds
            .partial_cmp(&a.seconds)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.speaker.cmp(&b.speaker))
    });
    Ok(out)
}

/// Sentiment label as Crunchr stores it: 'positive', 'neutral',
/// 'negative', or anything else (custom-trained models). Renderer
/// maps to a color band.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SentimentBand {
    Positive,
    Neutral,
    Negative,
    Other(String),
}

impl SentimentBand {
    pub fn from_label(label: &str) -> Self {
        match label.to_lowercase().as_str() {
            "positive" | "pos" => Self::Positive,
            "neutral" | "neu" => Self::Neutral,
            "negative" | "neg" => Self::Negative,
            other => Self::Other(other.to_string()),
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Positive => "positive",
            Self::Neutral => "neutral",
            Self::Negative => "negative",
            Self::Other(s) => s,
        }
    }
}

/// Per-recording sentiment snapshot. When per-window sentiment lands
/// in `video_analysis` the renderer plots multiple points; today
/// every recording contributes one.
#[derive(Debug, Clone)]
pub struct SentimentPoint {
    pub recording_id: String,
    pub label: SentimentBand,
}

pub fn sentiment_for_recording(
    conn: &Connection,
    recording_id: &str,
) -> Result<Option<SentimentPoint>> {
    let row: Option<Option<String>> = conn
        .query_row(
            "SELECT va.sentiment \
             FROM video_analysis va \
             JOIN videos v ON v.id = va.video_id \
             WHERE v.recording_id = ?1",
            [recording_id],
            |row| row.get(0),
        )
        .ok();
    Ok(row.flatten().map(|lbl| SentimentPoint {
        recording_id: recording_id.to_string(),
        label: SentimentBand::from_label(&lbl),
    }))
}

/// Per-recording sentiment snapshot, sourced from the canonical signal
/// store's `sentiment` signals instead of the crunchr database directly.
/// There is at most one `sentiment` signal per recording (the producer
/// writes it only when a `video_analysis` row exists), so this takes the
/// first match.
pub fn sentiment_for_recording_from_signals(
    store: &SignalStore,
    recording_id: &str,
) -> Result<Option<SentimentPoint>> {
    let rows = store.query_signals(
        &SignalQuery::new()
            .recording_id(recording_id)
            .kind("sentiment"),
    )?;
    Ok(rows.into_iter().next().map(|s| SentimentPoint {
        recording_id: recording_id.to_string(),
        label: SentimentBand::from_label(&s.label),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentiment_band_known_labels() {
        assert_eq!(
            SentimentBand::from_label("positive"),
            SentimentBand::Positive
        );
        assert_eq!(SentimentBand::from_label("POS"), SentimentBand::Positive);
        assert_eq!(SentimentBand::from_label("neutral"), SentimentBand::Neutral);
        assert_eq!(
            SentimentBand::from_label("negative"),
            SentimentBand::Negative
        );
    }

    #[test]
    fn sentiment_band_other_preserved() {
        match SentimentBand::from_label("anxious") {
            SentimentBand::Other(s) => assert_eq!(s, "anxious"),
            _ => panic!("custom labels must round-trip via Other"),
        }
    }
}
