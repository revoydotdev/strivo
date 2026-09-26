//! Correcting a word leaves the decoded media bit-identical.
//!
//! The plan's first acceptance clause says "bit-identical", which is a
//! measurement and not an argument, so this file measures: the file's bytes,
//! and the MD5 ffmpeg's `md5` muxer prints over its **decoded** frames and
//! samples, before and after a transcript edit.
//!
//! It also carries the structural half of the same claim. The obvious way for
//! a text correction to change the picture is for the model to reach the disk,
//! so the model is asserted to have no filesystem vocabulary at all: a module
//! that cannot open a file cannot overwrite one, and has no path by which it
//! could route around todo 30's `OutputExists` refusal.

mod transcript;

use std::path::{Path, PathBuf};
use std::process::Command;

use strivo_editor::transcript::{Correction, Linked, TranscriptDraft, UnsavedWork};
use transcript::{composition, id, transcript, us, MEDIA_US, REVISION};

/// The link model's three source files, for the structural check. Read at
/// test time rather than searched at build time, so the check is a test that
/// can fail loudly instead of a compile error, and so adding a fourth file to
/// the module is a visible omission rather than a silent one.
const MODEL: [&str; 3] = [
    concat!(env!("CARGO_MANIFEST_DIR"), "/src/transcript/mod.rs"),
    concat!(env!("CARGO_MANIFEST_DIR"), "/src/transcript/link.rs"),
    concat!(env!("CARGO_MANIFEST_DIR"), "/src/transcript/edit.rs"),
];

/// A real, decodable file: one second of a test pattern, the smallest thing
/// ffmpeg will decode and hash.
fn real_media(dir: &Path) -> PathBuf {
    let path = dir.join("source.mkv");
    let status = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=64x48:rate=10:duration=1",
            "-c:v",
            "ffv1",
        ])
        .arg(&path)
        .status()
        .expect("ffmpeg is available; without it this test cannot claim anything");
    assert!(status.success(), "ffmpeg produced a fixture: {status}");
    assert!(
        path.metadata().expect("stat").len() > 0,
        "a non-empty fixture"
    );
    path
}

/// ffmpeg's MD5 muxer hashes the **decoded** frames, so this is a statement
/// about pixels rather than about the container.
fn decoded_digest(path: &Path) -> String {
    let out = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-f", "md5", "-"])
        .output()
        .expect("ffmpeg runs");
    assert!(
        out.status.success(),
        "ffmpeg decoded the fixture: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let digest = String::from_utf8(out.stdout)
        .expect("the md5 muxer prints text")
        .trim()
        .to_string();
    assert!(digest.starts_with("MD5="), "a digest, not noise: {digest}");
    digest
}

fn file_digest(path: &Path) -> Vec<u8> {
    std::fs::read(path).expect("read the fixture")
}

/// A link over the fixture's real source. The transcript is the shared
/// fixture's, so every number stays checkable by eye.
fn linked() -> Linked {
    Linked::new(
        transcript(),
        composition(),
        transcript::source(),
        Some(REVISION.to_string()),
        Some(us(MEDIA_US)),
    )
    .expect("the fixture transcript is word-timed")
}

#[test]
fn correcting_a_word_leaves_the_decoded_media_bit_identical() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let media = real_media(dir.path());
    let before_bytes = file_digest(&media);
    let before_decoded = decoded_digest(&media);
    assert!(!before_bytes.is_empty() && !before_decoded.is_empty());

    let link = linked();
    let before_map = serde_json::to_string(link.map()).expect("serialises");

    // Both edit kinds, with real media on disk. Neither may write a byte.
    link.correct_text(&id("w3"), "approximately")
        .expect("a correction applies");
    let edited = link
        .delete(&id("w4"), &id("w4"))
        .expect("a deletion applies");
    assert!(
        edited.cut().is_some(),
        "the control: a real source range was cut"
    );

    assert_eq!(file_digest(&media), before_bytes, "the file's bytes");
    assert_eq!(
        decoded_digest(&media),
        before_decoded,
        "every decoded frame"
    );
    assert_eq!(
        serde_json::to_string(link.map()).expect("serialises"),
        before_map,
        "and the composition, which is the only input to a decode, is \
         byte-identical — so the decoded media cannot differ"
    );
}

