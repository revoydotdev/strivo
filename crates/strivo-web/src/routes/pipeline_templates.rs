//! Authored pipeline templates: the CRUD an editor in the browser writes to.
//!
//! A stored template is a real [`Pipeline`] — the same type the daemon
//! submits and the same type `routes::pipeline_graph` draws — so an authored
//! graph cannot describe a model the executor does not have. Round-tripping
//! is exact because the whole value is serialised: a reload reads back the
//! bytes that were written, not a projection of them.
//!
//! The save path runs the executor's own six rules
//! ([`PipelineRegistry::validation_errors`]) plus the seventh the executor
//! cannot express ([`authoring_violations`]) and refuses on any of them. That
//! ordering is the todo's explicit *Must NOT*: the browser is the first line
//! of defence, this is the second, and `PipelineRegistry::validate` remains
//! the backstop behind a caller with no editor in front of it.
//!
//! Concurrency is todo 49's mechanism, not a second one: the save names the
//! [`StoredTemplate::revision`] it was built on, and a save whose base is no
//! longer the stored revision is **reported** as [`SaveRefusal::Conflict`]
//! rather than resolved last-write-wins. Two clients hold different edits and
//! only the caller can say which survives.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use strivo_core::pipeline::executor::PipelineRegistry;
use strivo_core::pipeline::stage::{Pipeline, RuleViolation};
use strivo_core::plugin::registry::PluginRegistry;

use super::pipeline_graph::{authoring_violations, registered_plugin_names, RULE_UNKNOWN_DISPATCH};

/// The stable token a route reports for a refused save, so a client branches
/// on the class without matching on prose. `history/conflict` is todo 49's
/// own code for the same condition, reused verbatim: one conflict vocabulary,
/// not one per surface.
pub const CODE_INVALID: &str = "graph/invalid";
pub const CODE_CONFLICT: &str = "history/conflict";

pub fn templates_dir() -> PathBuf {
    strivo_core::config::AppConfig::data_dir()
        .join("pipelines")
        .join("templates")
}

/// `None` for an id that could escape the directory or is empty. The charset
/// is the one `chain_path` has always accepted.
pub fn template_path(id: &str) -> Option<PathBuf> {
    if id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return None;
    }
    Some(templates_dir().join(format!("{id}.json")))
}

/// A content revision of the stored graph, not a counter. Two clients that
/// saved the same bytes hold the same revision, so a save that changes
/// nothing is not a conflict; and there is no counter to keep in step with
/// the file.
pub fn revision_of(pipeline: &Pipeline) -> String {
    let mut hasher = DefaultHasher::new();
    // Hash the serialised value, not the struct: `Pipeline` holds an `Instant`
    // that serde skips, so two values that serialise identically are the same
    // graph as far as anything downstream can observe.
    serde_json::to_string(pipeline)
        .unwrap_or_default()
        .hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredTemplate {
    pub id: String,
    pub revision: String,
    pub pipeline: Pipeline,
}

/// Everything a refused save carries, with the fact that caused it.
#[derive(Debug)]
pub enum SaveRefusal {
    /// At least one of the seven rules. Every one of them, not the first:
    /// a user fixing a graph one error per save fixes it seven times.
    Invalid {
        errors: Vec<RuleViolation>,
    },
    Conflict {
        base: String,
        current: String,
    },
}

impl SaveRefusal {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Invalid { .. } => CODE_INVALID,
            Self::Conflict { .. } => CODE_CONFLICT,
        }
    }

    pub fn body(&self) -> Value {
        match self {
            Self::Invalid { errors } => json!({
                "code": CODE_INVALID,
                "title": "Unprocessable Content",
                "status": 400,
                "detail": errors.first().map_or_else(
                    || "the graph breaks a validation rule".to_string(),
                    |e| e.detail.clone().unwrap_or_else(|| e.reason.to_string()),
                ),
                "errors": errors,
            }),
            Self::Conflict { base, current } => json!({
                "code": CODE_CONFLICT,
                "title": "Conflict",
                "status": 409,
                "detail": format!(
                    "this edit was made against revision {base}, but {current} is now current"
                ),
                "base": base,
                "current": current,
            }),
        }
    }
}

/// Every rule an editor must enforce before a graph can be saved: the
/// executor's six, then the registry one. Shipped in the payload so the
/// browser renders labels by id and the ids have one source.
pub fn rule_ids() -> Vec<&'static str> {
    let mut ids: Vec<&'static str> = PipelineRegistry::RULE_IDS.to_vec();
    ids.push(RULE_UNKNOWN_DISPATCH);
    ids
}

