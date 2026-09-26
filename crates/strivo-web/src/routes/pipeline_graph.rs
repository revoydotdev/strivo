//! The pipeline graph, over the executor's own model.
//!
//! Every node here is one `strivo_core::pipeline::Stage` and every edge is one
//! entry of that stage's `inputs`. Nothing is tabulated: the node list *is*
//! the stage list, walked in order. `crates/pipelines-dag`'s capability-tag
//! `Node`/`Edge` are a different model and are not read here — they remain as
//! documentation, and `/api/v1/pipelines/dag` still serves them unchanged.
//!
//! A node reports **two independent facts and never merges them**:
//!
//! * `state` — what the job table recorded (the `StageState` the executor
//!   wrote). This is history.
//! * `runnable` / `missing` — what *this build* can dispatch, resolved
//!   against the same `PluginRegistry` the executor dispatches through. A
//!   stage the job table records as `Done` whose plugin is no longer
//!   registered is therefore both finished and not runnable, and the graph
//!   shows that rather than reconciling it away.
//!
//! The verdict is deliberately the narrow question "is there a registered
//! plugin with this name", which is the first of `PluginRegistry::execute_stage`
//! 's two gates. The second — `PluginStatus::Ready` — is not usable from a
//! web process: `registered_plugins()` never calls `init_all`, so every
//! plugin there reports `Initializing` and a status gate would report every
//! stage in the product as unrunnable. The per-plugin enable toggle is the
//! executor's runtime state, not a static property of the graph.

use strivo_core::pipeline::stage::{PipelineState, RuleViolation, Stage, StageState};
use strivo_core::pipeline::Pipeline;
use strivo_core::plugin::registry::PluginRegistry;

use serde_json::{json, Value};

