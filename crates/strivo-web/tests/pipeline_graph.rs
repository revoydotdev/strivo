//! Todo 52 — the node graph is over the executor's own `Pipeline`/`Stage`
//! model, and the graph's node set is *derived* from it rather than
//! tabulated beside it.
//!
//! The properties under test are the ones a rendered-node count cannot see:
//!
//!   1. BIDIRECTIONAL COMPLETENESS — every node resolves to a real `Stage`,
//!      and every `Stage` has exactly one node. A hand-kept table drifts in
//!      both directions and neither shows up in a screenshot.
//!   2. EDGE FIDELITY — every edge is a real `inputs` link, and every
//!      `inputs` link produced an edge.
//!   3. DERIVATION — a stage added to the pipeline after the graph exists
//!      appears in the graph with no edit to any table, which is the
//!      property a hand-kept table cannot have.
//!   4. HONESTY UNDER AN ABSENT EXECUTOR — a stage whose dispatch names no
//!      registered plugin is not runnable and says which plugin is absent.
//!      Todo 6 removed the hardcoded `status` string; runnability is
//!      resolved against the executor's registry, never asserted.
//!
//! Deliberately asserting on the *serialised* payload rather than on a Rust
//! struct: the struct is the producer, the JSON is what the graph renders.
#![cfg(feature = "creator")]

use strivo_core::pipeline::{Pipeline, Stage, StageDispatch, StageKind};
use strivo_core::plugin::registry::PluginRegistry;
use strivo_web::routes::pipeline_graph::{graph_payload, mapping_mismatches};

/// The registry the daemon and every route share. Built through
/// `strivo_plugins::register_all` rather than a hand-made list, so a
/// capability that stops being declared fails here for the same reason it
/// would fail in production.
fn registry() -> PluginRegistry {
    let mut registry = PluginRegistry::new();
    strivo_plugins::register_all(&mut registry);
    registry
}

fn templates() -> Vec<(&'static str, Pipeline)> {
    vec![
        (
            "creator_intelligence",
            strivo_plugins::pipeline_templates::creator_intelligence(uuid::Uuid::nil(), "manual"),
        ),
        (
            "creator_publish",
            strivo_plugins::pipeline_templates::creator_publish(uuid::Uuid::nil(), "manual"),
        ),
    ]
}

fn graph_for<'a>(payload: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    payload["graphs"]
        .as_array()
        .expect("graphs array")
        .iter()
        .find(|g| g["name"] == name)
        .unwrap_or_else(|| panic!("no graph named {name}"))
}

fn ids(value: &serde_json::Value) -> Vec<String> {
    value["nodes"]
        .as_array()
        .expect("nodes array")
        .iter()
        .map(|n| n["stage_id"].as_str().expect("stage_id string").to_string())
        .collect()
}

#[test]
fn every_node_maps_to_a_real_stage_and_every_stage_has_exactly_one_node() {
    // Built once: `Pipeline::new` mints fresh `StageId`s, so asking for the
    // templates twice compares two different graphs and fails for a reason
    // that has nothing to do with the mapping.
    let templates = templates();
    let payload = graph_payload(&[], &templates, &registry());
    let publish = graph_for(&payload, "Ultimate creator preparation");
    let pipeline = &templates[1].1;

    // The stage list, read from the model rather than from the payload.
    let stage_ids: Vec<_> = pipeline.stages.iter().map(|s| s.id.to_string()).collect();
    let node_ids = ids(publish);

    assert_eq!(node_ids.len(), stage_ids.len(), "one node per stage");
    assert_eq!(
        node_ids.iter().collect::<std::collections::BTreeSet<_>>(),
        stage_ids.iter().collect::<std::collections::BTreeSet<_>>(),
        "the node set IS the stage set, in both directions"
    );
    assert_eq!(publish["mapping"]["mismatches"], serde_json::json!([]));
}

#[test]
fn a_node_with_no_stage_is_detected() {
    let pipeline = Pipeline::new("graph");
    let payload = graph_payload(&[], &[("t", pipeline.clone())], &registry());
    let graph = graph_for(&payload, "graph");

    // Tamper with the payload the way a drifting producer would: a node for a
    // stage that does not exist. The check has to notice, or the graph draws
    // a phantom.
    let mut nodes = graph["nodes"].as_array().cloned().unwrap_or_default();
    nodes.push(serde_json::json!({ "stage_id": "not-a-stage" }));
    let mismatches = mapping_mismatches(&nodes, &[], &pipeline);
    assert_eq!(
        mismatches,
        vec!["node not-a-stage maps to no stage".to_string()]
    );
}

