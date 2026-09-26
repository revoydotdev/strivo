//! DAW-style independent pitch / time-stretch.
//!
//! Two slots every DAW exposes per clip:
//!
//! - **Tempo** — how fast playback runs, independent of pitch. The streamer
//!   use is *fit-to-window*: a 1h45 raw stream time-compressed to 1h00 for a
//!   YouTube slot keeps every voice's pitch unchanged.
//! - **Pitch** — semitones up/down, independent of tempo. The streamer use
//!   is transposing a stinger to match a song, or hiding leaked names by
//!   shifting a clip a half-tone.
//!
//! ffmpeg's `rubberband` filter does both in a single pass with a
//! formant-preserving mode for vocals. This crate is the typed model and
//! filter-string composer — the host wraps it; the render path is the only
//! place ffmpeg actually runs.
//!
//! Two constructors handle the workflows most streamers use directly:
//! [`PitchTime::fit_to_duration`] picks the tempo factor that lands a clip
//! on a target length; [`PitchTime::transpose_semitones`] converts a chosen
//! semitone shift into the pitch factor `rubberband` expects.

use serde::{Deserialize, Serialize};

/// Independent tempo + pitch settings for one rubberband pass.
///
/// Both factors are **multiplicative**, matching ffmpeg's `rubberband`
/// CLI: `1.0` = unchanged, `>1.0` = faster / higher, `<1.0` = slower /
/// lower. The semitone helpers convert in/out of dB-style cents the UI
/// shows the user.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct PitchTime {
    /// Tempo factor. 2.0 → twice as fast (half the duration). Clamped to
    /// `[0.25, 4.0]` by [`PitchTime::to_filter`] to match the rubberband
    /// library's documented safe range.
    pub tempo: f64,
    /// Pitch factor as a frequency ratio. 2.0 → up one octave. Use
    /// [`PitchTime::with_semitones`] when the UI works in semitones.
    pub pitch: f64,
    /// When true, preserves vocal formants — keeps the voice sounding
    /// like the same person at the new pitch. Off for music/SFX where
    /// timbre shift is the desired effect.
    pub formant_preserve: bool,
}

impl Default for PitchTime {
    /// Identity: 1× tempo, 1× pitch, formant preserve on (the
    /// safest default for voice recordings).
    fn default() -> Self {
        Self {
            tempo: 1.0,
            pitch: 1.0,
            formant_preserve: true,
        }
    }
}

impl PitchTime {
    /// `tempo` factor that maps `source_sec` to `target_sec`. Returns
    /// the identity factor (1.0) when either input is non-positive so
    /// the filter never panics on bad metadata.
    pub fn fit_to_duration(source_sec: f64, target_sec: f64) -> Self {
        let tempo = if source_sec > 0.0 && target_sec > 0.0 {
            source_sec / target_sec
        } else {
            1.0
        };
        Self {
            tempo,
            pitch: 1.0,
            formant_preserve: true,
        }
    }

    /// Pitch shift expressed in semitones (12 per octave). Tempo
    /// unchanged. Formant preserve on so vocals don't go chipmunk.
    pub fn transpose_semitones(semis: f64) -> Self {
        Self {
            tempo: 1.0,
            pitch: 2f64.powf(semis / 12.0),
            formant_preserve: true,
        }
    }

    /// Return a copy with `tempo` overridden — useful when the UI
    /// edits the two factors independently.
    pub fn with_tempo(mut self, tempo: f64) -> Self {
        self.tempo = tempo;
        self
    }

    /// Return a copy with the pitch factor derived from a semitone
    /// value. Symmetric counterpart of [`PitchTime::semitones`].
    pub fn with_semitones(mut self, semis: f64) -> Self {
        self.pitch = 2f64.powf(semis / 12.0);
        self
    }

    /// Inverse of [`PitchTime::with_semitones`] — surface the current
    /// pitch factor as semitones for the UI's slider readout.
    pub fn semitones(&self) -> f64 {
        12.0 * self.pitch.max(1e-9).log2()
    }

    /// True when both factors are unity and the chain has no effect.
    /// Callers skip the filter entirely in that case.
    pub fn is_identity(&self) -> bool {
        (self.tempo - 1.0).abs() < 1e-6 && (self.pitch - 1.0).abs() < 1e-6
    }

    /// Emit the ffmpeg `rubberband=` invocation. Empty string when
    /// identity, so the render path can splice the result into an
    /// `-af` chain without a guard.
    pub fn to_filter(&self) -> String {
        if self.is_identity() {
            return String::new();
        }
        let tempo = self.tempo.clamp(0.25, 4.0);
        let pitch = self.pitch.clamp(0.25, 4.0);
        // The option is `formant`, singular. `formants=preserved` is rejected
        // outright by ffmpeg with "Option not found", so every non-identity
        // pitch setting used to fail the whole render — which is why this went
        // unnoticed while the setting was never baked at all.
        let formants = if self.formant_preserve {
            ":formant=preserved"
        } else {
            ":formant=shifted"
        };
        format!(
            "rubberband=tempo={}:pitch={}{}",
            fmt_f(tempo),
            fmt_f(pitch),
            formants
        )
    }

