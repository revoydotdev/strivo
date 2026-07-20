//! End-to-end transcription runner.
//!
//! Spawned as a `PluginAction::SpawnTask` future from the webui
//! "Re-transcribe" verb (and, when wired, the tandem auto-trigger). Runs the
//! full chain for one recording and writes everything into `crunchr.db`:
//!
//!   extract audio → transcribe (diarized) → persist segments → chunk →
//!   embed (vectorize) → mark complete.
//!
//! `rusqlite::Connection` is not `Send`, so every DB touch is confined to a
//! synchronous block that opens, writes, and drops its connection before the
//! next `.await`. The long-running steps (ffmpeg, the transcription API call,
//! the embedding subprocess) happen between those blocks.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{bail, Context, Result};
use strivo_core::config::CrunchrConfig;
use strivo_core::signal_store::SignalStore;
use uuid::Uuid;

use super::transcribe::create_backend;
use super::{db, embed, pipeline, signals};

/// Per-recording result, handed back to the plugin via `on_plugin_event`
/// so it can surface a desktop notification.
pub struct RunnerOutcome {
    pub recording_id: Uuid,
    pub title: String,
    pub result: std::result::Result<RunnerStats, String>,
}

pub struct RunnerStats {
    pub segments: usize,
    pub speakers: usize,
    pub chunks: usize,
    pub embedded: usize,
}

/// Drive one recording through the whole pipeline. Never panics; any error
/// is captured into the outcome and the video row is marked `failed`.
pub async fn process_recording(
    cfg: CrunchrConfig,
    db_path: PathBuf,
    cache_dir: PathBuf,
    recording_id: Uuid,
    channel_name: String,
    title: String,
    video_path: PathBuf,
) -> RunnerOutcome {
    let rid = recording_id.to_string();
    let result = run_inner(
        &cfg,
        &db_path,
        &cache_dir,
        &rid,
        &channel_name,
        &title,
        &video_path,
    )
    .await
    .map_err(|e| format!("{e:#}"));

    if let Err(ref msg) = result {
        // Best-effort: record the failure on the video row.
        if let Ok(conn) = db::open_and_init(&db_path) {
            let _ = db::update_video_status(&conn, &rid, "failed", Some(msg));
        }
    }

    RunnerOutcome {
        recording_id,
        title,
        result,
    }
}

