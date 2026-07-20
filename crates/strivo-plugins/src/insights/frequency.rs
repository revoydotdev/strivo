//! Word frequency queries over the Crunchr DB, and their `SignalStore`
//! equivalents over the mirrored `word_frequency` signals. Read-only.

use std::collections::HashMap;

use anyhow::Result;
use rusqlite::Connection;

use strivo_core::signal_store::{SignalQuery, SignalStore};

/// One row in the frequency view: word + count.
#[derive(Debug, Clone)]
pub struct FrequencyRow {
    pub word: String,
    pub count: i64,
}

/// Curated English stopword list. Small enough to inline; loosely follows
/// the NLTK stopword set's "common, near-meaningless" filler words.
/// Toggle via `[s]` in the Insights pane.
pub const STOPWORDS: &[&str] = &[
    "the", "a", "an", "and", "or", "but", "if", "of", "to", "in", "on", "at", "for", "from",
    "with", "without", "by", "as", "is", "was", "were", "be", "been", "being", "are", "am", "i",
    "you", "he", "she", "it", "we", "they", "me", "him", "her", "us", "them", "this", "that",
    "these", "those", "my", "your", "his", "their", "our", "its", "do", "does", "did", "have",
    "has", "had", "will", "would", "should", "could", "can", "may", "might", "must", "what",
    "when", "where", "why", "how", "who", "which", "than", "then", "so", "just", "uh", "um",
    "yeah", "like", "okay", "right", "really", "actually", "kind", "sort", "very", "much", "many",
    "any", "all", "some", "no", "not", "only", "even", "also", "there", "here", "now", "well",
    "still", "more", "most", "back", "good", "great", "go", "get", "got", "going", "make", "made",
    "see", "look", "think", "know", "want", "say", "said", "tell", "told", "come", "came", "take",
    "took", "give", "gave",
];

fn stopword_set() -> std::collections::HashSet<&'static str> {
    STOPWORDS.iter().copied().collect()
}

/// Global frequency aggregate across every indexed recording.
pub fn top_words_global(
    conn: &Connection,
    limit: usize,
    include_stopwords: bool,
) -> Result<Vec<FrequencyRow>> {
    let mut stmt = conn.prepare(
        "SELECT word, SUM(count) AS total \
         FROM word_frequency \
         GROUP BY word \
         ORDER BY total DESC \
         LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit as i64 * 4], |row| {
        Ok(FrequencyRow {
            word: row.get(0)?,
            count: row.get(1)?,
        })
    })?;
    let mut out: Vec<FrequencyRow> = Vec::new();
    let stop = stopword_set();
    for r in rows {
        let row = r?;
        if !include_stopwords && stop.contains(row.word.to_lowercase().as_str()) {
            continue;
        }
        out.push(row);
        if out.len() >= limit {
            break;
        }
    }
    Ok(out)
}

/// Per-recording frequency. `recording_id` is the recording uuid; Crunchr
/// stores it as the `recording_id` text on the `videos` table.
pub fn top_words_for_recording(
    conn: &Connection,
    recording_id: &str,
    limit: usize,
    include_stopwords: bool,
) -> Result<Vec<FrequencyRow>> {
    let mut stmt = conn.prepare(
        "SELECT wf.word, wf.count \
         FROM word_frequency wf \
         JOIN videos v ON v.id = wf.video_id \
         WHERE v.recording_id = ?1 \
         ORDER BY wf.count DESC \
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(rusqlite::params![recording_id, limit as i64 * 4], |row| {
        Ok(FrequencyRow {
            word: row.get(0)?,
            count: row.get(1)?,
        })
    })?;
    let mut out: Vec<FrequencyRow> = Vec::new();
    let stop = stopword_set();
    for r in rows {
        let row = r?;
        if !include_stopwords && stop.contains(row.word.to_lowercase().as_str()) {
            continue;
        }
        out.push(row);
        if out.len() >= limit {
            break;
        }
    }
    Ok(out)
}

/// Sum `word_frequency` signal counts (`payload.count`) into a
/// word -> total map, mirroring the `SUM(count) GROUP BY word` shape of
/// the crunchr database query.
fn sum_word_counts(rows: &[strivo_core::signal_store::Signal]) -> HashMap<String, i64> {
    let mut totals: HashMap<String, i64> = HashMap::new();
    for row in rows {
        let count = row
            .payload
            .get("count")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        *totals.entry(row.label.clone()).or_insert(0) += count;
    }
    totals
}

/// Sort a word -> count map descending by count, apply the shared stopword
/// filter, and truncate to `limit` — the common tail of every frequency
/// query, whether DB-backed or signal-store-backed.
fn sort_filter_truncate(
    totals: HashMap<String, i64>,
    limit: usize,
    include_stopwords: bool,
) -> Vec<FrequencyRow> {
    let stop = stopword_set();
    let mut out: Vec<FrequencyRow> = totals
        .into_iter()
        .filter(|(word, _)| include_stopwords || !stop.contains(word.to_lowercase().as_str()))
        .map(|(word, count)| FrequencyRow { word, count })
        .collect();
    out.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.word.cmp(&b.word)));
    out.truncate(limit);
    out
}

/// Global frequency aggregate across every indexed recording, sourced from
/// the canonical signal store's `word_frequency` signals instead of
/// the crunchr database directly.
pub fn top_words_global_from_signals(
    store: &SignalStore,
    limit: usize,
    include_stopwords: bool,
) -> Result<Vec<FrequencyRow>> {
    let rows = store.query_signals(&SignalQuery::new().kind("word_frequency"))?;
    let totals = sum_word_counts(&rows);
    Ok(sort_filter_truncate(totals, limit, include_stopwords))
}

/// Per-recording frequency, sourced from the canonical signal store's
/// `word_frequency` signals instead of the crunchr database directly.
pub fn top_words_for_recording_from_signals(
    store: &SignalStore,
    recording_id: &str,
    limit: usize,
    include_stopwords: bool,
) -> Result<Vec<FrequencyRow>> {
    let rows = store.query_signals(
        &SignalQuery::new()
            .recording_id(recording_id)
            .kind("word_frequency"),
    )?;
    let totals = sum_word_counts(&rows);
    Ok(sort_filter_truncate(totals, limit, include_stopwords))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stopwords_include_common_fillers() {
        let s = stopword_set();
        assert!(s.contains("the"));
        assert!(s.contains("uh"));
        assert!(s.contains("um"));
    }

    #[test]
    fn stopwords_exclude_content_words() {
        let s = stopword_set();
        assert!(!s.contains("stream"));
        assert!(!s.contains("recording"));
    }
}
