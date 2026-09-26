//! The audio mixer: one surface over the primitives, feeding one plan.
//!
//! Report item E04. Mute, solo, gain, pan, source and master routing, ducking,
//! cleanup, fades and normalisation have to **agree between preview and
//! render**, and the only way to make that structural rather than a convention
//! is for every one of them to arrive as fields of the single
//! [`RenderRequest`](strivo_editor::render::RenderRequest) that todo 23's
//! compiler consumes. This module is that surface. It spells no ffmpeg of its
//! own: each control is projected onto a model some other crate already owns —
//! [`strivo_insert_fx::InsertChain`], [`strivo_submix::SubMix`],
//! [`strivo_automation::VolumeAutomation`], [`strivo_loudness::LoudnormTarget`],
//! and [`strivo_deadair`]'s calibration constants — and the fragment those
//! crates emit is what reaches the compiler.
//!
//! ## The three rules that are decided here rather than downstream
//!
//! 1. **Mute beats solo.** [`MixerTrack::muted`] silences a track even when it
//!    is soloed, and one solo silences everything not soloed. The compiler
//!    resolves that into a single `audible` field on the plan, so the preview
//!    and the executor cannot each pick a convention.
//! 2. **A master effect is not a pre-master tap.** A master chain runs on the
//!    sum, in the whole-programme chain. A per-bus tap runs on that bus's own
//!    signal, in the clip's sub-render, before the sum. The compiler refuses a
//!    tap declared in the programme chain
//!    ([`strivo_editor::render::RenderError::TapInProgrammeChain`]) rather than
//!    applying a voice's chain to the game audio and reporting success.
//! 3. **Normalisation is measure-then-apply.** It is projected onto
//!    [`LoudnessSpec`](strivo_editor::render::LoudnessSpec), which the executor
//!    runs as two `loudnorm` passes, and never onto the chain — where it would
//!    be a second, single-pass normalisation after the measured one. The
//!    compiler refuses that too
//!    ([`strivo_editor::render::RenderError::LoudnessAsChainStep`]).
//!
//! ## What is honest about the limit
//!
//! This render is a cut-and-concat of one source's streams, so a mixer track
//! carries **cuts**, not a separate input file. A track's taps reach the audio
//! of the cuts it carries, and the sum of the tracks is the programme. That is
//! enough for the multi-cut case and for a multitrack arrival once todo 34's
//! imported streams are cut list entries; it is **not** two independent inputs
//! summed by `amix`, and [`MixerState::apply`] says so rather than pretending.
//! See `not_met_disclosed` in this todo's evidence.

use serde::{Deserialize, Serialize};
use strivo_editor::render::{EffectKind, EffectStep, LoudnessSpec, RenderRequest, TrackSpec};
use strivo_insert_fx::InsertChain;
use strivo_loudness::LoudnormTarget;
use strivo_multitrack::AudioTrack;
use strivo_sidechain::SidechainKnobs;
use strivo_submix::{SubMix, TrackIn};
use strivo_vad::VoiceInterval;

/// The bus a whole-programme chain runs on. Named rather than implicit, because
/// a track that names a bus is a routing claim the compiler can check.
pub const MASTER: &str = "master";

/// One track's controls, as the mixer stores them.
///
/// A flat record of the buttons rather than a graph: audibility is resolved
/// once by the compiler, so there is nothing here for a caller to pre-compute
/// and nothing for two paths to disagree about.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MixerTrack {
    /// User-visible name, unique within the mixer. The compiler refuses two
    /// tracks under one name, so this is a routing claim rather than a label.
    #[serde(default)]
    pub label: String,
    /// Bus name. A track's taps run on its own signal; the master bus is the
    /// sum every track reaches.
    #[serde(default = "master_bus")]
    pub bus: String,
    /// Linear trim in dB. Not time, so a float is right.
    #[serde(default)]
    pub gain_db: f64,
    /// -1.0 hard left … +1.0 hard right.
    #[serde(default)]
    pub pan: f64,
    #[serde(default)]
    pub muted: bool,
    #[serde(default)]
    pub solo: bool,
    /// This track's own chain — a pre-master tap, applied inside the clip's
    /// sub-render before the sum. Empty means no chain.
    #[serde(default)]
    pub insert_fx: Option<InsertChain>,
    /// Which cuts this track carries, as indices into the revision's cut list.
    /// Empty means every cut, which is the one-track case and the default a
    /// freshly seeded mixer has.
    #[serde(default)]
    pub clips: Vec<usize>,
}

