//! OBS-style multitrack import: track identification and sample alignment.
//!
//! Two claims, asserted separately because they fail separately.
//!
//! **Identification** is not "N streams arrived". A track is identified when it
//! carries a *named* role and a *named* timebase — a rate a caller can read off
//! the value and repeat — and when the anti-vacuity control below proves N > 1
//! with more than one distinct timebase. A single-track session satisfies "N
//! tracks identified" for free, which is the eleventh-vacuous-pass shape.
//!
//! **Alignment** is a [`ClockMap`], so it carries an uncertainty bound, and this
//! file asserts the bound rather than asserting a feeling of correctness. The
//! claim is deliberately narrow: for streams sharing one container timeline the
//! mapping is **exact in container-timeline terms** and reports zero drift, and
//! the thing it cannot promise — a decoded-sample-accurate position, because
//! `-c copy` seeking into Matroska is keyframe-approximate (todo 23) — is
//! reported as a *measured* keyframe distance instead. A 300 ms offset and a
//! presentation-time jump are both stated as what they are, and neither is
//! smoothed into a constant that hides it.

use std::fs;
use std::path::{Path, PathBuf};

use strivo_editor::time::{MediaTime, TimeBase, TimeError};
use strivo_multitrack::TrackKind;
use tempfile::TempDir;

use super::super::ids::*;
use super::super::import::AdmittedFile;
use super::super::multitrack::import_multitrack;
use super::super::multitrack_align::MeasuredGap;
use super::super::multitrack_model::{
    MultitrackRequest, MultitrackSession, TrackMeasurement, MAX_TRACKS,
};
use super::super::multitrack_session::{read_multitrack_session, restore_multitrack_session};
use super::super::store::Store;
use super::fixtures::project;

/// One fixed instant for every import here, so a row this file reads back is
/// ordered by something other than the wall clock.
const AT: &str = "2026-09-26T00:00:00Z";

/// A microsecond count the fixture states the way a person reads it.
fn us(micros: i64) -> MediaTime {
    MediaTime::from_micros(micros).expect("a non-negative position")
}

/// A whole-second sample rate, the timebase of an audio track: one tick is one
/// sample.
fn audio_base(hz: i64) -> TimeBase {
    TimeBase::from_rate(hz, 1).expect("a positive tick rate")
}

/// A track whose whole span is one continuous run — the ordinary case, and the
/// one every alignment assertion would pass against if gaps were ignored.
fn track(label: &str, stream: u32, hz: i64, start_ms: i64, len_ms: i64) -> TrackMeasurement {
    TrackMeasurement {
        stream_index: stream,
        label: label.into(),
        time_base: audio_base(hz),
        start: us(start_ms * 1_000),
        coverage: us(len_ms * 1_000),
        gaps: Vec::new(),
        complete: true,
    }
}

fn with_gaps(mut t: TrackMeasurement, gaps: &[(i64, i64, i64)]) -> TrackMeasurement {
    t.gaps = gaps
        .iter()
        .map(|&(at_ms, len_ms, key_ms)| MeasuredGap {
            at: us(at_ms * 1_000),
            len: us(len_ms * 1_000),
            keyframe_distance: us(key_ms * 1_000),
        })
        .collect();
    t
}

fn corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/media")
        .join(name)
}

fn stage(dir: &Path, name: &str, fixture: &str) -> PathBuf {
    let path = dir.join(name);
    fs::create_dir_all(path.parent().expect("a temp dir")).expect("parent made");
    fs::copy(corpus(fixture), &path).expect("fixture copied");
    path
}

fn stage_project() -> (Store, ProjectId, TempDir) {
    let dir = TempDir::new().expect("tempdir");
    let store = Store::open_in_memory().expect("store opens");
    let p = project("multitrack import");
    store.insert_project(&p).expect("project inserted");
    (store, p.id, dir)
}

fn container(dir: &Path) -> AdmittedFile {
    AdmittedFile::admitted(stage(dir, "session.mkv", "multi-audio.mkv"))
}

