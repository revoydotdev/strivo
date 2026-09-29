//! Todo 54 — an authored graph is the run, not a description of one.
//!
//! The properties under test are the ones only a real request can falsify:
//!
//!   1. IDENTITY AT THREE POINTS. The `PipelineId` minted when the graph was
//!      authored is the id the HTTP response reports, the id the daemon
//!      receives, and the id `GET /api/v1/pipelines/runs` later lists. Read
//!      back at all three, because a re-mint in any one of them is invisible
//!      to the other two.
//!   2. ORDER, KINDS AND LOCKS. The run's stages equal the graph's nodes
//!      **in the same order**. A `HashMap` round trip happens to produce the
//!      right multiset, so a weaker assertion would pass it; the comparison is
//!      a `Vec` and the fixture's stages are mutually distinguishable.
//!   3. NO PARALLEL REPRESENTATION. The daemon receives the caller's own
//!      `Pipeline` value. Asserted by equality against the submitted bytes,
//!      not by a count of "it looked like a graph".
//!   4. RULE 7 IS STILL THE BACKSTOP. With no editor in front of the caller —
//!      which is exactly this test — a graph dispatching to a plugin this
//!      build does not register is refused *before* the IPC write, so it
//!      cannot fail hours later at dispatch on a real recording.
//!
//! The daemon is stood in for by a real `PipelineRegistry` writing the real
//! `pipelines.json` over a real socket at the path `IpcClient` dials, so the
//! history the runs route reads is written by the executor's own persistence
//! rather than by the test.
#![cfg(feature = "creator")]

use std::sync::OnceLock;
use std::time::Duration;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

use strivo_core::ipc::{ClientMessage, ServerMessage};
use strivo_core::pipeline::executor::PipelineRegistry;
use strivo_core::pipeline::stage::ResourceLock;
use strivo_core::pipeline::{Pipeline, Stage, StageDispatch, StageKind};
use strivo_core::platform::PlatformKind;
use strivo_core::recording::job::{RecordingJob, RecordingState};
use strivo_web::routes::pipeline_graph::RULE_UNKNOWN_DISPATCH;
use strivo_web::server::{build_router, AppState};

const KEY: &str = "t54-graph-to-run-key";

/// One sandbox for the whole binary. `XDG_DATA_HOME` is how
/// `AppConfig::data_dir()` resolves and `XDG_STATE_HOME` is how `state_dir()`
/// — and therefore `strivo.sock` — resolves, so both are pointed at a
/// throwaway directory.
///
/// Process-wide, because `set_var` is process-wide: the socket path is derived
/// from the environment, so two tests wanting two sandboxes would race on it
/// rather than get two. Concurrency is therefore confined to one test at a
/// time via [`serial`], and the two tests that need the daemon each take it in
/// turn.
fn sandbox() -> &'static str {
    static DIR: OnceLock<String> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("strivo-t54-web-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create sandbox");
        std::env::set_var("XDG_DATA_HOME", &dir);
        std::env::set_var("XDG_STATE_HOME", &dir);
        dir.to_string_lossy().to_string()
    })
}

/// The one shared sandbox, one test at a time. `strivo.sock` is a single path
/// derived from process-wide state, so two tests binding it concurrently
/// collide with `AddrInUse` — which is a harness artefact, not a product
/// defect, and must not be allowed to read as one.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn serial() -> tokio::sync::MutexGuard<'static, ()> {
    SERIAL.lock().await
}

/// What the fake daemon observed. Handed back so each test can prove the
/// request reached the IPC write rather than inferring it from a 202.
struct Daemon {
    received: tokio::sync::mpsc::UnboundedReceiver<Pipeline>,
    /// The finished recording the snapshot advertises. A graph rooted in a
    /// recording is gated on exactly this id, so the test needs the real one
    /// rather than the nil uuid.
    recording_id: uuid::Uuid,
    _dir: std::sync::Arc<std::path::PathBuf>,
}