fn master_bus() -> String {
    MASTER.to_string()
}

impl Default for MixerTrack {
    /// Hand-written rather than derived because the bus defaults to
    /// [`MASTER`], and a derived default would hand every `..Default::default()`
    /// track an empty bus name that no compiler check could explain.
    fn default() -> Self {
        Self {
            label: String::new(),
            bus: master_bus(),
            gain_db: 0.0,
            pan: 0.0,
            muted: false,
            solo: false,
            insert_fx: None,
            clips: Vec::new(),
        }
    }
}

/// Ducking: a sidechain envelope over the programme.
///
/// The trigger arrives as voice intervals rather than as a second signal
/// because that is what [`strivo_vad`] produces and what
/// [`strivo_sidechain::build_automation`] consumes — the crate pair already
/// hands off, and re-deriving the envelope here would be a second sidechain.
///
/// No `PartialEq`: `SidechainKnobs` does not derive it, and a hand-written
/// comparison for two saved mixers would be a second answer to "are these the
/// same" that the derived one over the serialised bytes already gives.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ducking {
    /// The intervals the key signal occupies. A recording with no detected
    /// speech is a resting curve, which is the honest answer and not an error.
    #[serde(default)]
    pub trigger: Vec<VoiceInterval>,
    #[serde(default)]
    pub knobs: SidechainKnobs,
}

impl Ducking {
    /// The gain curve this ducking produces, in `strivo-automation`'s own model.
    ///
    /// `total_duration_secs` is the composition's own output length, so a
    /// trigger interval past the end of the cut list is clamped by the
    /// sidechain crate rather than leaving the bus attenuated past the render.
    #[must_use]
    pub fn curve(&self, total_duration_secs: f32) -> strivo_automation::VolumeAutomation {
        strivo_sidechain::build_automation(&self.trigger, total_duration_secs, &self.knobs)
    }
}

/// Silence cleanup on a bus.
///
/// The thresholds are this struct's fields rather than constants, because they
/// are room-dependent: a studio's noise floor and a streamer's are not the same
/// number, and a hard-coded one is a knob nobody can turn. The defaults are
/// `strivo-deadair`'s, which is the crate that already owns the vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Cleanup {
    /// Noise floor in dBFS. `strivo_deadair::DEFAULT_NOISE_DB`.
    #[serde(default = "default_noise_db")]
    pub noise_db: f32,
    /// Shortest run treated as silence, seconds. `strivo_deadair::DEFAULT_MIN_SPAN_SECS`.
    #[serde(default = "default_min_span_secs")]
    pub min_span_secs: f32,
}

fn default_noise_db() -> f32 {
    strivo_deadair::DEFAULT_NOISE_DB
}

fn default_min_span_secs() -> f32 {
    strivo_deadair::DEFAULT_MIN_SPAN_SECS
}

impl Default for Cleanup {
    fn default() -> Self {
        Self {
            noise_db: default_noise_db(),
            min_span_secs: default_min_span_secs(),
        }
    }
}