// ── the control: a real four-track session identifies ───────────────────────

/// Given a four-track session and its picture
/// When it is imported
/// Then every track carries a named role and a named timebase, the anti-vacuity
/// conditions hold (more than one track, more than one distinct timebase), and
/// each track is its own registered source. This is the control: every refusal
/// below would also hold against an import that identified nothing.
#[test]
fn a_four_track_session_identifies_every_track_with_a_named_role_and_timebase() {
    let (store, p, dir) = stage_project();
    let file = container(dir.path());
    // Four audio stems on one timeline, exactly what OBS's multitrack recording
    // mode writes: a mixed fallback plus mic, game and voice chat.
    let tracks = vec![
        track("Mixed", 1, 48_000, 0, 3_000),
        track("Mic", 2, 48_000, 0, 3_000),
        track("Game", 3, 44_100, 0, 3_000),
        track("Discord", 4, 16_000, 0, 3_000),
    ];

    let session = import_multitrack(
        &store,
        &MultitrackRequest {
            project: p,
            container: &file,
            picture: None,
            tracks: &tracks,
            labels: None,
            scratch_root: dir.path(),
            at: AT,
        },
    )
    .expect("a four-track session imports");

    assert_eq!(session.tracks.len(), 4, "four audio stems in, four out");
    assert!(
        session.tracks.len() > 1,
        "anti-vacuity: a one-track session satisfies 'N tracks identified' for free"
    );
    let bases: std::collections::BTreeSet<_> =
        session.tracks.iter().map(|t| t.time_base.rate()).collect();
    assert!(
        bases.len() > 1,
        "anti-vacuity: identification that collapsed to one timebase would be free, \
         so more than one distinct rate must be present: {bases:?}"
    );

    let roles: Vec<_> = session.tracks.iter().map(|t| t.role).collect();
    assert_eq!(
        roles,
        vec![
            TrackKind::Mixed,
            TrackKind::Mic,
            TrackKind::Game,
            TrackKind::Discord
        ],
        "each track's role comes from its own name, so a swapped label would \
         show up here rather than as 'they all look identified'"
    );
    let named: Vec<_> = session
        .tracks
        .iter()
        .map(|t| (t.label.clone(), t.time_base.rate()))
        .collect();
    assert_eq!(
        named,
        vec![
            ("Mixed".to_string(), (48_000, 1)),
            ("Mic".to_string(), (48_000, 1)),
            ("Game".to_string(), (44_100, 1)),
            ("Discord".to_string(), (16_000, 1)),
        ],
        "and each carries the rate a caller can name: ticks per second, exactly"
    );
    // The timebase is a real converter, not a label: a frame index at this rate
    // lands on the microsecond it must.
    let game = &session.tracks[2];
    assert_eq!(
        game.time_base.frame_to_media(44_100).expect("one second"),
        us(1_000_000),
        "a 44 100 Hz timebase puts the 44 100th sample at exactly one second"
    );
    assert_eq!(
        session.tracks[2]
            .time_base
            .frame_to_media(1)
            .expect("frame 1"),
        us(23),
        "and the residual is the documented rounding, not a second scale"
    );

    // One container is one source. The stems are not four recordings, so making
    // four rows would either mean writing rows outside the import path or
    // inventing a second content identity for bytes that are the same file — and
    // a track is addressed by `(source, stream_index)`, which is what a
    // container stream already is.
    let stored = store.sources_of(p).expect("sources listed");
    assert_eq!(stored.len(), 1, "four streams of one file are one source");
    assert!(
        session.tracks.iter().all(|t| t.source == stored[0].id),
        "and every track names that one source"
    );
    let indexes: Vec<u32> = session.tracks.iter().map(|t| t.stream_index).collect();
    assert_eq!(
        indexes,
        vec![1, 2, 3, 4],
        "which is what tells the four apart"
    );
    assert_eq!(
        stored[0].track_type, "multitrack",
        "the row says what kind of recording this is. A per-track role would \
         be the fourth track overwriting the first three: one column, four roles"
    );
    assert_eq!(
        stored[0].duration,
        Some(us(3_000_000)),
        "and how long the session runs, measured"
    );
    // The per-track facts live where there is room for four of them.
    let stored_tracks = read_multitrack_session(&store, session.capture)
        .expect("read ok")
        .expect("the session row is there");
    assert_eq!(stored_tracks.tracks.len(), 4, "all four, none overwritten");
    let stored_roles: Vec<_> = stored_tracks
        .tracks
        .iter()
        .map(|t| (t.stream_index, t.role.as_str(), t.time_base))
        .collect();
    assert_eq!(
        stored_roles,
        vec![
            (1, "mixed", [48_000, 1]),
            (2, "mic", [48_000, 1]),
            (3, "game", [44_100, 1]),
            (4, "discord", [16_000, 1])
        ],
        "role and exact timebase per stream, as the measurements not as a \
         derived alignment"
    );
    assert!(
        session
            .tracks
            .iter()
            .all(|t| stored.iter().any(|s| s.id == t.source)),
        "and every aligned track names the source it resolved to"
    );
}

