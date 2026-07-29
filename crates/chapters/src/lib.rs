//! strivo-chapters — generate chapter markers for a recording.
//!
//! Reads the Crunchr SQLite (segments + topic_segmentation) and emits
//! `Chapter { start_sec, title }` rows suitable for YouTube/Twitch
//! publishing. Heuristic-first; the algorithm is deliberately simple
//! so a streamer can paste the output straight into a description.
//!
//! Strategy (default):
//!   1. Group segments into runs whose topic vector doesn't shift more
//!      than COS_THRESHOLD (cheap: bag-of-keywords cosine).
//!   2. Each run becomes a chapter; its title is the top-2 keywords of
//!      its concatenated text, in TitleCase.
//!   3. Drop chapters shorter than `min_seconds` (default 90) — they
//!      look like noise in a YouTube chapter list.
//!   4. Always emit a 00:00 "Intro" chapter as YouTube/Twitch require
//!      the first chapter to start at zero.
//!
//! No ML at runtime — uses a 200-word stopword list + bag-of-keywords
//! frequency. Quality tracks the Crunchr transcript quality, not the
//! chapter algorithm. A future LLM-summarisation backend can plug in
//! by implementing the [`ChapterTitler`] trait.

use std::collections::BTreeMap;

use anyhow::Result;
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};

pub mod store;

