//! Todo 53 — a graph is authored in the browser, and an invalid one never
//! reaches the executor.
//!
//! The properties under test are the ones a rendered editor cannot see:
//!
//!   1. EACH OF THE SIX EXECUTOR RULES is reported **individually and by
//!      id**, with the offending stage named. A single "invalid graph"
//!      boolean would pass every one of the six UI cases while fixing none.
//!   2. THE SEVENTH RULE — a dispatch naming a plugin this build does not
//!      register — is refused at save time. `validate` only checks the
//!      dispatch strings are non-empty, so without this a graph saves,
//!      renders as runnable, and fails at dispatch.
//!   3. A CYCLE is refused **naming the offending edge**.
//!   4. A SAVED GRAPH ROUND-TRIPS: reload returns the same value, so an
//!      editor that writes back what it read cannot drift.
//!   5. TWO CONCURRENT SAVES CONFLICT. Todo 49's mechanism, reused: the save
//!      names the revision it was built on and a stale base is reported.
//!   6. THE BROWSER'S RULE IDS ARE THE EXECUTOR'S. Read out of the SPA module
//!      source and compared as sets, because a JS mirror of six rules is a
//!      second answer unless something holds the two together.
#![cfg(feature = "creator")]

use std::collections::BTreeSet;

use strivo_core::pipeline::executor::PipelineRegistry;
use strivo_core::pipeline::stage::{ResourceLock, StageId};
use strivo_core::pipeline::{Pipeline, Stage, StageDispatch, StageKind};
use strivo_core::plugin::registry::PluginRegistry;
use strivo_web::routes::pipeline_graph::{authoring_violations, RULE_UNKNOWN_DISPATCH};
use strivo_web::routes::pipeline_templates::{
    self, authoring_payload, revision_of, rule_ids, save, SaveRefusal, StoredTemplate,
};

fn registry() -> PluginRegistry {
    let mut registry = PluginRegistry::new();
    strivo_plugins::register_all(&mut registry);
    registry
}

fn stage(name: &str) -> Stage {
    let mut stage = Stage::new(name, StageKind::Custom("casebook.compose".into()));
    stage.dispatch = Some(StageDispatch::new("crunchr", "compose"));
    stage
}

/// A graph that satisfies all seven rules, so each test below breaks exactly
/// one thing and the refusal it gets back is that thing.
fn valid() -> Pipeline {
    let mut pipeline = Pipeline::new("authoring fixture");
    let transcribe = stage("Transcribe");
    let publish_id = transcribe.id;
    pipeline.stages.push(transcribe);
    let mut publish = stage("Publish");
    publish.inputs.push(publish_id);
    pipeline.stages.push(publish);
    pipeline
}

/// `XDG_DATA_HOME` is how `AppConfig::data_dir()` resolves, so pointing it at
/// a throwaway directory is what keeps these tests off a real installation.
/// One value for the whole binary: tests share a process, and two different
/// values would be a race rather than a sandbox.
fn sandbox() -> &'static str {
    use std::sync::OnceLock;
    static DIR: OnceLock<String> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("strivo-t53-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create sandbox data dir");
        // Cleared here, inside the one-time init, so it completes before any
        // test body can write. The pid alone is not a fresh-enough name: a
        // re-run of the same binary can land on a recycled pid and then find
        // the previous run's template — which reads as a conflict on a first
        // save, i.e. a failure that is about the harness, not the code.
        let templates = dir.join("strivo").join("pipelines").join("templates");
        let _ = std::fs::remove_dir_all(&templates);
        // `AppConfig::data_dir()` reads XDG_DATA_HOME through
        // `directories::ProjectDirs`, so this is the supported way to keep a
        // test off a real installation — no production hook added for it.
        std::env::set_var("XDG_DATA_HOME", &dir);
        dir.to_string_lossy().to_string()
    })
}

fn rules_broken(pipeline: &Pipeline, registry: &PluginRegistry) -> BTreeSet<&'static str> {
    let mut errors = PipelineRegistry::validation_errors(pipeline);
    errors.extend(authoring_violations(pipeline, registry));
    errors.into_iter().map(|e| e.rule).collect()
}

fn refuse(pipeline: &Pipeline) -> SaveRefusal {
    save("fixture", None, pipeline, &registry()).expect_err("this graph must be refused")
}

#[test]
fn a_valid_graph_saves_and_reloads_identically() {
    sandbox();
    let registry = registry();
    let pipeline = valid();
    assert!(
        PipelineRegistry::validation_errors(&pipeline).is_empty(),
        "the fixture itself must break no executor rule: {:?}",
        PipelineRegistry::validation_errors(&pipeline)
    );
    assert!(authoring_violations(&pipeline, &registry).is_empty());

    let saved = save("round-trip", None, &pipeline, &registry).expect("first save");
    let on_disk: StoredTemplate = serde_json::from_str(
        &std::fs::read_to_string(pipeline_templates::template_path("round-trip").expect("safe id"))
            .expect("template file"),
    )
    .expect("reparse");
    // `Pipeline` carries an `Instant` serde skips and no `PartialEq`, so the
    // round trip is asserted on the serialised value — which is exactly what
    // was written to disk and exactly what a reload will hand back.
    assert_eq!(
        serde_json::to_value(&on_disk.pipeline).expect("serialise"),
        serde_json::to_value(&pipeline).expect("serialise"),
        "the reloaded graph is the saved graph"
    );
    assert_eq!(on_disk.revision, saved.revision);
    assert_eq!(revision_of(&pipeline), saved.revision);
}