/// The mixed fallback is preserved, and the session says which track it used as
/// its reference rather than leaving the caller to guess.
#[test]
fn the_mixed_fallback_track_is_preserved_and_named_as_the_reference() {
    let (store, p, dir) = stage_project();
    let file = container(dir.path());
    let tracks = vec![
        track("Mic", 1, 48_000, 0, 3_000),
        track("Mixed", 2, 48_000, 0, 3_000),
    ];

    let session = import_multitrack(
        &store,
        &MultitrackRequest {
            project: p,
            container: &file,
            picture: None,
            tracks: &tracks,
            labels: None,
            scratch_root: dir.path(),
            at: AT,
        },
    )
    .expect("import succeeds");

    assert_eq!(
        session.tracks.len(),
        2,
        "the mixed track is a track like any other, not a replacement for the set"
    );
    assert_eq!(
        session.reference.stream_index, 2,
        "the mixed track is the session's reference, so a listener who trusts \
         nothing else still has one synchronised timeline"
    );
    assert!(session.reference.mixed, "and says it is the fallback");
}

// ── the alignment claim, stated as a number ─────────────────────────────────

/// Given a session whose every track covers the whole timeline
/// When each track is aligned
/// Then the mapping is exact and reports **zero** drift, and a position on the
/// session timeline lands on the track's own position to the microsecond. This
/// is the claim this todo is willing to make: streams in one container share
/// one clock, so the bound is a measured zero rather than an assurance.
#[test]
fn a_complete_session_aligns_exactly_and_reports_zero_drift() {
    let (store, p, dir) = stage_project();
    let file = container(dir.path());
    let tracks = vec![
        track("Mixed", 1, 48_000, 0, 3_000),
        track("Mic", 2, 48_000, 0, 3_000),
    ];

    let session = import_multitrack(
        &store,
        &MultitrackRequest {
            project: p,
            container: &file,
            picture: None,
            tracks: &tracks,
            labels: None,
            scratch_root: dir.path(),
            at: AT,
        },
    )
    .expect("import succeeds");

    for aligned in &session.tracks {
        assert_eq!(
            aligned.offset,
            MediaTime::ZERO,
            "{} starts with the session, so there is no shift to hide",
            aligned.label
        );
        for wall_ms in [0, 250, 1_000, 2_999] {
            let mapped = aligned
                .aligned
                .media_at(wall_ms)
                .unwrap_or_else(|e| panic!("{}: {wall_ms} ms is covered: {e}", aligned.label));
            assert_eq!(
                mapped.at.micros(),
                wall_ms * 1_000,
                "{}: {wall_ms} ms on the session is the same microsecond on \
                 the track — they are one clock",
                aligned.label
            );
            assert_eq!(
                mapped.uncertainty_micros, 0,
                "{}: the drift bound is a measured zero, not an unstated one",
                aligned.label
            );
        }
    }
    assert_eq!(
        session.worst_drift_micros(),
        0,
        "and the session's own figure agrees"
    );
    assert!(
        session.tracks.iter().all(|t| t.complete),
        "the probe read every track to its end, so the zero above rests on a \
         whole measurement and not on a truncated scan"
    );
}

