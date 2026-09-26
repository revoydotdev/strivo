//! The multitrack import surface, driven against the same hostile inputs as the
//! media gate — and against real four-track and discontinuous containers.
//!
//! A multitrack import is a **third** way a client-named path can reach ffmpeg,
//! after an edit's cut and a local import. If it admitted anything todo 25's gate
//! refuses, the library could hold a source the next ffmpeg-bound request would
//! reject, and the two surfaces would have drifted. This file is the check that
//! they have not.
//!
//! It asserts **codes, not prose**: each case drives the real [`admit_all`] — the
//! production function the route calls — and asserts the typed
//! [`PathRejection`] class is the one todo 25 defined. Because `admit_all`
//! returns the gate's own error type the codes cannot differ by construction;
//! the value of the test is that it fails the moment someone routes around the
//! gate.
//!
//! **The order matters.** The first test is a real four-track OBS-shaped session
//! importing successfully, because every refusal assertion below would also hold
//! against a surface that refuses everything. A gate that rejects all input
//! satisfies "no traversal was imported" for free — the shape this plan has been
//! bitten by eleven times.

#![cfg(feature = "creator")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use strivo_editor::time::{MediaTime, TimeBase};
use strivo_research::creator::{
    import_multitrack, restore_multitrack_session, MediaGap, MultitrackRequest, ProjectId, Store,
    TrackMeasurement,
};
use strivo_web::routes::import::admit_all;
use strivo_web::routes::media_input::{admit_path, PathRejection};
use strivo_web::routes::multitrack_probe::probe_session;

const AT: &str = "2026-09-26T00:00:00Z";

fn ffmpeg_available() -> bool {
    Command::new("ffprobe")
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/media")
        .join(name)
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("strivo-t34-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch made");
    dir
}

/// Build an OBS-shaped multitrack container: one picture and four audio stems
/// named the way OBS names them, one of them the downmixed fallback. The Game
/// stem is 44 100 Hz where the rest are 48 000, which is ordinary — a track's
/// rate is whatever the source device negotiated — and it is what makes the
/// distinct-timebase anti-vacuity assertion below a real one rather than a
/// formality: four stems at one rate would satisfy "identified" for free.
///
/// Generated here rather than added to `fixtures/media/` because that tree is
/// todo 32's and ffprobe-proven as a whole; a new class belongs there with a
/// manifest entry, and this file needs a specific shape (four stems, mixed
/// first) rather than a new class of fixture.
fn four_track_session(path: &Path) {
    let sine = |hz: u32| format!("sine=frequency={hz}:sample_rate=48000:duration=3");
    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=128x72:rate=10:duration=3",
        ])
        .args(["-f", "lavfi", "-i", &sine(440)])
        .args(["-f", "lavfi", "-i", &sine(880)])
        .args(["-f", "lavfi", "-i", &sine(1200).replace("48000", "44100")])
        .args(["-f", "lavfi", "-i", &sine(200)])
        .args([
            "-map", "0:v", "-map", "1:a", "-map", "2:a", "-map", "3:a", "-map", "4:a",
        ])
        .args([
            "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "64k",
        ])
        .args(["-metadata:s:a:0", "title=Mixed"])
        .args(["-metadata:s:a:1", "title=Mic"])
        .args(["-metadata:s:a:2", "title=Game"])
        .args(["-metadata:s:a:3", "title=Discord"])
        .arg(path)
        .status()
        .expect("ffmpeg runs");
    assert!(status.success(), "the four-track fixture built");
}

/// The plan's own QA case: one track genuinely 300 ms late, which ffmpeg
/// produces from a real `-itsoffset` rather than from a header this test edited.
fn offset_session(path: &Path) {
    let sine = |hz: u32| format!("sine=frequency={hz}:sample_rate=48000:duration=3");
    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=128x72:rate=10:duration=3",
            "-f",
            "lavfi",
            "-i",
            &sine(440),
        ])
        .args(["-itsoffset", "0.3", "-f", "lavfi", "-i", &sine(880)])
        .args([
            "-map", "0:v", "-map", "1:a", "-map", "2:a", "-c:v", "libx264", "-pix_fmt", "yuv420p",
            "-c:a", "aac", "-b:a", "64k",
        ])
        .args(["-metadata:s:a:0", "title=Mic"])
        .args(["-metadata:s:a:1", "title=Game"])
        .arg(path)
        .status()
        .expect("ffmpeg runs");
    assert!(status.success(), "the offset fixture built");
}

