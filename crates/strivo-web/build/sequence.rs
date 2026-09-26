//! Build-time guard on SPA module sequence numbers.
//!
//! Filename order **is** the module order. There is deliberately no order file
//! or manifest, because a second list of module order is precisely the drift
//! this guard exists to catch. That makes a mistyped, duplicated or skipped
//! sequence number a *silent* change to what ships: the bundle still
//! assembles, still parses, and simply runs its modules in the wrong order, or
//! drops one of them.
//!
//! Three findings, all collected before failing so one build error names
//! everything that is wrong:
//!
//! 1. **malformed** — a file in a module directory whose name is not
//!    `<seq>[<letters>]-<pvr|creator>.<js|css>`. Such a file is either a
//!    module that can never be assembled (`.JS`, `012-pvr.js.bak`, a stray
//!    `.md`) or a second source of truth for the order, which this module
//!    scheme does not allow. A non-numeric or absent sequence lands here too.
//! 2. **duplicate** — two files claiming one slot. The slot is the parsed
//!    `(sequence, letter suffix, edition)`, so `012-pvr.js` and `012a-pvr.js`
//!    are distinct slots and `016-pvr.js` may share its number with a
//!    `016-creator.js` sibling. `012-pvr.js` and `12-pvr.js` are one slot
//!    claimed twice, and the unpadded name sorts into a different place in
//!    the bundle: exactly the silent reorder this is here to stop.
//! 3. **gap** — a sequence number with no module at all. Numbers are read
//!    across both editions, before the `-creator` filter, so the 1-byte PVR
//!    spacers (`016/022/024/026-pvr.js`) keep the numbering contiguous for a
//!    Creator sibling that does not exist yet, and a Creator-only module can
//!    never look like a gap to a PVR build. Reading them together is also why
//!    the rule is identical in both editions instead of two rules that drift.
//!
//! An empty directory is a fourth failure rather than a pass: a guard over
//! nothing satisfies every assertion it could make.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

/// Fail the build unless the module files in `dir` form a unique, gap-free
/// sequence. Every regular file in the directory is examined.
pub fn assert_sequence_ok(dir: &Path) {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("cannot read SPA module directory {}: {e}", dir.display()))
        .map(|e| {
            e.unwrap_or_else(|e| panic!("cannot read an entry of {}: {e}", dir.display()))
                .path()
        })
        .filter(|p| p.is_file())
        .collect();
    // Sorted so the findings come out in the same order the bundle would have
    // concatenated in: a failure then reads like the bundle order did.
    entries.sort();

    let findings = findings(dir, &entries);
    assert!(
        findings.is_empty(),
        "SPA module sequence check failed for {}:\n  - {}",
        dir.display(),
        findings.join("\n  - ")
    );
}

/// Every rule violation in `modules`, in filename order.
fn findings(dir: &Path, modules: &[std::path::PathBuf]) -> Vec<String> {
    if modules.is_empty() {
        return vec![format!(
            "{} holds no module files, so the bundle assembled from it would be empty",
            dir.display()
        )];
    }

    let mut numbers: BTreeSet<u64> = BTreeSet::new();
    let mut claimed: BTreeMap<(u64, &str, &str), &str> = BTreeMap::new();
    let mut out = Vec::new();

    for module in modules {
        let name = module.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let Some(slot) = parse_slot(name) else {
            out.push(format!(
                "malformed module name `{name}`: expected `<seq>[<letters>]-<pvr|creator>.<js|css>`"
            ));
            continue;
        };
        numbers.insert(slot.0);
        match claimed.entry(slot) {
            std::collections::btree_map::Entry::Vacant(v) => {
                v.insert(name);
            }
            std::collections::btree_map::Entry::Occupied(o) => out.push(format!(
                "duplicate slot {:?}: `{}` and `{name}` both claim it",
                slot,
                o.get()
            )),
        }
    }

    if let Some(max) = numbers.iter().next_back() {
        let missing: Vec<u64> = (0..=*max).filter(|n| !numbers.contains(n)).collect();
        if !missing.is_empty() {
            out.push(format!(
                "gap: no module at sequence {} (0..={max} expected, so every earlier module keeps its position)",
                ranges(&missing)
            ));
        }
    }

    out
}

/// Compress sorted numbers into `12, 41-119, 240`.
///
/// A one-module hole is one number; a mistyped sequence can leave a range
/// hundreds long, and dumping each element buries the edit that caused it.
/// The range's upper bound lands next to the `0..=max` it is compared against,
/// which is what names the offending module.
fn ranges(sorted: &[u64]) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut start = sorted[0];
    let mut prev = start;
    let push = |out: &mut Vec<String>, start: u64, prev: u64| {
        out.push(if start == prev {
            start.to_string()
        } else {
            format!("{start}-{prev}")
        });
    };
    for n in &sorted[1..] {
        if *n != prev + 1 {
            push(&mut out, start, prev);
            start = *n;
        }
        prev = *n;
    }
    push(&mut out, start, prev);
    out.join(", ")
}

/// Split `012a-pvr.js` into its slot `(12, "a", "pvr")`.
///
/// `None` for anything that is not a module file, including a sequence that
/// is absent, non-numeric, or too large to parse — a rejected name is a build
/// error, never a silently reordered bundle.
fn parse_slot(name: &str) -> Option<(u64, &str, &str)> {
    let (stem, ext) = name.rsplit_once('.')?;
    if !matches!(ext, "js" | "css") {
        return None;
    }
    let (seq, edition) = stem.rsplit_once('-')?;
    if !matches!(edition, "pvr" | "creator") {
        return None;
    }
    // Trailing lowercase is the sub-slot letter run, so `012a` is number 12
    // slot `a` and `012` is number 12 slot ``.
    let letters_len = seq.bytes().rev().take_while(u8::is_ascii_lowercase).count();
    let (number, letters) = seq.split_at(seq.len() - letters_len);
    if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((number.parse().ok()?, letters, edition))
}