/// The authoring payload: every template — shipped and authored — in the
/// exact shape a save takes back, plus the registry's plugin names and the
/// rule ids.
///
/// `registry.plugins` is [`registered_plugin_names`], the same list the node
/// verdict is resolved against. The browser matches a dispatch against it and
/// never decides for itself, so "can this run" has one answer and the client
/// is quoting it.
pub fn authoring_payload(shipped: &[(&str, Pipeline)], registry: &PluginRegistry) -> Value {
    let mut templates: Vec<Value> = Vec::new();
    for (id, pipeline) in shipped {
        templates.push(json!({
            "id": id,
            "source": "shipped",
            "revision": revision_of(pipeline),
            "pipeline": pipeline,
        }));
    }
    let (authored, unreadable) = authored_templates();
    for stored in &authored {
        templates.push(json!({
            "id": stored.id,
            "source": "authored",
            "revision": stored.revision,
            "pipeline": stored.pipeline,
        }));
    }
    json!({
        "templates": templates,
        "registry": { "plugins": registered_plugin_names(registry) },
        "rules": rule_ids(),
        "unreadable": unreadable,
    })
}

/// Every stored template, plus the files that could not be read.
///
/// A corrupt file is **reported, not skipped**: a template that silently
/// vanishes from the editor is a template the user believes they saved.
fn authored_templates() -> (Vec<StoredTemplate>, Vec<String>) {
    let mut out = Vec::new();
    let mut unreadable = Vec::new();
    let Ok(rd) = std::fs::read_dir(templates_dir()) else {
        return (out, unreadable);
    };
    for entry in rd.flatten() {
        let path = entry.path();
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        match std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|s| serde_json::from_str::<StoredTemplate>(&s).map_err(|e| e.to_string()))
        {
            Ok(stored) => out.push(stored),
            Err(e) => unreadable.push(format!("{name}: {e}")),
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    (out, unreadable)
}

/// Every rule a graph must break-free before it can be **saved or run**: the
/// executor's six, then the registry one.
///
/// One function, because "valid" must not have two answers. A graph that
/// `save` refuses and a graph the run route submits are the same question, and
/// a second copy of this check is exactly the drift todo 53 closed. The run
/// route calls it too — with no editor in front of the caller, this is the
/// backstop that `PipelineRegistry::validate` cannot be, because the executor
/// has no registry and so cannot express rule 7.
pub fn graph_errors(pipeline: &Pipeline, registry: &PluginRegistry) -> Vec<RuleViolation> {
    let mut errors = PipelineRegistry::validation_errors(pipeline);
    errors.extend(authoring_violations(pipeline, registry));
    errors
}

/// Save `pipeline` as template `id`, on top of `base`.
///
/// `base` is `None` for a first save. A save whose `base` is not the stored
/// revision is a [`SaveRefusal::Conflict`] and nothing is written: the two
/// clients hold different edits and the server cannot say which survives.
pub fn save(
    id: &str,
    base: Option<&str>,
    pipeline: &Pipeline,
    registry: &PluginRegistry,
) -> Result<StoredTemplate, SaveRefusal> {
    let path = template_path(id).ok_or_else(|| SaveRefusal::Invalid {
        errors: vec![RuleViolation {
            rule: "template-id",
            reason: "template id must be alphanumeric, dash or underscore",
            detail: None,
        }],
    })?;

    // Validation before the conflict check, and before any write: a save that
    // breaks a rule is refused on its own merits, and it never reaches the
    // executor in either order.
    let errors = graph_errors(pipeline, registry);
    if !errors.is_empty() {
        return Err(SaveRefusal::Invalid { errors });
    }

    let current = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str::<StoredTemplate>(&s).ok())
        .map(|stored| stored.revision);
    match (base, current.as_deref()) {
        (None, Some(_)) => {
            return Err(SaveRefusal::Conflict {
                base: "new".into(),
                current: current.unwrap_or_default(),
            })
        }
        (Some(base), Some(current)) if base != current => {
            return Err(SaveRefusal::Conflict {
                base: base.to_string(),
                current: current.to_string(),
            })
        }
        (Some(_), None) => {
            return Err(SaveRefusal::Conflict {
                base: base.unwrap_or_default().to_string(),
                current: String::new(),
            })
        }
        _ => {}
    }

    let stored = StoredTemplate {
        id: id.to_string(),
        revision: revision_of(pipeline),
        pipeline: pipeline.clone(),
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| SaveRefusal::Invalid {
            errors: vec![RuleViolation {
                rule: "storage",
                reason: "could not create the template directory",
                detail: Some(e.to_string()),
            }],
        })?;
    }
    let body = serde_json::to_string_pretty(&stored).map_err(|e| SaveRefusal::Invalid {
        errors: vec![RuleViolation {
            rule: "storage",
            reason: "could not serialise the template",
            detail: Some(e.to_string()),
        }],
    })?;
    std::fs::write(&path, body).map_err(|e| SaveRefusal::Invalid {
        errors: vec![RuleViolation {
            rule: "storage",
            reason: "could not write the template",
            detail: Some(e.to_string()),
        }],
    })?;
    Ok(stored)
}
