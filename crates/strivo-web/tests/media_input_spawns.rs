//! The pre-spawn proof: a refused media input reaches no child process at all.
//!
//! Every other media-input test asserts *what* the gate answers. This one
//! asserts *when*, and "when" is the whole claim — a gate that ran after ffmpeg
//! had already been handed the file would produce the same rejections, one
//! moment later, having already let ffmpeg open it.
//!
//! The method is a **`PATH` shim**. `strivo_core::tools::resolve_tool` falls back
//! to `which` (and the clipper spawns a bare `ffmpeg`), so an `ffprobe` and an
//! `ffmpeg` placed first on `PATH` intercept every media child this server can
//! create. Each records its argv to a marker file and exits non-zero. A spawn
//! therefore leaves a trace that cannot be missed by sampling, which a
//! process-count sampler cannot promise: an `ffprobe` that starts and exits
//! inside one 5 ms window is invisible to it.
//!
//! This binary holds exactly one test, because `PATH` is one value for the
//! process. The same reasoning `tests/media_pool.rs` gives for owning the XDG
//! roots.
//!
//! ## The controls, and why they are not optional
//!
//! "No marker file" is what a **broken shim** also produces, and a validator
//! that refuses everything produces the same result for every input. So this
//! file proves the rig can see a spawn before it trusts an absence:
//!
//! - **C1** a real probe of a valid file must record `ffprobe`;
//! - **C2** a real render of a valid stored EDL must record `ffmpeg`;
//! - **C3** the valid in-root save must be accepted, and a valid in-range clip
//!   extract must be accepted.
//!
//! Without C1 and C2 every "no marker" assertion below would be worth nothing.

#![cfg(feature = "creator")]

mod common;

use std::path::{Path, PathBuf};

use axum::http::StatusCode;

use common::{editor_store_path, edl_cutting, ffmpeg_available, video_with_audio, Sandbox};

/// What the `ffprobe` shim reports, in whole microseconds. A cut is inside its
/// source at or below this, and the out-of-range case is measured against it.
const SHIM_DURATION_US: i64 = 4_000_000;
const SHIM_DURATION_SEC: f32 = 4.0;

/// Marker files, one per tool, recording the argv of every intercepted spawn.
struct Markers {
    dir: tempfile::TempDir,
}