impl Cleanup {
    /// The filter a cleanup stage is. `silenceremove` is the same detector
    /// `strivo-deadair` parses, running as a filter rather than as an edit: a
    /// mixer control that deleted audio would be an edit, and an edit that
    /// changed the composition length would not belong here.
    #[must_use]
    pub fn to_filter(&self) -> String {
        format!(
            "silenceremove=start_periods=1:start_duration={:.3}:start_threshold={:.1}dB:\
             stop_periods=-1:stop_duration={:.3}:stop_threshold={:.1}dB",
            self.min_span_secs, self.noise_db, self.min_span_secs, self.noise_db
        )
    }
}

/// The saved mixer for one recording.
///
/// Every field is optional-with-a-default, so a mixer sidecar written by an
/// older build deserialises and a partially filled one is not a parse error.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MixerState {
    #[serde(default)]
    pub tracks: Vec<MixerTrack>,
    /// Runs once on the sum, after every track has been mixed.
    #[serde(default)]
    pub master_chain: Option<InsertChain>,
    /// Broadcast trim on the sum, in dB.
    #[serde(default)]
    pub master_gain_db: f64,
    #[serde(default)]
    pub ducking: Option<Ducking>,
    #[serde(default)]
    pub cleanup: Option<Cleanup>,
    /// An EBU R128 target. Measure-then-apply; see the module doc.
    #[serde(default)]
    pub normalise: Option<LoudnormTarget>,
}

impl MixerState {
    /// Seed a mixer from a multitrack recording's probed audio streams.
    ///
    /// `strivo-multitrack` already identified each stream's role from the
    /// recorder's own title tag, so this reuses that rather than asking the
    /// user to re-type what the file already said. Two OBS conventions are
    /// honoured because they are conventions every OBS recording follows: the
    /// downmixed track is kept as a track of its own rather than dropped as a
    /// duplicate of the sum, and stems with no title fall to their inferred
    /// kind's name so two unnamed tracks cannot collide.
    #[must_use]
    pub fn from_multitrack(stems: &[AudioTrack]) -> Self {
        let mut tracks: Vec<MixerTrack> = Vec::with_capacity(stems.len());
        for (position, stem) in stems.iter().enumerate() {
            let label = stem
                .title
                .clone()
                .filter(|title| !title.trim().is_empty())
                .unwrap_or_else(|| {
                    let kind = stem.inferred_kind.as_str();
                    if kind == "other" {
                        format!("track-{position}")
                    } else {
                        kind.to_string()
                    }
                });
            tracks.push(MixerTrack {
                label,
                ..MixerTrack::default()
            });
        }
        Self {
            tracks,
            ..Self::default()
        }
    }

    /// The tracks as the request's `TrackSpec`s, in a stable order.
    ///
    /// Sorted by label because the compiler sorts tracks by label too and a
    /// plan whose order depended on a sidecar's array order would hash
    /// differently for the same mix. `cut_count` resolves the "empty means
    /// every cut" default.
    #[must_use]
    pub fn track_specs(&self, cut_count: usize) -> Vec<TrackSpec> {
        let mut specs: Vec<TrackSpec> = self
            .tracks
            .iter()
            .map(|track| TrackSpec {
                label: track.label.clone(),
                bus: track.bus.clone(),
                gain_db: track.gain_db,
                pan: track.pan,
                muted: track.muted,
                solo: track.solo,
                insert: track
                    .insert_fx
                    .as_ref()
                    .map(|chain| chain.to_filter())
                    .unwrap_or_default(),
                clips: if track.clips.is_empty() {
                    (0..cut_count).collect()
                } else {
                    track.clips.clone()
                },
            })
            .collect();
        specs.sort_by(|a, b| a.label.cmp(&b.label));
        specs
    }

    /// The master chain and trim, as one whole-programme stage.
    ///
    /// Projected through `strivo-submix`'s own single-input projection rather
    /// than by spelling `alimiter,volume` here, so the master chain's spelling
    /// has exactly one implementation — the one the sub-mix crate's tests pin.
    /// The one synthetic track carries no gain and no chain: the sum is already
    /// the signal, and a track gain at this point would be a second place to
    /// trim the programme.
    #[must_use]
    pub fn master_filter(&self) -> Option<String> {
        let bus = SubMix {
            tracks: vec![TrackIn {
                label: MASTER.to_string(),
                input_index: 0,
                insert_fx: None,
                gain_db: 0.0,
            }],
            master_chain: self.master_chain.clone(),
            master_gain_db: self.master_gain_db,
        };
        bus.to_single_input_chain()
    }

