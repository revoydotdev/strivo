//! Shared helpers for the media interaction tests.
//!
//! `content_fingerprint` here is deliberately an INDEPENDENT implementation of
//! todo 33's rule, so a test never validates the product's key by asking the
//! product what the key is. The corpus location is the committed one.

#![allow(dead_code)]

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/media")
}

/// The rule under test: identity is the digest of the whole content, so a
/// rewrite that preserves path and mtime still changes it, and a rename that
/// preserves every byte does not.
pub fn content_fingerprint(path: &Path) -> String {
    let mut file = std::fs::File::open(path).expect("the source is readable");
    let mut digest = Sha256::new();
    let mut buf = vec![0_u8; 64 * 1024];
    loop {
        let read = std::io::Read::read(&mut file, &mut buf).expect("a readable source");
        if read == 0 {
            break;
        }
        digest.update(&buf[..read]);
    }
    format!("sha256:{:x}", digest.finalize())
}

/// Decode one frame from `path` at `secs`, straight to a PNG file, with a
/// direct ffmpeg call.
///
/// This is the ORACLE for the mapping tests, so it deliberately shares no code
/// with `strivo_core::media` — a helper that both the product and the test
/// call would validate the mapping with the thing being tested. It is also the
/// one place in these tests that decodes a source at source resolution, and it
/// is in the test, not in an interaction path.
pub async fn oracle_frame(path: &Path, secs: f64, width: u32) -> PathBuf {
    let out = std::env::temp_dir().join(format!(
        "t50-oracle-{}-{secs:.3}-{width}.png",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&out);
    let status = tokio::process::Command::new(strivo_core::tools::resolve_tool("ffmpeg"))
        .args(["-nostdin", "-y", "-ss", &format!("{secs:.6}"), "-i"])
        .arg(path)
        .args([
            "-frames:v",
            "1",
            "-vf",
            &format!("scale={width}:-2"),
            "-f",
            "image2",
            "-update",
            "1",
        ])
        .arg(&out)
        .status()
        .await
        .expect("spawn the oracle ffmpeg");
    assert!(status.success(), "the oracle ffmpeg exited {status}");
    out
}