// ── a 300 ms offset is reported, not smoothed ───────────────────────────────

/// Given a session where one track genuinely starts 300 ms late
/// When it is aligned
/// Then the offset is stated as an offset, the head it leaves is a gap the
/// aligned map refuses rather than clamping to the nearest frame, and the drift
/// bound is still zero — because a constant offset is not drift, and calling it
/// drift would be the dishonest version of the same silence.
#[test]
fn a_three_hundred_millisecond_offset_is_reported_as_an_offset_and_a_head_gap() {
    let (store, p, dir) = stage_project();
    let file = container(dir.path());
    let tracks = vec![
        track("Mic", 1, 48_000, 0, 3_000),
        track("Game", 2, 48_000, 300, 2_700),
    ];

    let session = import_multitrack(
        &store,
        &MultitrackRequest {
            project: p,
            container: &file,
            picture: None,
            tracks: &tracks,
            labels: None,
            scratch_root: dir.path(),
            at: AT,
        },
    )
    .expect("import succeeds");

    let late = &session.tracks[1];
    assert_eq!(
        late.offset,
        us(300_000),
        "the 300 ms is stated, not absorbed"
    );
    assert_eq!(late.offset, tracks[1].start, "and it is the measured start");
    assert_eq!(
        late.gaps.len(),
        1,
        "the head it leaves is a gap in the session, reported as one"
    );
    assert_eq!((late.gaps[0].at, late.gaps[0].len), (us(0), us(300_000)));
    assert_eq!(late.gaps[0].drift_after_micros, 0, "nothing after it slid");
    assert_eq!(
        session.worst_drift_micros(),
        0,
        "a constant offset is not drift"
    );

    assert_eq!(
        late.aligned
            .media_at(150)
            .expect_err("the head is not covered"),
        TimeError::OutsideClockMap { wall_ms: 150 },
        "and the map refuses it rather than clamping to the nearest frame"
    );
    let mapped = late.aligned.media_at(1_000).expect("1 s is covered");
    assert_eq!(
        mapped.at,
        us(700_000),
        "1 s on the session is 700 ms on the track"
    );
    assert_eq!(mapped.uncertainty_micros, 0);
}

// ── a presentation-time jump is a gap with a measured drift bound ───────────