/// Bind the socket the web client dials and serve it with a real
/// `PipelineRegistry`, answering `Hello` with a snapshot holding one finished
/// recording whose media file exists.
async fn fake_daemon() -> Daemon {
    let dir = std::sync::Arc::new(std::path::PathBuf::from(sandbox()));
    let state_dir = strivo_core::config::AppConfig::state_dir();
    std::fs::create_dir_all(&state_dir).expect("state dir");

    let media = dir.join("finished.mkv");
    std::fs::write(&media, b"not really a video").expect("media fixture");
    let mut job = RecordingJob::new(
        "channel".into(),
        "Channel".into(),
        PlatformKind::Twitch,
        media,
        false,
        None,
    );
    job.state = RecordingState::Finished;
    let recording_id = job.id;
    let snapshot = ServerMessage::StateSnapshot {
        version: 2,
        channels: Vec::new(),
        recordings: [(recording_id, job)].into_iter().collect(),
        twitch_connected: false,
        youtube_connected: false,
        patreon_connected: false,
        pending_auth: None,
        patreon_creators: Vec::new(),
        patreon_posts: Vec::new(),
        auth_issues: Vec::new(),
    };
    let snapshot_line = strivo_core::ipc::encode_message(&snapshot).expect("encode snapshot");

    let socket = state_dir.join("strivo.sock");
    let _ = std::fs::remove_file(&socket);
    let listener = tokio::net::UnixListener::bind(&socket).expect("bind daemon socket");
    // `is_daemon_running` gates the web client's connect, and it checks a pid
    // file. This process is alive, so its own pid is an honest answer.
    std::fs::write(state_dir.join("strivo.pid"), std::process::id().to_string())
        .expect("write pid file");

    let registry_path = strivo_core::config::AppConfig::data_dir().join("pipelines.json");
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut registry = PipelineRegistry::open(&registry_path).expect("open real registry");
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
            let mut reader = BufReader::new(&mut stream).lines();
            let Ok(Some(line)) = reader.next_line().await else {
                continue;
            };
            match serde_json::from_str::<ClientMessage>(&line) {
                Ok(ClientMessage::SubmitPipeline(pipeline)) => {
                    // The real registry's own refusal and its own persistence,
                    // so the history the runs route reads is written by the
                    // executor rather than by this test.
                    let _ = registry.submit(pipeline.clone());
                    let _ = tx.send(pipeline);
                }
                Ok(_) => {
                    let _ = stream.write_all(snapshot_line.as_bytes()).await;
                    let _ = stream.flush().await;
                }
                Err(error) => panic!("fake daemon could not decode {line:?}: {error}"),
            }
        }
    });

    Daemon {
        received: rx,
        recording_id,
        _dir: dir,
    }
}

fn app() -> axum::Router {
    build_router(AppState::test_state_at(
        KEY,
        strivo_core::ipc::Endpoint::Path(
            strivo_core::config::AppConfig::state_dir().join("strivo.sock"),
        ),
    ))
}

async fn post_run(app: axum::Router, body: Value) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/pipelines/runs")
        .header("x-api-key", KEY)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("request");
    let response = app.oneshot(request).await.expect("router ran");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1 << 20).await.expect("body");
    let parsed = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, parsed)
}