/// A project with a real row, so the import has somewhere to land.
fn project_store() -> (Store, ProjectId) {
    let store = Store::open_in_memory().expect("store opens");
    let p = strivo_research::creator::Project {
        id: ProjectId::generate(),
        name: "multitrack".into(),
        description: String::new(),
        channel: "twitch".into(),
        series: String::new(),
        settings_json: "{}".into(),
        created_at: AT.into(),
        updated_at: AT.into(),
    };
    store.insert_project(&p).expect("project inserted");
    (store, p.id)
}

// ── the control: a real four-track session imports ──────────────────────────

/// Given a real OBS-shaped four-track container
/// When it is imported through the real admission gate
/// Then every track is identified with a named role and a named timebase, more
/// than one track and more than one timebase are present, the alignment reports
/// a measured zero drift bound, and the measurements survive a read-back. This
/// is the control every refusal below depends on.
#[tokio::test]
async fn a_four_track_obs_session_imports_identifies_and_aligns() {
    if !ffmpeg_available() {
        eprintln!("skipping: ffprobe is not on PATH, so no container can be measured");
        return;
    }
    let dir = scratch("four");
    let path = dir.join("session.mkv");
    four_track_session(&path);
    let (store, project) = project_store();
    let admitted =
        admit_all(&dir, &[path.display().to_string()]).expect("a real session is admitted");

    let probe = probe_session(&admitted[0])
        .await
        .expect("the container is measurable");
    assert_eq!(probe.tracks.len(), 4, "four audio stems, measured");
    assert!(
        probe.picture.is_some(),
        "and a picture, which is the session timeline"
    );

    let session = import_multitrack(
        &store,
        &MultitrackRequest {
            project,
            container: &admitted[0],
            tracks: &probe.tracks,
            picture: probe.picture.as_ref(),
            labels: None,
            scratch_root: &dir,
            at: AT,
        },
    )
    .expect("a four-track session imports");

    assert_eq!(session.tracks.len(), 4);
    assert!(
        session.tracks.len() > 1,
        "anti-vacuity: one track would satisfy this for free"
    );
    let bases: std::collections::BTreeSet<_> =
        session.tracks.iter().map(|t| t.time_base.rate()).collect();
    assert_eq!(
        bases.len(),
        2,
        "anti-vacuity: three 48 kHz stems and one 44 100 Hz, so identification \
         cannot have collapsed to a single rate: {bases:?}"
    );
    let roles: Vec<_> = session
        .tracks
        .iter()
        .map(|t| (t.label.as_str(), t.role.as_str()))
        .collect();
    assert_eq!(
        roles,
        vec![
            ("Mixed", "mixed"),
            ("Mic", "mic"),
            ("Game", "game"),
            ("Discord", "discord")
        ],
        "every stem identified from the name OBS stamped on it"
    );
    assert!(
        session.tracks.iter().all(|t| t.identified),
        "and none of them is a guess"
    );
    assert!(
        session.reference.mixed,
        "the fallback is the session's reference"
    );
    assert_eq!(
        session.worst_drift_micros(),
        0,
        "a complete session in one container aligns exactly: a measured zero, not \
         an assurance"
    );

    // The measurement is real, not the container's stated length: a 3 s fixture
    // whose audio frames span a little over 3 s, read to the microsecond.
    assert!(
        session.tracks.iter().all(|t| t.complete),
        "and the probe read every stream to its end"
    );
    let mic = &session.attributes.tracks[1];
    assert_eq!(mic.label, "Mic");
    assert!(
        mic.coverage_us > 2_900_000,
        "three seconds of 48 kHz audio is about three million microseconds, \
         measured from the frame timestamps: {}",
        mic.coverage_us
    );

    // And the stored measurements re-derive the same placement.
    let stored = restore_multitrack_session(
        &strivo_research::creator::read_multitrack_session(&store, session.capture)
            .expect("read ok")
            .expect("the session row is there"),
    )
    .expect("restore ok");
    assert_eq!(stored.placements.len(), 4);
    for (restored, fresh) in stored.placements.iter().zip(&session.tracks) {
        assert_eq!(
            restored.aligned.aligned, fresh.aligned,
            "{}: same clock",
            fresh.label
        );
        assert_eq!(restored.role, fresh.role);
    }
}