/// Given a track whose presentation times jump — the shape
/// `fixtures/media/discontinuities.mkv` has, a 1.04 s pts jump with a keyframe
/// on the far side
/// When it is aligned
/// Then the hole is a gap carrying the content lost after it, the aligned map
/// refuses every position inside it, and the two segments on either side are
/// separate — so no single constant offset can describe the track and quietly
/// drop a second of picture.
#[test]
fn a_presentation_time_jump_is_a_gap_with_a_drift_bound_not_a_constant_offset() {
    let (store, p, dir) = stage_project();
    let file = container(dir.path());
    // Measured from discontinuities.mkv: frames at 0..560 ms, then 1600 ms, at
    // 25 fps — so the 600 ms frame is missing and 1600 ms sits exactly on a
    // keyframe.
    let jumped = with_gaps(track("Game", 1, 25, 0, 2_160), &[(600, 1_000, 0)]);
    let tracks = vec![jumped, track("Mic", 2, 48_000, 0, 2_197)];

    let session = import_multitrack(
        &store,
        &MultitrackRequest {
            project: p,
            container: &file,
            picture: None,
            tracks: &tracks,
            labels: None,
            scratch_root: dir.path(),
            at: AT,
        },
    )
    .expect("import succeeds");

    let game = &session.tracks[0];
    assert_eq!(
        game.gaps.len(),
        1,
        "the jump is one measured hole, not a stream of jitter"
    );
    let gap = game.gaps[0];
    assert_eq!((gap.at, gap.len), (us(600_000), us(1_000_000)));
    assert_eq!(
        gap.drift_after_micros, 1_000_000,
        "the bound a caller must carry for anything after the hole is the \
         content it lost — a second, measured, not a hopeful figure"
    );
    assert_eq!(
        gap.keyframe_distance,
        MediaTime::ZERO,
        "and the fixture's far side is a keyframe, so a `-c copy` seek can land \
         on this position; the value is measured, not assumed"
    );
    assert_eq!(
        game.aligned.segments().len(),
        2,
        "two runs, so nothing reduced the track to one constant offset"
    );
    for inside in [601, 1_000, 1_599] {
        assert_eq!(
            game.aligned.media_at(inside).expect_err("inside the hole"),
            TimeError::OutsideClockMap { wall_ms: inside },
            "{inside} ms is inside the gap, and the nearest edge is not an answer"
        );
    }
    // One millisecond at the leading edge is answered, and the reason is
    // `ClockMap`'s own convention rather than anything decided here:
    // `covered_wall_ms` reports a segment's wall range from `wall_of(end)`, and
    // `media_at` tests it inclusively, so a run's last millisecond and the hole's
    // first overlap by exactly one. Quantified here rather than hidden, because a
    // caller doing millisecond-accurate work off this map needs to know it is
    // there — and it is why the accuracy claim above says "container-timeline
    // terms" and not "sample terms".
    assert_eq!(
        game.aligned
            .media_at(600)
            .expect("the inclusive end of the run before")
            .at,
        us(600_000),
        "600 ms is the boundary millisecond, owned by the run that ends there"
    );
    let before = game.aligned.media_at(560).expect("before the hole");
    assert_eq!(before.at, us(560_000));
    let after = game.aligned.media_at(1_600).expect("after the hole");
    assert_eq!(
        after.at,
        us(600_000),
        "the track's own timeline is contiguous across its own hole; the hole \
         is in the session, which is what the session axis is for"
    );
    assert_eq!(session.worst_drift_micros(), 1_000_000);
}

// ── identification from a label, and from the user ──────────────────────────

/// Given a track OBS left unnamed
/// When it is imported
/// Then it is reported as `Other` — a stated absence, not a guess — and the
/// session still aligns it, so an unknown role costs nothing but a label.
#[test]
fn an_unnamed_track_reports_other_and_still_aligns() {
    let (store, p, dir) = stage_project();
    let file = container(dir.path());
    let tracks = vec![
        track("", 1, 48_000, 0, 3_000),
        track("Track 4", 2, 48_000, 0, 3_000),
    ];

    let session = import_multitrack(
        &store,
        &MultitrackRequest {
            project: p,
            container: &file,
            picture: None,
            tracks: &tracks,
            labels: None,
            scratch_root: dir.path(),
            at: AT,
        },
    )
    .expect("import succeeds");

    assert_eq!(session.tracks[0].role, TrackKind::Other);
    assert!(
        !session.tracks[0].identified,
        "and it says the role is undetermined rather than presenting Other as \
         an identification"
    );
    assert!(
        !session.tracks[1].identified,
        "a name we do not recognise is the same absence"
    );
    assert_eq!(
        session.tracks[1].role,
        TrackKind::Other,
        "so 'Misc Track 4' does not become a microphone"
    );
    assert_eq!(
        session.tracks[0]
            .aligned
            .media_at(0)
            .expect("still aligned")
            .at,
        us(0)
    );
}

