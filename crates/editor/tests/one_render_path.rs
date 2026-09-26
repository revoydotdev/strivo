//! **One render path, statically.** The measured companion to
//! `tests/mixer_falsifier.rs`.
//!
//! ## Why a source-level probe at all
//!
//! The falsifier binary proves the *measured* claim: preview and render agree
//! because they read one plan. It cannot prove the *structural* claim, and the
//! structural claim is the one that keeps the measured one true. A second
//! function that builds a filter string, or a second way to construct a
//! `CompiledPlan`, is invisible to every assertion in `mixer.rs` — those compare
//! a plan against an artifact, and a parallel path that nothing routes through
//! agrees with both of them perfectly.
//!
//! So this file reads the source and counts things.
//!
//! ## What it counts, and what it deliberately does not
//!
//! | Probe | Claim |
//! |---|---|
//! | [`plan_literals`] | `CompiledPlan` is constructed in exactly one place — the compiler |
//! | [`filter_spellings`] | the editor spells an ffmpeg **audio** filter in exactly one place — `invoke.rs`, which is the plan becoming argv |
//!
//! Two scoping decisions, both load-bearing:
//!
//! * **String literals only.** A filter can only reach an ffmpeg command line
//!   from a string literal, so the probe looks inside literals and ignores prose.
//!   Without that, every doc comment naming a filter would be a violation and the
//!   probe would be unusable within a week.
//! * **Non-test sources only**, with the `render::tests` module excluded by name.
//!   A test may legitimately spell a filter to assert on it; the probe states
//!   that exclusion rather than inheriting it silently.
//!
//! ## The probe can report a violation
//!
//! Every count here has a matching anti-vacuity test, and the detectors are
//! plain functions over `(path, source)` pairs so the *failing* branch is
//! exercised too. A probe that has only ever reported "clean" has never been
//! shown to be able to report anything else, which is the same defect
//! `one_source_of_truth.rs` records as `the_shape_probe_reports_zero_and_many`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The ffmpeg audio filters the render pipeline spells. A subset on purpose:
/// these are the ones todo 44's surface routes, and a vocabulary that grows with
/// whatever the grep happens to match stops being a contract.
const AUDIO_FILTERS: &[&str] = &[
    "volume=",
    "pan=",
    "amix=",
    "alimiter=",
    "highpass=",
    "lowpass=",
    "silenceremove=",
    "loudnorm=",
    "asendcmd=",
    "rubberband=",
    "acompressor",
    "adeesser",
    "anl3denoise",
    "aeval=",
    "aecho=",
    "atrim=",
    "atempo=",
    "apad=",
];

/// The one file allowed to spell an audio filter for the render.
///
/// `invoke.rs` is where a plan becomes argv, so it is where a filter *has* to
/// be spelled. The two other spellings in the tree are not this probe's business
/// and are allowlisted with a reason rather than by accident:
///
/// * `plan.rs` spells `loudnorm=I=…` because `Loudness::from_spec` builds the
///   plan's own target. That string is never executed; the executor composes its
///   own from the target's numbers (see `measure_filter` / `normalize_filter`).
/// * `example.rs` is the canonical example revision, and its whole purpose is to
///   *hand* pre-spelled filters to the compiler as `EffectStep`s — which is the
///   seam working as designed, not a second path.
const FILTER_ALLOWLIST: &[&str] = &["invoke.rs", "plan.rs", "example.rs"];

/// The one file allowed to construct a `CompiledPlan`.
const PLAN_LITERAL_ALLOWLIST: &[&str] = &["compile.rs"];

fn workspace_root() -> PathBuf {
    // `crates/editor` → the workspace root.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/editor has a workspace root two levels up")
        .to_path_buf()
}

/// Every non-test Rust source under `crates/editor/src`.
fn editor_sources() -> Vec<PathBuf> {
    let root = workspace_root().join("crates/editor/src");
    let mut found: Vec<PathBuf> = walk(&root)
        .into_iter()
        .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
        .filter(|path| {
            // `render::tests` is a test module that lives in the source tree
            // rather than under `tests/`. Excluded by name, and the exclusion is
            // asserted below so a rename cannot quietly widen the probe's scope.
            !path.ends_with("render/tests.rs") && !path.to_string_lossy().contains("/render/tests/")
        })
        .collect();
    found.sort();
    found
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out
}