#[test]
fn a_stage_with_no_node_is_detected() {
    let mut pipeline = Pipeline::new("graph");
    let orphan = pipeline.add_stage(Stage::new("orphan", StageKind::Concat));
    let payload = graph_payload(&[], &[("t", pipeline.clone())], &registry());
    let graph = graph_for(&payload, "graph");

    // The producer cannot produce this today, which is the point: the check
    // is what proves the producer will not, so it must fail on a node list
    // the producer did not emit.
    let nodes: Vec<serde_json::Value> = Vec::new();
    let mismatches = mapping_mismatches(&nodes, graph["edges"].as_array().unwrap(), &pipeline);
    assert_eq!(mismatches, vec![format!("stage {orphan} has no node")]);
}

#[test]
fn every_edge_maps_to_a_real_inputs_link_and_every_inputs_link_produced_one() {
    let templates = templates();
    let payload = graph_payload(&[], &templates, &registry());
    let publish = graph_for(&payload, "Ultimate creator preparation");
    let pipeline = &templates[1].1;

    let edges = publish["edges"].as_array().expect("edges array");
    // Both directions, counted from the model.
    let links: Vec<(String, String)> = pipeline
        .stages
        .iter()
        .flat_map(|s| {
            s.inputs
                .iter()
                .map(move |i| (i.to_string(), s.id.to_string()))
        })
        .collect();
    assert_eq!(edges.len(), links.len());
    let rendered: Vec<(String, String)> = edges
        .iter()
        .map(|e| {
            (
                e["from"].as_str().unwrap().to_string(),
                e["to"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(
        rendered.iter().collect::<std::collections::BTreeSet<_>>(),
        links.iter().collect::<std::collections::BTreeSet<_>>()
    );
    assert_eq!(publish["mapping"]["mismatches"], serde_json::json!([]));
}

#[test]
fn an_edge_with_no_inputs_link_is_detected() {
    // Two stages, one real `inputs` link, and an edge that the model does not
    // contain. The pair is the smallest shape where a spurious edge and a
    // missing edge are separately countable.
    let mut pipeline = Pipeline::new("edges");
    let root = pipeline.add_stage(Stage::new("root", StageKind::Concat));
    let leaf = pipeline.add_stage(Stage::new("leaf", StageKind::Concat).with_inputs(vec![root]));
    let payload = graph_payload(&[], &[("t", pipeline.clone())], &registry());
    let graph = graph_for(&payload, "edges");
    let nodes = graph["nodes"].as_array().cloned().unwrap_or_default();
    let mut edges = graph["edges"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        edges.len(),
        1,
        "the model really does have exactly one link"
    );

    // An edge the model does not contain, alongside the nodes that do map.
    edges.push(serde_json::json!({ "from": leaf, "to": root }));
    let mismatches = mapping_mismatches(&nodes, &edges, &pipeline);
    assert_eq!(
        mismatches,
        vec![format!("edge {leaf}->{root} has no inputs link")]
    );

    // And the other direction: a real link that no edge represents.
    let mismatches = mapping_mismatches(&nodes, &[], &pipeline);
    assert_eq!(
        mismatches,
        vec![format!("inputs link {root}->{leaf} has no edge")]
    );
}

#[test]
fn the_mapping_is_derived_so_a_stage_added_later_still_appears() {
    let mut pipeline = templates()[1].1.clone();
    let before = pipeline.stages.len();
    let added = pipeline.add_stage(Stage::new("Late arrival", StageKind::Concat));
    let payload = graph_payload(&[], &[("t", pipeline.clone())], &registry());
    let graph = graph_for(&payload, "Ultimate creator preparation");
    let node_ids = ids(graph);

    // No table in this crate mentions "Late arrival". If a hand-kept mapping
    // were the source, this assertion is the one that fails.
    assert_eq!(node_ids.len(), before + 1);
    assert!(node_ids.contains(&added.to_string()));
    assert_eq!(graph["mapping"]["mismatches"], serde_json::json!([]));
}

#[test]
fn a_stage_whose_executor_is_absent_is_not_runnable_and_names_it() {
    let mut pipeline = Pipeline::new("ghost");
    pipeline.add_stage(
        Stage::new("Phantom", StageKind::Custom("ghost.verb".into()))
            .with_dispatch(StageDispatch::new("no-such-plugin", "transcribe")),
    );
    let payload = graph_payload(&[], &[("t", pipeline)], &registry());
    let node = &graph_for(&payload, "ghost")["nodes"][0];

    assert_eq!(node["runnable"], serde_json::json!(false));
    assert_eq!(
        node["missing"],
        serde_json::json!(["dispatch plugin no-such-plugin"])
    );
    // Not runnable means no capabilities are claimed for it: a node must not
    // advertise what its absent executor would have provided.
    assert_eq!(node["capabilities"], serde_json::json!([]));
}

#[test]
fn a_stage_with_no_dispatch_is_not_runnable() {
    let mut pipeline = Pipeline::new("source");
    pipeline.add_stage(Stage::new("Source", StageKind::Concat));
    let payload = graph_payload(&[], &[("t", pipeline)], &registry());
    let node = &graph_for(&payload, "source")["nodes"][0];
    assert_eq!(node["runnable"], serde_json::json!(false));
    assert_eq!(node["missing"], serde_json::json!(["no dispatch target"]));
}

#[test]
fn a_registered_executor_is_runnable_and_reports_the_capabilities_it_declares() {
    let templates = templates();
    let payload = graph_payload(&[], &templates, &registry());
    let nodes = graph_for(&payload, "Ultimate creator preparation")["nodes"]
        .as_array()
        .cloned()
        .expect("nodes");

    let by_dispatch = |plugin: &str| {
        nodes
            .iter()
            .find(|n| n["dispatch"]["plugin"] == plugin)
            .unwrap_or_else(|| panic!("no node dispatched to {plugin}"))
    };

    let crunchr = by_dispatch("crunchr");
    assert_eq!(crunchr["runnable"], serde_json::json!(true));
    assert_eq!(crunchr["missing"], serde_json::json!([]));
    assert_eq!(
        crunchr["capabilities"],
        // Sorted, because the registry indexes by capability tag and a
        // BTreeMap is the order it hands back. Declaration order is not a
        // fact the registry has.
        serde_json::json!([
            "diarisation",
            "topic_segmentation",
            "transcription",
            "word_timestamps"
        ])
    );

    let artifacts = by_dispatch("artifacts");
    assert_eq!(artifacts["runnable"], serde_json::json!(true));
    // Read from the registry's own index, so this is the executor's answer
    // and not a restatement of the plugin's source list.
    assert_eq!(artifacts["capabilities"].as_array().unwrap().len(), 9);
}

#[test]
fn the_job_table_and_the_executor_registry_are_both_reported_and_never_merged() {
    let mut pipeline = Pipeline::new("disagreement");
    pipeline.add_stage(
        Stage::new(
            "Done then unregistered",
            StageKind::Custom("artifacts.clips".into()),
        )
        .with_dispatch(StageDispatch::new("artifacts", "clips")),
    );
    pipeline.stages[0].state = strivo_core::pipeline::stage::StageState::Done;
    pipeline.stages[0].artifacts = vec![serde_json::json!({ "path": "/tmp/a.md" })];
    pipeline.stages[0].attempts = 1;
    let payload = graph_payload(&[], &[("t", pipeline)], &registry());
    let node = &graph_for(&payload, "disagreement")["nodes"][0];

    // The job table says Done with an artifact; the registry says runnable.
    // Both are on the node, and neither is allowed to overwrite the other.
    assert_eq!(node["state"], serde_json::json!("Done"));
    assert_eq!(node["artifacts"], serde_json::json!(1));
    assert_eq!(node["attempts"], serde_json::json!(1));
    assert_eq!(node["runnable"], serde_json::json!(true));

    // Now the other direction: the same stage's executor is gone, while the
    // job table still records it as finished. The disagreement is visible
    // because `state` and `runnable` are independent fields.
    let mut without = Pipeline::new("disagreement");
    without.add_stage(
        Stage::new(
            "Done then unregistered",
            StageKind::Custom("artifacts.clips".into()),
        )
        .with_dispatch(StageDispatch::new("artifacts", "clips")),
    );
    without.stages[0].state = strivo_core::pipeline::stage::StageState::Done;
    let payload = graph_payload(&[], &[("t", without)], &PluginRegistry::new());
    let node = &graph_for(&payload, "disagreement")["nodes"][0];
    assert_eq!(node["state"], serde_json::json!("Done"));
    assert_eq!(node["runnable"], serde_json::json!(false));
}

#[test]
fn an_empty_pipeline_yields_no_nodes_and_is_not_runnable() {
    let payload = graph_payload(&[], &[("t", Pipeline::new("empty"))], &registry());
    let graph = graph_for(&payload, "empty");
    // `[]` nodes is the honest answer for an empty stage list. It is NOT the
    // same answer as a pipeline whose capabilities are unresolved, and the
    // two must not collapse into one another.
    assert_eq!(graph["nodes"], serde_json::json!([]));
    assert_eq!(graph["edges"], serde_json::json!([]));
    assert_eq!(graph["runnable"], serde_json::json!(false));
    assert_eq!(graph["mapping"]["mismatches"], serde_json::json!([]));
}

#[test]
fn a_run_carries_its_own_state_apart_from_the_templates() {
    let mut run = Pipeline::new("Actual run").with_trigger("recording-finished");
    run.state = strivo_core::pipeline::stage::PipelineState::Running;
    run.stages.push(
        Stage::new("Working", StageKind::Custom("artifacts.clips".into()))
            .with_dispatch(StageDispatch::new("artifacts", "clips")),
    );
    run.stages[0].state = strivo_core::pipeline::stage::StageState::Running { started_at_ms: 12 };

    let payload = graph_payload(&[run.clone()], &templates(), &registry());
    let graph = graph_for(&payload, "Actual run");
    assert_eq!(graph["kind"], serde_json::json!("run"));
    assert_eq!(graph["run_id"], serde_json::json!(run.id.to_string()));
    assert_eq!(graph["state"], serde_json::json!("Running"));
    assert_eq!(graph["nodes"][0]["state"], serde_json::json!("Running"));
    // The template of the same shape is a different graph, not a merge.
    let publish = graph_for(&payload, "Ultimate creator preparation");
    assert_eq!(publish["kind"], serde_json::json!("template"));
    assert_eq!(publish["nodes"][0]["state"], serde_json::json!("Pending"));
}

#[test]
fn the_totals_count_runnable_and_not_runnable_separately() {
    let templates = templates();
    let payload = graph_payload(&[], &templates, &registry());
    let totals = &payload["totals"];
    assert_eq!(totals["graphs"], serde_json::json!(2));
    assert_eq!(totals["nodes"], serde_json::json!(11));
    assert_eq!(totals["edges"], serde_json::json!(16));
    assert_eq!(totals["runnable"], serde_json::json!(11));
    assert_eq!(totals["not_runnable"], serde_json::json!(0));
    // The templates as shipped resolve every stage's dispatch. Todo 6's 9/5
    // is a different question over a different model — see the notepad.
    assert_eq!(
        templates[1].1.stages.len(),
        totals["nodes"].as_u64().unwrap() as usize - 1
    );
}

#[test]
fn resource_locks_are_named_and_a_zero_capacity_is_visible() {
    let mut pipeline = Pipeline::new("locks");
    pipeline.add_stage(
        Stage::new("Heavy", StageKind::Custom("crunchr.process".into()))
            .with_dispatch(StageDispatch::new("crunchr", "transcribe"))
            .with_requires(vec![
                strivo_core::pipeline::stage::ResourceLock::Gpu,
                strivo_core::pipeline::stage::ResourceLock::Cpu { cap: 2 },
                strivo_core::pipeline::stage::ResourceLock::Api {
                    name: "openai".into(),
                    cap: 1,
                },
            ]),
    );
    let payload = graph_payload(&[], &[("t", pipeline)], &registry());
    let node = &graph_for(&payload, "locks")["nodes"][0];
    assert_eq!(
        node["requires"],
        serde_json::json!(["gpu", "cpu:2", "api:openai:1"])
    );
}