/// Given a track whose role the caller relabels
/// When it is imported
/// Then the user's word wins over the inferred one, and the original name is
/// kept beside it so a later read can see what was overridden.
#[test]
fn a_user_label_overrides_the_inferred_role_and_keeps_both_words() {
    let (store, p, dir) = stage_project();
    let file = container(dir.path());
    let tracks = vec![track("Mic", 1, 48_000, 0, 3_000)];
    let labels = vec!["Discord".to_string()];

    let session = import_multitrack(
        &store,
        &MultitrackRequest {
            project: p,
            container: &file,
            picture: None,
            tracks: &tracks,
            labels: Some(&labels),
            scratch_root: dir.path(),
            at: AT,
        },
    )
    .expect("import succeeds");

    let t = &session.tracks[0];
    assert_eq!(t.role, TrackKind::Discord, "the user's word is the role");
    assert_eq!(t.label, "Discord", "and the label it was given");
    assert_eq!(
        t.measured_label, "Mic",
        "the name the container carried is kept, so the override is visible"
    );
    assert!(t.identified, "a relabelled track is identified");
}

// ── malformed sessions: typed states, never a panic ─────────────────────────

/// Given a session with no audio track at all
/// When it is imported
/// Then it is refused by name with nothing registered and no job left behind —
/// a silent-video file is a legal recording and an illegal multitrack session.
#[test]
fn a_session_with_no_audio_track_is_refused_by_name() {
    let (store, p, dir) = stage_project();
    let file = container(dir.path());
    let ghost = MultitrackRequest {
        project: p,
        container: &file,
        picture: None,
        tracks: &[],
        labels: None,
        scratch_root: dir.path(),
        at: AT,
    };

    let refusal = import_multitrack(&store, &ghost).expect_err("no tracks refuses");

    assert_eq!(refusal.code(), "multitrack/no-tracks");
    assert_eq!(store.sources_of(p).expect("listed").len(), 0);
    assert!(store.jobs(p).expect("jobs listed").is_empty());
}

/// Given a session carrying more streams than the bound allows
/// When it is imported
/// Then it is refused rather than probed — an unbounded track count is a way to
/// make a server enumerate a client's file, so the bound is checked before the
/// work, not after it.
#[test]
fn a_track_count_beyond_the_bound_is_refused_before_any_work() {
    let (store, p, dir) = stage_project();
    let file = container(dir.path());
    let many: Vec<TrackMeasurement> = (0..(MAX_TRACKS as u32 + 40))
        .map(|i| track(&format!("Mic {i}"), i, 48_000, 0, 3_000))
        .collect();
    assert_eq!(many.len(), MAX_TRACKS + 40, "past the bound by forty");

    let refusal = import_multitrack(
        &store,
        &MultitrackRequest {
            project: p,
            container: &file,
            picture: None,
            tracks: &many,
            labels: None,
            scratch_root: dir.path(),
            at: AT,
        },
    )
    .expect_err("past the bound refuses");

    assert_eq!(refusal.code(), "multitrack/too-many-tracks");
    assert_eq!(store.sources_of(p).expect("listed").len(), 0);
}

/// Given a track with a stream index the container does not have, an inverted
/// span, or a duplicated index
/// When the session is imported
/// Then each is its own typed refusal. A panic in a loader that walks a
/// client-supplied file is a crash, not a diagnostic.
#[test]
fn malformed_track_measurements_are_each_a_typed_refusal() {
    let (store, p, dir) = stage_project();
    let file = container(dir.path());
    let ok = track("Mic", 1, 48_000, 0, 3_000);
    // Each case names its own labels, because a label-list mismatch is itself
    // one of the refusals and would otherwise mask the case under test.
    let matching = |n: usize| -> Vec<String> { vec!["Discord".to_string(); n] };
    /// One malformed case: what it is, its measurements, its labels, the code.
    type Case = (
        &'static str,
        Vec<TrackMeasurement>,
        Option<Vec<String>>,
        &'static str,
    );
    let cases: Vec<Case> = vec![
        (
            "a track with no content at all",
            vec![TrackMeasurement {
                coverage: MediaTime::ZERO,
                ..ok.clone()
            }],
            Some(matching(1)),
            "multitrack/empty-track",
        ),
        (
            "a hole at or past the end of the track that claims it",
            vec![with_gaps(ok.clone(), &[(3_000, 500, 0)])],
            Some(matching(1)),
            "multitrack/gap-outside-track",
        ),
        (
            "two tracks claiming the same stream index",
            vec![ok.clone(), track("Mic copy", 1, 48_000, 0, 3_000)],
            Some(matching(2)),
            "multitrack/duplicate-stream",
        ),
        (
            "more labels than tracks, so a label names nothing",
            vec![ok.clone(), track("Game", 2, 48_000, 0, 3_000)],
            Some(matching(3)),
            "multitrack/label-count",
        ),
    ];

    for (what, tracks, labels, code) in cases {
        let refusal = import_multitrack(
            &store,
            &MultitrackRequest {
                project: p,
                container: &file,
                picture: None,
                tracks: &tracks,
                labels: labels.as_deref(),
                scratch_root: dir.path(),
                at: AT,
            },
        )
        .expect_err(what);
        assert_eq!(refusal.code(), code, "for {what}");
        assert_eq!(
            store.sources_of(p).expect("listed").len(),
            0,
            "{what} registered nothing"
        );
    }
}