/// The plan's failure case: a track that genuinely starts 300 ms late must be
/// reported as an offset with a head gap, not smoothed into an alignment that
/// hides it.
#[tokio::test]
async fn a_three_hundred_millisecond_offset_is_reported_rather_than_hidden() {
    if !ffmpeg_available() {
        return;
    }
    let dir = scratch("offset");
    let path = dir.join("offset.mkv");
    offset_session(&path);
    let (store, project) = project_store();
    let admitted = admit_all(&dir, &[path.display().to_string()]).expect("admitted");

    let probe = probe_session(&admitted[0]).await.expect("measurable");
    let late = probe
        .tracks
        .iter()
        .find(|t| t.start.micros() > 0)
        .expect("ffmpeg's -itsoffset really did start a stream late");
    assert_eq!(
        late.start.micros(),
        300_000,
        "the fixture is a real 300 ms offset"
    );

    let session = import_multitrack(
        &store,
        &MultitrackRequest {
            project,
            container: &admitted[0],
            tracks: &probe.tracks,
            picture: probe.picture.as_ref(),
            labels: None,
            scratch_root: &dir,
            at: AT,
        },
    )
    .expect("import succeeds");

    let game = session
        .tracks
        .iter()
        .find(|t| t.offset.micros() > 0)
        .expect("the late stream is a track");
    assert_eq!(
        game.offset.micros(),
        300_000,
        "the 300 ms is stated as an offset"
    );
    assert_eq!(
        game.gaps.len(),
        1,
        "and the head it leaves is a reported gap"
    );
    let head: &MediaGap = &game.gaps[0];
    assert_eq!((head.at.micros(), head.len.micros()), (0, 300_000));
    assert_eq!(
        head.drift_after_micros, 0,
        "a constant offset is not drift, and calling it drift would be the \
         dishonest version of the same silence"
    );
    assert_eq!(
        session.worst_drift_micros(),
        0,
        "so the session's figure is zero while the offset is still reported"
    );
    // The map refuses the head rather than clamping to the nearest frame.
    let refused = game
        .aligned
        .media_at(150)
        .expect_err("the head has no content");
    assert_eq!(refused.to_string(), "no clock segment covers 150 ms");
    let mapped = game.aligned.media_at(1_000).expect("1 s is covered");
    assert_eq!(
        mapped.at.micros(),
        700_000,
        "1 s on the session is 700 ms on the track"
    );
    assert_eq!(mapped.uncertainty_micros, 0);
}

// ── the discontinuity case ──────────────────────────────────────────────────

