#![allow(dead_code)]
//! Media fixtures shared by the two media-input test binaries.
//!
//! Every fixture here is a **real** file produced by the host's `ffmpeg`, and
//! every test that asserts a rejection of one states the `ffprobe` reading that
//! established the rejection. That is the plan's media gate: a decoded-media
//! assertion, never a file-size or exit-code check standing in for one. The
//! readings were established by running `ffprobe -show_format -show_streams`
//! on each of these and are restated at each assertion site.

use std::path::{Path, PathBuf};

pub mod dsp;
use std::process::Command;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use strivo_web::server::{build_router, AppState};

/// Whether this host can produce and read fixtures at all. Without it the
/// decoded-media assertions cannot run, and saying so is better than a green
/// test that asserted nothing.
pub fn ffmpeg_available() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn ffmpeg(args: &[&str]) {
    let status = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error"])
        .args(args)
        .status()
        .expect("ffmpeg runs");
    assert!(status.success(), "ffmpeg failed: ffmpeg {args:?}");
}

/// `format.duration` and the `(codec_type, codec_name)` pairs `ffprobe` reports,
/// or `None` when `ffprobe` cannot read the file at all.
#[derive(Debug)]
pub struct Probe {
    pub duration: Option<String>,
    pub streams: Vec<(String, String)>,
}

/// What `ffprobe` actually says about a file. Used by the tests to confirm a
/// fixture is the shape they claim before asserting a rejection of it.
pub fn probe(path: &Path) -> Option<Probe> {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(path)
        .output()
        .expect("ffprobe runs");
    if !out.status.success() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    Some(Probe {
        duration: v
            .get("format")
            .and_then(|f| f.get("duration"))
            .and_then(|d| d.as_str())
            .map(str::to_string),
        streams: v
            .get("streams")
            .and_then(|s| s.as_array())
            .map(|a| {
                a.iter()
                    .map(|s| {
                        (
                            s.get("codec_type")
                                .and_then(|x| x.as_str())
                                .unwrap_or("?")
                                .to_string(),
                            s.get("codec_name")
                                .and_then(|x| x.as_str())
                                .unwrap_or("?")
                                .to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// H.264 + AAC in Matroska, `secs` long. ffprobe: one video and one audio
/// stream, `format.duration` present.
pub fn video_with_audio(path: &Path, secs: u32) {
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        &format!("testsrc=duration={secs}:size=160x120:rate=10"),
        "-f",
        "lavfi",
        "-i",
        &format!("sine=frequency=440:duration={secs}"),
        "-shortest",
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
        &path.to_string_lossy(),
    ]);
}

/// AAC only, in an MP4 container. ffprobe: one audio stream and no video stream,
/// `format.duration` present. Used both under its own name and renamed to
/// `.mkv`, which is the extension-lies case.
pub fn audio_only(path: &Path, secs: u32) {
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        &format!("sine=frequency=440:duration={secs}"),
        "-c:a",
        "aac",
        &path.to_string_lossy(),
    ]);
}

/// A 440 Hz tone that stops half way and never restarts, over a silent video.
/// The second half of the audio is digital silence.
///
/// For a control whose entire job is removing silence: a continuous tone has
/// none, so a cleanup assertion over one is a claim about nothing. The video
/// track is there because media admission refuses an audio-only container
/// (`media-input/no-video-stream`) — the render is a video render.
pub fn tone_then_silence(path: &Path, tone_secs: u32, total_secs: u32) {
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        &format!("testsrc=duration={total_secs}:size=160x120:rate=10"),
        "-f",
        "lavfi",
        "-i",
        &format!("sine=frequency=440:duration={tone_secs}"),
        "-f",
        "lavfi",
        "-i",
        &format!("anullsrc=r=48000:cl=mono:d={}", total_secs - tone_secs),
        "-filter_complex",
        "[1:a][2:a]concat=n=2:v=0:a=1[a]",
        "-map",
        "0:v",
        "-map",
        "[a]",
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
        &path.to_string_lossy(),
    ]);
}

/// A bare H.264 elementary stream: a real codec's bitstream with no container.
/// ffprobe reports a video stream and **no** `format.duration` at all, which is
/// the `DurationMissing` case — a legitimate file that no cut can be
/// range-checked against.
pub fn raw_h264(path: &Path, secs: u32) {
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        &format!("testsrc=duration={secs}:size=160x120:rate=10"),
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-pix_fmt",
        "yuv420p",
        "-f",
        "h264",
        &path.to_string_lossy(),
    ]);
}

