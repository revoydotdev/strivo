//! The media-input gate and the import surface, driven against the same
//! hostile inputs.
//!
//! Import is a **second way in**: a client can name a path in an import request
//! exactly as it can name one in an edit's cut. If import admitted anything the
//! gate refuses, the library could hold a source that the next ffmpeg-bound
//! request would reject — and the two would have drifted. This file is the
//! check that they have not.
//!
//! It asserts **codes, not prose**: for each input it drives the real
//! [`admit_all`] — the production function the route calls — and asserts the
//! typed [`PathRejection`] class is the one todo 25 defined. Because `admit_all`
//! returns the gate's own error type, the codes cannot differ by construction;
//! the value of the test is that it fails the moment someone routes around the
//! gate, and the first test proves the gate admits what it should so a suite
//! that refuses everything cannot pass the rest.

#![cfg(feature = "creator")]

use std::fs;
use std::path::Path;

use strivo_web::routes::import::admit_all;
use strivo_web::routes::media_input::{admit_path, PathRejection};

/// A real media file, so a refusal is about the path and not about a decodable
/// file being unreadable. The corpus manifest records this file's sha256.
fn fixture(dir: &Path) -> std::path::PathBuf {
    dir.join("landmarks.mkv")
}

fn corpus(dir: &Path) {
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/media/landmarks.mkv"),
        fixture(dir),
    )
    .expect("the corpus fixture is committed and readable");
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("strivo-t33-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch made");
    dir
}

fn path_str(p: &Path) -> String {
    p.display().to_string()
}

/// The control. Without it, every assertion below would also hold against an
/// `admit_all` that refused everything — which is the vacuous-pass shape this
/// plan has been bitten by eleven times.
#[test]
fn a_real_media_file_inside_the_root_is_admitted() {
    let dir = scratch("valid");
    let root = dir.join("root");
    fs::create_dir_all(&root).expect("root made");
    corpus(&root);

    let admitted = admit_all(&root, &[path_str(&fixture(&root))]).expect("a real file is admitted");

    assert_eq!(admitted.len(), 1, "one path in, one admitted file out");
    assert_eq!(
        admitted[0].locator(),
        fixture(&root).canonicalize().expect("canonical"),
        "and it is the canonical path, symlinks resolved"
    );
}

/// The load-bearing parity claim, one class at a time. Each case asserts the
/// **same code the ffmpeg-bound path produces**, obtained by calling the gate
/// directly, so a divergence between the two surfaces fails here rather than in
/// production.
#[test]
fn import_refuses_exactly_what_the_media_gate_refuses() {
    let dir = scratch("parity");
    let root = dir.join("root");
    fs::create_dir_all(&root).expect("root made");
    corpus(&root);
    // A real, non-empty, readable file *outside* the root: the traversal target.
    let outside = dir.join("outside.mkv");
    fs::copy(fixture(&root), &outside).expect("outside fixture");
    let empty = root.join("empty.mkv");
    fs::write(&empty, b"").expect("empty file");
    let fifo = root.join("pipe.mkv");
    // A fifo is the one that hangs: opening it for reading blocks until a
    // writer appears, so an implementation that read it before checking its
    // file type would wedge rather than refuse. `mkfifo` is a real binary; when
    // it is unavailable the case is skipped rather than faked.
    let have_fifo = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let unicode = root.join("日本語 セッション 🎬.mkv");
    fs::copy(fixture(&root), &unicode).expect("unicode fixture");

    let protocol = |scheme: &str| PathRejection::ProtocolNotAllowed {
        scheme: scheme.into(),
    };
    let cases: Vec<(&str, String, PathRejection)> = vec![
        (
            "a traversal out of the root",
            path_str(&root.join("..").join("outside.mkv")),
            PathRejection::OutsideRoot,
        ),
        (
            "a file:// URL",
            format!("file://{}", path_str(&outside)),
            protocol("file"),
        ),
        (
            "an ffmpeg concat: protocol",
            format!("concat:{}", path_str(&fixture(&root))),
            protocol("concat"),
        ),
        (
            // Named with everything up to the colon, not just `subfile`: the
            // gate's rule is deliberately positional rather than a URI grammar,
            // because a grammar-only parser would hand this string to the
            // filesystem as a filename. What it reports is what it caught.
            "a subfile: hop, which is a protocol to ffmpeg and not a valid scheme",
            format!("subfile,,start,0:{}", path_str(&outside)),
            protocol("subfile,,start,0"),
        ),
        (
            "a directory where a file was expected",
            path_str(&root),
            PathRejection::NotAFile { kind: "directory" },
        ),
        (
            "a zero-byte file",
            path_str(&empty),
            PathRejection::EmptyFile,
        ),
        (
            "a path that does not resolve",
            path_str(&root.join("nope.mkv")),
            PathRejection::NotFound,
        ),
        ("an empty path", "   ".into(), PathRejection::EmptyPath),
    ];

    for (what, raw, expected) in cases {
        let batch = [raw.clone()];
        // What the ffmpeg-bound path does with the same string.
        let gate = admit_path(Some(&root), &raw).expect_err("the gate refuses it too");
        assert_eq!(gate, expected, "the gate's own class for {what}");
        // And what import does.
        let import = admit_all(&root, &batch).expect_err("import refuses it");
        assert_eq!(
            import,
            gate,
            "import refused {what} as {:?}, but the gate refuses it as {:?}: the \\
             two surfaces have drifted",
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
        let import = admit_all(&root, &[path_str(&fifo)]).expect_err("a fifo is refused, not read");
        assert_eq!(
            import,
            PathRejection::NotAFile {
                kind: "special file"
            },
            "a fifo is refused by file type before anything opens it, so an \
             import cannot block on a writer that never comes"
        );
    } else {
        eprintln!("skipping the fifo case: mkfifo is not available");
    }

    // The positive case that shares the root with all the refusals: a unicode
    // name is admitted, because nothing about its bytes or its name is a
    // protocol or a traversal.
    let admitted =
        admit_all(&root, &[path_str(&unicode)]).expect("a unicode name is ordinary data");
    assert_eq!(
        admitted.len(),
        1,
        "refusing the hostile inputs did not cost the real one"
    );
}