/// `(template id, the pipeline it builds)`. The templates are the real
/// `Pipeline` the daemon submits, so a graph exists on a fresh install
/// instead of only after the first run.
pub type TemplateGraph<'a> = &'a [(&'static str, Pipeline)];

/// Graphs for every run and every shipped template, plus totals.
pub fn graph_payload(
    runs: &[Pipeline],
    templates: TemplateGraph<'_>,
    registry: &PluginRegistry,
) -> Value {
    let mut graphs: Vec<Value> = Vec::with_capacity(runs.len() + templates.len());
    for (id, pipeline) in templates {
        graphs.push(graph_value(
            "template",
            &format!("template:{id}"),
            None,
            pipeline,
            registry,
        ));
    }
    for run in runs {
        graphs.push(graph_value(
            "run",
            &format!("run:{}", run.id),
            Some(run),
            run,
            registry,
        ));
    }

    let count = |key: &str| -> u64 {
        graphs
            .iter()
            .filter_map(|g| g["totals"][key].as_u64())
            .sum()
    };
    json!({
        "graphs": graphs,
        "totals": {
            "graphs": graphs.len(),
            "nodes": count("nodes"),
            "edges": count("edges"),
            "runnable": count("runnable"),
            "not_runnable": count("not_runnable"),
        },
    })
}

/// One graph: nodes derived from the stage list, edges derived from `inputs`.
///
/// `run` is `Some` for a real run and `None` for a template. They are the
/// same derivation over the same type, so a template cannot drift from the
/// run it describes — the difference is only which state the stages carry.
fn graph_value(
    kind: &str,
    key: &str,
    run: Option<&Pipeline>,
    pipeline: &Pipeline,
    registry: &PluginRegistry,
) -> Value {
    let nodes: Vec<Value> = pipeline
        .stages
        .iter()
        .map(|stage| node_value(stage, registry))
        .collect();
    let edges: Vec<Value> = pipeline
        .stages
        .iter()
        .flat_map(|stage| {
            stage
                .inputs
                .iter()
                .map(move |input| json!({ "from": input.to_string(), "to": stage.id.to_string() }))
        })
        .collect();

    let runnable_count = nodes
        .iter()
        .filter(|n| n["runnable"] == json!(true))
        .count();
    let nodes_len = nodes.len();
    let inputs_links: usize = pipeline.stages.iter().map(|stage| stage.inputs.len()).sum();
    json!({
        "key": key,
        "kind": kind,
        "run_id": run.map(|r| r.id.to_string()),
        "name": pipeline.name,
        "state": pipeline_state_name(&pipeline.state),
        "trigger": pipeline.trigger,
        "error": pipeline.error,
        "runnable": !nodes.is_empty() && runnable_count == nodes.len(),
        "nodes": nodes,
        "edges": edges,
        "totals": {
            "nodes": nodes_len,
            "edges": edges.len(),
            "runnable": runnable_count,
            "not_runnable": nodes_len - runnable_count,
        },
        "mapping": {
            "stages": pipeline.stages.len(),
            "nodes": nodes.len(),
            "edges": edges.len(),
            "inputs_links": inputs_links,
            "mismatches": mapping_mismatches(&nodes, &edges, pipeline),
        },
    })
}

fn node_value(stage: &Stage, registry: &PluginRegistry) -> Value {
    // The executor's own answer, and the only one: the name the stage's
    // dispatch resolves to, looked up in the registry the executor uses.
    let (plugin, missing) = match stage.dispatch.as_ref() {
        None => (None, json!(["no dispatch target"])),
        Some(dispatch) => {
            if registered_plugin_names(registry).contains(&dispatch.plugin) {
                (Some(dispatch.plugin.as_str()), json!([]))
            } else {
                (
                    None,
                    json!([format!("dispatch plugin {}", dispatch.plugin)]),
                )
            }
        }
    };
    json!({
        "stage_id": stage.id.to_string(),
        "name": stage.name,
        "kind": stage.kind.label(),
        "state": stage_state_name(&stage.state),
        "attempts": stage.attempts,
        "max_attempts": stage.max_attempts,
        "requires": stage.requires.iter().map(|l| l.label()).collect::<Vec<_>>(),
        "cost_cents_estimate": stage.cost_cents_estimate,
        "artifacts": stage.artifacts.len(),
        "fallback_provider": stage.fallback_provider,
        "dispatch": stage.dispatch.as_ref().map(|d| json!({
            "plugin": d.plugin,
            "verb": d.verb,
        })),
        "runnable": missing.as_array().is_some_and(|m| m.is_empty()),
        "missing": missing,
        // Only what the *resolved* executor declares. An absent executor
        // claims nothing, so a node can never advertise capability its
        // provider does not have.
        "capabilities": plugin.map(|p| capabilities_of(registry, p)).unwrap_or_default(),
    })
}

/// Capability tags the registry has indexed against `plugin`. Read from the
/// registry's own index, so this is the executor's answer and not a second
/// transcription of a plugin's `capabilities()`.
fn capabilities_of(registry: &PluginRegistry, plugin: &str) -> Vec<Value> {
    registry
        .capability_map()
        .into_iter()
        .filter(|(_, providers)| providers.contains(&plugin))
        .map(|(capability, _)| json!(capability))
        .collect()
}

/// Bidirectional mapping check, reported in the payload rather than asserted
/// once in a test: the graph is drawn from this JSON by a client, so a client
/// can see that its own invariant is broken instead of rendering a phantom
/// node or silently dropping a stage.
///
/// Both directions, because a table drifts in both: a node with no stage, a
/// stage with no node, an edge no `inputs` link backs, an `inputs` link no
/// edge draws.
pub fn mapping_mismatches(nodes: &[Value], edges: &[Value], pipeline: &Pipeline) -> Vec<String> {
    let stage_ids: Vec<String> = pipeline.stages.iter().map(|s| s.id.to_string()).collect();
    let node_ids: Vec<String> = nodes
        .iter()
        .filter_map(|n| n["stage_id"].as_str().map(str::to_string))
        .collect();
    let links: Vec<(String, String)> = pipeline
        .stages
        .iter()
        .flat_map(|s| {
            s.inputs
                .iter()
                .map(move |i| (i.to_string(), s.id.to_string()))
        })
        .collect();
    let pairs: Vec<(String, String)> = edges
        .iter()
        .map(|e| {
            (
                e["from"].as_str().unwrap_or_default().to_string(),
                e["to"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();

    let mut out: Vec<String> = Vec::new();
    for id in &node_ids {
        if !stage_ids.contains(id) {
            out.push(format!("node {id} maps to no stage"));
        }
    }
    for id in &stage_ids {
        if !node_ids.contains(id) {
            out.push(format!("stage {id} has no node"));
        }
    }
    for (from, to) in &pairs {
        if !links.contains(&(from.clone(), to.clone())) {
            out.push(format!("edge {from}->{to} has no inputs link"));
        }
    }
    for (from, to) in &links {
        if !pairs.contains(&(from.clone(), to.clone())) {
            out.push(format!("inputs link {from}->{to} has no edge"));
        }
    }
    out
}

fn pipeline_state_name(state: &PipelineState) -> &'static str {
    match state {
        PipelineState::Pending => "Pending",
        PipelineState::Running => "Running",
        PipelineState::Done => "Done",
        PipelineState::Failed => "Failed",
        PipelineState::Cancelled => "Cancelled",
    }
}

/// The state a node shows, as one word. The serialised form is externally
/// tagged (`"Pending"`, or `{"Running": {...}}`), so a client that read the
/// enum directly would carry that `typeof === "string"` dance for every node
/// and could get it wrong per node.
fn stage_state_name(state: &StageState) -> &'static str {
    match state {
        StageState::Pending => "Pending",
        StageState::Running { .. } => "Running",
        StageState::Done => "Done",
        StageState::Failed { .. } => "Failed",
        StageState::Exhausted { .. } => "Exhausted",
        StageState::Cancelled => "Cancelled",
        StageState::Skipped => "Skipped",
    }
}

/// The plugin names this build's executor registry answers to — the same
/// registry `PluginRegistry::execute_stage` dispatches through, read once so
/// the graph and the editor cannot resolve a name two different ways.
pub fn registered_plugin_names(registry: &PluginRegistry) -> Vec<String> {
    registry
        .plugin_statuses()
        .into_iter()
        .map(|(name, _, _)| name.to_string())
        .collect()
}

/// The seventh authoring rule, and the one the executor's own `validate`
/// cannot express: `StageKind::Custom` carries an opaque identifier, so
/// `validate` only checks that `dispatch.plugin`/`verb` are non-empty. A
/// graph naming a plugin no build registers therefore passes all six rules,
/// saves, renders as runnable, and fails at dispatch — hours later, on a
/// recording.
///
/// The check is the registry's answer, read off the same
/// [`registered_plugin_names`] the node verdict uses. The browser is handed
/// that list in the payload and matches against it, so the client never
/// becomes a second opinion about what can run.
pub const RULE_UNKNOWN_DISPATCH: &str = "unknown-dispatch-target";

/// Every stage whose dispatch names a plugin this build does not register.
/// A stage with no dispatch at all is not reported: `dispatch: None` is
/// valid for a source or artifact node, and rule 6 already covers a dispatch
/// that is present but empty.
pub fn authoring_violations(pipeline: &Pipeline, registry: &PluginRegistry) -> Vec<RuleViolation> {
    let known = registered_plugin_names(registry);
    pipeline
        .stages
        .iter()
        .filter_map(|stage| {
            let dispatch = stage.dispatch.as_ref()?;
            if known.contains(&dispatch.plugin) {
                return None;
            }
            Some(RuleViolation {
                rule: RULE_UNKNOWN_DISPATCH,
                reason: "pipeline dispatches to a plugin this build does not register",
                detail: Some(format!(
                    "{} dispatches {}·{} — this build registers no such plugin, so it would fail at dispatch",
                    stage.name, dispatch.plugin, dispatch.verb
                )),
            })
        })
        .collect()
}