/// Given todo 32's discontinuity fixture — 30 frames with a 1.04 s
/// presentation-time jump at index 15 and a keyframe on the far side
/// When it is imported
/// Then the missing second is a gap with a measured drift bound, the session's
/// own timeline refuses every position inside it, and the two runs either side
/// are separate segments — so nothing reduced the picture to one constant offset
/// and dropped a second of it.
#[tokio::test]
async fn a_presentation_time_jump_is_a_gap_with_a_drift_bound() {
    if !ffmpeg_available() {
        return;
    }
    let dir = scratch("disc");
    let path = dir.join("discontinuities.mkv");
    fs::copy(corpus("discontinuities.mkv"), &path).expect("fixture copied");
    let (store, project) = project_store();
    let admitted = admit_all(&dir, &[path.display().to_string()]).expect("admitted");

    let probe = probe_session(&admitted[0]).await.expect("measurable");
    let picture = probe.picture.as_ref().expect("it has a picture");
    assert_eq!(
        picture.gaps.len(),
        1,
        "exactly one hole: the fixture's 1.04 s jump, and the 25 fps frame step \
         around it is far below the tolerance so ordinary jitter is not a hole"
    );
    let measured = picture.gaps[0];
    assert_eq!(
        measured.at.micros(),
        600_000,
        "the frame at 600 ms is the one that is missing"
    );
    assert_eq!(
        measured.len.micros(),
        1_000_000,
        "the step is 1040 ms and the nominal step is 40 ms, so exactly a second \
         of picture is gone"
    );
    assert_eq!(
        measured.keyframe_distance.micros(),
        0,
        "and the far side is a keyframe, measured from ffprobe's own key_frame \
         flag rather than assumed"
    );

    let session = import_multitrack(
        &store,
        &MultitrackRequest {
            project,
            container: &admitted[0],
            tracks: &probe.tracks,
            picture: probe.picture.as_ref(),
            labels: None,
            scratch_root: &dir,
            at: AT,
        },
    )
    .expect("import succeeds");

    let reported = &session.timeline.gaps[0];
    assert_eq!(
        (reported.at.micros(), reported.len.micros()),
        (600_000, 1_000_000)
    );
    assert_eq!(
        reported.drift_after_micros, 1_000_000,
        "the bound a caller must carry for anything after the hole is the content \
         it lost: a measured second"
    );
    assert_eq!(
        session.worst_drift_micros(),
        1_000_000,
        "and the session's own figure is that second, not zero"
    );
    for inside in [601, 1_000, 1_599] {
        assert!(
            session.timeline.clock.media_at(inside).is_err(),
            "{inside} ms is inside the hole, and the nearest edge is not an answer"
        );
    }
    assert!(
        session.timeline.clock.media_at(560).is_ok(),
        "before the hole is covered"
    );
    assert!(
        session.timeline.clock.media_at(1_600).is_ok(),
        "and after it"
    );
    assert_eq!(
        session.timeline.clock.segments().len(),
        2,
        "two runs, so no single constant offset could have described this picture"
    );

    // The audio track in this fixture is continuous, and says so: a per-track
    // answer, not a session-wide one. Its own timeline has content at 1.0 s
    // because the sine really is still running.
    let audio = &session.tracks[0];
    assert!(audio.gaps.is_empty(), "the sine never stopped");
    assert!(
        audio.aligned.media_at(1_000).is_ok(),
        "so 1 s does have audio on it"
    );
}

// ── boundary parity: the same codes as every other way in ───────────────────

/// The load-bearing parity claim, one class at a time. Each case asserts the
/// **same code the ffmpeg-bound path produces**, obtained by calling the gate
/// directly, so a divergence between the surfaces fails here rather than in
/// production.
#[test]
fn multitrack_import_refuses_exactly_what_the_media_gate_refuses() {
    let dir = scratch("parity");
    let root = dir.join("root");
    fs::create_dir_all(&root).expect("root made");
    fs::copy(corpus("landmarks.mkv"), root.join("ok.mkv")).expect("a real file inside the root");
    let outside = dir.join("outside.mkv");
    fs::copy(corpus("landmarks.mkv"), &outside).expect("outside fixture");
    let empty = root.join("empty.mkv");
    fs::write(&empty, b"").expect("empty file");
    let fifo = root.join("pipe.mkv");
    let have_fifo = Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    let protocol = |scheme: &str| PathRejection::ProtocolNotAllowed {
        scheme: scheme.into(),
    };
    let cases: Vec<(&str, String, PathRejection)> = vec![
        (
            "a traversal out of the root, the shape an OBS folder import takes",
            format!("{}/../outside.mkv", root.display()),
            PathRejection::OutsideRoot,
        ),
        (
            "a file:// URL",
            format!("file://{}", outside.display()),
            protocol("file"),
        ),
        (
            "a subfile: hop, a protocol to ffmpeg and not a valid scheme",
            format!("subfile,,start,0:{}", outside.display()),
            protocol("subfile,,start,0"),
        ),
        (
            "a concat: protocol",
            format!("concat:{}", root.join("ok.mkv").display()),
            protocol("concat"),
        ),
        (
            "a directory where a container was expected",
            root.display().to_string(),
            PathRejection::NotAFile { kind: "directory" },
        ),
        (
            "a zero-byte file",
            empty.display().to_string(),
            PathRejection::EmptyFile,
        ),
        (
            "a path that does not resolve",
            root.join("nope.mkv").display().to_string(),
            PathRejection::NotFound,
        ),
        ("an empty path", "   ".into(), PathRejection::EmptyPath),
    ];

    for (what, raw, expected) in cases {
        let gate = admit_path(Some(&root), &raw).expect_err("the gate refuses it too");
        assert_eq!(gate, expected, "the gate's own class for {what}");
        let import =
            admit_all(&root, std::slice::from_ref(&raw)).expect_err("multitrack import refuses it");
        assert_eq!(
            import,
            gate,
            "multitrack import refused {what} as {:?} but the gate refuses it as {:?}: \
             the two surfaces have drifted",
            import.code(),
            gate.code()
        );
        assert_eq!(
            import.code(),
            expected.code(),
            "and both name it with the same stable code"
        );
    }

    if have_fifo {
        let import = admit_all(&root, &[fifo.display().to_string()])
            .expect_err("a fifo is refused, not read");
        assert_eq!(
            import,
            PathRejection::NotAFile {
                kind: "special file"
            },
            "a fifo is refused by file type before anything opens it, so an import \
             cannot block on a writer that never comes"
        );
    } else {
        eprintln!("skipping the fifo case: mkfifo is not available");
    }

    // The positive case sharing the root with every refusal.
    let admitted = admit_all(&root, &[root.join("ok.mkv").display().to_string()])
        .expect("a real media file inside the root is admitted");
    assert_eq!(
        admitted.len(),
        1,
        "refusing the hostile inputs did not cost the real one"
    );
}

