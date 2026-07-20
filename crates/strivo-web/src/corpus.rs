//! Server-side corpus hydration — build a [`strivo_dataviz::Corpus`] scoped
//! by recording / playlist / channel (+ optional date range) straight from
//! the canonical signal store.
//!
//! Previously the SPA hand-assembled the corpus client-side from raw
//! transcript reads before handing it to `POST /api/v1/dataviz/run`. This
//! module moves that assembly server-side: [`hydrate_corpus`] is the pure
//! core (no IO, testable against an in-memory store), and
//! `routes::plugins::dataviz_corpus` is the thin HTTP wrapper that resolves
//! recording metadata and wires it up.
//!
//! `strivo_dataviz` itself stays pure/no-IO by design — this crate owns the
//! store access and hands the dataviz crate only finished [`Corpus`] values.

use strivo_core::signal_store::{SignalQuery, SignalStore, SignalStoreError};
use strivo_dataviz::{Corpus, Episode, Utterance};

/// Recording metadata needed to resolve corpus scope + frame an episode.
///
/// Resolved server-side (see `routes::plugins::dataviz_corpus`) from the
/// crunchr `videos` table / persisted job records; kept as a plain struct
/// here so [`hydrate_corpus`] can be unit tested without touching any
/// database.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordingMeta {
    pub id: String,
    pub title: String,
    /// Sortable/filterable date stamp. Any format that compares correctly
    /// as a string works (ISO-8601 `YYYY-MM-DD...` or SQLite's zero-padded
    /// `YYYY-MM-DD HH:MM:SS` both do); callers should keep one format.
    pub date: String,
    pub channel: String,
    /// Playlist membership, when known. No first-party plugin currently
    /// tracks playlist grouping (there is no playlist data source in the
    /// codebase yet), so this is `None` for every recording resolved by
    /// today's endpoint. The `Playlist` scope variant and its filter are
    /// implemented so a future playlist-aware ingest source can populate
    /// this field without any change to `hydrate_corpus` itself.
    pub playlist: Option<String>,
}

/// What subset of recordings a hydrated corpus should cover.
#[derive(Debug, Clone, PartialEq)]
pub enum CorpusScope {
    Recording { id: String },
    Playlist { id: String },
    Channel { name: String },
}

/// Build a [`Corpus`] from `store`, scoped to the recordings in `recordings`
/// that match `scope` and (if given) fall within `date_range`.
///
/// For each in-scope recording, `speaker_segment` signals are queried,
/// sorted by `t_start`, and turned into one [`Episode`] per recording —
/// including recordings with zero segments (an episode with no
/// utterances), so scope membership stays independent of transcription
/// state: a recording that matches `scope` but hasn't been transcribed yet
/// still shows up as an empty episode rather than silently vanishing from
/// the corpus.
///
/// Episodes are returned in chronological order (`RecordingMeta.date`,
/// ascending).
pub fn hydrate_corpus(
    store: &SignalStore,
    recordings: &[RecordingMeta],
    scope: &CorpusScope,
    date_range: Option<(&str, &str)>,
) -> Result<Corpus, SignalStoreError> {
    let label = match scope {
        CorpusScope::Recording { id } => format!("Recording {id}"),
        CorpusScope::Playlist { id } => format!("Playlist {id}"),
        CorpusScope::Channel { name } => format!("Channel {name}"),
    };

    let in_scope: Vec<&RecordingMeta> = recordings
        .iter()
        .filter(|r| match scope {
            CorpusScope::Recording { id } => &r.id == id,
            CorpusScope::Playlist { id } => r.playlist.as_deref() == Some(id.as_str()),
            CorpusScope::Channel { name } => &r.channel == name,
        })
        .filter(|r| match date_range {
            Some((from, to)) => r.date.as_str() >= from && r.date.as_str() <= to,
            None => true,
        })
        .collect();

    let mut episodes = Vec::with_capacity(in_scope.len());
    for rec in in_scope {
        let signals = store.query_signals(
            &SignalQuery::new()
                .recording_id(rec.id.clone())
                .kind("speaker_segment"),
        )?;
        let mut utterances: Vec<Utterance> = signals
            .into_iter()
            .map(|s| Utterance {
                speaker: s.label,
                text: s
                    .payload
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                start_sec: s.t_start,
                end_sec: s.t_end,
            })
            .collect();
        utterances.sort_by(|a, b| {
            a.start_sec
                .partial_cmp(&b.start_sec)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        episodes.push(Episode {
            id: rec.id.clone(),
            title: rec.title.clone(),
            date: rec.date.clone(),
            utterances,
        });
    }
    episodes.sort_by(|a, b| a.date.cmp(&b.date));

    Ok(Corpus { label, episodes })
}
