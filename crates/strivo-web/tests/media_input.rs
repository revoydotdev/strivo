//! The media-input gate, driven through the real HTTP routes.
//!
//! Report item F03: `POST /api/v1/plugins/editor/{id}` takes an
//! `strivo_editor::Edl` in which the **client** names every cut's path and
//! every cut's `start_sec`/`end_sec`. Before this gate the server stored that
//! document and the next render handed it to ffmpeg, so a hand-crafted request
//! could name any file on the host and any range against it.
//!
//! Every test here posts a body no SPA would send. A gate proven only through
//! the SPA is a UI check, which the plan forbids in as many words. Each
//! rejection asserts the **typed code** rather than a message, and each fixture
//! is a real file whose `ffprobe` reading is restated at the assertion.
//!
//! The pre-spawn proof — that a refusal happens before any child process
//! exists — is a separate binary, `media_input_spawns.rs`, because it has to own
//! `PATH` for the whole process and this one has other tests to run.

#![cfg(feature = "creator")]

mod common;

use std::path::Path;

use axum::http::StatusCode;

use common::{
    audio_only, editor_store_path, edl_cutting, ffmpeg_available, multi_stream, probe, raw_h264,
    unrecognisable, video_with_audio, Sandbox, EDITOR_STORE,
};

/// The plan's own acceptance case, one clause at a time: a traversal path and a
/// `file://` source are each refused with their own code, and neither reaches a
/// media tool.
#[tokio::test]
async fn a_traversal_path_and_a_file_url_source_are_each_refused_by_code() {
    if !ffmpeg_available() {
        eprintln!("skipping: ffmpeg is not on PATH, so no media fixture can be built");
        return;
    }
    let sb = Sandbox::new();
    let outside = sb.dir.path().join("outside.mkv");
    video_with_audio(&outside, 2);
    // `..` out of the root: a real, decodable file, named by a relative hop.
    let traversal = sb.root.join("..").join("outside.mkv");

    let (status, body) = sb.save_edl(edl_cutting(&traversal, 0.0, 1.0)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        Sandbox::detail(&body).starts_with("media-input/outside-root"),
        "a decodable file reached by traversal must be refused as unauthorized, \
         not decoded: {body}"
    );

    let file_url = format!("file://{}", outside.to_string_lossy());
    let (status, body) = sb
        .save_edl(edl_cutting(Path::new(&file_url), 0.0, 1.0))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        Sandbox::detail(&body).starts_with("media-input/protocol-not-allowed"),
        "a `file://` source is an ffmpeg protocol, not a path, and must be \
         refused before ffmpeg can resolve it: {body}"
    );
}