/// Given a session where the user supplied exactly one label per track
/// When it is imported
/// Then it succeeds — the label count is checked, not merely the label contents,
/// so a matching list is not refused for some other reason.
#[test]
fn one_label_per_track_is_accepted() {
    let (store, p, dir) = stage_project();
    let file = container(dir.path());
    let tracks = vec![
        track("Mic", 1, 48_000, 0, 3_000),
        track("Mic", 2, 48_000, 0, 3_000),
    ];
    let labels = vec!["Mic".to_string(), "Discord".to_string()];

    let session = import_multitrack(
        &store,
        &MultitrackRequest {
            project: p,
            container: &file,
            picture: None,
            tracks: &tracks,
            labels: Some(&labels),
            scratch_root: dir.path(),
            at: AT,
        },
    )
    .expect("a matching label list is accepted");

    assert_eq!(session.tracks[0].role, TrackKind::Mic);
    assert_eq!(session.tracks[1].role, TrackKind::Discord);
}

// ── stale state: re-import, identity, untrusted text ───────────────────────

/// Given a multitrack session that has already been imported
/// When it is imported again from the same bytes
/// Then it is idempotent: the same identities, no second set of sources, and the
/// same operation — which is what makes a re-queued import safe to re-run.
#[test]
fn reimporting_the_same_session_is_idempotent() {
    let (store, p, dir) = stage_project();
    let file = container(dir.path());
    let tracks = vec![
        track("Mic", 1, 48_000, 0, 3_000),
        track("Game", 2, 48_000, 0, 3_000),
    ];
    let ask = || MultitrackRequest {
        project: p,
        container: &file,
        picture: None,
        tracks: &tracks,
        labels: None,
        scratch_root: dir.path(),
        at: AT,
    };

    let first = import_multitrack(&store, &ask()).expect("first import");
    let second = import_multitrack(&store, &ask()).expect("second import");

    let ids = |s: &MultitrackSession| -> Vec<SourceAssetId> {
        s.tracks.iter().map(|t| t.source).collect()
    };
    assert_eq!(
        ids(&first),
        ids(&second),
        "the same identities, in the same order"
    );
    assert_eq!(
        store.sources_of(p).expect("listed").len(),
        1,
        "and no second source: the same bytes are the same asset"
    );
    assert_eq!(first.manifest.cache_key, second.manifest.cache_key);
    assert_eq!(
        first.manifest.imported(),
        1,
        "one container, one new source"
    );
    assert_eq!(
        second.manifest.imported(),
        0,
        "the second run created nothing; the content was already there"
    );
    assert_eq!(second.manifest.duplicates(), 1, "and it says so");

    // The measurements are stored, and re-deriving them gives back the same
    // placement — which is the only reason there is no stored alignment to
    // disagree with this one.
    let stored = read_multitrack_session(&store, second.capture)
        .expect("read ok")
        .expect("the session row is there");
    let restored = restore_multitrack_session(&stored).expect("restore ok");
    assert_eq!(restored.placements.len(), second.tracks.len());
    for (restored, fresh) in restored.placements.iter().zip(&second.tracks) {
        assert_eq!(
            restored.offset, fresh.offset,
            "{}: the same offset",
            fresh.label
        );
        assert_eq!(
            restored.aligned.aligned, fresh.aligned,
            "{}: and the same clock, re-derived rather than read back",
            fresh.label
        );
        assert_eq!(restored.role, fresh.role);
    }
    assert_eq!(
        stored.worst_drift_micros,
        second.worst_drift_micros(),
        "and the stored bound is the one the import reported"
    );
}

