//! Runtime enforcement of the cold-media bounds in `docs/PERFORMANCE.md`.
//!
//! The September 8 tranche report says these controls shipped; the only tests it
//! names for them are per-ID coalescing and the failure cache. The pool sizes,
//! the coalescing behaviour and the process deadlines were not covered by a test
//! that observes behaviour, and a claim satisfied by a constant in the source is
//! not a claim about a running server — so this file drives the real routes with
//! real ffmpeg/ffprobe children and watches what the process tree actually does.
//!
//! It is a separate integration-test binary on purpose. The thumbnail route
//! resolves its cache directory from `AppConfig::data_dir()`, which reads
//! `$XDG_DATA_HOME` from the process environment, so the test that reaches
//! `/thumb` has to redirect that root to a tempdir. One binary, one test that
//! touches the environment, no second test in it to race.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

use strivo_web::server::{build_router, AppState};

type Live = BTreeMap<String, BTreeSet<u32>>;

/// The test process's live ffmpeg/ffprobe children, keyed by name then pid.
/// This is the same "active child-process count" the perf harness reads out of
/// /proc and the quantity `docs/PERFORMANCE.md` bounds. Pids are kept as well as
/// counts because "two requests for one id spawned one process" is a claim about
/// spawns, and a peak of 1 cannot tell one spawn from two sequential ones.
fn media_children() -> Live {
    let me = std::process::id();
    let mut out: Live = BTreeMap::new();
    for entry in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|p| p.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(status) = std::fs::read_to_string(format!("/proc/{pid}/status")) else {
            continue;
        };
        let ppid = status
            .lines()
            .find_map(|l| l.strip_prefix("PPid:"))
            .and_then(|v| v.trim().parse::<u32>().ok());
        if ppid != Some(me) {
            continue;
        }
        let Ok(comm) = std::fs::read_to_string(format!("/proc/{pid}/comm")) else {
            continue;
        };
        let comm = comm.trim();
        if comm != "ffmpeg" && comm != "ffprobe" {
            continue;
        }
        // A SIGKILLed child lingers as a zombie until it is reaped; it is not
        // running media work and must not count toward the bound.
        let state = status
            .lines()
            .find_map(|l| l.strip_prefix("State:"))
            .unwrap_or("");
        if state.trim_start().starts_with('Z') {
            continue;
        }
        out.entry(comm.to_string()).or_default().insert(pid);
    }
    out
}

#[derive(Default)]
struct Observation {
    peak_total: usize,
    peak_by_name: BTreeMap<String, usize>,
    /// Every distinct pid ever seen, per name: a count of spawns.
    spawned: BTreeMap<String, BTreeSet<u32>>,
    samples: usize,
}

struct Sampler {
    stop: Arc<AtomicBool>,
    seen: Arc<Mutex<Observation>>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Sampler {
    fn start() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let seen = Arc::new(Mutex::new(Observation::default()));
        let handle = {
            let (stop, seen) = (stop.clone(), seen.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let live = media_children();
                    let mut o = seen.lock().unwrap();
                    o.samples += 1;
                    o.peak_total = o.peak_total.max(live.values().map(BTreeSet::len).sum());
                    for (name, pids) in live {
                        let peak = o.peak_by_name.entry(name.clone()).or_insert(0);
                        *peak = (*peak).max(pids.len());
                        o.spawned.entry(name).or_default().extend(pids);
                    }
                    drop(o);
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            })
        };
        Self {
            stop,
            seen,
            handle: Some(handle),
        }
    }

    fn finish(mut self) -> Observation {
        self.stop.store(true, Ordering::Relaxed);
        self.handle.take().unwrap().join().unwrap();
        let o = self.seen.lock().unwrap();
        Observation {
            peak_total: o.peak_total,
            peak_by_name: o.peak_by_name.clone(),
            spawned: o.spawned.clone(),
            samples: o.samples,
        }
    }
}