/// Two video and two audio streams in one container. A legitimate multi-stream
/// recording; the gate must admit it rather than count streams.
pub fn multi_stream(path: &Path, secs: u32) {
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        &format!("testsrc=duration={secs}:size=160x120:rate=10"),
        "-f",
        "lavfi",
        "-i",
        &format!("testsrc2=duration={secs}:size=160x120:rate=10"),
        "-f",
        "lavfi",
        "-i",
        &format!("sine=frequency=440:duration={secs}"),
        "-f",
        "lavfi",
        "-i",
        &format!("sine=frequency=880:duration={secs}"),
        "-map",
        "0:v",
        "-map",
        "1:v",
        "-map",
        "2:a",
        "-map",
        "3:a",
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
        &path.to_string_lossy(),
    ]);
}

/// A file with length and no recognisable container. ffprobe exits non-zero on
/// it; that non-zero exit *is* the fixture, and it is what a recording looks
/// like when the writer died before the header landed.
pub fn unrecognisable(path: &Path) {
    std::fs::write(path, vec![0u8; 4096]).expect("fixture");
}

/// The first `bytes` of a real Matroska file.
pub fn truncated_from(source: &Path, path: &Path, bytes: usize) {
    let whole = std::fs::read(source).expect("source");
    std::fs::write(path, &whole[..bytes.min(whole.len())]).expect("fixture");
}

pub const API_KEY: &str = "media-input-test-key";

/// Serialises the tests that open the editor store.
///
/// `EdlStore::open` takes a SQLite lock with no busy timeout, and the store
/// lives at one process-wide path (see [`shared_xdg_roots`]), so two tests
/// opening it at the same time fail with `database is locked`. It is not
/// deterministic — the same binary passed 13/13 in isolation and then failed 4
/// of 13 under `--workspace`, and a third run failed a different 3 — which is
/// the signature of a shared resource rather than a defect in any one test.
/// Every test that opens the store holds this for its whole body.
pub static EDITOR_STORE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The XDG roots, set once for the whole binary.
///
/// `AppConfig::data_dir()` and therefore the editor store read the process
/// environment, and `XDG_DATA_HOME` is one value for the process. A per-test
/// tempdir would have eleven tests pointing eleven different tests at eleven
/// different SQLite files — the run failed with `database is locked` and
/// `attempt to write a readonly database` before this was fixed. The store is
/// shared instead, which is safe because every sandbox registers its own random
/// recording id, so no two tests name the same row.
pub fn shared_xdg_roots() -> &'static Path {
    static ROOTS: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    ROOTS
        .get_or_init(|| {
            let dir = tempfile::tempdir().expect("xdg tempdir");
            std::env::set_var("XDG_DATA_HOME", dir.path().join("data"));
            std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
            std::env::set_var("XDG_CACHE_HOME", dir.path().join("cache"));
            dir
        })
        .path()
}

/// A sandboxed install: its own `config.toml` with a `recording_dir` inside its
/// own tempdir, its own `jobs.db`, and one registered recording. Nothing here can
/// reach a real `$HOME` or a real library.
pub struct Sandbox {
    pub dir: tempfile::TempDir,
    pub root: PathBuf,
    pub recording: PathBuf,
    pub recording_id: String,
    pub state: AppState,
}

