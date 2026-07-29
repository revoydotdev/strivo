//! Transcript-derived artifact executors for the Creator publish DAG.

use std::any::Any;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use strivo_core::plugin::{
    Plugin, PluginContext, StageExecutionResult, StageFuture, StatusSlot, VerbContext,
};
use uuid::Uuid;

use crate::crunchr::db;

pub struct ArtifactPlugin {
    data_dir: PathBuf,
    crunchr_db: PathBuf,
}

impl Default for ArtifactPlugin {
    fn default() -> Self {
        Self {
            data_dir: PathBuf::new(),
            crunchr_db: PathBuf::new(),
        }
    }
}

#[derive(Clone)]
struct RecordingInput {
    id: Uuid,
    title: String,
    channel_name: String,
    source_path: PathBuf,
    duration_sec: f32,
    started_at: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct StageCache {
    version: u8,
    source_len: u64,
    source_mtime_ns: u128,
    transcript_len: u64,
    transcript_mtime_ns: u128,
    artifacts: Vec<serde_json::Value>,
}

impl ArtifactPlugin {
    pub fn new() -> Self {
        Self::default()
    }

    fn artifact_dir(&self, id: Uuid) -> PathBuf {
        self.data_dir.join(id.to_string())
    }

    fn descriptor(kind: &str, path: &Path, mime: &str) -> serde_json::Value {
        serde_json::json!({
            "kind": kind,
            "path": path,
            "mime": mime,
        })
    }
}

impl Plugin for ArtifactPlugin {
    fn name(&self) -> &'static str {
        "artifacts"
    }

    fn display_name(&self) -> &str {
        "Creator Artifacts"
    }

    fn init(&mut self, ctx: &PluginContext) -> Result<()> {
        self.data_dir = ctx.data_dir.clone();
        std::fs::create_dir_all(&self.data_dir)?;
        self.crunchr_db = ctx
            .data_dir
            .parent()
            .context("artifact plugin data directory has no plugin root")?
            .join("crunchr")
            .join("crunchr.db");
        Ok(())
    }

    fn execute_stage(
        &mut self,
        verb: &str,
        selection: &[Uuid],
        _payload: &serde_json::Value,
        ctx: &VerbContext,
    ) -> Option<StageFuture> {
        let id = *selection.first()?;
        let recording = ctx.recordings.get(&id)?;
        let input = RecordingInput {
            id,
            title: recording
                .stream_title
                .clone()
                .unwrap_or_else(|| recording.channel_name.clone()),
            channel_name: recording.channel_name.clone(),
            source_path: recording.output_path.clone(),
            duration_sec: recording.duration_secs as f32,
            started_at: recording.started_at.to_rfc3339(),
        };
        let verb = verb.to_string();
        let crunchr_db = self.crunchr_db.clone();
        let output_dir = self.artifact_dir(id);
        Some(Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                std::fs::create_dir_all(&output_dir)
                    .map_err(|error| format!("create artifact directory: {error}"))?;
                if let Some(hit) =
                    load_stage_cache(&verb, &input.source_path, &crunchr_db, &output_dir)
                {
                    tracing::info!(stage = %verb, recording_id = %input.id, "reused creator artifacts");
                    return Ok(hit);
                }
                let result = match verb.as_str() {
                    "chapters" => chapters(input.id, &crunchr_db, &output_dir),
                    "captions" => captions(input.id, &crunchr_db, &output_dir),
                    "brandsafe" => brandsafe(input.id, &crunchr_db, &output_dir),
                    "cuepoints" => cuepoints(&input, &output_dir),
                    "highlights" => highlights(&input, &output_dir),
                    "clips" => clips(&input, &output_dir),
                    "thumbnails" => thumbnails(&input, &output_dir),
                    "reuse" => reuse(&input, &crunchr_db, &output_dir),
                    "casebook" => casebook(&input, &crunchr_db, &output_dir),
                    _ => Err(format!("unknown artifact executor verb: {verb}")),
                }?;
                save_stage_cache(
                    &verb,
                    &input.source_path,
                    &crunchr_db,
                    &output_dir,
                    &result.artifacts,
                )?;
                Ok(result)
            })
            .await
            .map_err(|error| format!("artifact worker crashed: {error}"))?
        }))
    }

    fn status_line(&self) -> Option<String> {
        None
    }

    fn status_slot(&self) -> StatusSlot {
        StatusSlot::None
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

fn file_fingerprint(path: &Path) -> (u64, u128) {
    let Ok(metadata) = std::fs::metadata(path) else {
        return (0, 0);
    };
    let modified = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|value| value.as_nanos())
        .unwrap_or(0);
    (metadata.len(), modified)
}