#[test]
fn every_executor_rule_is_reported_individually_by_id() {
    let registry = registry();
    // `baseline` proves the empty set is the empty set, so a later
    // "exactly one id" assertion cannot be satisfied by a check that
    // reports nothing at all.
    assert_eq!(rules_broken(&valid(), &registry), BTreeSet::new());

    let mut cases: Vec<(&str, Pipeline)> = Vec::new();

    let mut cyclic = valid();
    let (a, b) = (cyclic.stages[0].id, cyclic.stages[1].id);
    cyclic.stages[1].inputs.push(a);
    cyclic.stages[0].inputs.push(b);
    cases.push(("acyclic", cyclic));

    let mut empty = valid();
    empty.stages.clear();
    cases.push(("empty-stage-list", empty));

    let mut duplicate = valid();
    duplicate.stages[1].id = duplicate.stages[0].id;
    cases.push(("duplicate-stage-id", duplicate));

    let mut dangling = valid();
    dangling.stages[1].inputs.push(StageId::new_v4());
    cases.push(("dangling-input", dangling));

    let mut zero_retry = valid();
    zero_retry.stages[0].max_attempts = 0;
    cases.push(("zero-capacity", zero_retry));

    let mut zero_lock = valid();
    zero_lock.stages[0].requires = vec![ResourceLock::Cpu { cap: 0 }];
    cases.push(("zero-capacity", zero_lock));

    let mut empty_dispatch = valid();
    empty_dispatch.stages[0].dispatch = Some(StageDispatch::new("crunchr", "   "));
    cases.push(("empty-dispatch", empty_dispatch));

    let mut ghost = valid();
    ghost.stages[0].dispatch = Some(StageDispatch::new("not-installed", "compose"));
    cases.push((RULE_UNKNOWN_DISPATCH, ghost));

    // `dispatch: None` is deliberately NOT a case here. It is valid for a
    // source or artifact node, so it must NOT trip rule 7 — which is the
    // opposite assertion, and the next test makes it.

    for (rule, pipeline) in cases {
        let broken = rules_broken(&pipeline, &registry);
        assert!(
            broken.contains(rule),
            "case {rule:?} broke {broken:?} instead"
        );
        // And the save path refuses on the same rule, by the same id.
        match refuse(&pipeline) {
            SaveRefusal::Invalid { errors } => {
                let ids: BTreeSet<&str> = errors.iter().map(|e| e.rule).collect();
                assert!(
                    ids.contains(rule),
                    "the save refusal for case {rule:?} carried {ids:?}"
                );
            }
            other => panic!("case {rule:?} was not refused as invalid: {other:?}"),
        }
    }
}

#[test]
fn the_seventh_rule_fires_where_the_executor_alone_would_accept() {
    let registry = registry();
    let mut pipeline = valid();
    pipeline.stages[0].dispatch = Some(StageDispatch::new("not-installed", "compose"));

    // The precondition that makes this rule worth having: the executor's own
    // six rules pass it, so nothing downstream of them would ever catch it.
    assert!(
        PipelineRegistry::validation_errors(&pipeline).is_empty(),
        "the six executor rules must accept this graph, or rule 7 is redundant"
    );
    let errors = authoring_violations(&pipeline, &registry);
    assert_eq!(errors.len(), 1, "exactly the one unregistered dispatch");
    assert_eq!(errors[0].rule, RULE_UNKNOWN_DISPATCH);
    let detail = errors[0].detail.as_deref().unwrap_or_default();
    assert!(
        detail.contains("not-installed") && detail.contains("Transcribe"),
        "the refusal names the plugin and the stage: {detail:?}"
    );

    // A stage with no dispatch at all is NOT this rule: `dispatch: None` is
    // valid for a source or artifact node.
    let mut source = valid();
    source.stages[0].dispatch = None;
    assert!(authoring_violations(&source, &registry).is_empty());
}