fn ffmpeg_available() -> bool {
    std::process::Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn encode(path: &std::path::Path, seconds: u32) {
    let source = format!("testsrc=duration={seconds}:size=160x120:rate=10");
    let tone = format!("sine=frequency=440:duration={seconds}");
    let status = std::process::Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-f", "lavfi", "-i"])
        .arg(&source)
        .args(["-f", "lavfi", "-i"])
        .arg(&tone)
        .args([
            "-shortest",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("ffmpeg runs");
    assert!(
        status.success(),
        "ffmpeg failed to encode {}",
        path.display()
    );
}

fn request(api_key: &str, path: &str) -> Request<Body> {
    Request::builder()
        .uri(path)
        .header("x-api-key", api_key)
        .body(Body::empty())
        .unwrap()
}

/// Drives the real `/thumb` and `/probe` routes with real ffmpeg/ffprobe and
/// asserts the three claims that only a running server can settle: the shared
/// pool never runs more than two media children, thumbnails never run more than
/// one, and two concurrent requests for one recording id start exactly one
/// extraction. Both binaries must actually appear, or the assertions would hold
/// vacuously.
#[tokio::test]
async fn cold_media_routes_bound_the_pool_and_coalesce_by_id() {
    if !ffmpeg_available() {
        eprintln!("skipping: ffmpeg is not on PATH, so no media child can be observed");
        return;
    }
    use strivo_core::platform::PlatformKind;
    use strivo_core::recording::job::{RecordingJob, RecordingState};
    use strivo_core::recording::persist::{PersistDb, PersistedJob};

    let api_key = "media-pool-test-key";
    let dir = tempfile::tempdir().unwrap();
    // `AppConfig::data_dir()` is what the thumbnail route caches under, and it
    // reads the process environment, so redirect it before anything resolves it.
    std::env::set_var("XDG_DATA_HOME", dir.path().join("data"));
    std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
    std::env::set_var("XDG_CACHE_HOME", dir.path().join("cache"));

    let recording_dir = dir.path().join("recordings");
    std::fs::create_dir_all(&recording_dir).unwrap();
    let config_path = dir.path().join("config.toml");
    strivo_core::config::AppConfig {
        recording_dir: recording_dir.clone(),
        ..Default::default()
    }
    .save(Some(&config_path))
    .unwrap();

    // Five distinct files: the probe cache is keyed by path, so several requests
    // against one file would collapse into one child and measure nothing. They
    // are all longer than the 10 s the thumbnail route seeks to, because a
    // shorter one makes the first extraction attempt fail and retry, which is
    // two ffmpeg spawns for one request and would mask a coalescing regression.
    let mut ids = Vec::new();
    let db_path = dir.path().join("jobs.db");
    let db = PersistDb::open(&db_path).unwrap();
    for i in 0..5 {
        let media = recording_dir.join(format!("pool-{i}.mkv"));
        encode(&media, 12);
        let mut job = RecordingJob::new(
            "pool-channel".into(),
            "Pool channel".into(),
            PlatformKind::Twitch,
            media,
            false,
            Some(format!("Pool {i}")),
        );
        job.state = RecordingState::Finished;
        job.bytes_written = 4096;
        db.upsert_job(&PersistedJob {
            id: job.id.to_string(),
            kind: "Recording".into(),
            payload: serde_json::to_string(&job).unwrap(),
            state: "finished".into(),
            attempts: 0,
            last_error: None,
            episode_dir: None,
        })
        .await
        .unwrap();
        ids.push(job.id);
    }

    let mut state = AppState::test_state(api_key);
    state.config_path = Some(Arc::new(config_path));
    state.jobs_db_path = Arc::new(db_path);
    let router = build_router(state);

    // Phase 1: two cold thumbnails and two cold probes, all in flight together.
    // The thumbnails must serialise on their own one-slot limit while the probes
    // compete for the same two-slot pool.
    let paths: Vec<String> = ids
        .iter()
        .enumerate()
        .map(|(i, id)| {
            if i < 2 {
                format!("/api/v1/recordings/{id}/thumb")
            } else {
                format!("/api/v1/recordings/{id}/probe")
            }
        })
        .collect();

    let sampler = Sampler::start();
    let mut in_flight = Vec::new();
    for path in &paths {
        let router = router.clone();
        let req = request(api_key, path);
        in_flight.push(tokio::spawn(async move { router.oneshot(req).await }));
    }
    let mut statuses = Vec::new();
    for handle in in_flight {
        statuses.push(handle.await.unwrap().unwrap().status());
    }
    let pool = sampler.finish();

    assert!(
        pool.spawned.contains_key("ffmpeg") && pool.spawned.contains_key("ffprobe"),
        "no ffmpeg/ffprobe child was ever observed, so the pool bound was never exercised: \
         spawned={:?} peak_by_name={:?} samples={}",
        pool.spawned,
        pool.peak_by_name,
        pool.samples
    );
    assert!(
        pool.peak_total <= 2,
        "{} media children ran at once; docs/PERFORMANCE.md declares a two-slot pool \
         (per-name peaks {:?}, distinct spawns {:?})",
        pool.peak_total,
        pool.peak_by_name,
        pool.spawned
    );
    assert!(
        pool.peak_total >= 1,
        "no media child was live at any of the {} samples: {:?}",
        pool.samples,
        pool.spawned
    );
    assert!(
        pool.peak_by_name.get("ffmpeg").copied().unwrap_or(0) <= 1,
        "{} thumbnails ran at once; docs/PERFORMANCE.md declares a one-slot thumbnail limit",
        pool.peak_by_name.get("ffmpeg").copied().unwrap_or(0)
    );
    for (path, status) in paths.iter().zip(&statuses) {
        assert_eq!(*status, StatusCode::OK, "{path} did not succeed");
    }

    // Phase 2: two concurrent requests for one never-requested id must start a
    // single extraction. The second waits on the per-id lock and then re-reads
    // the cache file the first one installed.
    let coalesced = format!("/api/v1/recordings/{}/thumb", ids[4]);
    let sampler = Sampler::start();
    let mut in_flight = Vec::new();
    for _ in 0..2 {
        let router = router.clone();
        let req = request(api_key, &coalesced);
        in_flight.push(tokio::spawn(async move { router.oneshot(req).await }));
    }
    let mut coalesced_statuses = Vec::new();
    for handle in in_flight {
        coalesced_statuses.push(handle.await.unwrap().unwrap().status());
    }
    let by_id = sampler.finish();

    assert_eq!(
        by_id.spawned.get("ffmpeg").map(BTreeSet::len).unwrap_or(0),
        1,
        "two concurrent requests for one recording id started {} ffmpeg extractions; \
         per-id coalescing must make it exactly one (spawns {:?})",
        by_id.spawned.get("ffmpeg").map(BTreeSet::len).unwrap_or(0),
        by_id.spawned
    );
    for status in &coalesced_statuses {
        assert_eq!(*status, StatusCode::OK, "coalesced request did not succeed");
    }
}

/// Structural guard for the two claims a behaviour test cannot cheaply reach:
/// every media child has a deadline, and every media child is killed when the
/// request that owns it is dropped. A count comparison fails the moment a fifth
/// media spawn, or a spawn without one of the two, is added.
#[test]
fn every_media_child_process_has_a_deadline_and_kill_on_drop() {
    let src = include_str!("../src/routes/settings.rs");
    let spawns = src
        .matches("Command::new(strivo_core::tools::resolve_tool(")
        .count();
    let killed = src.matches("kill_on_drop(true)").count();
    let deadlines = src.matches("tokio::time::timeout(").count();
    assert_eq!(
        spawns, 4,
        "expected the four media spawns (probe, 2x thumbnail, remux)"
    );
    assert_eq!(
        spawns, killed,
        "every media child must set kill_on_drop(true) so a cancelled request does not leave \
         an orphan ffmpeg holding a pool permit"
    );
    assert_eq!(
        spawns, deadlines,
        "every media child must be wrapped in a tokio::time::timeout, or a wedged ffmpeg \
         occupies a pool slot forever"
    );
}