fn sqlite_fingerprint(path: &Path) -> (u64, u128) {
    let base = file_fingerprint(path);
    let mut wal = path.as_os_str().to_os_string();
    wal.push("-wal");
    let wal = file_fingerprint(Path::new(&wal));
    (base.0.saturating_add(wal.0), base.1.max(wal.1))
}

fn stage_cache_path(output_dir: &Path, verb: &str) -> PathBuf {
    let safe = verb
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>();
    output_dir.join(format!(".stage-{safe}.json"))
}

fn artifacts_exist(artifacts: &[serde_json::Value]) -> bool {
    !artifacts.is_empty()
        && artifacts.iter().all(|artifact| {
            artifact
                .get("path")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|path| Path::new(path).is_file())
        })
}

fn load_stage_cache(
    verb: &str,
    source: &Path,
    transcript_db: &Path,
    output_dir: &Path,
) -> Option<StageExecutionResult> {
    let bytes = std::fs::read(stage_cache_path(output_dir, verb)).ok()?;
    let cache: StageCache = serde_json::from_slice(&bytes).ok()?;
    let (source_len, source_mtime_ns) = file_fingerprint(source);
    let (transcript_len, transcript_mtime_ns) = sqlite_fingerprint(transcript_db);
    if cache.version != 1
        || cache.source_len != source_len
        || cache.source_mtime_ns != source_mtime_ns
        || cache.transcript_len != transcript_len
        || cache.transcript_mtime_ns != transcript_mtime_ns
        || !artifacts_exist(&cache.artifacts)
    {
        return None;
    }
    Some(StageExecutionResult {
        artifacts: cache.artifacts,
        actions: Vec::new(),
    })
}

fn save_stage_cache(
    verb: &str,
    source: &Path,
    transcript_db: &Path,
    output_dir: &Path,
    artifacts: &[serde_json::Value],
) -> Result<(), String> {
    let (source_len, source_mtime_ns) = file_fingerprint(source);
    let (transcript_len, transcript_mtime_ns) = sqlite_fingerprint(transcript_db);
    write_json(
        &stage_cache_path(output_dir, verb),
        &StageCache {
            version: 1,
            source_len,
            source_mtime_ns,
            transcript_len,
            transcript_mtime_ns,
            artifacts: artifacts.to_vec(),
        },
    )
}

fn load_detail(path: &Path, id: Uuid) -> Result<db::RecordingDetail, String> {
    let conn = db::open_and_init(path).map_err(|error| format!("open Crunchr DB: {error:#}"))?;
    db::recording_detail(&conn, &id.to_string())
        .map_err(|error| format!("load transcript: {error:#}"))?
        .ok_or_else(|| "recording has no Crunchr transcript".to_string())
}