    /// The audio stages this mixer contributes, in no particular order — the
    /// compiler is what fixes the order, and a caller that pre-sorted them
    /// would be a second answer to "what order do these run in".
    #[must_use]
    pub fn effect_steps(&self, total_duration_secs: f32) -> Vec<EffectStep> {
        let mut steps = Vec::new();
        if let Some(cleanup) = &self.cleanup {
            steps.push(EffectStep {
                id: "mixer-cleanup".into(),
                kind: EffectKind::InsertFx,
                filter: cleanup.to_filter(),
                arity: 1,
                bus: None,
            });
        }
        if let Some(bus) = self.master_filter() {
            steps.push(EffectStep {
                id: "mixer-master".into(),
                kind: EffectKind::SubMix,
                filter: bus,
                arity: 1,
                bus: None,
            });
        }
        if let Some(ducking) = &self.ducking {
            let curve = ducking.curve(total_duration_secs);
            // "No points" is the wrong test for a resting curve and the
            // sidechain crate's own tests show why: with no trigger it still
            // anchors 0 dB at 0 and at the end, so the point list is non-empty
            // and the stage is a real filter asking ffmpeg to hold unity for the
            // whole programme. A curve that never leaves 0 dB is the no-op.
            if curve.points.iter().any(|point| point.gain_db.abs() > 1e-6) {
                steps.push(EffectStep {
                    id: "mixer-ducking".into(),
                    kind: EffectKind::VolumeAutomation,
                    filter: curve.build_audio_filter(),
                    arity: 1,
                    bus: None,
                });
            }
        }
        steps
    }

    /// Project onto a request, replacing its audio surface.
    ///
    /// `request.tracks` becomes the mixer's tracks and `request.effects` gains
    /// the mixer's stages **after** the ones already there, so a mixer and a
    /// pitch setting coexist with the stage order the compiler fixes rather
    /// than with a merge order this function invents. `request.loudness` is the
    /// mixer's target when it declares one and is left alone when it does not.
    ///
    /// A mixer with no tracks is left entirely alone: an empty mixer is a
    /// recording nobody has opened, and projecting it would replace a working
    /// programme track with nothing.
    pub fn apply(&self, request: &mut RenderRequest) {
        if self.tracks.is_empty() {
            return;
        }
        request.tracks = self.track_specs(request.clips.len());
        let total = request.output_duration_hint().unwrap_or(0.0);
        request.effects.extend(self.effect_steps(total));
        if let Some(target) = self.normalise {
            request.loudness = Some(LoudnessSpec {
                platform: "mixer".into(),
                i: target.i,
                lra: target.lra,
                tp: target.tp,
            });
        }
    }
}

/// The length of a request's composition, in whole seconds, for the primitives
/// that still work in seconds (`strivo-automation` and `strivo-sidechain`).
///
/// Derived by summing the placed clips rather than by asking the caller: the
/// mixer has no other way to know how long the render is, and a ducking curve
/// built against a guessed length is a curve that lands at the wrong time.
trait OutputDurationHint {
    fn output_duration_hint(&self) -> Option<f32>;
}