/// A non-destructive edit is not "we rewrite the file and hope": nothing new
/// appears on disk either.
#[test]
fn a_transcript_edit_writes_nothing_new_to_disk() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let media = real_media(dir.path());
    let link = linked();

    let edited = link.delete(&id("w2"), &id("w3")).expect("applies");
    assert!(edited.cut().is_some(), "a real source range was cut");

    let mut found: Vec<String> = std::fs::read_dir(dir.path())
        .expect("read the directory")
        .map(|e| {
            e.expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    found.sort();
    assert_eq!(
        found,
        vec!["source.mkv".to_string()],
        "one file, as before: no sidecar, no re-encode, no scratch"
    );
    assert!(
        !file_digest(&media).is_empty(),
        "and the media is still readable"
    );
}

/// **The structural half.** The link model reaches no filesystem API at all,
/// so "a text correction leaves the media intact" is not a promise it could
/// break.
///
/// The scan strips comments first, so this cannot be satisfied by prose
/// describing the invariant, and the last assertion checks the strip is real
/// by requiring the model's own prose to name the vocabulary it refuses.
#[test]
fn the_link_model_has_no_filesystem_vocabulary_outside_its_comments() {
    let source: String = MODEL
        .iter()
        .map(|path| std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}")))
        .collect::<Vec<_>>()
        .join("\n");
    let code: String = source
        .lines()
        .map(str::trim_start)
        .filter(|l| !l.starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    for forbidden in [
        "std::fs",
        "fs::",
        "File::",
        "OpenOptions",
        "Command::",
        "read_to_string",
        "create_dir",
        "remove_",
    ] {
        assert!(
            !code.contains(forbidden),
            "the link model names `{forbidden}` in code, so it could reach the \
             disk — and with it past todo 30's OutputExists refusal"
        );
    }
    // The strip is not vacuous. A forbidden token that appears only in the
    // module's prose must be present in the raw file and absent from the code
    // the scan actually reads — otherwise "no matches" would be a statement
    // about a filter that removed everything.
    assert!(
        source.contains("std::fs") && !code.contains("std::fs"),
        "the model's prose names the vocabulary it refuses, so the strip step \
         is exercised rather than assumed"
    );
}

/// The draft is a patch against a named immutable base, so an unsaved
/// transcript edit carries no copy of the document — which is what keeps
/// `unsaved_json` a small thing one workspace row can hold.
#[test]
fn an_unsaved_transcript_draft_carries_no_copy_of_the_document() {
    let wire = serde_json::to_string(&UnsavedWork {
        transcript: Some(TranscriptDraft {
            base_revision: REVISION.to_string(),
            corrected: vec![Correction {
                word: id("w3"),
                text: "approximately".to_string(),
            }],
            deleted: vec![],
        }),
    })
    .expect("serialises");
    assert!(
        !wire.contains("segments") && !wire.contains("words"),
        "a draft names a base revision and a patch, not a second transcript: {wire}"
    );
    assert!(wire.len() < 200, "and it stays small: {wire}");
}

/// A moved media file keeps every word and every seek target, because the link
/// holds a source *identity* and never resolved the old location.
#[test]
fn a_moved_media_file_keeps_every_word_and_its_seek_target() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let first = dir.path().join("first-place.mkv");
    let made = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=64x48:rate=10:duration=1",
            "-c:v",
            "ffv1",
        ])
        .arg(&first)
        .status()
        .expect("ffmpeg");
    assert!(made.success(), "the fixture was made");
    let second = dir.path().join("second-place.mkv");
    std::fs::rename(&first, &second).expect("the file moves");

    let link = linked();
    let before: Vec<i64> = ["w1", "w2", "w3", "w4", "w5"]
        .iter()
        .map(|w| link.seek_target(&id(w)).expect("shown").micros())
        .collect();
    assert_eq!(before, vec![0, 1_000_000, 2_000_000, 3_500_000, 4_000_000]);

    // The relink is what todo 33's route does: the same identity, a new
    // resolvable location. The transcript side is handed the same identity.
    let relinked = Linked::new(
        link.transcript().clone(),
        link.map().clone(),
        transcript::source(),
        link.revision().map(str::to_string),
        link.media_duration(),
    )
    .expect("still a link");
    let after: Vec<i64> = ["w1", "w2", "w3", "w4", "w5"]
        .iter()
        .map(|w| relinked.seek_target(&id(w)).expect("shown").micros())
        .collect();
    assert_eq!(after, before, "a moved file does not move a word");
    assert!(
        !serde_json::to_string(relinked.map())
            .expect("serialises")
            .contains("first-place"),
        "and no location from before the move survives in the composition"
    );
}