async fn run_inner(
    cfg: &CrunchrConfig,
    db_path: &Path,
    cache_dir: &Path,
    rid: &str,
    channel_name: &str,
    title: &str,
    video_path: &Path,
) -> Result<RunnerStats> {
    if !video_path.exists() {
        bail!("recording file is missing: {}", video_path.display());
    }
    let video_str = video_path.to_string_lossy().to_string();

    // ── register the video row ────────────────────────────────────────────
    let video_id = {
        let conn = db::open_and_init(db_path)?;
        let id = db::insert_video(&conn, rid, channel_name, title, &video_str)?;
        db::update_video_status(&conn, rid, "extracting_audio", None)?;
        id
    };

    // ── extract audio (16 kHz mono mp3, small enough to upload whole) ──────
    let audio_path = extract_audio(video_path, cache_dir, rid)
        .await
        .context("audio extraction failed")?;
    let audio_str = audio_path.to_string_lossy().to_string();
    {
        let conn = db::open_and_init(db_path)?;
        db::update_video_audio_path(&conn, rid, &audio_str)?;
        db::update_video_status(&conn, rid, "transcribing", None)?;
    }

    // ── transcribe (diarized) ─────────────────────────────────────────────
    let backend = create_backend(cfg);
    let transcription = backend
        .transcribe(&audio_path)
        .await
        .context("transcription backend failed")?;
    let mut segments = transcription.segments;
    let full_text = transcription.full_text;

    // Re-diarize: replace the backend's unreliable speaker labels with
    // voice-embedding clusters. Best-effort — keep the original labels on
    // failure so a transcript is never lost to a diarization hiccup.
    if cfg.diarize {
        match super::cluster::recluster_speakers(&audio_path, &mut segments, cfg.diarize_speakers)
            .await
        {
            Ok(n) => tracing::info!(recording_id = %rid, speakers = n, "crunchr: re-diarized"),
            Err(e) => {
                tracing::warn!(recording_id = %rid, "crunchr: speaker re-clustering failed: {e:#}")
            }
        }
    }

    let speakers = {
        let mut s: Vec<&str> = segments
            .iter()
            .filter_map(|seg| seg.speaker.as_deref())
            .collect();
        s.sort_unstable();
        s.dedup();
        s.len()
    };

    // ── persist segments + transcript text ────────────────────────────────
    {
        let conn = db::open_and_init(db_path)?;
        let owned: Vec<(usize, f64, f64, String, Option<String>, Option<f64>)> = segments
            .iter()
            .map(|s| {
                (
                    s.index,
                    s.start_sec,
                    s.end_sec,
                    s.text.clone(),
                    s.speaker.clone(),
                    s.confidence,
                )
            })
            .collect();
        let borrowed: Vec<(usize, f64, f64, &str, Option<&str>, Option<f64>)> = owned
            .iter()
            .map(|(i, st, en, t, sp, c)| (*i, *st, *en, t.as_str(), sp.as_deref(), *c))
            .collect();
        db::insert_segments(&conn, video_id, &borrowed)?;
        db::update_video_transcript(&conn, rid, &full_text)?;
        db::update_video_status(&conn, rid, "chunking", None)?;
    }

    // ── chunk for retrieval + embedding ───────────────────────────────────
    let chunks = pipeline::chunk_segments(&segments, 512);
    {
        let conn = db::open_and_init(db_path)?;
        let owned: Vec<(usize, String, f64, f64, usize)> = chunks
            .iter()
            .enumerate()
            .map(|(i, c)| (i, c.text.clone(), c.start_sec, c.end_sec, c.token_count))
            .collect();
        let borrowed: Vec<(usize, &str, f64, f64, usize)> = owned
            .iter()
            .map(|(i, t, st, en, tok)| (*i, t.as_str(), *st, *en, *tok))
            .collect();
        db::insert_chunks(&conn, video_id, &borrowed)?;
        db::update_video_status(&conn, rid, "analyzing", None)?;
    }

    // ── vectorize: embed each chunk locally, persist as f32 BLOB ───────────
    let chunk_rows = {
        let conn = db::open_and_init(db_path)?;
        db::chunks_for_embedding(&conn, video_id)?
    };
    let texts: Vec<String> = chunk_rows.iter().map(|(_, t)| t.clone()).collect();
    let embedded = match embed::embed_texts(&texts).await {
        Ok(vectors) => {
            let conn = db::open_and_init(db_path)?;
            let mut n = 0usize;
            for ((chunk_id, _), vec) in chunk_rows.iter().zip(vectors.iter()) {
                db::set_chunk_embedding(&conn, *chunk_id, &embed::vector_to_blob(vec))?;
                n += 1;
            }
            n
        }
        Err(e) => {
            // Embedding is best-effort: a transcript without vectors is still
            // useful. Record the warning but don't fail the whole job.
            tracing::warn!(recording_id = %rid, "crunchr: embedding step failed: {e:#}");
            0
        }
    };

    // ── word frequencies + canonical signal-store mirror ────────────────────
    // Best-effort, same as embedding above: a transcript must never be lost
    // to a word-frequency or signal-store hiccup.
    match db::open_and_init(db_path) {
        Ok(conn) => {
            let freqs = pipeline::word_frequencies(&full_text);
            if let Err(e) = db::insert_word_frequencies(&conn, video_id, &freqs) {
                tracing::warn!(recording_id = %rid, "crunchr: persisting word frequencies failed: {e:#}");
            }

            let signals_db_path = db_path.with_file_name("signals.db");
            match SignalStore::open(&signals_db_path) {
                Ok(store) => match signals::write_recording_signals(&conn, &store, rid) {
                    Ok(n) => {
                        tracing::info!(recording_id = %rid, signals = n, "crunchr: mirrored analytics into signal store")
                    }
                    Err(e) => {
                        tracing::warn!(recording_id = %rid, "crunchr: writing signals failed: {e:#}")
                    }
                },
                Err(e) => {
                    tracing::warn!(recording_id = %rid, "crunchr: opening signal store failed: {e:#}")
                }
            }
        }
        Err(e) => {
            tracing::warn!(recording_id = %rid, "crunchr: opening crunchr.db for word frequencies failed: {e:#}")
        }
    }

    {
        let conn = db::open_and_init(db_path)?;
        db::update_video_status(&conn, rid, "complete", None)?;
    }

    Ok(RunnerStats {
        segments: segments.len(),
        speakers,
        chunks: chunks.len(),
        embedded,
    })
}

/// Extract a compact 16 kHz mono mp3 from the recording. Speech-grade
/// bitrate keeps even a 3-hour episode well under the transcription API's
/// upload ceiling while staying within Voxtral's quality envelope.
async fn extract_audio(video_path: &Path, cache_dir: &Path, rid: &str) -> Result<PathBuf> {
    tokio::fs::create_dir_all(cache_dir).await?;
    let audio_path = cache_dir.join(format!("{rid}.mp3"));

    let status = tokio::process::Command::new("ffmpeg")
        .args([
            "-i",
            &video_path.to_string_lossy(),
            "-vn",
            "-ac",
            "1",
            "-ar",
            "16000",
            "-c:a",
            "libmp3lame",
            "-b:a",
            "48k",
            "-y",
            &audio_path.to_string_lossy(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await?;

    if !status.status.success() {
        let stderr = String::from_utf8_lossy(&status.stderr);
        bail!(
            "ffmpeg exited with {}: {}",
            status.status,
            stderr
                .chars()
                .rev()
                .take(300)
                .collect::<String>()
                .chars()
                .rev()
                .collect::<String>()
        );
    }
    if !audio_path.exists() {
        bail!("ffmpeg produced no output at {}", audio_path.display());
    }
    Ok(audio_path)
}