    /// Duration the output will land at given a source length. Useful
    /// for the SPA to show "1h45 → 1h00" before the render runs.
    pub fn output_duration_sec(&self, source_sec: f64) -> f64 {
        if self.tempo <= 0.0 {
            return source_sec;
        }
        source_sec / self.tempo
    }
}

fn fmt_f(v: f64) -> String {
    if (v.fract()).abs() < 1e-9 {
        format!("{:.1}", v)
    } else {
        let s = format!("{:.6}", v);
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_emits_empty_filter() {
        let p = PitchTime::default();
        assert!(p.is_identity());
        assert_eq!(p.to_filter(), "");
    }

    #[test]
    fn fit_to_duration_computes_tempo() {
        // 1h45 → 1h00: tempo 1.75
        let p = PitchTime::fit_to_duration(6300.0, 3600.0);
        assert!((p.tempo - 1.75).abs() < 1e-6);
        assert_eq!(p.pitch, 1.0);
        assert!(p.formant_preserve);
    }

    #[test]
    fn fit_to_duration_clamps_in_filter() {
        // 10× target shorter than source → tempo 10, clamped to 4.0
        let p = PitchTime::fit_to_duration(36000.0, 3600.0);
        assert_eq!(p.tempo, 10.0);
        // Filter clamps before emitting
        assert!(p.to_filter().contains("tempo=4.0"));
    }

    #[test]
    fn fit_to_duration_bad_inputs_yield_identity() {
        let zero = PitchTime::fit_to_duration(0.0, 3600.0);
        assert_eq!(zero.tempo, 1.0);
        let neg = PitchTime::fit_to_duration(3600.0, -1.0);
        assert_eq!(neg.tempo, 1.0);
    }

    #[test]
    fn transpose_one_octave_gives_factor_two() {
        let p = PitchTime::transpose_semitones(12.0);
        assert!((p.pitch - 2.0).abs() < 1e-9);
        assert!(p.formant_preserve);
    }

    #[test]
    fn transpose_one_semitone_down() {
        let p = PitchTime::transpose_semitones(-1.0);
        // 2^(-1/12) ≈ 0.94387
        assert!((p.pitch - 0.943874).abs() < 1e-5);
    }

    #[test]
    fn semitones_round_trip() {
        for n in [-12.0, -7.0, -2.0, 0.5, 5.0, 12.0] {
            let p = PitchTime::default().with_semitones(n);
            assert!((p.semitones() - n).abs() < 1e-6, "n={}", n);
        }
    }

    #[test]
    fn filter_string_for_tempo_only() {
        let p = PitchTime::default().with_tempo(1.5);
        assert_eq!(
            p.to_filter(),
            "rubberband=tempo=1.5:pitch=1.0:formant=preserved"
        );
    }

    #[test]
    fn filter_string_for_pitch_only() {
        let p = PitchTime::transpose_semitones(12.0);
        assert_eq!(
            p.to_filter(),
            "rubberband=tempo=1.0:pitch=2.0:formant=preserved"
        );
    }

    #[test]
    fn filter_string_when_formants_off() {
        let mut p = PitchTime::default().with_tempo(0.5);
        p.formant_preserve = false;
        assert_eq!(
            p.to_filter(),
            "rubberband=tempo=0.5:pitch=1.0:formant=shifted"
        );
    }

    #[test]
    fn output_duration_under_tempo() {
        let p = PitchTime::fit_to_duration(6300.0, 3600.0);
        assert!((p.output_duration_sec(6300.0) - 3600.0).abs() < 1e-3);
    }

    #[test]
    fn output_duration_handles_zero_tempo() {
        let p = PitchTime {
            tempo: 0.0,
            ..Default::default()
        };
        assert_eq!(p.output_duration_sec(100.0), 100.0);
    }

    #[test]
    fn pitch_clamps_in_filter_string() {
        let mut p = PitchTime {
            pitch: 100.0,
            ..Default::default()
        };
        assert!(p.to_filter().contains("pitch=4.0"));
        p.pitch = 0.001;
        assert!(p.to_filter().contains("pitch=0.25"));
    }

    #[test]
    fn serde_round_trips_with_formant_flag() {
        let p = PitchTime::transpose_semitones(-3.0);
        let json = serde_json::to_string(&p).unwrap();
        let back: PitchTime = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn near_identity_still_emits_filter_when_outside_epsilon() {
        // Half-percent tempo nudge — should still produce a filter.
        let p = PitchTime::default().with_tempo(1.005);
        assert!(!p.is_identity());
        assert!(p.to_filter().contains("tempo=1.005"));
    }
}
