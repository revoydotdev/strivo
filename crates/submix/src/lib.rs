//! Sub-mix / bus routing — per-track InsertChain + master InsertChain
//! composing into one ffmpeg `filter_complex` graph.
//!
//! Streamer use: route the mic into a Voice bus that gets the voice
//! insert preset; route the game / music into a Game bus that gets
//! the game insert preset; sum both into the Master bus which gets
//! the broadcast-safe limiter. One mixdown per render.
//!
//! Pure-data: model + filtergraph composer. The host already knows
//! how to splice a `filter_complex` value into the render pipeline
//! via the existing multitrack plugin.

use serde::{Deserialize, Serialize};
use strivo_insert_fx::InsertChain;

/// One input track contributing to the sub-mix.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrackIn {
    /// User-visible label ("voice", "game", "music").
    pub label: String,
    /// ffmpeg input index (e.g. `0` → `[0:a]`).
    pub input_index: u32,
    /// Optional per-track insert chain. Empty / None means the track
    /// passes straight through into the bus.
    #[serde(default)]
    pub insert_fx: Option<InsertChain>,
    /// Bus gain in dB applied after the chain, before the mixer.
    #[serde(default)]
    pub gain_db: f64,
}

/// The whole sub-mix bus model.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SubMix {
    pub tracks: Vec<TrackIn>,
    /// Master insert chain — runs once after the amix.
    #[serde(default)]
    pub master_chain: Option<InsertChain>,
    /// Optional master gain in dB (broadcast trim).
    #[serde(default)]
    pub master_gain_db: f64,
}

impl SubMix {
    /// Compose the full `filter_complex` value the host hands to
    /// ffmpeg. Empty string when no tracks are present so the
    /// render path can skip the `-filter_complex` arg entirely.
    pub fn to_filter_complex(&self) -> String {
        if self.tracks.is_empty() {
            return String::new();
        }
        let mut parts: Vec<String> = vec![];
        let mut bus_labels: Vec<String> = vec![];
        for (i, tr) in self.tracks.iter().enumerate() {
            // Build the per-track chain: ffmpeg filter chain inside
            // the [input]→[bus<i>] segment.
            let mut chain: Vec<String> = vec![];
            if let Some(c) = &tr.insert_fx {
                let s = c.to_filter();
                if !s.is_empty() {
                    chain.push(s);
                }
            }
            if (tr.gain_db).abs() > 1e-6 {
                chain.push(format!("volume={:.2}dB", tr.gain_db));
            }
            let bus = format!("bus{i}");
            if chain.is_empty() {
                parts.push(format!("[{idx}:a]anull[{bus}]", idx = tr.input_index));
            } else {
                parts.push(format!(
                    "[{idx}:a]{filters}[{bus}]",
                    idx = tr.input_index,
                    filters = chain.join(","),
                ));
            }
            bus_labels.push(bus);
        }
        // amix bus0 + bus1 + … → [mix]
        let inputs: String = bus_labels.iter().map(|b| format!("[{b}]")).collect();
        parts.push(format!(
            "{inputs}amix=inputs={n}:normalize=0[mix]",
            n = bus_labels.len()
        ));
        // Master chain + master gain on top.
        let mut master: Vec<String> = vec![];
        if let Some(c) = &self.master_chain {
            let s = c.to_filter();
            if !s.is_empty() {
                master.push(s);
            }
        }
        if (self.master_gain_db).abs() > 1e-6 {
            master.push(format!("volume={:.2}dB", self.master_gain_db));
        }
        if master.is_empty() {
            parts.push("[mix]anull[out]".into());
        } else {
            parts.push(format!("[mix]{}[out]", master.join(",")));
        }
        parts.join(";")
    }