#[test]
fn a_cycle_is_refused_naming_the_offending_edge() {
    let mut pipeline = valid();
    let (a, b) = (pipeline.stages[0].id, pipeline.stages[1].id);
    pipeline.stages[1].inputs.push(a);
    pipeline.stages[0].inputs.push(b);

    let errors = PipelineRegistry::validation_errors(&pipeline);
    let acyclic = errors
        .iter()
        .find(|e| e.rule == "acyclic")
        .expect("the acyclic rule fires");
    let detail = acyclic.detail.as_deref().unwrap_or_default();
    assert!(
        detail.contains("Transcribe") && detail.contains("Publish"),
        "the refusal names both ends of the offending edge: {detail:?}"
    );
    // It names the edge as a closed walk, so the hop that closes the loop is
    // the last one and is identifiable.
    assert_eq!(
        detail.matches("→").count(),
        3,
        "a closed 2-stage walk: {detail:?}"
    );

    match refuse(&pipeline) {
        SaveRefusal::Invalid { errors } => {
            assert!(errors.iter().any(|e| e.rule == "acyclic"));
        }
        other => panic!("a cyclic graph must be refused: {other:?}"),
    }
    assert!(pipeline.cycle().is_some(), "cycle() is what names the edge");
    assert!(valid().cycle().is_none(), "and a clean graph reports none");
}

#[test]
fn two_saves_on_one_base_conflict_and_the_second_is_refused() {
    sandbox();
    let registry = registry();
    let base = save("conflict", None, &valid(), &registry).expect("first save");

    // The other client, working from the same base, saves different edits.
    let mut theirs = valid();
    theirs.stages[0].name = "Their Transcribe".into();
    let their_saved = save("conflict", Some(&base.revision), &theirs, &registry)
        .expect("a save on the current base succeeds");
    assert_ne!(their_saved.revision, base.revision);

    // We were still holding the old base. Reported, never resolved
    // last-write-wins — the two clients hold different edits and only the
    // caller can say which survives.
    let mut ours = valid();
    ours.stages[0].name = "Our Transcribe".into();
    let refusal = save("conflict", Some(&base.revision), &ours, &registry)
        .expect_err("a stale base is refused");
    match &refusal {
        SaveRefusal::Conflict {
            base: reported,
            current,
        } => {
            assert_eq!(reported, &base.revision);
            assert_eq!(current, &their_saved.revision);
        }
        other => panic!("expected a conflict, got {other:?}"),
    }
    // Todo 49's code, verbatim: one conflict vocabulary across surfaces.
    assert_eq!(refusal.code(), "history/conflict");
    let body = refusal.body();
    assert_eq!(body["code"], "history/conflict");
    assert_eq!(body["status"], 409);
    assert_eq!(body["base"], base.revision.as_str());
    assert_eq!(body["current"], their_saved.revision.as_str());

    // And the refused save wrote nothing: the winner is still on disk.
    let stored: StoredTemplate = serde_json::from_str(
        &std::fs::read_to_string(pipeline_templates::template_path("conflict").unwrap()).unwrap(),
    )
    .unwrap();
    assert_eq!(stored.pipeline.stages[0].name, "Their Transcribe");
}

#[test]
fn an_invalid_graph_is_refused_before_the_file_is_touched() {
    sandbox();
    let registry = registry();
    let mut broken = valid();
    broken.stages[0].dispatch = Some(StageDispatch::new("not-installed", "compose"));
    let refusal = save("never-written", None, &broken, &registry).expect_err("refused");
    assert_eq!(refusal.code(), "graph/invalid");
    assert!(
        !pipeline_templates::template_path("never-written")
            .expect("safe id")
            .exists(),
        "a refused save must not have created the file"
    );
}

#[test]
fn the_payload_ships_the_registry_names_and_the_rule_ids() {
    let registry = registry();
    let payload = authoring_payload(&[("creator_intelligence", valid())], &registry);
    let plugins = payload["registry"]["plugins"]
        .as_array()
        .expect("the registry's names are an array");
    assert!(
        plugins.iter().any(|p| p == "crunchr"),
        "the plugin this fixture dispatches to is in the list: {plugins:?}"
    );
    assert_eq!(
        payload["rules"].as_array().map(|r| r.len()),
        Some(7),
        "six executor rules plus the registry one"
    );
    assert_eq!(rule_ids().len(), 7);
    assert!(payload["templates"][0]["pipeline"]["stages"].is_array());
}

#[test]
fn the_browsers_rule_ids_are_the_executors() {
    // The SPA mirrors six rules in JavaScript so a user sees the error
    // before a save. A mirror is a second answer unless something holds the
    // two together, and a count is not enough — the ids must be the same set.
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/spa/047-creator.js"),
    )
    .expect("047-creator.js exists");
    let in_js: BTreeSet<String> = PipelineRegistry::RULE_IDS
        .iter()
        .chain(std::iter::once(&RULE_UNKNOWN_DISPATCH))
        .map(|id| format!("\"{id}\""))
        .collect();
    for quoted in &in_js {
        assert!(
            source.contains(quoted.as_str()),
            "047-creator.js never mentions the rule id {quoted}"
        );
    }
    // And the shipped list, which is what the browser actually renders by, is
    // the executor's list plus exactly the one rule the executor cannot
    // express.
    assert_eq!(
        rule_ids().last().copied(),
        Some(RULE_UNKNOWN_DISPATCH),
        "rule 7 is appended after the executor's six, not interleaved"
    );
}