impl Sandbox {
    pub fn new() -> Self {
        use strivo_core::platform::PlatformKind;
        use strivo_core::recording::job::{RecordingJob, RecordingState};
        use strivo_core::recording::persist::{PersistDb, PersistedJob};

        let dir = tempfile::tempdir().expect("tempdir");
        shared_xdg_roots();

        let root = dir.path().join("recordings");
        std::fs::create_dir_all(&root).expect("recordings");
        let recording = root.join("rec-1.mkv");
        video_with_audio(&recording, 4);

        let config_path = dir.path().join("config.toml");
        strivo_core::config::AppConfig {
            recording_dir: root.clone(),
            ..Default::default()
        }
        .save(Some(&config_path))
        .expect("config");

        let db_path = dir.path().join("jobs.db");
        let db = PersistDb::open(&db_path).expect("jobs db");
        let mut job = RecordingJob::new(
            "channel".into(),
            "Channel".into(),
            PlatformKind::Twitch,
            recording.clone(),
            false,
            Some("Recording".into()),
        );
        job.state = RecordingState::Finished;
        job.bytes_written = 1;
        let recording_id = job.id.to_string();
        futures::executor::block_on(db.upsert_job(&PersistedJob {
            id: recording_id.clone(),
            kind: "Recording".into(),
            payload: serde_json::to_string(&job).expect("job json"),
            state: "finished".into(),
            attempts: 0,
            last_error: None,
            episode_dir: None,
        }))
        .expect("register job");

        let mut state = AppState::test_state(API_KEY);
        state.config_path = Some(Arc::new(config_path));
        state.jobs_db_path = Arc::new(db_path);
        Self {
            dir,
            root,
            recording,
            recording_id,
            state,
        }
    }

    pub async fn post(
        &self,
        path: &str,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let router = build_router(self.state.clone());
        let request = Request::builder()
            .method("POST")
            .uri(path)
            .header("x-api-key", API_KEY)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .expect("request");
        let response = router.oneshot(request).await.expect("response");
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("body");
        let parsed = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, parsed)
    }

    pub async fn save_edl(&self, edl: serde_json::Value) -> (StatusCode, serde_json::Value) {
        let path = format!("/api/v1/plugins/editor/{}", self.recording_id);
        self.post(&path, edl).await
    }

    /// The `detail` string of a `application/problem+json` body, which is where
    /// the typed code travels.
    pub fn detail(body: &serde_json::Value) -> String {
        body["detail"].as_str().unwrap_or_default().to_string()
    }
}

/// One cut naming `path`, reading `start`..`end`.
pub fn edl_cutting(path: &Path, start: f32, end: f32) -> serde_json::Value {
    serde_json::json!({
        "recording_id": "ignored-by-the-server",
        "cuts": [{
            "start_sec": start,
            "end_sec": end,
            "kind": { "kind": "source", "source_path": path.to_string_lossy() },
            "fade_in_sec": 0.0,
            "fade_out_sec": 0.0,
        }],
    })
}

/// The same editor store path the handlers use, so a planted document lands
/// where the render will look for it.
pub fn editor_store_path() -> PathBuf {
    strivo_core::config::AppConfig::data_dir()
        .join("plugins")
        .join("editor")
        .join("editor.db")
}

/// Make the Creator database exist before parallel tests race to be its first
/// writer.
///
/// SQLite does **not** invoke the busy handler for a `journal_mode` change, so
/// two connections becoming the first to open a brand-new file race for the
/// exclusive lock and the loser gets `SQLITE_BUSY` however long `busy_timeout`
/// is. This is an invariant of the running server rather than of a test:
/// `server::serve` calls `creator_schema::ensure_creator_schema()` once, before
/// it binds, so by the time a request opens the store the file already exists and
/// is already WAL. `Once` reproduces that: exactly one test is first.
pub fn ensure_creator_db() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        strivo_research::creator::Store::open(strivo_web::creator_schema::creator_db_path())
            .expect("creator db opens");
    });
}