impl OutputDurationHint for RenderRequest {
    fn output_duration_hint(&self) -> Option<f32> {
        let total: i64 = self
            .clips
            .iter()
            .map(|clip| {
                clip.span
                    .range
                    .duration()
                    .ok()
                    .map(|d| d.micros())
                    .unwrap_or(0)
            })
            .sum();
        if total > 0 {
            Some(total as f32 / 1_000_000.0)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mixer_with_one_track() -> MixerState {
        MixerState {
            tracks: vec![MixerTrack {
                gain_db: -6.0,
                ..MixerTrack::default()
            }],
            ..MixerState::default()
        }
    }

    #[test]
    fn an_empty_mixer_changes_nothing() {
        let mut request = strivo_editor::render::example::request();
        let before = request.clone();
        MixerState::default().apply(&mut request);
        assert_eq!(
            request, before,
            "an unopened mixer must not replace a working plan"
        );
    }

    #[test]
    fn one_track_with_no_clips_declared_carries_every_cut() {
        let specs = mixer_with_one_track().track_specs(3);
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].clips, vec![0, 1, 2]);
        assert_eq!(specs[0].gain_db, -6.0);
        assert!(specs[0].insert.is_empty(), "no chain means no filter");
    }

    #[test]
    fn a_track_with_an_explicit_clip_list_routes_only_those() {
        let mixer = MixerState {
            tracks: vec![
                MixerTrack {
                    label: "voice".into(),
                    clips: vec![0],
                    ..MixerTrack::default()
                },
                MixerTrack {
                    label: "game".into(),
                    clips: vec![1],
                    ..MixerTrack::default()
                },
            ],
            ..MixerState::default()
        };
        let specs = mixer.track_specs(2);
        assert_eq!(specs[0].label, "game");
        assert_eq!(specs[0].clips, vec![1]);
        assert_eq!(specs[1].clips, vec![0]);
    }

    #[test]
    fn a_track_tap_reaches_the_track_and_not_the_programme_chain() {
        let mixer = MixerState {
            tracks: vec![MixerTrack {
                insert_fx: Some(InsertChain::new(vec![
                    strivo_insert_fx::InsertEffect::HighPass { freq_hz: 80.0 },
                ])),
                ..MixerTrack::default()
            }],
            ..MixerState::default()
        };
        let specs = mixer.track_specs(1);
        assert!(
            specs[0].insert.contains("highpass"),
            "the tap is on the track"
        );
        assert!(
            mixer.effect_steps(4.0).is_empty(),
            "a tap is not a programme stage: the programme chain runs on the sum"
        );
    }