    /// Output pad label the host uses to wire the encoder. Always
    /// `out` so the host can hard-code `-map [out]`.
    pub fn output_pad() -> &'static str {
        "out"
    }

    /// Project the bus onto a **single** input stream, as a `-af` value.
    ///
    /// [`Self::to_filter_complex`] needs one ffmpeg input per track, which a
    /// cut-and-concat render of one recording does not have. This is the same
    /// graph with the mixer collapsed: `amix` of one input at
    /// `normalize=0` is the identity, and an `anull` pad on both sides of it
    /// is a no-op, so for one track the two are the same signal.
    ///
    /// `None` when the bus declares more than one track. That is **not** a
    /// fallback to a partial mix — it is the honest answer, because a second
    /// track needs a second input and this render has none. The caller
    /// surfaces it rather than rendering one bus and calling it the mix.
    /// Multi-track arrivals are the timeline work that adds a second source.
    pub fn to_single_input_chain(&self) -> Option<String> {
        if self.tracks.len() != 1 {
            return None;
        }
        let track = &self.tracks[0];
        let mut chain: Vec<String> = Vec::new();
        if let Some(c) = &track.insert_fx {
            let s = c.to_filter();
            if !s.is_empty() {
                chain.push(s);
            }
        }
        if (track.gain_db).abs() > 1e-6 {
            chain.push(format!("volume={:.2}dB", track.gain_db));
        }
        if let Some(c) = &self.master_chain {
            let s = c.to_filter();
            if !s.is_empty() {
                chain.push(s);
            }
        }
        if (self.master_gain_db).abs() > 1e-6 {
            chain.push(format!("volume={:.2}dB", self.master_gain_db));
        }
        if chain.is_empty() {
            None
        } else {
            Some(chain.join(","))
        }
    }

    /// How many separate signals the full graph mixes.
    #[must_use]
    pub fn signal_count(&self) -> usize {
        self.tracks.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_submix_emits_empty_filtergraph() {
        let s = SubMix::default();
        assert_eq!(s.to_filter_complex(), "");
    }

    #[test]
    fn single_passthrough_track_anulls_and_mixes() {
        let s = SubMix {
            tracks: vec![TrackIn {
                label: "voice".into(),
                input_index: 0,
                insert_fx: None,
                gain_db: 0.0,
            }],
            master_chain: None,
            master_gain_db: 0.0,
        };
        let f = s.to_filter_complex();
        assert!(f.contains("[0:a]anull[bus0]"));
        assert!(f.contains("[bus0]amix=inputs=1:normalize=0[mix]"));
        assert!(f.ends_with("[mix]anull[out]"));
    }

    #[test]
    fn voice_plus_game_buses_amix_and_master_limiter() {
        let s = SubMix {
            tracks: vec![
                TrackIn {
                    label: "voice".into(),
                    input_index: 0,
                    insert_fx: Some(InsertChain::voice_bus_default()),
                    gain_db: 0.0,
                },
                TrackIn {
                    label: "game".into(),
                    input_index: 1,
                    insert_fx: Some(InsertChain::game_bus_default()),
                    gain_db: -3.0,
                },
            ],
            master_chain: Some(InsertChain::new(vec![
                strivo_insert_fx::InsertEffect::Limiter {
                    ceiling_db: -1.0,
                    release_sec: 0.05,
                },
            ])),
            master_gain_db: 0.0,
        };
        let f = s.to_filter_complex();
        assert!(f.contains("[0:a]"), "voice input mapped");
        assert!(f.contains("[1:a]"), "game input mapped");
        assert!(
            f.contains("highpass=f=80.0"),
            "voice preset applied per-track"
        );
        assert!(f.contains("acompressor"), "game compressor applied");
        assert!(f.contains("volume=-3.00dB"), "game gain trim applied");
        assert!(f.contains("amix=inputs=2:normalize=0[mix]"));
        assert!(f.contains("[mix]alimiter"), "master limiter on master bus");
        assert!(f.ends_with("[out]"));
    }

    #[test]
    fn master_gain_only_no_chain() {
        let s = SubMix {
            tracks: vec![TrackIn {
                label: "x".into(),
                input_index: 0,
                insert_fx: None,
                gain_db: 0.0,
            }],
            master_chain: None,
            master_gain_db: -1.5,
        };
        assert!(s.to_filter_complex().contains("[mix]volume=-1.50dB[out]"));
    }

    #[test]
    fn serde_round_trip() {
        let s = SubMix {
            tracks: vec![TrackIn {
                label: "v".into(),
                input_index: 0,
                insert_fx: Some(InsertChain::voice_bus_default()),
                gain_db: 1.0,
            }],
            master_chain: Some(InsertChain::game_bus_default()),
            master_gain_db: -0.5,
        };
        let j = serde_json::to_string(&s).unwrap();
        let back: SubMix = serde_json::from_str(&j).unwrap();
        assert_eq!(back.tracks.len(), 1);
        assert_eq!(back.master_gain_db, -0.5);
    }

    #[test]
    fn output_pad_is_stable() {
        assert_eq!(SubMix::output_pad(), "out");
    }
}