impl Markers {
    /// Write an `ffprobe` and an `ffmpeg` into a fresh directory and return it.
    ///
    /// The `ffprobe` shim **succeeds**, printing a fixed one-video-stream,
    /// four-second response; the `ffmpeg` shim **fails**. Both record their argv
    /// first. Failing both is simpler, but then a valid input could never be
    /// admitted — every request would be refused, and a gate that refuses
    /// everything satisfies every "nothing was spawned" assertion below. With a
    /// succeeding probe, acceptance and refusal are both observable in the same
    /// rig, and telling them apart is the thing under test.
    ///
    /// The scripted answer is not a way around `ffprobe`: *what* the gate
    /// answers is tested in `media_input.rs`, against the host's real ffprobe
    /// and real fixtures. What is tested here is only *whether a probe is
    /// called at all*, and for that the answer is irrelevant — only the record
    /// is not.
    ///
    /// The scripts live in `bin/` and the markers in `markers/`. They started
    /// out in one directory named after the tool, and a shim appending to
    /// `<dir>/ffmpeg` was appending to *itself* — so `reset()` deleted the shims
    /// and every absence after the first control was measured against a rig
    /// that no longer existed. The first version of this file passed its own
    /// anti-vacuity control for exactly that reason.
    fn install() -> Self {
        let dir = tempfile::tempdir().expect("marker tempdir");
        let bin = dir.path().join("bin");
        let marks = dir.path().join("markers");
        std::fs::create_dir_all(&bin).expect("bin");
        std::fs::create_dir_all(&marks).expect("markers");
        // Built by concatenation, not `format!`. The script is full of `%s` and
        // `%%s`, and a `format!` template in the same file was observed emitting
        // a literal `%%s` into the generated shim — which `printf` then printed
        // as a bare `%s`, so the marker recorded *that a tool ran* and silently
        // dropped the argv it was handed. Nothing here needs `format!`'s
        // substitution, so it is not used.
        let record = |tool: &str| {
            let marker = marks.join(tool);
            let marker = marker.display();
            let mut script = String::new();
            script.push_str(&format!("printf '%s' \"{tool}\" >> '{marker}'\n"));
            script.push_str(&format!("printf ' %s' \"$@\" >> '{marker}'\n"));
            script.push_str(&format!("printf '\\n' >> '{marker}'\n"));
            script
        };
        let probe = format!(
            "#!/bin/sh\n{}printf '%s' '{}'\nexit 0\n",
            record("ffprobe"),
            serde_json::json!({
                "streams": [
                    {"codec_type": "video", "codec_name": "h264"},
                    {"codec_type": "audio", "codec_name": "aac"},
                ],
                "format": {"duration": "4.000000"},
            }),
        );
        for (name, script) in [
            ("ffprobe", probe),
            ("ffmpeg", format!("#!/bin/sh\n{}exit 1\n", record("ffmpeg"))),
        ] {
            let path = bin.join(name);
            std::fs::write(&path, script).expect("write shim");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                    .expect("chmod shim");
            }
        }
        Self { dir }
    }

    /// The directory to prepend to `PATH`.
    fn bin(&self) -> PathBuf {
        self.dir.path().join("bin")
    }

    fn marker(&self, tool: &str) -> PathBuf {
        self.dir.path().join("markers").join(tool)
    }

    /// The argv lines the shim recorded for `tool`, empty if it never ran.
    fn recorded(&self, tool: &str) -> Vec<String> {
        std::fs::read_to_string(self.marker(tool))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn reset(&self) {
        for tool in ["ffprobe", "ffmpeg"] {
            let _ = std::fs::remove_file(self.marker(tool));
        }
    }

    /// Assert on both tools at once, so every "nothing was spawned" claim in
    /// this file reads the same and no assertion can forget one of them.
    fn assert_spawned(&self, tool: &str, what: &str) {
        assert!(
            !self.recorded(tool).is_empty(),
            "{what}: expected a {tool} spawn, and the shim recorded none. The shim is \
             not intercepting, so every absence asserted in this file would be vacuous."
        );
    }

    fn assert_no_media_child(&self, what: &str) {
        for tool in ["ffprobe", "ffmpeg"] {
            assert!(
                self.recorded(tool).is_empty(),
                "{what}: a {tool} child was created, but the input was refused. The gate \
                 ran after the media tool was already handed the file. Recorded: {:?}",
                self.recorded(tool)
            );
        }
    }
}

/// A stored EDL naming `path`, written straight into the editor store so the
/// only thing that can refuse it is the render's own gate.
fn plant(sb: &Sandbox, path: &Path, start_sec: f32, end_sec: f32) {
    let edl = strivo_editor::Edl {
        recording_id: sb.recording_id.clone(),
        cuts: vec![strivo_editor::Cut {
            start_sec,
            end_sec,
            kind: strivo_editor::CutKind::Source {
                source_path: path.to_string_lossy().to_string(),
            },
            fade_in_sec: 0.0,
            fade_out_sec: 0.0,
        }],
    };
    let store = strivo_editor::store::EdlStore::open(&editor_store_path()).expect("editor store");
    store.save_with_label(&edl, "planted").expect("plant");
}

