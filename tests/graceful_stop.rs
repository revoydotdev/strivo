//! The acceptance gate for `FfmpegProcess::stop()`.
//!
//! A PVR's whole product is a clean library, and ffmpeg only writes the
//! Matroska trailer and cue index when it shuts down *gracefully*. Kill it with
//! SIGKILL (Unix) or `TerminateProcess` (Windows) and the file is truncated:
//! it may still play, but duration is wrong and seeking is broken. That damage
//! is silent — nothing errors, and the operator finds out weeks later.
//!
//! Unix has always sent SIGINT here. Windows previously had no graceful path at
//! all (`self.child.kill()`), so this test exists to prove the platform-specific
//! stop paths actually finalise a file, on whichever platform it runs.
//!
//! It drives the real `FfmpegBuilder`/`FfmpegProcess` code path rather than
//! re-implementing the stop logic, so it fails if that path regresses.
//!
//! Skipped when ffmpeg/ffprobe are absent, so it never breaks a machine that
//! simply has no media tooling installed.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use strivo_core::config::ResolvedFormat;
use strivo_core::recording::ffmpeg::FfmpegBuilder;

fn tool(name: &str) -> Option<PathBuf> {
    which::which(name).ok()
}

/// Build a finite source clip long enough that the recorder is still running
/// when we stop it. Returns None if ffmpeg cannot produce one.
fn make_source(ffmpeg: &Path, dir: &Path) -> Option<PathBuf> {
    let src = dir.join("source.mkv");
    let status = Command::new(ffmpeg)
        .args([
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=900:size=1280x720:rate=30",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(&src)
        .status()
        .ok()?;
    (status.success() && src.exists()).then_some(src)
}

/// ffprobe the container-level duration. `None` means ffprobe could not read
/// one, which is exactly what a missing trailer looks like.
fn probe_duration_secs(ffprobe: &Path, file: &Path) -> Option<f64> {
    let out = Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(file)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<f64>()
        .ok()
}

#[tokio::test]
async fn stop_finalises_the_recording_so_it_is_seekable() {
    let (Some(ffmpeg), Some(ffprobe)) = (tool("ffmpeg"), tool("ffprobe")) else {
        eprintln!("skipping: ffmpeg/ffprobe not installed");
        return;
    };

    let dir = tempfile::tempdir().expect("tempdir");
    let Some(source) = make_source(&ffmpeg, dir.path()) else {
        eprintln!("skipping: this ffmpeg build cannot encode the test source");
        return;
    };

    let output = dir.path().join("capture.mkv");
    // Re-encode rather than stream-copy: copying a local file completes far
    // faster than realtime, so the process would exit before stop() and the
    // test would prove nothing about finalisation.
    let mut proc = FfmpegBuilder::new(source.to_string_lossy().into_owned(), output.clone())
        .format(ResolvedFormat {
            format: "test".into(),
            bitrate_kbps: None,
            container: "mkv".into(),
            video_codec: "libx264".into(),
            audio_codec: "copy".into(),
        })
        .build()
        .expect("spawn ffmpeg");

    // Let it capture a little, then stop it the way the daemon does.
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(
        proc.try_wait().expect("try_wait").is_none(),
        "ffmpeg exited on its own before stop() — the source was too short to test finalisation"
    );

    proc.stop().await.expect("stop() reported failure");

    assert!(output.exists(), "no output file was produced");
    assert!(
        output.metadata().expect("stat").len() > 0,
        "output is empty"
    );

    // The real assertion: a finalised Matroska has a readable container
    // duration. A hard-killed one does not, because the trailer never landed.
    let duration = probe_duration_secs(&ffprobe, &output).unwrap_or_else(|| {
        panic!(
            "ffprobe could not read a container duration from {} — \
             the file was not finalised (no Matroska trailer), which is the \
             corruption this test exists to catch",
            output.display()
        )
    });
    assert!(
        duration > 0.5,
        "container duration {duration}s is implausibly short for a ~3s capture; \
         the file looks truncated"
    );
}