/// Given a track name carrying markup and shell metacharacters
/// When the session is imported
/// Then the name is carried verbatim as data, the role is undetermined rather
/// than guessed from a substring of it, and nothing interpreted the string.
#[test]
fn a_track_name_is_carried_as_data_and_never_interpreted() {
    let (store, p, dir) = stage_project();
    let file = container(dir.path());
    // No role keyword anywhere in it. `onerror=alert(1)` classifies as
    // `Browser` through the existing table's `"alert"` rule, which is correct
    // behaviour and would make this test measure a substring coincidence
    // instead of the claim it is about: the name is data and nothing ran it.
    let hostile = "<img src=x onerror=confirm(1)>; $(id) `whoami` | tee";
    let tracks = vec![
        track(hostile, 1, 48_000, 0, 3_000),
        track("Mic", 2, 48_000, 0, 3_000),
    ];

    let session = import_multitrack(
        &store,
        &MultitrackRequest {
            project: p,
            container: &file,
            picture: None,
            tracks: &tracks,
            labels: None,
            scratch_root: dir.path(),
            at: AT,
        },
    )
    .expect("import succeeds");

    assert_eq!(session.tracks[0].label, hostile, "verbatim");
    assert_eq!(session.tracks[0].role, TrackKind::Other);
    assert_eq!(
        session.tracks[1].role,
        TrackKind::Mic,
        "and the real one still works"
    );
    let stored = read_multitrack_session(&store, session.capture)
        .expect("read ok")
        .expect("the session row is there");
    assert_eq!(
        stored.tracks[0].label, hostile,
        "stored as data; escaping is a UI's job"
    );
    assert_eq!(
        stored.tracks[0].role, "other",
        "and the role is the ordinary word, not the name"
    );
    assert!(
        !stored.tracks[0].identified,
        "so nothing downstream can mistake it for a determined role"
    );
}

/// Given a session whose every track is a duplicate of one already imported
/// When it is imported
/// Then the tracks resolve to the existing sources, so a re-recorded stem is
/// detectable as a change by its content rather than silently merged.
#[test]
fn a_rerecorded_stem_is_a_different_identity_not_a_silent_merge() {
    let (store, p, dir) = stage_project();
    let file = container(dir.path());
    let tracks = vec![track("Mic", 1, 48_000, 0, 3_000)];
    let first = import_multitrack(
        &store,
        &MultitrackRequest {
            project: p,
            container: &file,
            picture: None,
            tracks: &tracks,
            labels: None,
            scratch_root: dir.path(),
            at: AT,
        },
    )
    .expect("first import");

    // The operator re-recorded and the container's bytes changed.
    fs::copy(corpus("silent-video.mkv"), file.locator()).expect("the file is rewritten");
    let second = import_multitrack(
        &store,
        &MultitrackRequest {
            project: p,
            container: &file,
            picture: None,
            tracks: &tracks,
            labels: None,
            scratch_root: dir.path(),
            at: AT,
        },
    )
    .expect("second import");

    assert_ne!(
        first.tracks[0].source, second.tracks[0].source,
        "different bytes are a different source, so a re-record is visible"
    );
    assert_eq!(second.manifest.imported(), 1);
}