fn write_json(path: &Path, value: &impl serde::Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("encode {}: {error}", path.display()))?;
    write_atomic(path, &bytes)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension(format!(
        "{}.tmp",
        path.extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("artifact")
    ));
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&tmp)
        .map_err(|error| format!("open {}: {error}", tmp.display()))?;
    file.write_all(bytes)
        .map_err(|error| format!("write {}: {error}", tmp.display()))?;
    file.sync_all()
        .map_err(|error| format!("sync {}: {error}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|error| format!("publish {}: {error}", path.display()))
}

fn chapters(
    id: Uuid,
    crunchr_db: &Path,
    output_dir: &Path,
) -> Result<StageExecutionResult, String> {
    let request = strivo_chapters::ChapterRequest {
        recording_id: id.to_string(),
        min_seconds: None,
        cos_threshold: None,
    };
    let chapters =
        strivo_chapters::generate_chapters(crunchr_db, &request, &strivo_chapters::KeywordTitler)
            .map_err(|error| format!("generate chapters: {error:#}"))?;
    let json_path = output_dir.join("chapters.json");
    let text_path = output_dir.join("chapters.txt");
    write_json(&json_path, &chapters)?;
    write_atomic(
        &text_path,
        strivo_chapters::format_for_description(&chapters).as_bytes(),
    )?;
    Ok(StageExecutionResult {
        artifacts: vec![
            ArtifactPlugin::descriptor("chapters", &json_path, "application/json"),
            ArtifactPlugin::descriptor("chapters_description", &text_path, "text/plain"),
        ],
        actions: Vec::new(),
    })
}

fn caption_segments(detail: &db::RecordingDetail) -> Vec<strivo_captions::Segment> {
    detail
        .segments
        .iter()
        .map(|segment| strivo_captions::Segment {
            start_sec: segment.start_sec as f32,
            end_sec: segment.end_sec as f32,
            text: segment.text.clone(),
            speaker: segment.speaker.clone(),
        })
        .collect()
}

fn captions(
    id: Uuid,
    crunchr_db: &Path,
    output_dir: &Path,
) -> Result<StageExecutionResult, String> {
    let detail = load_detail(crunchr_db, id)?;
    let segments = caption_segments(&detail);
    let outputs = [
        (
            "captions.srt",
            "captions_srt",
            "application/x-subrip",
            strivo_captions::to_srt(&segments),
        ),
        (
            "captions.vtt",
            "captions_vtt",
            "text/vtt",
            strivo_captions::to_vtt(&segments),
        ),
        (
            "transcript.txt",
            "transcript_text",
            "text/plain",
            strivo_captions::to_txt(&segments),
        ),
    ];
    let mut artifacts = Vec::new();
    for (name, kind, mime, body) in outputs {
        let path = output_dir.join(name);
        write_atomic(&path, body.as_bytes())?;
        artifacts.push(ArtifactPlugin::descriptor(kind, &path, mime));
    }
    Ok(StageExecutionResult {
        artifacts,
        actions: Vec::new(),
    })
}

fn brandsafe(
    id: Uuid,
    crunchr_db: &Path,
    output_dir: &Path,
) -> Result<StageExecutionResult, String> {
    let detail = load_detail(crunchr_db, id)?;
    let segments: Vec<strivo_brandsafe::Segment> = detail
        .segments
        .iter()
        .map(|segment| strivo_brandsafe::Segment {
            start_sec: segment.start_sec as f32,
            end_sec: segment.end_sec as f32,
            text: segment.text.clone(),
        })
        .collect();
    let verdicts =
        strivo_brandsafe::scan_all(&segments, &detail.channel_name, &["Twitch", "YouTube"]);
    let path = output_dir.join("brandsafe.json");
    write_json(&path, &verdicts)?;
    Ok(StageExecutionResult {
        artifacts: vec![ArtifactPlugin::descriptor(
            "brand_safety",
            &path,
            "application/json",
        )],
        actions: Vec::new(),
    })
}

fn cuepoints(input: &RecordingInput, output_dir: &Path) -> Result<StageExecutionResult, String> {
    let points = strivo_cuepoints::extract_cuepoints(&input.source_path, 0.32)
        .map_err(|error| format!("extract scene changes: {error:#}"))?;
    let path = output_dir.join("cuepoints.json");
    write_json(&path, &points)?;
    Ok(StageExecutionResult {
        artifacts: vec![ArtifactPlugin::descriptor(
            "cuepoints",
            &path,
            "application/json",
        )],
        actions: Vec::new(),
    })
}

fn load_highlights(output_dir: &Path) -> Result<Vec<strivo_clipper::Highlight>, String> {
    let path = output_dir.join("highlights.json");
    let bytes =
        std::fs::read(&path).map_err(|error| format!("read {}: {error}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("decode {}: {error}", path.display()))
}

fn highlights(input: &RecordingInput, output_dir: &Path) -> Result<StageExecutionResult, String> {
    let cuepoint_path = output_dir.join("cuepoints.json");
    let bytes = std::fs::read(&cuepoint_path)
        .map_err(|error| format!("read {}: {error}", cuepoint_path.display()))?;
    let points: Vec<strivo_cuepoints::Cuepoint> = serde_json::from_slice(&bytes)
        .map_err(|error| format!("decode {}: {error}", cuepoint_path.display()))?;
    let mut scored = strivo_clipper::score_highlights(
        &points,
        strivo_clipper::DEFAULT_WINDOW_SECS,
        strivo_clipper::DEFAULT_TOP_K,
    );
    // A visually static recording is still publishable. Seed a deterministic
    // midpoint candidate so clips and thumbnails do not turn "no cuts" into
    // a terminal workflow failure.
    if scored.is_empty() && input.duration_sec > 0.0 {
        scored.push(strivo_clipper::Highlight {
            time_sec: input.duration_sec / 2.0,
            score: 0.0,
            density: 0,
            suggested_duration: strivo_clipper::DEFAULT_CLIP_DURATION_SECS,
        });
    }
    let set = strivo_clipper::HighlightSet {
        recording_id: input.id.to_string(),
        window_secs: strivo_clipper::DEFAULT_WINDOW_SECS,
        highlights: scored,
    };
    let path = output_dir.join("highlights.json");
    write_json(&path, &set.highlights)?;
    Ok(StageExecutionResult {
        artifacts: vec![ArtifactPlugin::descriptor(
            "highlights",
            &path,
            "application/json",
        )],
        actions: Vec::new(),
    })
}

fn clips(input: &RecordingInput, output_dir: &Path) -> Result<StageExecutionResult, String> {
    let highlights = load_highlights(output_dir)?;
    let clips_dir = output_dir.join("clips");
    std::fs::create_dir_all(&clips_dir)
        .map_err(|error| format!("create {}: {error}", clips_dir.display()))?;
    let mut manifest = Vec::new();
    let mut artifacts = Vec::new();
    for (index, highlight) in highlights.iter().take(3).enumerate() {
        let (start, duration) = strivo_clipper::clamp_request(
            highlight.time_sec - strivo_clipper::DEFAULT_PRE_PAD_SECS,
            highlight.suggested_duration,
            Some(input.duration_sec),
        );
        let path = clips_dir.join(format!("highlight-{:02}.mkv", index + 1));
        let bytes = strivo_clipper::extract_clip(&input.source_path, &path, start, duration)
            .map_err(|error| format!("extract clip {}: {error:#}", index + 1))?;
        manifest.push(strivo_clipper::ClipResult {
            recording_id: input.id.to_string(),
            clip_path: path.to_string_lossy().to_string(),
            start_sec: start,
            duration_sec: duration,
            bytes,
        });
        artifacts.push(ArtifactPlugin::descriptor(
            "highlight_clip",
            &path,
            "video/x-matroska",
        ));
    }
    let manifest_path = output_dir.join("clips.json");
    write_json(&manifest_path, &manifest)?;
    artifacts.push(ArtifactPlugin::descriptor(
        "clip_manifest",
        &manifest_path,
        "application/json",
    ));
    Ok(StageExecutionResult {
        artifacts,
        actions: Vec::new(),
    })
}

fn thumbnails(input: &RecordingInput, output_dir: &Path) -> Result<StageExecutionResult, String> {
    let highlights = load_highlights(output_dir)?;
    let timestamps: Vec<f32> = highlights
        .iter()
        .take(6)
        .map(|item| item.time_sec)
        .collect();
    if timestamps.is_empty() {
        return Err("no highlight timestamps available for thumbnails".to_string());
    }
    let result = strivo_thumbnails::generate_candidates(
        &input.source_path,
        (1920, 1080),
        &strivo_thumbnails::GenerateOptions {
            timestamps,
            out_dir: output_dir.join("thumbnails"),
            stem: "candidate".to_string(),
            facecam: None,
        },
        &input.id.to_string(),
    )
    .map_err(|error| format!("generate thumbnails: {error:#}"))?;
    let manifest_path = output_dir.join("thumbnails.json");
    write_json(&manifest_path, &result)?;
    let mut artifacts = vec![ArtifactPlugin::descriptor(
        "thumbnail_manifest",
        &manifest_path,
        "application/json",
    )];
    artifacts.extend(result.candidates.iter().map(|candidate| {
        ArtifactPlugin::descriptor("thumbnail", Path::new(&candidate.path), "image/jpeg")
    }));
    Ok(StageExecutionResult {
        artifacts,
        actions: Vec::new(),
    })
}

fn reuse(
    input: &RecordingInput,
    crunchr_db: &Path,
    output_dir: &Path,
) -> Result<StageExecutionResult, String> {
    let detail = load_detail(crunchr_db, input.id)?;
    let conn =
        db::open_and_init(crunchr_db).map_err(|error| format!("open Crunchr DB: {error:#}"))?;
    let top_words = crate::insights::frequency::top_words_for_recording(
        &conn,
        &input.id.to_string(),
        30,
        false,
    )
    .unwrap_or_default()
    .into_iter()
    .map(|word| word.word)
    .collect();
    let chapters_block =
        std::fs::read_to_string(output_dir.join("chapters.txt")).unwrap_or_default();
    let source = strivo_reuse::SourceRecording {
        recording_id: input.id.to_string(),
        title: input.title.clone(),
        channel_name: input.channel_name.clone(),
        source_path: input.source_path.to_string_lossy().to_string(),
        duration_sec: input.duration_sec,
    };
    let inputs = strivo_reuse::DraftInputs {
        top_words,
        topics: detail.topics,
        clip_starts: load_highlights(output_dir)
            .unwrap_or_default()
            .into_iter()
            .map(|highlight| highlight.time_sec)
            .collect(),
        chapters_block,
        summary: detail.summary.unwrap_or_default(),
    };
    let drafts = strivo_reuse::generate_drafts(&source, &inputs);
    let path = output_dir.join("publish-drafts.json");
    write_json(&path, &drafts)?;
    Ok(StageExecutionResult {
        artifacts: vec![ArtifactPlugin::descriptor(
            "publish_drafts",
            &path,
            "application/json",
        )],
        actions: Vec::new(),
    })
}

fn casebook(
    input: &RecordingInput,
    crunchr_db: &Path,
    output_dir: &Path,
) -> Result<StageExecutionResult, String> {
    let detail = load_detail(crunchr_db, input.id)?;
    let conn =
        db::open_and_init(crunchr_db).map_err(|error| format!("open Crunchr DB: {error:#}"))?;
    let top_words = crate::insights::frequency::top_words_for_recording(
        &conn,
        &input.id.to_string(),
        30,
        false,
    )
    .unwrap_or_default()
    .into_iter()
    .map(|word| strivo_casebook::WordCount {
        word: word.word,
        count: word.count.max(0) as u64,
    })
    .collect();
    let chapters: Vec<strivo_casebook::Chapter> = std::fs::read(output_dir.join("chapters.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Vec<strivo_chapters::Chapter>>(&bytes).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|chapter| strivo_casebook::Chapter {
            start_sec: chapter.start_sec,
            title: chapter.title,
        })
        .collect();
    let verdicts: Vec<strivo_brandsafe::Verdict> = std::fs::read(output_dir.join("brandsafe.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    let mut brandsafe_counts = strivo_casebook::BrandsafeCounts::default();
    for verdict in verdicts {
        match verdict.severity {
            strivo_brandsafe::Severity::Critical => brandsafe_counts.critical += 1,
            strivo_brandsafe::Severity::High => brandsafe_counts.high += 1,
            strivo_brandsafe::Severity::Medium => brandsafe_counts.medium += 1,
            strivo_brandsafe::Severity::Low => brandsafe_counts.low += 1,
        }
    }
    let report = strivo_casebook::compose_report(&strivo_casebook::CasebookInputs {
        recording_id: input.id.to_string(),
        title: input.title.clone(),
        channel_name: input.channel_name.clone(),
        started_at: Some(input.started_at.clone()),
        duration_sec: input.duration_sec,
        summary: detail.summary.unwrap_or_default(),
        topics: detail.topics,
        top_words,
        chapters,
        highlights: load_highlights(output_dir)
            .unwrap_or_default()
            .into_iter()
            .map(|highlight| strivo_casebook::Highlight {
                time_sec: highlight.time_sec,
                score: highlight.score,
            })
            .collect(),
        viewbot_score: None,
        brandsafe_counts,
    });
    let json_path = output_dir.join("casebook.json");
    let markdown_path = output_dir.join("casebook.md");
    write_json(&json_path, &report)?;
    write_atomic(
        &markdown_path,
        strivo_casebook::to_markdown(&report).as_bytes(),
    )?;
    Ok(StageExecutionResult {
        artifacts: vec![
            ArtifactPlugin::descriptor("casebook", &json_path, "application/json"),
            ArtifactPlugin::descriptor("casebook_markdown", &markdown_path, "text/markdown"),
        ],
        actions: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf, RecordingInput) {
        let dir = tempfile::tempdir().unwrap();
        let crunchr = dir.path().join("crunchr.db");
        let output = dir.path().join("artifacts");
        std::fs::create_dir_all(&output).unwrap();
        let id = Uuid::new_v4();
        let conn = db::open_and_init(&crunchr).unwrap();
        let video_id = db::insert_video(
            &conn,
            &id.to_string(),
            "Fixture",
            "Launch stream",
            "/tmp/a.mkv",
        )
        .unwrap();
        db::insert_segments(
            &conn,
            video_id,
            &[
                (
                    0,
                    0.0,
                    120.0,
                    "welcome to the launch stream with excellent gameplay",
                    Some("Host"),
                    Some(0.99),
                ),
                (
                    1,
                    120.0,
                    260.0,
                    "damn this music boss fight changed everything",
                    Some("Host"),
                    Some(0.98),
                ),
            ],
        )
        .unwrap();
        db::insert_word_frequencies(
            &conn,
            video_id,
            &[("launch".to_string(), 8), ("gameplay".to_string(), 5)],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO video_analysis (video_id, summary, topics, sentiment) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![video_id, "A launch recap.", "[\"launch\",\"gameplay\"]", "positive"],
        )
        .unwrap();
        (
            dir,
            crunchr,
            output,
            RecordingInput {
                id,
                title: "Launch stream".to_string(),
                channel_name: "Fixture".to_string(),
                source_path: PathBuf::from("/tmp/a.mkv"),
                duration_sec: 260.0,
                started_at: "2026-01-01T00:00:00Z".to_string(),
            },
        )
    }

    #[test]
    fn transcript_artifact_chain_writes_real_outputs() {
        let (_dir, crunchr, output, input) = fixture();
        assert_eq!(
            chapters(input.id, &crunchr, &output)
                .unwrap()
                .artifacts
                .len(),
            2
        );
        assert_eq!(
            captions(input.id, &crunchr, &output)
                .unwrap()
                .artifacts
                .len(),
            3
        );
        assert_eq!(
            brandsafe(input.id, &crunchr, &output)
                .unwrap()
                .artifacts
                .len(),
            1
        );
        assert_eq!(reuse(&input, &crunchr, &output).unwrap().artifacts.len(), 1);
        assert_eq!(
            casebook(&input, &crunchr, &output).unwrap().artifacts.len(),
            2
        );

        for name in [
            "chapters.json",
            "chapters.txt",
            "captions.srt",
            "captions.vtt",
            "transcript.txt",
            "brandsafe.json",
            "publish-drafts.json",
            "casebook.json",
            "casebook.md",
        ] {
            let path = output.join(name);
            assert!(path.exists(), "missing {}", path.display());
            assert!(std::fs::metadata(path).unwrap().len() > 0);
        }
    }

    #[test]
    fn missing_transcript_fails_instead_of_emitting_empty_artifacts() {
        let dir = tempfile::tempdir().unwrap();
        let crunchr = dir.path().join("crunchr.db");
        let output = dir.path().join("artifacts");
        std::fs::create_dir_all(&output).unwrap();
        let error = match captions(Uuid::new_v4(), &crunchr, &output) {
            Ok(_) => panic!("missing transcript unexpectedly produced captions"),
            Err(error) => error,
        };
        assert!(error.contains("no Crunchr transcript"), "{error}");
    }

    #[test]
    fn stage_cache_reuses_outputs_and_invalidates_missing_artifacts() {
        let (_dir, crunchr, output, input) = fixture();
        let result = captions(input.id, &crunchr, &output).unwrap();
        save_stage_cache(
            "captions",
            &input.source_path,
            &crunchr,
            &output,
            &result.artifacts,
        )
        .unwrap();
        let cached = load_stage_cache("captions", &input.source_path, &crunchr, &output).unwrap();
        assert_eq!(cached.artifacts.len(), 3);
        std::fs::remove_file(output.join("captions.vtt")).unwrap();
        assert!(load_stage_cache("captions", &input.source_path, &crunchr, &output).is_none());
    }
}