#[tokio::test]
async fn a_refused_media_input_reaches_no_child_process() {
    if !ffmpeg_available() {
        eprintln!("skipping: ffmpeg is not on PATH, so no fixture can be built");
        return;
    }
    // The fixtures are real and are built with the host's own ffmpeg, while it
    // is still the one on PATH. The shim takes over afterwards, and from then
    // on the *facts* about a file come from `SHIM_DURATION_US` below rather
    // than from a probe of it.
    let sb = Sandbox::new();
    let real = sb.recording.clone();
    let outside = sb.dir.path().join("outside.mkv");
    video_with_audio(&outside, 2);

    let markers = Markers::install();
    let saved_path = std::env::var("PATH").unwrap_or_default();
    std::env::set_var(
        "PATH",
        format!("{}:{}", markers.bin().display(), saved_path),
    );

    // ---- C1: the shim sees a probe, and the gate admits what it reports. --
    // Both halves matter. A recorded spawn proves the rig can see one; the
    // admitted result proves the gate is not simply refusing everything, which
    // is what would make every "no marker" assertion below worthless.
    let probed = strivo_web::routes::media_input::probe_admitted(&real, 1)
        .await
        .expect("a readable in-root file must be admitted through the shimmed probe");
    markers.assert_spawned("ffprobe", "C1, a direct probe of a valid file");
    assert_eq!(
        probed.duration.micros(),
        SHIM_DURATION_US,
        "the admitted duration must be the one the shim reported, or the shim is \
         not the probe this gate used"
    );
    markers.reset();

    // ---- C2: the shim sees an ffmpeg spawn. ------------------------------
    // Not posed through the render route, and the reason matters: with the
    // shim installed the route's *own* gate probe fails first, so the render
    // never reaches ffmpeg. That is the gate working, but it makes the route
    // unusable as its own control. So the control calls the editor's render
    // directly — the same function the route calls — which proves the shim
    // intercepts the transcoder. The other half of the claim, that the route
    // really does spawn that transcoder, is proved in `media_input.rs` by
    // `an_authorized_in_root_edit_still_renders`, which renders with the host's
    // real ffmpeg and ffprobes the output. Neither half alone would do.
    let direct = strivo_editor::Edl {
        recording_id: sb.recording_id.clone(),
        cuts: vec![strivo_editor::Cut {
            start_sec: 0.0,
            end_sec: 1.0,
            kind: strivo_editor::CutKind::Source {
                source_path: real.to_string_lossy().to_string(),
            },
            fade_in_sec: 0.0,
            fade_out_sec: 0.0,
        }],
    };
    let out_dir = sb.root.join("direct");
    let out = out_dir.join("out.mkv");
    let _ = strivo_editor::render_edl_with_filters(&direct, &out, None, None);
    markers.assert_spawned("ffmpeg", "C2, a direct EDL render");
    markers.reset();

    // ---- C3: valid inputs are accepted, so the refusals below are the
    //          gate's doing and not a blanket refusal. -------------------
    let (status, body) = sb.save_edl(edl_cutting(&real, 0.0, 1.0)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "C3, a valid save must be accepted: {body}"
    );
    // Asserted on the marker, not on the status: the shimmed ffmpeg exits
    // non-zero, so an admitted extract reports 500 *after* it has already been
    // handed the file. Reaching ffmpeg is the fact that matters — it is the
    // same fact the refused-extract case below asserts the absence of.
    let _ = sb
        .post(
            &format!("/api/v1/plugins/clipper/{}/extract", sb.recording_id),
            serde_json::json!({ "start_sec": 0.0, "duration_sec": 20.0 }),
        )
        .await;
    markers.assert_spawned("ffmpeg", "C3, a valid clip extract");
    markers.reset();

    // ---- The render boundary: a poisoned stored EDL. --------------------
    // This is the load-bearing case. The document was written into the store
    // directly, so the save gate never saw it; if the refusal here came from
    // ffmpeg's stderr instead of from the gate, the marker would be non-empty
    // and the assertion below fails.
    let render = format!("/api/v1/plugins/editor/{}/render", sb.recording_id);
    plant(&sb, &outside, 0.0, 1.0);
    let (status, body) = sb.post(&render, serde_json::json!({})).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "planted out-of-root EDL: {body}"
    );
    assert!(
        Sandbox::detail(&body).starts_with("media-input/outside-root"),
        "planted out-of-root EDL: {body}"
    );
    markers.assert_no_media_child("a render of a stored EDL naming a path outside the root");
    markers.reset();

    // ---- The save boundary: every static rejection class. ---------------
    let traversal = sb.root.join("..").join("outside.mkv");
    let cases: Vec<(&str, std::path::PathBuf)> = vec![
        ("a traversal path", traversal),
        (
            "a file:// source",
            PathBuf::from(format!("file://{}", outside.display())),
        ),
        (
            "an http:// source",
            PathBuf::from("http://example.invalid/a.mkv"),
        ),
        ("a concat: source", PathBuf::from("concat:/a.mkv|/b.mkv")),
        (
            "a subfile: source",
            PathBuf::from("subfile,,start,0:/a.mkv"),
        ),
        ("a pipe: source", PathBuf::from("pipe:0")),
        ("an empty path", PathBuf::from("   ")),
        ("a path that does not resolve", sb.root.join("nope.mkv")),
        ("a directory", sb.root.clone()),
    ];
    for (what, path) in cases {
        let (status, body) = sb.save_edl(edl_cutting(&path, 0.0, 1.0)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{what}: {body}");
        markers.assert_no_media_child(&format!("editor_save refusing {what}"));
    }
    markers.reset();

    // ---- A traversal hidden behind a legitimate cut. ---------------------
    // The case that decides whether the gate resolves *every* path before it
    // probes *any* of them. A single interleaved pass — admit cut 0, probe cut
    // 0, admit cut 1, probe cut 1 — refuses this document just as correctly,
    // one `ffprobe` later, having already handed the first cut to a media tool.
    let hidden = serde_json::json!({
        "recording_id": "ignored-by-the-server",
        "cuts": [
            {"start_sec": 0.0, "end_sec": 1.0,
             "kind": {"kind": "source", "source_path": real.to_string_lossy()}},
            {"start_sec": 0.0, "end_sec": 1.0,
             "kind": {"kind": "broll", "broll_path": sb.root.join("..").join("outside.mkv").to_string_lossy()}},
        ],
    });
    let (status, body) = sb.save_edl(hidden).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a traversal behind a valid cut: {body}"
    );
    assert!(
        Sandbox::detail(&body).contains("cut 1"),
        "the refusal must name the cut that is at fault, not the first: {body}"
    );
    markers.assert_no_media_child("editor_save refusing a traversal in the second cut");
    markers.reset();

    // ---- The one class that legitimately costs a probe. -----------------
    // An out-of-range cut cannot be recognised without the source's duration,
    // so exactly one `ffprobe` is the correct behaviour and this test says so
    // rather than pretending otherwise. What must not appear is `ffmpeg`: the
    // range is decided before the file is handed to a transcoder.
    let (status, body) = sb
        .save_edl(edl_cutting(&real, 0.0, SHIM_DURATION_SEC + 600.0))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "out-of-range cut: {body}");
    markers.assert_spawned(
        "ffprobe",
        "an out-of-range cut is only knowable from the source's duration",
    );
    assert!(
        markers.recorded("ffmpeg").is_empty(),
        "an out-of-range cut spawned ffmpeg: {:?}",
        markers.recorded("ffmpeg")
    );
    markers.reset();

    // ---- The clipper boundary. ------------------------------------------
    let (status, body) = sb
        .post(
            &format!("/api/v1/plugins/clipper/{}/extract", sb.recording_id),
            serde_json::json!({ "start_sec": 100_000.0, "duration_sec": 20.0 }),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a clip read position past the end: {body}"
    );
    assert!(
        markers.recorded("ffmpeg").is_empty(),
        "a refused clip extract spawned ffmpeg: {:?}",
        markers.recorded("ffmpeg")
    );

    std::env::set_var("PATH", saved_path);
}
