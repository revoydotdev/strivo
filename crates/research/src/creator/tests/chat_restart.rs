//! The QA failure scenario, run against a real process: `SIGKILL` the daemon
//! mid-collection and prove the gap is **recorded** rather than hidden.
//!
//! The in-process test next door
//! (`super::chat::collection_survives_a_daemon_restart_and_the_dead_interval_is_recorded`)
//! drops a `Store` and a collection without closing either, which is the same
//! *state* but not the same event — a Rust `Drop` runs no destructor here, yet
//! it is still an orderly return from `main`. This file closes that gap by
//! killing an actual child process with an unhandleable signal, which is what a
//! power cut or an OOM kill looks like, and then reopening the database from
//! scratch.
//!
//! The assertion is deliberately about the *recorded* gap and not about an
//! absence: a collection the dead process left open must produce a durable
//! `daemon_down` interval whose end is the moment the new process noticed. An
//! implementation that quietly closed the row on open would leave the messages
//! intact and this test would still pass, so the interval is checked too — the
//! same lesson as mutation probe M7.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use strivo_editor::time::{ClockMap, MediaTime};

use super::super::ids::CaptureSessionId;
use super::super::{GapReason, RawChatMessage, Store};

/// The env var that turns a test binary into the doomed daemon. A name no
/// ordinary run sets, so the child only ever does this when asked.
const CHILD: &str = "STRIVO_T35_DOOMED_DAEMON";

const T0: i64 = 1_789_000_000_000;

fn scratch() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("strivo-t35-kill-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

fn window() -> super::super::CaptureWindow {
    super::super::CaptureWindow::new(T0, T0 + 60_000).expect("forward")
}

fn clock() -> ClockMap {
    let mut map = ClockMap::new();
    map.push(strivo_editor::time::ClockSegment::exact(
        T0,
        MediaTime::ZERO,
        MediaTime::from_millis(60_000).expect("60 s"),
    ))
    .expect("one segment");
    map
}

/// When re-executed as the child: open a collection, store two messages, and
/// then wait to be killed. It never closes anything.
fn run_as_doomed_daemon() -> ! {
    let db = std::env::var("STRIVO_T35_DB").expect("the parent named a database");
    let store = Store::open(&db).expect("open");
    let project = super::fixtures::project("doomed");
    let project_id = project.id;
    store.insert_project(&project).expect("project");
    let session = CaptureSessionId::generate();
    store
        .insert_capture_session(&super::super::entities::CaptureSession {
            id: session,
            project_id,
            source_asset_id: None,
            started_at: "2026-09-07T00:00:00Z".into(),
            ended_at: None,
            finalized: false,
            attributes_json: "{}".into(),
            created_at: "2026-09-07T00:00:00Z".into(),
        })
        .expect("session");
    let collection = store
        .open_chat_collection(session, project_id, "twitch", "streamer", T0)
        .expect("open a collection");
    store
        .append_chat(
            &collection,
            &[
                RawChatMessage {
                    wall_ms: T0 + 1_000,
                    room: "streamer".into(),
                    sender: "ann".into(),
                    text: "sent before the kill".into(),
                    is_action: false,
                    provider_id: Some("m1".into()),
                },
                RawChatMessage {
                    wall_ms: T0 + 2_000,
                    room: "streamer".into(),
                    sender: "bo".into(),
                    text: "also before the kill".into(),
                    is_action: false,
                    provider_id: Some("m2".into()),
                },
            ],
            &window(),
            &clock(),
        )
        .expect("append");
    // Hand the parent the ids it needs, then block. No close, no flush, no
    // orderly return: this is the `kill -9` the QA scenario asks for.
    println!("READY {session}");
    std::io::Write::flush(&mut std::io::stdout()).expect("flush");
    loop {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

#[test]
fn killing_the_daemon_mid_collection_records_the_gap_rather_than_hiding_it() {
    if std::env::var(CHILD).is_ok() {
        run_as_doomed_daemon();
    }

    let dir = scratch();
    let db = dir.join("research.db");
    let exe = std::env::current_exe().expect("this test binary");

    // Spawn the doomed daemon and wait until it says it has written.
    let mut child = Command::new(&exe)
        .env(CHILD, "1")
        .env("STRIVO_T35_DB", &db)
        .args([
            "--exact",
            "creator::tests::chat_restart::killing_the_daemon_mid_collection_records_the_gap_rather_than_hiding_it",
            "--nocapture",
        ])
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn the doomed daemon");

    let session = {
        use std::io::BufRead;
        let stdout = child.stdout.take().expect("piped");
        let mut reader = std::io::BufReader::new(stdout);
        let mut line = String::new();
        // The child is a test binary too, so its stdout carries harness noise
        // ("running 1 test", …) before the announcement. Read until the marker
        // rather than assuming it is the first line — an assumption here would
        // fail for a reason that has nothing to do with the daemon dying.
        let mut found = None;
        for _ in 0..64 {
            line.clear();
            if reader.read_line(&mut line).is_err() {
                break;
            }
            if let Some(id) = line.trim().strip_prefix("READY ") {
                found = Some(id.to_string());
                break;
            }
        }
        found
            .expect("the child announced itself")
            .parse::<uuid::Uuid>()
            .expect("a session id")
    };

    // The kill. `libc::kill` is not a dependency of this crate, and the
    // portable equivalent is `Child::kill`, which on Unix sends SIGKILL.
    child.kill().expect("SIGKILL the daemon");
    let status = child.wait().expect("reap");
    assert!(
        !status.success(),
        "the daemon really died, rather than exiting cleanly: {status}"
    );

    // A new process opens the same database and finds the hole.
    let store = Store::open(&db).expect("the new process");
    let session = CaptureSessionId::from_uuid(session);
    let reaped = store
        .reap_open_collections(session, T0 + 30_000)
        .expect("reap");

    assert_eq!(reaped.len(), 1, "the dangling collection produced one gap");
    assert_eq!(reaped[0].reason, GapReason::DaemonDown);
    assert_eq!(reaped[0].start_wall_ms, T0);
    assert_eq!(reaped[0].end_wall_ms, T0 + 30_000);

    let gaps = store.chat_gaps(session).expect("gaps");
    assert_eq!(gaps.len(), 1, "and it is durable");
    assert_eq!(gaps[0].reason, GapReason::DaemonDown);
    assert_eq!(
        (gaps[0].start_wall_ms, gaps[0].end_wall_ms),
        (T0, T0 + 30_000),
        "spanning the interval the dead process was covering — not a point, and \
         not the whole window"
    );

    // And the messages it did manage to write are still there. The point of
    // the scenario is that the record is *partial and labelled*, not empty.
    let messages = store.chat_messages(session).expect("read");
    assert_eq!(messages.len(), 2, "what it collected survived the kill");
    assert_eq!(messages[0].text, "sent before the kill");

    // Finally the figure a report would print: partial, and it says why.
    let coverage = store.chat_coverage(session, &window()).expect("coverage");
    assert!(
        coverage.per_mille < 1_000,
        "30 s of a 60 s window is not full coverage"
    );
    assert!(
        coverage
            .gaps
            .iter()
            .any(|g| g.reason == GapReason::DaemonDown),
        "and the shortfall names the daemon being down: {:?}",
        coverage.gaps
    );
    coverage
        .validated()
        .expect("a partial figure with a reason is reportable");

    let _ = std::fs::remove_dir_all(&dir);
}
