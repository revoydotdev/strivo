//! Daemon-owned durable chat: acquisition survives a restart, dedupe leaves
//! no duplicate, and every gap is recorded rather than counted away.
//!
//! The claims under test, in the order the plan states them:
//!
//! 1. a collection runs with no browser, survives a restart, and replays
//!    aligned to a capture landmark;
//! 2. a reconnection produces neither a duplicate nor an unrecorded gap;
//! 3. every gap is either recorded or explicitly stated, never silently empty.
//!
//! **Claim 1 is the one that was false before this todo.** Chat was a browser
//! WebSocket (`crates/strivo-web/src/routes/chat.rs:1-9`), so the record
//! existed only while a tab was open, and
//! `docs/STRATEGY-NVIVO-RIVERSIDE.md:26-30` claimed otherwise. The test that
//! matters is therefore
//! [`collection_survives_a_daemon_restart_and_the_dead_interval_is_recorded`]:
//! it drops the store and the collection without closing either, reopens both,
//! and asserts the messages are still there **and** the interval the dead
//! process was covering is a recorded gap rather than an absence.
//!
//! **A gap here is a recorded interval, never a counter.** There is no
//! `dropped: u64` anywhere in this model, and [`chat_batch_never_loses_an_event`]
//! is what holds the line: a batch's outcome *partitions* its input, so an
//! event that is neither stored nor a known duplicate has nowhere to go.

use std::path::{Path, PathBuf};

use strivo_editor::time::{ClockMap, MediaTime};

use super::super::ids::{CaptureSessionId, ProjectId};
use super::super::{
    ChatDeletionTarget, ChatIdentity, ChatReplay, CollectionEnd, Coverage, GapReason, Landmark,
    ObservationError, RawChatDeletion, RawChatMessage, Store, MAX_LABEL_BYTES,
};

/// 0-based wall clock, so no assertion here can be a wall-clock flake.
pub(super) const T0: i64 = 1_789_000_000_000;

/// The real capture this todo's replay aligns to.
///
/// `fixtures/media/landmarks.mkv` is todo 32's committed corpus: 8.0 s of
/// media over 200 frames, whose `sha256` the manifest records. The landmark is
/// its first frame, and the clock map below is anchored to it, so the offsets a
/// replay reports are offsets on a real recording rather than on a number
/// invented for the test. The manifest — not a live `ffprobe` — is the source
/// of the 8.0 s, because no CI job installs ffmpeg (todo 37 measured that);
/// the consequence is disclosed rather than hidden.
fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/media")
}

fn landmarks_mkv() -> PathBuf {
    corpus().join("landmarks.mkv")
}

pub(super) fn window() -> super::super::CaptureWindow {
    super::super::CaptureWindow::new(T0, T0 + 8_000).expect("a forward window")
}

/// One unbroken 8 s capture at real time, anchored to `landmarks.mkv`'s first
/// frame. Exact rate, so the clock's own uncertainty across it is a measured
/// zero and every non-zero bound in these tests comes from somewhere else.
pub(super) fn clock_over_the_real_recording() -> ClockMap {
    let mut clock = ClockMap::new();
    clock
        .push(strivo_editor::time::ClockSegment::exact(
            T0,
            MediaTime::ZERO,
            MediaTime::from_millis(8_000).expect("8 s"),
        ))
        .expect("one segment");
    clock
}

fn go_live() -> Landmark {
    Landmark::new("landmarks.mkv frame 0", MediaTime::ZERO)
}

pub(super) fn with_session(store: &Store, name: &str) -> (ProjectId, CaptureSessionId) {
    let project = super::fixtures::project(name);
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
        .expect("capture session");
    (project_id, session)
}

fn msg(wall_offset_ms: i64, id: &str, sender: &str, text: &str) -> RawChatMessage {
    RawChatMessage {
        wall_ms: T0 + wall_offset_ms,
        room: "streamer".into(),
        sender: sender.into(),
        text: text.into(),
        is_action: false,
        provider_id: Some(id.into()),
    }
}

// --- the corpus is real --------------------------------------------------