/// The string literals on one line.
///
/// A deliberately small scanner rather than a parser: it only has to be right
/// about which bytes are inside a `"`-delimited literal on a single line, and it
/// reports the literals rather than the line so a filter in a doc comment cannot
/// be mistaken for a filter in a command.
fn string_literals(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut in_literal = false;
    let mut escaped = false;
    for c in line.chars() {
        if in_literal {
            if escaped {
                current.push(c);
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                out.push(std::mem::take(&mut current));
                in_literal = false;
            } else {
                current.push(c);
            }
        } else if c == '"' {
            in_literal = true;
            current.clear();
        }
    }
    out
}

/// Which audio filters this source spells, as `(file name, filter)` pairs.
fn filter_spellings(sources: &[(String, String)]) -> BTreeSet<(String, String)> {
    let mut hits = BTreeSet::new();
    for (name, source) in sources {
        for line in source.lines() {
            for literal in string_literals(line) {
                for filter in AUDIO_FILTERS {
                    if literal.contains(filter) {
                        hits.insert((name.clone(), (*filter).to_string()));
                    }
                }
            }
        }
    }
    hits
}

/// Which files construct a `CompiledPlan`.
///
/// The two shapes that are **not** constructions, both of which the first
/// version of this detector got wrong:
///
/// * `pub struct CompiledPlan {` — the declaration. A `=` test missed nothing
///   here, but the *next* version of that test missed the real construction,
///   which is `Ok(CompiledPlan {` and carries no `=` at all.
/// * `impl CompiledPlan {` — an inherent impl block. It opens a brace and
///   mentions the type, so it reads as a construction to anything matching on
///   the name.
/// * `pub fn compiled(..) -> CompiledPlan {` — a return type, which also opens
///   a brace and would otherwise read as a construction.
///
/// So the discriminator is those three and nothing else: a line that mentions
/// `CompiledPlan {` and is none of a declaration, an impl block or a signature.
/// Each of the three was found by this probe going red on real code, not by
/// imagining it.
fn plan_literals(sources: &[(String, String)]) -> BTreeSet<String> {
    let mut hits = BTreeSet::new();
    for (name, source) in sources {
        for line in source.lines() {
            let code = line.split("//").next().unwrap_or(line);
            if !code.contains("CompiledPlan {") {
                continue;
            }
            if code.contains("struct CompiledPlan")
                || code.contains("impl CompiledPlan")
                || code.contains("-> CompiledPlan")
            {
                continue;
            }
            hits.insert(name.clone());
        }
    }
    hits
}

fn read_sources() -> Vec<(String, String)> {
    editor_sources()
        .into_iter()
        .map(|path| {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            (name, std::fs::read_to_string(&path).unwrap_or_default())
        })
        .collect()
}

// ── the claims ─────────────────────────────────────────────────────────

#[test]
fn a_compiled_plan_is_constructed_in_exactly_one_place() {
    let sources = read_sources();
    let sites = plan_literals(&sources);
    assert_eq!(
        sites,
        PLAN_LITERAL_ALLOWLIST
            .iter()
            .map(|s| (*s).to_string())
            .collect::<BTreeSet<_>>(),
        "a CompiledPlan built outside the compiler is a second way to decide what a \
         revision renders as, which is the defect this file exists to end"
    );
}

#[test]
fn the_editor_spells_an_audio_filter_in_exactly_one_place() {
    let sources = read_sources();
    let hits: BTreeSet<String> = filter_spellings(&sources)
        .into_iter()
        .map(|(file, _)| file)
        .collect();
    assert_eq!(
        hits,
        FILTER_ALLOWLIST
            .iter()
            .map(|s| (*s).to_string())
            .collect::<BTreeSet<_>>(),
        "a filter spelled outside invoke.rs is a second implementation of a stage \
         that the owning plugin crate already emits as an EffectStep — the drift the \
         render module's seam exists to prevent"
    );
}

/// The narrower and more useful half: *which* filter each allowed file spells.
/// `plan.rs` must not gain `amix`, and `invoke.rs` must not start spelling a
/// stage that arrives pre-spelled.
#[test]
fn each_allowed_file_spellings_the_filters_it_owns() {
    let sources = read_sources();
    let per_file: std::collections::BTreeMap<String, BTreeSet<String>> =
        filter_spellings(&sources).into_iter().fold(
            std::collections::BTreeMap::new(),
            |mut acc, (file, filter)| {
                acc.entry(file).or_default().insert(filter);
                acc
            },
        );
    assert_eq!(
        per_file.get("invoke.rs").map(|f| f.contains("volume=")),
        Some(true),
        "anti-vacuity: invoke.rs must really spell `volume=`, or the probe is \
         matching nothing and a planted path would pass"
    );
    assert_eq!(
        per_file.get("plan.rs").map(|f| f.contains("loudnorm=")),
        Some(true),
        "anti-vacuity: plan.rs must really spell `loudnorm=` in Loudness::from_spec"
    );
    assert_eq!(
        per_file.get("plan.rs").map(|f| f.contains("pan=")),
        Some(false),
        "plan.rs builds the loudness target and nothing else; a pan here would be a \
         second place a track's position is decided"
    );
}