/// The other protocol classes ffmpeg resolves itself. One code, but each
/// scheme named, so the refusal says which one it caught.
#[tokio::test]
async fn every_other_media_protocol_is_refused_by_the_same_typed_code() {
    if !ffmpeg_available() {
        return;
    }
    let sb = Sandbox::new();
    for raw in [
        "http://example.invalid/a.mkv",
        "https://example.invalid/a.mkv",
        "concat:/a.mkv|/b.mkv",
        "subfile,,start,0:/a.mkv",
        "pipe:0",
        "cache:pipe:0",
    ] {
        let (status, body) = sb.save_edl(edl_cutting(Path::new(raw), 0.0, 1.0)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{raw}: {body}");
        assert!(
            Sandbox::detail(&body).starts_with("media-input/protocol-not-allowed"),
            "{raw} was not refused as a protocol: {body}"
        );
    }
}

/// A bare path field that ffmpeg would read as a scheme is a path, and is not
/// refused for being one. A Unix recording path has no colon before the first
/// separator; a Windows drive letter is a single character.
#[tokio::test]
async fn a_plain_recording_path_is_not_mistaken_for_a_protocol() {
    if !ffmpeg_available() {
        return;
    }
    let _store = EDITOR_STORE.lock().await;
    let sb = Sandbox::new();
    let (status, body) = sb.save_edl(edl_cutting(&sb.recording, 0.0, 1.0)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "an in-root recording must be admitted: {body}"
    );
}

/// The plan's second acceptance case: an out-of-range `start_sec`/`end_sec`,
/// measured against the duration `ffprobe` actually reports for the file.
#[tokio::test]
async fn an_out_of_range_cut_is_refused_against_the_probed_duration() {
    if !ffmpeg_available() {
        return;
    }
    let sb = Sandbox::new();
    let probed = probe(&sb.recording).expect("ffprobe reads the fixture");
    let seconds: f64 = probed
        .duration
        .as_deref()
        .expect("duration")
        .parse()
        .expect("f64");
    assert!(
        (3.0..5.0).contains(&seconds),
        "fixture should be about 4s, ffprobe says {seconds}"
    );

    let (status, body) = sb
        .save_edl(edl_cutting(&sb.recording, 0.0, seconds as f32 + 600.0))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let detail = Sandbox::detail(&body);
    assert!(
        detail.starts_with("media-input/cut-out-of-range"),
        "a cut past the end must be refused as out of range: {body}"
    );
    assert!(
        detail.contains(&format!("{}", (seconds * 1_000_000.0).round() as i64)),
        "the refusal must carry the source's real duration so the client can \
         see the bound it crossed: {detail}"
    );
}

/// Coordinates that are not a position on any timeline.
///
/// Two different refusals, and the split is worth stating rather than papering
/// over. A **negative** start and an **inverted** span are both expressible in
/// JSON, so they reach the handler and are refused by the gate as
/// `media-input/cut-range`. A **NaN or an infinity** cannot: JSON has no literal
/// for either, `serde_json` writes `null`, and `null` does not deserialize into
/// the EDL's `f32` — so that refusal is the extractor's 422, before any gate
/// code runs. The gate still refuses them, and the only place a NaN can arrive
/// from is the editor store, which
/// `a_render_refuses_a_stored_edl_with_a_non_finite_coordinate` covers.
#[tokio::test]
async fn negative_and_inverted_coordinates_are_refused_by_the_gate() {
    if !ffmpeg_available() {
        return;
    }
    let sb = Sandbox::new();
    let (status, body) = sb.save_edl(edl_cutting(&sb.recording, -5.0, 1.0)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let detail = Sandbox::detail(&body);
    assert!(
        detail.starts_with("media-input/cut-range") && detail.contains("start_sec"),
        "a negative start must be refused naming the field: {body}"
    );

    // An inverted span is a different `TimeError`, and it reports the interval
    // rather than a field: both endpoints are finite, so the field is not what
    // is wrong.
    let (status, body) = sb.save_edl(edl_cutting(&sb.recording, 3.0, 1.0)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let detail = Sandbox::detail(&body);
    assert!(
        detail.starts_with("media-input/cut-range")
            && detail.contains("3000000")
            && detail.contains("1000000"),
        "an inverted cut must report both endpoints: {body}"
    );
}

#[tokio::test]
async fn a_non_finite_coordinate_is_refused_by_the_json_extractor() {
    if !ffmpeg_available() {
        return;
    }
    let sb = Sandbox::new();
    // `f64::NAN` has no JSON representation, so this is literally what a client
    // sending a NaN puts on the wire.
    for start in [f64::NAN, f64::INFINITY] {
        let (status, body) = sb
            .save_edl(edl_cutting(&sb.recording, start as f32, 1.0))
            .await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{start} must be refused, not coerced: {body}"
        );
    }
}

/// A malformed *file*, one typed reason each, from real fixtures whose
/// `ffprobe` reading is asserted first. A validator that answered every one of
/// these with the same string would be doing none of the work.
#[tokio::test]
async fn each_malformed_source_is_refused_with_its_own_reason() {
    if !ffmpeg_available() {
        return;
    }
    let sb = Sandbox::new();

    // No video stream: a real AAC file. ffprobe: one audio stream, no video.
    let audio = sb.root.join("audio-only.m4a");
    audio_only(&audio, 3);
    let read = probe(&audio).expect("ffprobe reads the audio fixture");
    assert!(
        read.streams.iter().all(|(t, _)| t == "audio") && !read.streams.is_empty(),
        "fixture must be audio-only, ffprobe says {:?}",
        read.streams
    );

    // Duration absent: a real bare H.264 stream. ffprobe: a video stream and no
    // `format.duration` key at all.
    let raw = sb.root.join("raw.h264");
    raw_h264(&raw, 3);
    let read = probe(&raw).expect("ffprobe reads the raw stream");
    assert!(
        read.duration.is_none() && read.streams.iter().any(|(t, _)| t == "video"),
        "fixture must be a video stream with no container duration, ffprobe says \
         duration={:?} streams={:?}",
        read.duration,
        read.streams
    );

    // Undecodable: length without a container. ffprobe exits non-zero.
    let garbage = sb.root.join("garbage.mkv");
    unrecognisable(&garbage);
    assert!(
        probe(&garbage).is_none(),
        "fixture must be unreadable by ffprobe, or it is not the undecodable case"
    );

    for (path, code) in [
        (&audio, "media-input/no-video-stream"),
        (&raw, "media-input/duration-missing"),
        (&garbage, "media-input/undecodable"),
    ] {
        let (status, body) = sb.save_edl(edl_cutting(path, 0.0, 1.0)).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "{}: {body}",
            path.display()
        );
        assert!(
            Sandbox::detail(&body).starts_with(code),
            "{} should be refused as {code}, got {body}",
            path.display()
        );
    }
}

/// The extension is not the evidence. A real AAC file wearing a `.mkv` name is
/// read as what it is and refused for having no video stream — the opposite of
/// what extension-sniffing admission would do with it.
#[tokio::test]
async fn a_file_whose_extension_lies_is_judged_by_its_content() {
    if !ffmpeg_available() {
        return;
    }
    let sb = Sandbox::new();
    let liar = sb.root.join("actually-audio.mkv");
    audio_only(&sb.root.join("real.m4a"), 3);
    std::fs::copy(sb.root.join("real.m4a"), &liar).expect("copy");

    let (status, body) = sb.save_edl(edl_cutting(&liar, 0.0, 1.0)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        Sandbox::detail(&body).starts_with("media-input/no-video-stream"),
        "a `.mkv` name over audio content must be refused on what ffprobe read, \
         which is also the proof that the name is not what is checked: {body}"
    );
}

/// A multi-stream recording is normal and must be admitted. A gate that
/// counted streams would refuse real library files.
#[tokio::test]
async fn a_multi_stream_recording_is_admitted() {
    if !ffmpeg_available() {
        return;
    }
    let _store = EDITOR_STORE.lock().await;
    let sb = Sandbox::new();
    let multi = sb.root.join("multi.mkv");
    multi_stream(&multi, 3);
    let read = probe(&multi).expect("ffprobe reads the multi-stream fixture");
    assert_eq!(
        read.streams.iter().filter(|(t, _)| t == "video").count(),
        2,
        "fixture must carry two video streams, ffprobe says {:?}",
        read.streams
    );

    let (status, body) = sb.save_edl(edl_cutting(&multi, 0.0, 1.0)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "multi-stream media is legal: {body}"
    );
}

/// The plan's positive criterion: an authorized asset still renders, through
/// the real ffmpeg render the editor performs.
#[tokio::test]
async fn an_authorized_in_root_edit_still_renders() {
    if !ffmpeg_available() {
        return;
    }
    let _store = EDITOR_STORE.lock().await;
    let sb = Sandbox::new();
    let (status, body) = sb.save_edl(edl_cutting(&sb.recording, 0.0, 2.0)).await;
    assert_eq!(status, StatusCode::OK, "save: {body}");

    let (status, body) = sb
        .post(
            &format!("/api/v1/plugins/editor/{}/render", sb.recording_id),
            serde_json::json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "render: {body}");

    // The path comes from the render's own answer rather than being re-derived
    // here. The route writes one immutable file per attempt rather than a
    // stable `<recording_id>.mkv`, because a stable name meant a second render
    // silently overwrote an artifact a user had already approved.
    let artifact_id = body["artifact_id"]
        .as_str()
        .unwrap_or_else(|| panic!("the render must name its artifact: {body}"));
    assert!(
        !artifact_id.is_empty(),
        "an empty artifact id addresses nothing: {body}"
    );
    let rendered = sb
        .root
        .join(body["output_path"].as_str().expect("output_path"));
    assert!(
        rendered.is_file(),
        "the render reported {rendered:?} but wrote no file there"
    );
    assert!(
        rendered.starts_with(&sb.root),
        "the reported artifact escaped the sandbox root: {rendered:?}"
    );
    let read = probe(&rendered).expect("the render is real media");
    assert!(
        read.duration.is_some(),
        "the rendered output must itself be readable media: {read:?}"
    );
}

/// A stored EDL is re-admitted at the render boundary, not trusted because the
/// save path admitted it. The document here is written straight into the editor
/// store, so the only thing that can refuse it is the render's own gate.
#[tokio::test]
async fn a_render_refuses_a_stored_edl_that_names_a_path_outside_the_root() {
    if !ffmpeg_available() {
        return;
    }
    let _store = EDITOR_STORE.lock().await;
    let sb = Sandbox::new();
    let outside = sb.dir.path().join("planted.mkv");
    video_with_audio(&outside, 2);
    let planted = strivo_editor::Edl {
        recording_id: sb.recording_id.clone(),
        cuts: vec![strivo_editor::Cut {
            start_sec: 0.0,
            end_sec: 1.0,
            kind: strivo_editor::CutKind::Source {
                source_path: outside.to_string_lossy().to_string(),
            },
            fade_in_sec: 0.0,
            fade_out_sec: 0.0,
        }],
    };
    let store = strivo_editor::store::EdlStore::open(&editor_store_path()).expect("editor store");
    store.save(None, &planted, "planted").expect("plant");

    let (status, body) = sb
        .post(
            &format!("/api/v1/plugins/editor/{}/render", sb.recording_id),
            serde_json::json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        Sandbox::detail(&body).starts_with("media-input/outside-root"),
        "a stored EDL must be re-admitted before it is rendered: {body}"
    );
    assert!(
        !sb.root.join("edl").exists(),
        "no output directory may be created for a refused render"
    );
}

/// Where a non-finite coordinate actually stops.
///
/// The request body cannot carry one (above), and neither can the store: every
/// persistence path is JSON, `serde_json` writes a non-finite `f32` as `null`,
/// and `null` does not read back as an `f32`. So the gate's non-finite branch
/// is unreachable from any writer that exists today, which is why it is tested
/// at stage 3 against an in-memory document rather than through a route. If
/// this test ever fails, a non-JSON EDL format is in play and that branch has
/// become reachable.
#[tokio::test]
async fn a_non_finite_coordinate_does_not_survive_the_editor_store() {
    if !ffmpeg_available() {
        return;
    }
    let _store = EDITOR_STORE.lock().await;
    let sb = Sandbox::new();
    let planted = strivo_editor::Edl {
        recording_id: sb.recording_id.clone(),
        cuts: vec![strivo_editor::Cut {
            start_sec: f32::NAN,
            end_sec: 1.0,
            kind: strivo_editor::CutKind::Source {
                source_path: sb.recording.to_string_lossy().to_string(),
            },
            fade_in_sec: 0.0,
            fade_out_sec: 0.0,
        }],
    };
    let store = strivo_editor::store::EdlStore::open(&editor_store_path()).expect("editor store");
    store
        .save(None, &planted, "planted")
        .expect("write attempt");

    // The read now *refuses* rather than reporting "no edit": the old `load`
    // turned an unparseable row into `None`, which is indistinguishable from a
    // recording that was never edited. Asserting the refusal also keeps this
    // test from passing for free had the store written nothing at all.
    assert!(
        matches!(
            store.current(&sb.recording_id),
            Err(strivo_editor::store::RevisionError::Unreadable { .. })
        ),
        "a document with a non-finite coordinate must be refused on read, not \
         silently reported as absent"
    );
}

/// `clamp_request` is called with no source duration, so the clipper's read
/// position is the client's alone. A seek past the end of the source is refused;
/// a position inside it is not.
#[tokio::test]
async fn a_clip_read_position_past_the_end_of_the_source_is_refused() {
    if !ffmpeg_available() {
        return;
    }
    let sb = Sandbox::new();
    let extract = format!("/api/v1/plugins/clipper/{}/extract", sb.recording_id);

    let (status, body) = sb
        .post(
            &extract,
            serde_json::json!({ "start_sec": 100_000.0, "duration_sec": 20.0 }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        Sandbox::detail(&body).starts_with("media-input/cut-out-of-range"),
        "a read position past the end of the source must be refused: {body}"
    );
    assert!(
        !sb.root.join("clips").exists(),
        "no clip directory may be created for a refused extract"
    );

    let (status, body) = sb
        .post(
            &extract,
            serde_json::json!({ "start_sec": 0.0, "duration_sec": 20.0 }),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a position inside the source is legal, and refusing it would be a \
         regression: {body}"
    );
}