#[test]
fn the_replay_anchor_is_the_committed_corpus_recording() {
    let file = landmarks_mkv();
    let bytes = std::fs::read(&file).expect("todo 32 committed the corpus");
    let manifest = std::fs::read_to_string(corpus().join("MANIFEST.json")).expect("manifest");
    let entry = manifest
        .split("\"name\": \"landmarks.mkv\"")
        .nth(1)
        .expect("the manifest names the landmark fixture");
    let declared_sha = entry
        .split("\"sha256\": \"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("a recorded sha256");
    let declared_len: usize = entry
        .split("\"bytes\": ")
        .nth(1)
        .and_then(|rest| rest.split(',').next())
        .and_then(|n| n.trim().parse().ok())
        .expect("a recorded byte count");

    // The landmark is a real file, not a placeholder: the hash and the length
    // are re-derived from the bytes on disk, and the manifest's own duration
    // is the 8 s the clock map covers.
    assert_eq!(
        bytes.len(),
        declared_len,
        "the corpus file is the manifest's"
    );
    use sha2::{Digest, Sha256};
    let actual = format!("{:x}", Sha256::digest(&bytes));
    assert_eq!(
        actual, declared_sha,
        "the corpus file is the manifest's bytes"
    );
    assert!(
        manifest.contains("\"duration_secs\": 8.0"),
        "the clock map's 8 s of media is the recording's real duration"
    );
}

// --- claim 1: daemon-owned, survives a restart, replays on the clock ------

#[test]
fn a_collected_message_replays_at_its_exact_offset_from_the_landmark() {
    let store = Store::open_in_memory().expect("store");
    let (project, session) = with_session(&store, "stream");
    let collection = store
        .open_chat_collection(session, project, "twitch", "streamer", T0)
        .expect("the daemon opens the collection, not the browser");
    store
        .append_chat(
            &collection,
            &[
                msg(1_000, "m1", "ann", "hello"),
                msg(3_000, "m2", "bo", "raid at 3s"),
            ],
            &window(),
            &clock_over_the_real_recording(),
        )
        .expect("append");

    let replay: ChatReplay = store
        .replay_chat(session, &window(), &go_live())
        .expect("replay");

    assert_eq!(replay.landmark.label, "landmarks.mkv frame 0");
    assert_eq!(replay.messages.len(), 2, "both messages replay");
    // The alignment is the claim: a message the platform stamped 3 000 ms
    // after go-live replays exactly 3 000 000 µs after the landmark, on an
    // exact-rate segment, so the answer is a measurement and not a rounding.
    let second = &replay.messages[1];
    assert_eq!(second.offset_from_landmark.micros(), 3_000_000);
    assert_eq!(
        second.position.uncertainty_micros, 0,
        "a point event on an exact segment has a measured zero bound"
    );
    assert_eq!(replay.messages[0].offset_from_landmark.micros(), 1_000_000);
}

#[test]
fn collection_survives_a_daemon_restart_and_the_dead_interval_is_recorded() {
    // Given a daemon that collected for three seconds and then died without
    // closing its collection — the process is gone, not shut down.
    let dir = std::env::temp_dir().join(format!("strivo-t35-restart-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch");
    let db = dir.join("research.db");
    let (project, session, first_collection) = {
        let store = Store::open(&db).expect("first process");
        let (project, session) = with_session(&store, "stream");
        let collection = store
            .open_chat_collection(session, project, "twitch", "streamer", T0)
            .expect("open");
        store
            .append_chat(
                &collection,
                &[msg(500, "m1", "ann", "before the crash")],
                &window(),
                &clock_over_the_real_recording(),
            )
            .expect("append");
        (project, session, collection)
        // `store` and the collection are dropped here. No close call, no
        // flush, no goodbye: this is `kill -9`, not a shutdown.
    };
    assert_eq!(
        first_collection.opened_at_wall_ms, T0,
        "the collection really was open when the process ended"
    );

    // When a new process starts and opens a collection for the same room
    let store = Store::open(&db).expect("second process");
    let reopened = store
        .open_chat_collection(session, project, "twitch", "streamer", T0 + 5_000)
        .expect("reopen");
    let reaped = store
        .reap_open_collections(session, T0 + 5_000)
        .expect("reap the row the dead process left open");

    // Then the record survived the restart ...
    let replay = store
        .replay_chat(session, &window(), &go_live())
        .expect("replay");
    assert_eq!(
        replay.messages.len(),
        1,
        "closing the browser — here, killing the process — does not stop the record"
    );
    assert_eq!(replay.messages[0].message.text, "before the crash");

    // ... and the interval the dead process was covering is RECORDED, not
    // absent. One gap, from the first collection's open instant to the moment
    // the new process noticed, and it names the cause.
    assert_eq!(reaped.len(), 1, "the dangling collection produced one gap");
    assert_eq!(reaped[0].reason, GapReason::DaemonDown);
    assert_eq!(reaped[0].start_wall_ms, T0);
    assert_eq!(reaped[0].end_wall_ms, T0 + 5_000);
    let gaps = store.chat_gaps(session).expect("gaps");
    assert_eq!(gaps.len(), 1, "and it is durable, not only returned");
    assert_eq!(gaps[0].reason, GapReason::DaemonDown);
    // The *persisted* interval, not just the returned one. A reaper that closed
    // the dangling row and wrote a degenerate gap beside it would pass an
    // assertion on `reaped` alone, which is the shape of probe M7 below — the
    // row is the record, so the row's interval is what has to be right.
    assert_eq!(
        (gaps[0].start_wall_ms, gaps[0].end_wall_ms),
        (T0, T0 + 5_000),
        "the durable gap spans the interval the dead process was covering"
    );
    assert_ne!(
        reopened.id, first_collection.id,
        "a new collection, so the two are distinguishable"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_unclosed_collection_is_visible_as_open_until_something_closes_it() {
    let store = Store::open_in_memory().expect("store");
    let (project, session) = with_session(&store, "stream");
    let collection = store
        .open_chat_collection(session, project, "twitch", "streamer", T0)
        .expect("open");
    let open = store.open_chat_collections(session).expect("read");
    assert_eq!(open.len(), 1);
    assert!(
        open[0].is_open(),
        "ended_wall_ms NULL is the record of a live one"
    );

    store
        .close_chat_collection(&collection, T0 + 2_000, CollectionEnd::Closed)
        .expect("close");
    let open = store.open_chat_collections(session).expect("read");
    assert!(open.is_empty(), "a closed collection is not open");
    let all = store.chat_collections(session).expect("read");
    assert_eq!(all.len(), 1, "and it is still in the record");
}

#[test]
fn closing_a_collection_early_does_not_claim_the_rest_of_the_window_was_covered() {
    // The anti-vacuity control for coverage: a collection that stopped after
    // 2 s of an 8 s window has 6 s nobody watched. A quiet room is not a gap;
    // a stopped collector is.
    let store = Store::open_in_memory().expect("store");
    let (project, session) = with_session(&store, "stream");
    let collection = store
        .open_chat_collection(session, project, "twitch", "streamer", T0)
        .expect("open");
    store
        .close_chat_collection(&collection, T0 + 2_000, CollectionEnd::Disconnected)
        .expect("close");

    let coverage = store.chat_coverage(session, &window()).expect("coverage");
    assert!(
        coverage.per_mille < 1_000,
        "an 8 s window with a 2 s collector is not full: {}",
        coverage.per_mille
    );
    assert!(
        !coverage.gaps.is_empty(),
        "and the shortfall names its interval"
    );
    assert!(
        coverage
            .gaps
            .iter()
            .any(|g| g.end_wall_ms >= T0 + 7_999 && g.reason != GapReason::NoSamples),
        "the uncovered tail is a real reason, got {:?}",
        coverage.gaps
    );
}

// --- claim 2: a reconnect leaves no duplicate and no unrecorded gap -------

#[test]
fn a_reconnect_redelivers_a_message_without_duplicating_it() {
    // Given a collector that lost the socket and reconnected, and the
    // platform replaying the last message it sent before the drop.
    let store = Store::open_in_memory().expect("store");
    let (project, session) = with_session(&store, "stream");
    let clock = clock_over_the_real_recording();
    let first = store
        .open_chat_collection(session, project, "twitch", "streamer", T0)
        .expect("open");
    let before = store
        .append_chat(
            &first,
            &[msg(1_000, "m1", "ann", "the redelivered one")],
            &window(),
            &clock,
        )
        .expect("first append");
    assert_eq!(before.stored, 1);
    store
        .close_chat_collection(&first, T0 + 2_000, CollectionEnd::Disconnected)
        .expect("the socket dropped");
    store
        .record_chat_gap(
            &first,
            &super::super::CoverageGap {
                start_wall_ms: T0 + 2_000,
                end_wall_ms: T0 + 4_000,
                reason: GapReason::Disconnected,
            },
        )
        .expect("the hole is recorded at the moment it happens");
    let second = store
        .open_chat_collection(session, project, "twitch", "streamer", T0 + 4_000)
        .expect("reconnected");

    // When the platform redelivers the same message after the reconnect
    let after = store
        .append_chat(
            &second,
            &[msg(1_000, "m1", "ann", "the redelivered one")],
            &window(),
            &clock,
        )
        .expect("second append");

    // Then it is a duplicate, stated as one — and stored exactly once
    assert_eq!(after.stored, 0, "not stored a second time");
    assert_eq!(after.already_present, 1, "and it is named, not dropped");
    assert!(
        after.is_accounted_for(1),
        "the batch is fully accounted for"
    );
    let replay = store
        .replay_chat(session, &window(), &go_live())
        .expect("replay");
    assert_eq!(replay.messages.len(), 1, "one message, one row");
    // And the hole is still in the record, from before the reconnect.
    let gaps = store.chat_gaps(session).expect("gaps");
    assert_eq!(gaps.len(), 1, "the disconnect hole was recorded");
    assert_eq!(gaps[0].reason, GapReason::Disconnected);
    assert_eq!(
        (gaps[0].start_wall_ms, gaps[0].end_wall_ms),
        (T0 + 2_000, T0 + 4_000)
    );
}

#[test]
fn dedupe_works_on_content_when_the_platform_supplies_no_id() {
    // The positive control for the fingerprint half of the identity rule: a
    // log with no `id` tags at all, replayed through a reconnect. If the
    // fingerprint path were never taken this test could not fail.
    let store = Store::open_in_memory().expect("store");
    let (project, session) = with_session(&store, "stream");
    let clock = clock_over_the_real_recording();
    let anonymous = RawChatMessage {
        wall_ms: T0 + 900,
        room: "streamer".into(),
        sender: "anon".into(),
        text: "no id tag here".into(),
        is_action: false,
        provider_id: None,
    };
    let first = store
        .open_chat_collection(session, project, "twitch", "streamer", T0)
        .expect("open");
    assert_eq!(
        store
            .append_chat(&first, std::slice::from_ref(&anonymous), &window(), &clock)
            .expect("append")
            .stored,
        1
    );
    let second = store
        .open_chat_collection(session, project, "twitch", "streamer", T0 + 2_000)
        .expect("reopen");

    let again = store
        .append_chat(&second, std::slice::from_ref(&anonymous), &window(), &clock)
        .expect("re-append");

    assert_eq!(again.stored, 0, "the same content is the same message");
    assert_eq!(again.already_present, 1);
    let stored = store.chat_messages(session).expect("read");
    assert_eq!(stored.len(), 1);
    assert_eq!(
        stored[0].identity,
        ChatIdentity::fingerprint("streamer", "anon", T0 + 900, "no id tag here"),
        "and its identity is the content fingerprint, stated not guessed"
    );
}

#[test]
fn the_same_log_imported_twice_adds_nothing() {
    // Todo 33's rule applied to chat: identity is content, so re-importing the
    // very same bytes is a no-op rather than a doubled corpus.
    let store = Store::open_in_memory().expect("store");
    let (project, session) = with_session(&store, "stream");
    let clock = clock_over_the_real_recording();
    let log: Vec<RawChatMessage> = (0..5)
        .map(|i| {
            msg(
                1_000 + i * 500,
                &format!("m{i}"),
                "ann",
                &format!("line {i}"),
            )
        })
        .collect();
    let first = store
        .open_chat_collection(session, project, "twitch", "streamer", T0)
        .expect("open");
    let a = store
        .append_chat(&first, &log, &window(), &clock)
        .expect("first");
    assert_eq!(a.stored, 5);
    let second = store
        .open_chat_collection(session, project, "twitch", "streamer", T0 + 4_000)
        .expect("reopen");
    let b = store
        .append_chat(&second, &log, &window(), &clock)
        .expect("again");

    assert_eq!(b.stored, 0, "a re-import stores nothing new");
    assert_eq!(b.already_present, 5, "every line is named as already held");
    assert!(b.is_accounted_for(5));
    assert_eq!(store.chat_messages(session).expect("read").len(), 5);
}

// --- claim 3: nothing is dropped silently --------------------------------

#[test]
fn chat_batch_never_loses_an_event() {
    // The structural form of "no counter for silently dropped events": the
    // outcome partitions its input, so a line that is neither stored nor a
    // known duplicate cannot be represented.
    let store = Store::open_in_memory().expect("store");
    let (project, session) = with_session(&store, "stream");
    let clock = clock_over_the_real_recording();
    let collection = store
        .open_chat_collection(session, project, "twitch", "streamer", T0)
        .expect("open");
    let batch = vec![
        msg(1_000, "m1", "ann", "one"),
        msg(1_100, "m1", "ann", "one"),
        msg(1_200, "m2", "bo", "two"),
    ];
    let outcome = store
        .append_chat(&collection, &batch, &window(), &clock)
        .expect("append");
    assert_eq!(outcome.stored, 2);
    assert_eq!(outcome.already_present, 1);
    assert!(
        outcome.is_accounted_for(batch.len()),
        "3 in, 2 stored + 1 duplicate, and no third place for a line to go"
    );
    assert!(
        !outcome.is_accounted_for(4),
        "and the check is not vacuous: it refuses a wrong total"
    );
}

#[test]
fn a_gap_is_never_silently_empty() {
    // Two directions, and the second is the one that bites.
    let store = Store::open_in_memory().expect("store");
    let (project, session) = with_session(&store, "stream");

    // A session nothing ever collected for is 0 per mille WITH a reason, not
    // 1 000 per mille with no messages.
    let nothing = store.chat_coverage(session, &window()).expect("coverage");
    assert_eq!(nothing.per_mille, 0, "no collection, no coverage");
    assert_eq!(nothing.gaps.len(), 1, "and the whole window is named");
    assert_eq!(nothing.gaps[0].reason, GapReason::NotConnected);
    assert_eq!(nothing.gaps[0].start_wall_ms, T0);
    assert_eq!(nothing.gaps[0].end_wall_ms, T0 + 8_000);
    nothing
        .validated()
        .expect("a zero figure with a reason is reportable");

    // A quiet room is NOT a gap: collections covered the window, nobody said
    // anything, and that is a complete record. This is the contrast that
    // proves the figure is about acquisition rather than talkativeness.
    let collection = store
        .open_chat_collection(session, project, "twitch", "streamer", T0)
        .expect("open");
    store
        .close_chat_collection(&collection, T0 + 8_000, CollectionEnd::Closed)
        .expect("close");
    let quiet = store.chat_coverage(session, &window()).expect("coverage");
    assert_eq!(quiet.per_mille, 1_000, "a silent room is fully covered");
    assert!(
        quiet.gaps.is_empty(),
        "and needs no reason, because none is owed"
    );
    assert_eq!(store.chat_messages(session).expect("read").len(), 0);
}

#[test]
fn a_batch_the_clock_cannot_place_is_refused_whole_and_the_interval_is_recorded() {
    // Gap refusal, not clamping: a message in a reconnect hole has no media
    // coordinate, so no row is written — and the interval it belongs to is
    // recorded as a gap, because a refused batch is a hole in the record.
    let store = Store::open_in_memory().expect("store");
    let (project, session) = with_session(&store, "stream");
    let mut clock = ClockMap::new();
    clock
        .push(strivo_editor::time::ClockSegment::exact(
            T0,
            MediaTime::ZERO,
            MediaTime::from_millis(2_000).expect("2 s"),
        ))
        .expect("first part");
    clock
        .push(strivo_editor::time::ClockSegment::exact(
            T0 + 6_000,
            MediaTime::from_millis(6_000).expect("6 s"),
            MediaTime::from_millis(2_000).expect("2 s"),
        ))
        .expect("second part");
    let collection = store
        .open_chat_collection(session, project, "twitch", "streamer", T0)
        .expect("open");

    let refused = store.append_chat(
        &collection,
        &[
            msg(1_000, "m1", "ann", "inside part one"),
            msg(4_000, "m2", "bo", "inside the hole"),
            msg(7_000, "m3", "cy", "inside part two"),
        ],
        &window(),
        &clock,
    );
    assert!(
        matches!(refused, Err(ObservationError::OutsideClockMap { .. })),
        "a message in the hole is refused, got {refused:?}"
    );
    assert_eq!(
        store.chat_messages(session).expect("read").len(),
        0,
        "the whole batch was refused: no row is half-written"
    );
    store
        .record_chat_gap(
            &collection,
            &super::super::CoverageGap {
                start_wall_ms: T0 + 2_000,
                end_wall_ms: T0 + 6_000,
                reason: GapReason::ReconnectGap,
            },
        )
        .expect("record the hole");

    // The hole is now visible in the report, which is the point: it is not
    // merely an error the caller swallowed.
    let coverage = store.chat_coverage(session, &window()).expect("coverage");
    assert!(
        coverage
            .gaps
            .iter()
            .any(|g| g.reason == GapReason::ReconnectGap),
        "the hole is in the coverage report, got {:?}",
        coverage.gaps
    );
    // And a message that *is* placeable still stores, so the refusal did not
    // poison the session.
    let after = store
        .append_chat(
            &collection,
            &[msg(7_000, "m3", "cy", "inside part two")],
            &window(),
            &clock,
        )
        .expect("placeable");
    assert_eq!(after.stored, 1);
}

// --- deletion is recorded, never applied ---------------------------------

#[test]
fn a_deleted_message_is_recorded_as_deleted_and_its_text_is_still_here() {
    // A moderator removing a message from the live room is a fact about the
    // room, not an instruction to erase the archive. Applying it would leave
    // a hole nothing could see — exactly the failure this todo exists to
    // prevent, one layer down.
    let store = Store::open_in_memory().expect("store");
    let (project, session) = with_session(&store, "stream");
    let clock = clock_over_the_real_recording();
    let collection = store
        .open_chat_collection(session, project, "twitch", "streamer", T0)
        .expect("open");
    store
        .append_chat(
            &collection,
            &[msg(1_000, "m1", "ann", "oops")],
            &window(),
            &clock,
        )
        .expect("append");

    let outcome = store
        .apply_chat_deletions(
            session,
            &[RawChatDeletion {
                wall_ms: T0 + 1_500,
                by: "mod".into(),
                target: ChatDeletionTarget::ProviderId("m1".into()),
            }],
        )
        .expect("apply");

    assert_eq!(outcome.marked, 1, "the message we hold was marked");
    assert_eq!(outcome.unresolved, 0);
    let replay = store
        .replay_chat(session, &window(), &go_live())
        .expect("replay");
    assert_eq!(replay.messages.len(), 1, "the row is not erased");
    let marked = replay.messages[0]
        .message
        .deletion
        .as_ref()
        .expect("marked deleted");
    assert_eq!(marked.by, "mod");
    assert_eq!(marked.wall_ms, T0 + 1_500);
    assert_eq!(
        replay.messages[0].message.text, "oops",
        "the text is retained verbatim"
    );
    assert!(replay.messages[0].is_deleted());
}

#[test]
fn a_deletion_of_a_message_we_never_got_is_stated_rather_than_dropped() {
    let store = Store::open_in_memory().expect("store");
    let (_project, session) = with_session(&store, "stream");
    let outcome = store
        .apply_chat_deletions(
            session,
            &[
                RawChatDeletion {
                    wall_ms: T0 + 1_500,
                    by: "mod".into(),
                    target: ChatDeletionTarget::ProviderId("never-seen".into()),
                },
                RawChatDeletion {
                    wall_ms: T0 + 1_600,
                    by: "mod".into(),
                    target: ChatDeletionTarget::Unidentifiable,
                },
            ],
        )
        .expect("apply");
    assert_eq!(outcome.marked, 0);
    assert_eq!(
        outcome.unresolved, 2,
        "both are stated: one names a message we do not hold, one could not name a message at all"
    );
    // And they are durable, so a replay can report the deletions whose
    // message is missing instead of showing a room that simply never said it.
    let replay = store
        .replay_chat(session, &window(), &go_live())
        .expect("replay");
    assert_eq!(replay.unresolved_deletions, 2);
}

#[test]
fn a_deletion_arriving_before_its_message_still_marks_it() {
    // The other order, and the one a reconnect produces: the deletion is
    // recorded, the message turns up in the next batch, and it must not come
    // back live.
    let store = Store::open_in_memory().expect("store");
    let (project, session) = with_session(&store, "stream");
    let clock = clock_over_the_real_recording();
    store
        .apply_chat_deletions(
            session,
            &[RawChatDeletion {
                wall_ms: T0 + 900,
                by: "mod".into(),
                target: ChatDeletionTarget::ProviderId("m1".into()),
            }],
        )
        .expect("deletion first");
    let collection = store
        .open_chat_collection(session, project, "twitch", "streamer", T0)
        .expect("open");
    let appended = store
        .append_chat(
            &collection,
            &[msg(1_000, "m1", "ann", "already removed")],
            &window(),
            &clock,
        )
        .expect("append after the deletion");

    assert_eq!(appended.stored, 1);
    let replay = store
        .replay_chat(session, &window(), &go_live())
        .expect("replay");
    assert!(
        replay.messages[0].is_deleted(),
        "a message that arrives already tombstoned does not come back live"
    );
}

// --- bounds --------------------------------------------------------------

#[test]
fn an_oversized_body_is_refused_rather_than_truncated() {
    let store = Store::open_in_memory().expect("store");
    let (project, session) = with_session(&store, "stream");
    let clock = clock_over_the_real_recording();
    let collection = store
        .open_chat_collection(session, project, "twitch", "streamer", T0)
        .expect("open");
    let huge = RawChatMessage {
        wall_ms: T0 + 1_000,
        room: "streamer".into(),
        sender: "ann".into(),
        text: "x".repeat(MAX_LABEL_BYTES + 1),
        is_action: false,
        provider_id: Some("big".into()),
    };
    let outcome = store.append_chat(&collection, &[huge], &window(), &clock);
    assert!(
        matches!(outcome, Err(ObservationError::LabelTooLong { .. })),
        "got {outcome:?}"
    );
    assert_eq!(store.chat_messages(session).expect("read").len(), 0);
}

#[test]
fn the_coverage_a_replay_carries_is_the_same_figure_the_standalone_read_gives() {
    // One denominator, one figure: a replay that quietly reported full
    // coverage while the standalone read did not would be a second answer.
    let store = Store::open_in_memory().expect("store");
    let (project, session) = with_session(&store, "stream");
    let clock = clock_over_the_real_recording();
    let collection = store
        .open_chat_collection(session, project, "twitch", "streamer", T0)
        .expect("open");
    store
        .append_chat(
            &collection,
            &[msg(1_000, "m1", "ann", "hi")],
            &window(),
            &clock,
        )
        .expect("append");
    store
        .close_chat_collection(&collection, T0 + 2_000, CollectionEnd::Disconnected)
        .expect("close early");

    let replay = store
        .replay_chat(session, &window(), &go_live())
        .expect("replay");
    let standalone: Coverage = store.chat_coverage(session, &window()).expect("coverage");
    assert_eq!(replay.coverage.per_mille, standalone.per_mille);
    assert_eq!(replay.coverage.gaps.len(), standalone.gaps.len());
    assert!(replay.coverage.per_mille < 1_000);
    replay
        .coverage
        .validated()
        .expect("and it names its shortfall");
}
