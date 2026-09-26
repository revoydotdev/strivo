//! Every vendored browser library matches the SHA-256 recorded in its
//! `PROVENANCE.md`.
//!
//! The record is read out of the document rather than restated here, so the two
//! cannot drift: a library that is swapped, re-minified, or fetched from a
//! different registry stops matching and the test goes red. That is the only
//! thing protecting a repo that commits third-party bytes it never builds.

use sha2::{Digest, Sha256};
use std::path::Path;

fn vendor_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/vendor")
}

fn provenance() -> String {
    std::fs::read_to_string(vendor_dir().join("PROVENANCE.md"))
        .expect("assets/vendor/PROVENANCE.md exists; it is the licence and integrity record")
}

/// The recorded `| \`file\` | \`<64 hex>\` | <bytes> |` rows, as (name, hash, bytes).
fn recorded() -> Vec<(String, String, u64)> {
    let mut rows = Vec::new();
    for line in provenance().lines() {
        let Some(cells) = line
            .strip_prefix('|')
            .and_then(|l| l.strip_suffix('|'))
            .map(|l| l.split('|').map(str::trim).collect::<Vec<_>>())
        else {
            continue;
        };
        let [name, hash, bytes] = cells[..] else {
            continue;
        };
        let Some(name) = name.strip_prefix('`').and_then(|n| n.strip_suffix('`')) else {
            continue;
        };
        let Some(hash) = hash.strip_prefix('`').and_then(|h| h.strip_suffix('`')) else {
            continue;
        };
        let Ok(bytes) = bytes.replace([' ', ','], "").parse::<u64>() else {
            continue;
        };
        assert_eq!(
            hash.len(),
            64,
            "PROVENANCE.md records a {}-character hash for {name}, not a SHA-256",
            hash.len()
        );
        rows.push((name.to_owned(), hash.to_owned(), bytes));
    }
    rows
}

#[test]
fn every_vendored_library_matches_its_recorded_sha256() {
    let rows = recorded();
    assert!(
        rows.len() >= 3,
        "expected at least three recorded vendored libraries, found {} — \
         PROVENANCE.md is the integrity record and cannot be empty",
        rows.len()
    );

    for (name, want_hash, want_bytes) in rows {
        let path = vendor_dir().join(&name);
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|e| panic!("recorded library {name} is missing: {e}"));
        assert_eq!(
            bytes.len() as u64,
            want_bytes,
            "{name} is {} bytes, PROVENANCE.md records {want_bytes}",
            bytes.len()
        );
        let got = format!("{:x}", Sha256::digest(&bytes));
        assert_eq!(got, want_hash, "{name} does not match its recorded SHA-256");
    }
}

/// Every vendored script is accounted for. A new file dropped into the tree
/// without a provenance row would otherwise ship unreviewed and unattributed.
#[test]
fn every_vendored_script_has_a_provenance_row() {
    let mut recorded: Vec<String> = recorded().into_iter().map(|(n, _, _)| n).collect();
    recorded.sort();
    let mut scripts: Vec<String> = std::fs::read_dir(vendor_dir())
        .expect("assets/vendor exists")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".js") || n.ends_with(".mjs"))
        .collect();
    scripts.sort();
    assert_eq!(
        scripts, recorded,
        "the vendored scripts and the PROVENANCE.md rows disagree"
    );
}