async fn get_runs(app: axum::Router) -> Value {
    let request = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/pipelines/runs")
        .header("x-api-key", KEY)
        .body(Body::empty())
        .expect("request");
    let response = app.oneshot(request).await.expect("router ran");
    assert_eq!(response.status(), StatusCode::OK, "runs route answered");
    let bytes = to_bytes(response.into_body(), 1 << 22).await.expect("body");
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

/// The authored graph, as the browser sends it. Three stages that differ in
/// **all three** dimensions the acceptance names — kind, lock, and attempts —
/// so no assertion below can be satisfied by a permutation.
fn authored_graph(recording_id: uuid::Uuid) -> Pipeline {
    let mut pipeline = Pipeline::new("authored graph")
        .for_recording(recording_id)
        .with_trigger("manual");
    let root = pipeline.add_stage(
        Stage::new(
            "Detect cuepoints",
            StageKind::Custom("crunchr.extract".into()),
        )
        // These must be names `strivo_plugins::register_all` installs, or the
        // seventh rule refuses the graph before the daemon sees it and the test
        // passes for the wrong reason. They were `cuepoints`/`captions`, which
        // no build registers.
        .with_dispatch(StageDispatch::new("crunchr", "extract").for_recording(recording_id))
        .with_requires(vec![ResourceLock::Cpu { cap: 2 }])
        .with_max_attempts(5),
    );
    let branch = pipeline.add_stage(
        Stage::new("Export captions", StageKind::Subtitle)
            .with_dispatch(StageDispatch::new("artifacts", "captions").for_recording(recording_id))
            .with_inputs(vec![root])
            .with_requires(vec![ResourceLock::Gpu])
            .with_max_attempts(2),
    );
    pipeline.add_stage(
        Stage::new("Concat outputs", StageKind::Concat)
            .with_dispatch(StageDispatch::new("artifacts", "concat").for_recording(recording_id))
            .with_inputs(vec![root, branch])
            .with_requires(vec![ResourceLock::Api {
                name: "openai".into(),
                cap: 7,
            }])
            .with_max_attempts(1),
    );
    pipeline
}

/// One ordered row per stage: `(name, kind, lock labels, attempts)`. A `Vec`,
/// so a re-ordering is a difference; and read off the serialised payload for
/// the HTTP assertions, off the value for the daemon-side one.
///
/// `kind` and `requires` are externally-tagged enums, so they arrive as
/// objects (`{"Custom":"…"}`, `{"Cpu":{"cap":2}}`) and `as_str` would silently
/// yield `""` for every one. Each row is re-parsed into the real type and
/// labelled by its own `label()`, so the HTTP rows and the daemon-side rows
/// are produced by the same code rather than by a second formatting rule that
/// can drift from the first.
fn shape_from_value(pipeline: &Value) -> Vec<(String, String, Vec<String>, u64)> {
    pipeline["stages"]
        .as_array()
        .expect("stages array")
        .iter()
        .map(|stage| {
            let kind: StageKind =
                serde_json::from_value(stage["kind"].clone()).expect("a stage kind round-trips");
            let requires: Vec<ResourceLock> =
                serde_json::from_value(stage["requires"].clone()).expect("stage locks round-trip");
            (
                stage["name"].as_str().unwrap_or_default().to_string(),
                kind.label(),
                requires.iter().map(|l| l.label()).collect(),
                stage["max_attempts"].as_u64().unwrap_or_default(),
            )
        })
        .collect()
}

fn shape_from_pipeline(pipeline: &Pipeline) -> Vec<(String, String, Vec<String>, u8)> {
    pipeline
        .stages
        .iter()
        .map(|stage| {
            (
                stage.name.clone(),
                stage.kind.label(),
                stage.requires.iter().map(|l| l.label()).collect(),
                stage.max_attempts,
            )
        })
        .collect()
}

fn expected_shape(pipeline: &Pipeline) -> Vec<(String, String, Vec<String>, u64)> {
    pipeline
        .stages
        .iter()
        .map(|stage| {
            (
                stage.name.clone(),
                stage.kind.label(),
                stage.requires.iter().map(|l| l.label()).collect(),
                u64::from(stage.max_attempts),
            )
        })
        .collect()
}

/// The whole todo, over the real router: submit an authored graph, and read
/// its identity and its ordered stages back at all three points.
#[tokio::test]
async fn an_authored_graph_reaches_the_executor_and_history_with_its_identity() {
    let _guard = serial().await;
    sandbox();
    let mut daemon = fake_daemon().await;
    let graph = authored_graph(daemon.recording_id);
    let authored_id = graph.id.to_string();
    let expected = expected_shape(&graph);
    assert_eq!(expected.len(), 3);

    let (status, body) = post_run(app(), json!({ "graph": graph })).await;
    assert_eq!(status, StatusCode::ACCEPTED, "body was {body}");
    assert_eq!(
        body["pipeline_id"].as_str(),
        Some(authored_id.as_str()),
        "the response reports the id the author minted, not one minted here"
    );

    // Point two: what the daemon actually received.
    let received = tokio::time::timeout(Duration::from_secs(10), daemon.received.recv())
        .await
        .expect("the daemon received a submission")
        .expect("channel open");
    assert_eq!(received.id.to_string(), authored_id, "same id over IPC");
    assert_eq!(
        shape_from_pipeline(&received),
        shape_from_pipeline(&graph),
        "the daemon received the author's graph: same order, kinds and locks, no translation"
    );

    // Point three: what the runs route reports.
    let runs = get_runs(app()).await;
    let listed = runs["runs"]
        .as_array()
        .expect("runs array")
        .iter()
        .find(|run| run["id"].as_str() == Some(authored_id.as_str()))
        .unwrap_or_else(|| panic!("the authored id is not in runs: {runs}"));
    assert_eq!(
        shape_from_value(listed),
        expected,
        "history holds the same ordered stages, kinds and locks"
    );
    // And the graph the runs payload draws is the same run, not a template.
    let run_graph = runs["graph"]["graphs"]
        .as_array()
        .expect("graphs")
        .iter()
        .find(|g| g["run_id"].as_str() == Some(authored_id.as_str()))
        .expect("the run is drawn as a run graph");
    assert_eq!(run_graph["kind"], json!("run"));
    let drawn = run_graph["nodes"]
        .as_array()
        .expect("nodes")
        .iter()
        .map(|n| n["kind"].as_str().unwrap_or_default().to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        drawn,
        expected
            .iter()
            .map(|(_, kind, _, _)| kind.clone())
            .collect::<Vec<_>>(),
        "the drawn graph's node order is the authored order"
    );
}

/// Falsifiability, mechanical: on this fixture, a submission path that
/// reordered the stage list is detected rather than absorbed. Without this the
/// ordered assertions above could pass vacuously.
#[test]
fn the_ordered_comparison_rejects_a_reordered_graph() {
    let graph = authored_graph(uuid::Uuid::nil());
    let expected = expected_shape(&graph);
    let mut reordered = graph.clone();
    reordered.stages.reverse();
    let reordered_shape = expected_shape(&reordered);
    assert_ne!(
        expected, reordered_shape,
        "a reordered graph must not compare equal, or the order assertion proves nothing"
    );
    assert_eq!(expected[0].1, "custom(crunchr.extract)");
    assert_eq!(reordered_shape[0].1, "concat");
    // And the multiset is identical, which is precisely why a set-based
    // comparison would have passed the defect.
    let mut a = expected.clone();
    let mut b = reordered_shape;
    a.sort();
    b.sort();
    assert_eq!(a, b, "only the order differs");
}

/// Rule 7 cannot be the executor's job — it has no plugin registry — so this
/// route is the backstop for a caller with no editor. The refusal must land
/// **before** the IPC write, so an unresolvable dispatch never becomes a run.
#[tokio::test]
async fn an_unresolvable_dispatch_is_refused_before_the_daemon_ever_sees_it() {
    let _guard = serial().await;
    sandbox();
    let mut daemon = fake_daemon().await;
    let mut graph = authored_graph(daemon.recording_id);
    graph.stages[0].dispatch = Some(StageDispatch::new("not-installed", "extract"));

    let (status, body) = post_run(app(), json!({ "graph": graph })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body was {body}");
    assert_eq!(body["code"], json!("graph/invalid"));
    let errors = body["errors"].as_array().expect("errors array");
    assert_eq!(errors[0]["rule"], json!(RULE_UNKNOWN_DISPATCH));
    assert!(
        errors[0]["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("not-installed"),
        "the refusal names the plugin: {errors:?}"
    );

    assert!(
        tokio::time::timeout(Duration::from_millis(300), daemon.received.recv())
            .await
            .is_err(),
        "nothing may reach the executor: the backstop is before the IPC write"
    );
    // And the runs route has no record of THIS graph. Asserted by id rather
    // than by count: the sandbox is shared across the binary's tests, so an
    // earlier test's legitimate run is in the same registry, and counting
    // would blame the refusal for a run it did not create.
    let runs = get_runs(app()).await;
    let refused_id = graph.id.to_string();
    let listed = runs["runs"]
        .as_array()
        .expect("runs array")
        .iter()
        .find(|run| run["id"].as_str() == Some(refused_id.as_str()));
    assert!(
        listed.is_none(),
        "a refused graph must leave no run: {listed:?}"
    );
}