    #[test]
    fn the_master_chain_and_trim_are_one_programme_stage() {
        let mixer = MixerState {
            tracks: vec![MixerTrack::default()],
            master_chain: Some(InsertChain::new(vec![
                strivo_insert_fx::InsertEffect::Limiter {
                    ceiling_db: -1.0,
                    release_sec: 0.05,
                },
            ])),
            master_gain_db: -1.5,
            ..MixerState::default()
        };
        let steps = mixer.effect_steps(4.0);
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].kind, EffectKind::SubMix);
        assert!(steps[0].filter.contains("alimiter"), "master chain");
        assert!(steps[0].filter.contains("volume=-1.50dB"), "master trim");
        assert!(steps[0].bus.is_none(), "the master chain is not a tap");
    }

    #[test]
    fn ducking_projects_a_sidechain_curve_not_a_new_filter_spelling() {
        let mixer = MixerState {
            tracks: vec![MixerTrack::default()],
            ducking: Some(Ducking {
                trigger: vec![VoiceInterval {
                    start_sec: 1.0,
                    end_sec: 2.0,
                    mean_db: -15.0,
                }],
                knobs: SidechainKnobs {
                    duck_db: -12.0,
                    ..SidechainKnobs::default()
                },
            }),
            ..MixerState::default()
        };
        let steps = mixer.effect_steps(6.0);
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].kind, EffectKind::VolumeAutomation);
        assert!(
            steps[0].filter.starts_with("volume='") && steps[0].filter.ends_with("':eval=frame"),
            "the envelope must reach the render as a per-frame expression: {}",
            steps[0].filter
        );
    }

    #[test]
    fn a_ducking_with_no_detected_speech_contributes_no_stage() {
        let mixer = MixerState {
            tracks: vec![MixerTrack::default()],
            ducking: Some(Ducking {
                trigger: Vec::new(),
                knobs: SidechainKnobs::default(),
            }),
            ..MixerState::default()
        };
        let resting = mixer
            .ducking
            .as_ref()
            .expect("the fixture has ducking")
            .curve(6.0);
        assert!(
            !resting.points.is_empty(),
            "anti-vacuity: the sidechain crate DOES emit points for a resting curve, \
             so a test that passed on an empty point list would be passing for free"
        );
        assert!(
            mixer.effect_steps(6.0).is_empty(),
            "a curve that never leaves unity is not a render cost"
        );
    }

    #[test]
    fn cleanup_carries_deadairs_calibration_rather_than_a_new_default() {
        let spoken = Cleanup::default();
        assert_eq!(spoken.noise_db, strivo_deadair::DEFAULT_NOISE_DB);
        assert_eq!(spoken.min_span_secs, strivo_deadair::DEFAULT_MIN_SPAN_SECS);
        assert!(spoken.to_filter().contains("silenceremove"));
        // The knob is a knob: a noisier room must change the filter, not be
        // silently overridden by the crate's constant.
        let tuned = Cleanup {
            noise_db: -50.0,
            ..Cleanup::default()
        };
        assert!(tuned.to_filter().contains("-50.0dB"));
    }

    #[test]
    fn normalisation_lands_on_the_loudness_target_and_never_on_the_chain() {
        let mixer = MixerState {
            tracks: vec![MixerTrack::default()],
            normalise: Some(LoudnormTarget {
                i: -16.0,
                lra: 11.0,
                tp: -1.5,
            }),
            ..MixerState::default()
        };
        assert!(
            mixer.effect_steps(4.0).is_empty(),
            "normalisation is not a chain stage; it is the executor's second pass"
        );
        let mut request = strivo_editor::render::example::request();
        request.loudness = None;
        mixer.apply(&mut request);
        let target = request.loudness.expect("the target lands on the plan");
        assert_eq!((target.i, target.lra, target.tp), (-16.0, 11.0, -1.5));
        assert!(
            !request
                .effects
                .iter()
                .any(|e| e.kind == EffectKind::Loudness),
            "a loudness stage in the chain would normalise the programme twice"
        );
    }

    #[test]
    fn a_multitrack_recording_seeds_one_track_per_probed_stream() {
        let stems = vec![
            AudioTrack {
                index: 1,
                codec: "aac".into(),
                channels: 2,
                sample_rate: 48_000,
                title: Some("Mic/Aux".into()),
                language: None,
                inferred_kind: strivo_multitrack::TrackKind::Mic,
            },
            AudioTrack {
                index: 2,
                codec: "aac".into(),
                channels: 2,
                sample_rate: 48_000,
                title: None,
                language: None,
                inferred_kind: strivo_multitrack::TrackKind::Game,
            },
        ];
        let mixer = MixerState::from_multitrack(&stems);
        let labels: Vec<&str> = mixer.tracks.iter().map(|t| t.label.as_str()).collect();
        assert_eq!(
            labels,
            vec!["Mic/Aux", "game"],
            "the recorder's own title wins; an untitled stem falls to its inferred kind"
        );
        assert!(mixer.tracks.iter().all(|t| t.bus == MASTER));
    }

    #[test]
    fn two_untitled_stems_of_the_same_kind_do_not_collide() {
        let same = |index| AudioTrack {
            index,
            codec: "aac".into(),
            channels: 2,
            sample_rate: 48_000,
            title: None,
            language: None,
            inferred_kind: strivo_multitrack::TrackKind::Other,
        };
        let mixer = MixerState::from_multitrack(&[same(1), same(2)]);
        let names: Vec<&str> = mixer.tracks.iter().map(|t| t.label.as_str()).collect();
        assert_eq!(names, vec!["track-0", "track-1"]);
    }
}