/// Cosine similarity below this triggers a chapter boundary.
const COS_THRESHOLD: f32 = 0.45;
/// Default minimum chapter length. YouTube enforces ≥10s but a 10s
/// chapter wastes a slot — 90s feels like a sensible default for a
/// 4-hour stream.
const DEFAULT_MIN_SECONDS: f32 = 90.0;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chapter {
    pub start_sec: f32,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChapterRequest {
    pub recording_id: String,
    /// Override the default minimum chapter length.
    #[serde(default)]
    pub min_seconds: Option<f32>,
    /// Override the cosine boundary threshold.
    #[serde(default)]
    pub cos_threshold: Option<f32>,
}

/// Strategy trait — heuristic title today, LLM later. Implementations
/// take the concatenated text of a chapter and return its title.
pub trait ChapterTitler: Send + Sync {
    fn title(&self, text: &str) -> String;
}

/// Default heuristic titler: pick the two most frequent non-stopword
/// content terms, TitleCase them, separate with " · ".
pub struct KeywordTitler;

impl ChapterTitler for KeywordTitler {
    fn title(&self, text: &str) -> String {
        let mut counts: BTreeMap<String, u32> = BTreeMap::new();
        for word in text.split_whitespace() {
            let w: String = word
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .flat_map(|c| c.to_lowercase())
                .collect();
            if w.len() < 4 || STOPWORDS.contains(&w.as_str()) {
                continue;
            }
            *counts.entry(w).or_default() += 1;
        }
        let mut ranked: Vec<(String, u32)> = counts.into_iter().collect();
        // Tie-break: prefer longer words (more semantically distinctive)
        // before falling back to alphabetical for determinism.
        ranked.sort_by(|a, b| {
            b.1.cmp(&a.1)
                .then_with(|| b.0.len().cmp(&a.0.len()))
                .then_with(|| a.0.cmp(&b.0))
        });
        let top: Vec<String> = ranked
            .into_iter()
            .take(2)
            .map(|(w, _)| title_case(&w))
            .collect();
        if top.is_empty() {
            "Untitled".to_string()
        } else {
            top.join(" · ")
        }
    }
}

fn title_case(w: &str) -> String {
    let mut chars = w.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

/// Build chapters for a recording by reading from Crunchr's SQLite.
/// `crunchr_db_path` is the absolute path to `crunchr.db`.
pub fn generate_chapters(
    crunchr_db_path: &std::path::Path,
    req: &ChapterRequest,
    titler: &dyn ChapterTitler,
) -> Result<Vec<Chapter>> {
    let conn = Connection::open_with_flags(
        crunchr_db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let segments = read_segments(&conn, &req.recording_id)?;
    let min_secs = req.min_seconds.unwrap_or(DEFAULT_MIN_SECONDS);
    let cos_t = req.cos_threshold.unwrap_or(COS_THRESHOLD);
    Ok(build_chapters(&segments, min_secs, cos_t, titler))
}

#[derive(Debug, Clone)]
struct Segment {
    start_sec: f32,
    end_sec: f32,
    text: String,
}

fn read_segments(conn: &Connection, recording_id: &str) -> Result<Vec<Segment>> {
    let mut stmt = conn.prepare(
        "SELECT s.start_sec, s.end_sec, s.text
         FROM segments s
         JOIN videos v ON v.id = s.video_id
         WHERE v.recording_id = ?1
         ORDER BY s.start_sec",
    )?;
    let rows = stmt
        .query_map([recording_id], |r| {
            Ok(Segment {
                start_sec: r.get::<_, f64>(0)? as f32,
                end_sec: r.get::<_, f64>(1)? as f32,
                text: r.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn build_chapters(
    segments: &[Segment],
    min_seconds: f32,
    cos_threshold: f32,
    titler: &dyn ChapterTitler,
) -> Vec<Chapter> {
    if segments.is_empty() {
        return Vec::new();
    }
    // Greedy boundary detection: walk segments, maintain a running
    // bag-of-words of the active chapter. Boundary when cosine between
    // the active vector and the next segment drops below threshold AND
    // the current chapter is long enough.
    let mut chapters: Vec<Chapter> = Vec::new();
    let mut buf: Vec<&Segment> = Vec::new();
    let mut buf_vec: BTreeMap<String, u32> = BTreeMap::new();

    let flush = |buf: &mut Vec<&Segment>,
                 buf_vec: &mut BTreeMap<String, u32>,
                 chapters: &mut Vec<Chapter>| {
        if buf.is_empty() {
            return;
        }
        let text: String = buf
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let title = titler.title(&text);
        chapters.push(Chapter {
            start_sec: buf[0].start_sec,
            title,
        });
        buf.clear();
        buf_vec.clear();
    };

    for seg in segments {
        let seg_vec = bag_of_keywords(&seg.text);
        if buf.is_empty() {
            buf.push(seg);
            merge_bag(&mut buf_vec, &seg_vec);
            continue;
        }
        let span = seg.end_sec - buf[0].start_sec;
        let cos = cosine(&buf_vec, &seg_vec);
        if cos < cos_threshold && span >= min_seconds {
            flush(&mut buf, &mut buf_vec, &mut chapters);
        }
        buf.push(seg);
        merge_bag(&mut buf_vec, &seg_vec);
    }
    flush(&mut buf, &mut buf_vec, &mut chapters);

    // Always anchor with an Intro chapter at 0.0 — YouTube/Twitch
    // require the first marker to be at 0:00.
    if chapters.first().map(|c| c.start_sec > 0.5).unwrap_or(true) {
        chapters.insert(
            0,
            Chapter {
                start_sec: 0.0,
                title: "Intro".into(),
            },
        );
    }
    chapters
}

fn bag_of_keywords(text: &str) -> BTreeMap<String, u32> {
    let mut out: BTreeMap<String, u32> = BTreeMap::new();
    for word in text.split_whitespace() {
        let w: String = word
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .flat_map(|c| c.to_lowercase())
            .collect();
        if w.len() < 4 || STOPWORDS.contains(&w.as_str()) {
            continue;
        }
        *out.entry(w).or_default() += 1;
    }
    out
}

fn merge_bag(into: &mut BTreeMap<String, u32>, other: &BTreeMap<String, u32>) {
    for (k, v) in other {
        *into.entry(k.clone()).or_default() += v;
    }
}

fn cosine(a: &BTreeMap<String, u32>, b: &BTreeMap<String, u32>) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0_f32;
    for (k, va) in a {
        if let Some(vb) = b.get(k) {
            dot += (*va as f32) * (*vb as f32);
        }
    }
    let norm_a = (a.values().map(|v| (*v as f32) * (*v as f32)).sum::<f32>()).sqrt();
    let norm_b = (b.values().map(|v| (*v as f32) * (*v as f32)).sum::<f32>()).sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

/// Format chapters as a YouTube/Twitch description block:
///   00:00 Intro
///   01:23 Diablo · Hardcore
pub fn format_for_description(chapters: &[Chapter]) -> String {
    let fmt = |sec: f32| -> String {
        let s = sec.max(0.0) as u64;
        let h = s / 3600;
        let m = (s / 60) % 60;
        let r = s % 60;
        if h > 0 {
            format!("{h:02}:{m:02}:{r:02}")
        } else {
            format!("{m:02}:{r:02}")
        }
    };
    chapters
        .iter()
        .map(|c| format!("{} {}", fmt(c.start_sec), c.title))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Conservative English stopword list (curated subset of common
/// function words). Kept inline as a `&[&str]` so the consumer doesn't
/// have to bundle a data file.
const STOPWORDS: &[&str] = &[
    "about",
    "after",
    "again",
    "ahead",
    "akin",
    "alike",
    "alone",
    "along",
    "also",
    "amongst",
    "around",
    "back",
    "because",
    "been",
    "being",
    "between",
    "could",
    "didn",
    "does",
    "doesn",
    "doing",
    "down",
    "during",
    "each",
    "either",
    "enough",
    "every",
    "from",
    "further",
    "going",
    "gonna",
    "hasn",
    "have",
    "haven",
    "having",
    "here",
    "into",
    "just",
    "keep",
    "kinda",
    "know",
    "like",
    "look",
    "make",
    "many",
    "more",
    "much",
    "need",
    "needs",
    "never",
    "next",
    "only",
    "other",
    "over",
    "really",
    "right",
    "same",
    "since",
    "some",
    "such",
    "than",
    "that",
    "thats",
    "their",
    "them",
    "then",
    "there",
    "these",
    "they",
    "thing",
    "things",
    "think",
    "this",
    "those",
    "through",
    "today",
    "told",
    "took",
    "tried",
    "trying",
    "twice",
    "under",
    "until",
    "very",
    "want",
    "wants",
    "well",
    "went",
    "were",
    "what",
    "whats",
    "when",
    "where",
    "which",
    "while",
    "with",
    "without",
    "would",
    "wouldn",
    "yeah",
    "year",
    "your",
    "yours",
    "youre",
    "youve",
    "whatever",
    "whenever",
    "wherever",
    "whoever",
    "however",
    "somebody",
    "somehow",
    "someone",
    "something",
    "somewhere",
    "anybody",
    "anyhow",
    "anyone",
    "anything",
    "anywhere",
    "everybody",
    "everyone",
    "everything",
    "everywhere",
    "nobody",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(start: f32, end: f32, text: &str) -> Segment {
        Segment {
            start_sec: start,
            end_sec: end,
            text: text.into(),
        }
    }

    #[test]
    fn empty_segments_yields_no_chapters() {
        let out = build_chapters(&[], 60.0, 0.5, &KeywordTitler);
        assert!(out.is_empty());
    }

    #[test]
    fn single_topic_collapses_to_one_chapter_at_zero() {
        let segments = vec![
            seg(0.0, 30.0, "Hardcore Diablo speedrun starts now"),
            seg(30.0, 60.0, "Going for the Diablo Hardcore world record"),
            seg(60.0, 90.0, "Diablo dungeons hardcore mode all the way"),
        ];
        let out = build_chapters(&segments, 60.0, 0.4, &KeywordTitler);
        // One coherent topic → one chapter. The titler picks the
        // dominant keywords; we don't force "Intro" because that
        // loses information for a single-topic stream.
        assert_eq!(out.len(), 1, "got {out:?}");
        assert_eq!(out[0].start_sec, 0.0);
        assert!(
            out[0].title.to_lowercase().contains("diablo")
                || out[0].title.to_lowercase().contains("hardcore"),
            "expected Diablo/Hardcore in title, got {:?}",
            out[0].title
        );
    }

    #[test]
    fn distinct_topics_make_separate_chapters() {
        // Each topic gets repeated keywords so the bag-of-words
        // actually has rank ordering — single-mention terms tie at
        // count 1 and fall to the length/alphabetical tiebreaker.
        let segments = vec![
            seg(0.0, 100.0, "diablo speedrun hardcore diablo"),
            seg(100.0, 200.0, "diablo dungeons hardcore diablo speedrun"),
            seg(200.0, 320.0, "cooking pasta tomato pasta recipes"),
            seg(320.0, 420.0, "pasta sauce recipe pasta garlic"),
        ];
        let out = build_chapters(&segments, 60.0, 0.3, &KeywordTitler);
        assert!(out.len() >= 2, "expected >=2 chapters, got {out:?}");
        assert_eq!(out[0].start_sec, 0.0);
        let lower: Vec<String> = out.iter().map(|c| c.title.to_lowercase()).collect();
        assert!(
            lower
                .iter()
                .any(|t| t.contains("diablo") || t.contains("hardcore")),
            "expected diablo/hardcore in {lower:?}"
        );
        assert!(
            lower
                .iter()
                .any(|t| t.contains("pasta") || t.contains("recipe")),
            "expected pasta/recipe in {lower:?}"
        );
    }

    #[test]
    fn format_renders_youtube_compatible_lines() {
        let chapters = vec![
            Chapter {
                start_sec: 0.0,
                title: "Intro".into(),
            },
            Chapter {
                start_sec: 65.0,
                title: "Diablo".into(),
            },
            Chapter {
                start_sec: 3725.0,
                title: "Cooking".into(),
            },
        ];
        let out = format_for_description(&chapters);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "00:00 Intro");
        assert_eq!(lines[1], "01:05 Diablo");
        assert_eq!(lines[2], "01:02:05 Cooking");
    }

    #[test]
    fn stopwords_dropped_from_titler() {
        let title = KeywordTitler.title("the very much that going gonna whatever");
        assert_eq!(title, "Untitled", "got {title:?}");
    }
}
