//! Experiment registry — enumerates the `strivo_dataviz::Experiment`
//! variants a caller (the SPA's Data Viz hub, or any other API consumer)
//! can run, with a human label and the params each one takes.
//!
//! Before this module existed, the SPA hardcoded the six `Experiment`
//! variant names (`DATAVIZ_EXPERIMENTS` in `assets/spa.js`) because there
//! was no discoverable list. `GET /api/v1/dataviz/experiments`
//! (`routes::plugins::dataviz_experiments_list`) serves [`list_experiments`]
//! so that hardcoded list can be replaced with a fetch.
//!
//! Pure/no-IO, same contract as `crate::corpus` — this module only
//! describes what `strivo_dataviz::run` accepts, it never hydrates or
//! runs anything itself.

use serde::Serialize;
use strivo_dataviz::Experiment;

/// One parameter an experiment accepts. `kind` is a caller-facing type
/// hint ("usize" today); kept as a string so a new param type doesn't
/// need a registry format bump.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ExperimentParam {
    pub name: String,
    pub kind: String,
    pub default: serde_json::Value,
}

/// One row in the experiment catalog: the machine `kind` (the exact
/// string `strivo_dataviz::run`'s `Experiment` JSON tag expects), a
/// human label for the picker, and any params the caller must supply
/// alongside `kind` to build a valid `Experiment` value.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ExperimentDescriptor {
    pub kind: String,
    pub label: String,
    pub params: Vec<ExperimentParam>,
}

/// List every experiment `strivo_dataviz::run` supports.
///
/// Each entry's `kind` is read back off a real `Experiment` value via
/// serde (not hand-copied as a literal string) — if `Experiment`'s
/// `#[serde(tag = "kind", rename_all = "snake_case")]` output ever
/// changes, this list changes with it instead of silently drifting from
/// what `POST /api/v1/dataviz/run` actually accepts. Order matches the
/// `Experiment` enum's declaration order.
pub fn list_experiments() -> Vec<ExperimentDescriptor> {
    let entries: Vec<(Experiment, &str, Vec<ExperimentParam>)> = vec![
        (
            Experiment::WordFrequency { top_n: 30 },
            "Top words",
            vec![ExperimentParam {
                name: "top_n".into(),
                kind: "usize".into(),
                default: serde_json::json!(30),
            }],
        ),
        (Experiment::SpeakerTime, "Speaker minutes", vec![]),
        (
            Experiment::EpisodesPerMonth,
            "Episodes per month",
            vec![],
        ),
        (
            Experiment::SpeakerEpisodeCount,
            "Speaker appearances",
            vec![],
        ),
        (
            Experiment::EpisodeDurations,
            "Episode durations",
            vec![],
        ),
        (
            Experiment::SpeakerCooccurrence,
            "Speaker co-occurrence",
            vec![],
        ),
    ];
    entries
        .into_iter()
        .map(|(exp, label, params)| {
            let kind = experiment_kind(&exp);
            ExperimentDescriptor {
                kind,
                label: label.to_string(),
                params,
            }
        })
        .collect()
}

/// Read the serde `kind` tag off a real `Experiment` value.
fn experiment_kind(exp: &Experiment) -> String {
    let value = serde_json::to_value(exp).expect("Experiment always serializes");
    value
        .get("kind")
        .and_then(|k| k.as_str())
        .expect("Experiment's serde tag is always named \"kind\"")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn experiment_registry_lists_all_six_expected_kinds() {
        let kinds: Vec<String> = list_experiments().into_iter().map(|e| e.kind).collect();
        assert_eq!(
            kinds,
            vec![
                "word_frequency",
                "speaker_time",
                "episodes_per_month",
                "speaker_episode_count",
                "episode_durations",
                "speaker_cooccurrence",
            ]
        );
    }

    #[test]
    fn experiment_registry_kind_strings_round_trip_into_real_experiment_variants() {
        // Every descriptor's `kind` (+ its params' defaults) must
        // deserialize back into a live `Experiment` value — proves the
        // registry describes exactly what `strivo_dataviz::run` accepts,
        // not a stale/hand-typed copy of it.
        for desc in list_experiments() {
            let mut obj = serde_json::Map::new();
            obj.insert("kind".to_string(), serde_json::json!(desc.kind));
            for p in &desc.params {
                obj.insert(p.name.clone(), p.default.clone());
            }
            let parsed: Result<Experiment, _> =
                serde_json::from_value(serde_json::Value::Object(obj));
            assert!(
                parsed.is_ok(),
                "descriptor {:?} did not round-trip into an Experiment: {:?}",
                desc,
                parsed.err()
            );
        }
    }

    #[test]
    fn experiment_registry_word_frequency_carries_its_top_n_param() {
        let wf = list_experiments()
            .into_iter()
            .find(|e| e.kind == "word_frequency")
            .expect("word_frequency must be listed");
        assert_eq!(wf.params.len(), 1);
        assert_eq!(wf.params[0].name, "top_n");
    }
}