// ── the probe can report a violation ────────────────────────────────────

#[test]
fn the_filter_probe_reports_a_planted_parallel_path() {
    // The same shape as `one_source_of_truth`'s
    // `the_shape_probe_reports_zero_and_many`: a probe that has only ever said
    // "clean" has not been shown able to say anything else.
    let planted = vec![(
        "compose.rs".to_string(),
        // Exactly what a real parallel path looks like: someone resolving audibility
        // and re-deriving the filter rather than reading the plan's `audible`.
        "fn parallel(plan: &CompiledPlan) -> String {\n    format!(\"volume={}dB\", plan.tracks[0].gain_db)\n}\n"
            .to_string(),
    )];
    let hits = filter_spellings(&planted);
    assert_eq!(
        hits,
        BTreeSet::from([("compose.rs".to_string(), "volume=".to_string())]),
        "the probe must SEE a filter spelled in a file it does not allowlist, or it \
         cannot see the duplicate it exists to find"
    );

    // And it must not fire on the same filter spelled in a *comment*, which is
    // the scoping decision that makes the probe usable at all.
    let commented = vec![(
        "compose.rs".to_string(),
        "// this is where volume= would go if we had a second path\nfn fine() {}\n".to_string(),
    )];
    assert!(
        filter_spellings(&commented).is_empty(),
        "a filter named in prose is documentation, not a second implementation"
    );
}

#[test]
fn the_plan_literal_probe_reports_a_planted_constructor() {
    // The construction on its own line, which is how real code writes it: put on
    // one line with the signature it would be indistinguishable from a return
    // type, and the probe would have to choose between two false answers.
    let planted = vec![(
        "parallel.rs".to_string(),
        "fn build() -> CompiledPlan {\n    CompiledPlan {\n        subject: \"x\".into(),\n        ..todo!()\n    }\n}\n"
            .to_string(),
    )];
    assert_eq!(
        plan_literals(&planted),
        BTreeSet::from(["parallel.rs".to_string()]),
        "the probe must see a CompiledPlan built outside the compiler"
    );

    // None of the declaration, the impl block or the return type is a
    // construction. Getting any of them wrong makes the probe fire on `plan.rs`
    // or `example.rs` forever and train everyone to ignore it — which is the
    // failure mode a permanently-red static check always reaches.
    let not_a_construction = vec![(
        "plan.rs".to_string(),
        "pub struct CompiledPlan {\n    pub subject: String,\n}\n\nimpl CompiledPlan {\n    pub fn work_items(&self) -> usize { 0 }\n}\n"
            .to_string(),
    ), (
        "example.rs".to_string(),
        "pub fn compiled(request: &RenderRequest) -> CompiledPlan {\n    super::compile(request).unwrap()\n}\n"
            .to_string(),
    )];
    assert!(
        plan_literals(&not_a_construction).is_empty(),
        "a declaration and a return type are not construction sites"
    );
}

#[test]
fn the_probe_scope_is_what_it_says_it_is() {
    let sources = editor_sources();
    let names: Vec<String> = sources
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert!(
        !names.iter().any(|n| n == "mixer.rs" || n == "refusals.rs"),
        "anti-vacuity: the test modules must be excluded, or the probe's scope has \
         silently widened to the fixtures: {names:?}"
    );
    assert!(
        names.contains(&"invoke.rs".to_string()) && names.contains(&"compose.rs".to_string()),
        "anti-vacuity: the probe must actually be reading the files it claims to \
         police, or it is inspecting nothing: {names:?}"
    );
}

#[test]
fn the_filter_vocabulary_is_not_empty() {
    assert!(
        AUDIO_FILTERS.len() >= 10,
        "a vocabulary that shrinks to nothing makes the probe pass for free"
    );
    for filter in ["volume=", "pan=", "loudnorm="] {
        assert!(
            AUDIO_FILTERS.contains(&filter),
            "{filter} left the vocabulary; the probe would stop watching it"
        );
    }
}