// ── malformed sessions: typed states, not crashes ───────────────────────────

/// Given a container with no audio stream at all, and a measurement set that
/// lies about itself
/// When each is imported
/// Then it is refused by its own typed code with nothing registered — a
/// `silent-video.mkv` is a legal recording and an illegal multitrack session.
#[tokio::test]
async fn a_container_that_is_not_a_multitrack_session_is_refused_by_name() {
    if !ffmpeg_available() {
        return;
    }
    let dir = scratch("silent");
    let path = dir.join("silent-video.mkv");
    fs::copy(corpus("silent-video.mkv"), &path).expect("fixture copied");
    let (store, project) = project_store();
    let admitted = admit_all(&dir, &[path.display().to_string()]).expect("admitted");

    let probe = probe_session(&admitted[0]).await.expect("measurable");
    assert!(
        probe.tracks.is_empty(),
        "the fixture really has no audio stream"
    );

    let refusal = import_multitrack(
        &store,
        &MultitrackRequest {
            project,
            container: &admitted[0],
            tracks: &probe.tracks,
            picture: probe.picture.as_ref(),
            labels: None,
            scratch_root: &dir,
            at: AT,
        },
    )
    .expect_err("no audio stream refuses");
    assert_eq!(refusal.code(), "multitrack/no-tracks");
    assert_eq!(
        store.sources_of(project).expect("listed").len(),
        0,
        "nothing registered"
    );
    assert!(
        store.jobs(project).expect("jobs listed").is_empty(),
        "and no job submitted"
    );
}

/// Given a label list that does not line up with the tracks
/// When the session is imported
/// Then it is refused by its own code, because a label that names nothing is
/// worse than no label.
#[test]
fn a_label_list_that_does_not_match_the_tracks_is_refused() {
    let dir = scratch("labels");
    let path = dir.join("ok.mkv");
    fs::copy(corpus("multi-audio.mkv"), &path).expect("fixture copied");
    let (store, project) = project_store();
    let admitted = admit_all(&dir, &[path.display().to_string()]).expect("admitted");
    let probe = vec![measured(1, "Mic"), measured(2, "Game")];

    let labels = vec!["Mic".to_string(); probe.len() + 1];
    let refusal = import_multitrack(
        &store,
        &MultitrackRequest {
            project,
            container: &admitted[0],
            tracks: &probe,
            picture: None,
            labels: Some(&labels),
            scratch_root: &dir,
            at: AT,
        },
    )
    .expect_err("a mismatched label list refuses");
    assert_eq!(refusal.code(), "multitrack/label-count");
    assert_eq!(store.sources_of(project).expect("listed").len(), 0);
}

/// One stream's measurement, stated directly. The pure boundary cases have no
/// child process, so they state the measurement the probe would have produced.
fn measured(stream: u32, label: &str) -> TrackMeasurement {
    TrackMeasurement {
        stream_index: stream,
        label: label.into(),
        time_base: TimeBase::from_rate(48_000, 1).expect("a positive rate"),
        start: MediaTime::ZERO,
        coverage: MediaTime::from_micros(3_000_000).expect("a length"),
        gaps: Vec::new(),
        complete: true,
    }
}
