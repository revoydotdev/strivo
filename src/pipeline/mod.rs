//! Pipeline DAG engine — host-side orchestrator for multi-stage plugin jobs.
//!
//! Plugins compose a [`Pipeline`] of typed [`Stage`]s (extract / transcribe /
//! diarize / subtitle / analyze / clip / concat / …) and submit it via
//! `PluginAction::SubmitPipeline`. The host executor walks the DAG: stages
//! with all inputs satisfied are dispatched; each owns a `CancellationToken`
//! and declares resource locks (GPU, per-provider rate limits) so concurrent
//! pipelines don't trample each other.
//!
//! The daemon runtime persists the registry, re-queues interrupted work,
//! dispatches dependency-ready stages through plugin capability executors,
//! coordinates resource locks, and emits live state snapshots.
//!
//! See `docs/PIPELINE.md` for the execution contract and growth model.

pub mod executor;
pub mod runtime;
pub mod stage;
pub mod templates;

pub use executor::{PipelineRegistry, ResourceRegistry};
pub use runtime::PipelineRuntime;
pub use stage::{
    Pipeline, PipelineId, PipelineState, Stage, StageDispatch, StageId, StageKind, StageState,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipeline_round_trip() {
        let p = Pipeline::new("test".to_string());
        assert!(matches!(p.state, PipelineState::Pending));
        assert_eq!(p.stages.len(), 0);
    }

    #[test]
    fn stage_topology_acyclic() {
        let mut p = Pipeline::new("x".to_string());
        let a = p.add_stage(Stage::new("a", StageKind::Extract));
        let b = p.add_stage(Stage::new("b", StageKind::Custom("noop".into())).with_inputs(vec![a]));
        assert!(p.assert_acyclic().is_ok());
        // Cycle: introduce a back-edge from a → b → a
        if let Some(s) = p.stages.iter_mut().find(|s| s.id == a) {
            s.inputs.push(b);
        }
        assert!(p.assert_acyclic().is_err());
    }
}
