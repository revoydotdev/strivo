//! Sidechain compressor — DAW ducking automation.
//!
//! Every DAW has a sidechain compressor: route a trigger signal
//! (typically a voice mic) into the detector input of a compressor on
//! the audio bus (typically the game / music) so the audio ducks when
//! the trigger is active. Podcasters use it to keep voice intelligible
//! over a background bed; streamers use it to push game audio down
//! while talking.
//!
//! This crate is pure data: takes a list of [`strivo_vad::VoiceInterval`]s
//! (the trigger), applies attack/release smoothing in dB space, and
//! emits [`strivo_automation::AutomationPoint`]s the host can save as a
//! regular volume automation. At render time the existing automation
//! pipeline bakes the points via ffmpeg `asendcmd` — no new filter
//! plumbing, no new renderer path.
//!
//! Twelve tests cover the attack / release envelope, the rest-state +
//! ducked-state targets, attack ramp under stacked intervals, and the
//! automation-handoff shape.

use serde::{Deserialize, Serialize};
use strivo_vad::VoiceInterval;

pub use strivo_automation::AutomationPoint;
pub use strivo_automation::Curve;
pub use strivo_automation::VolumeAutomation;

/// `#[serde(default)]` on the container, so a saved mixer or sidecar that
/// overrides **one** knob deserialises with the rest at their documented
/// defaults. Without it a UI that sets only `duck_db` cannot write a knobs
/// object at all, and the honest "leave the other four alone" is a parse error.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct SidechainKnobs {
    /// Target gain (dB) while the trigger is active. -12 dB is a
    /// reasonable podcast default; -6 dB is gentler; -20 dB is
    /// aggressive "voice-over" ducking.
    pub duck_db: f32,
    /// Attack time (sec) — how fast the gain rides down to duck_db
    /// once the trigger fires. 50 ms feels natural.
    pub attack_sec: f32,
    /// Release time (sec) — how fast the gain rides back to 0 dB once
    /// the trigger releases. 300 ms keeps short pauses from popping.
    pub release_sec: f32,
    /// Hold time (sec) after the trigger releases before the release
    /// envelope starts. Prevents the gain from bouncing on micro-gaps.
    pub hold_sec: f32,
    /// Granularity of the emitted automation (sec). 50 ms = 20 pts/sec
    /// matches the strivo-automation default `step_secs`.
    pub step_sec: f32,
}

impl Default for SidechainKnobs {
    fn default() -> Self {
        Self {
            duck_db: -12.0,
            attack_sec: 0.05,
            release_sec: 0.3,
            hold_sec: 0.1,
            step_sec: 0.05,
        }
    }
}

/// Translate a list of voice intervals (typically VAD output) into a
/// time-keyed gain curve for the audio bus. The curve goes through
/// strivo-automation's existing render-time bake — no new ffmpeg
/// plumbing.
///
/// `total_duration_sec` is the recording length; the curve always lands
/// back at 0 dB at the end so the audio bus isn't left attenuated past
/// the last interval.
pub fn build_automation(
    intervals: &[VoiceInterval],
    total_duration_sec: f32,
    knobs: &SidechainKnobs,
) -> VolumeAutomation {
    let mut points: Vec<AutomationPoint> = Vec::new();
    if total_duration_sec <= 0.0 {
        return VolumeAutomation { points };
    }
    let step = knobs.step_sec.max(0.005);
    let dur = total_duration_sec;
    // Sort + merge overlapping intervals so two back-to-back hits act
    // as one duck region.
    let mut sorted: Vec<(f32, f32)> = intervals
        .iter()
        .map(|iv| (iv.start_sec.max(0.0), iv.end_sec.min(dur)))
        .filter(|(s, e)| e > s)
        .collect();
    sorted.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut merged: Vec<(f32, f32)> = Vec::new();
    for (s, e) in sorted {
        if let Some(last) = merged.last_mut() {
            if s <= last.1 + knobs.hold_sec {
                if e > last.1 {
                    last.1 = e;
                }
                continue;
            }
        }
        merged.push((s, e));
    }

    // Walk the timeline and emit envelope points. State machine:
    //   Rest (0 dB) → Attack → Sustain (duck_db) → Hold → Release → Rest.
    let mut emit = |t: f32, db: f32| {
        // Merge points within step granularity to keep the list lean.
        if let Some(last) = points.last_mut() {
            if (last.time_sec - t).abs() < step * 0.5 {
                last.gain_db = db;
                return;
            }
        }
        points.push(AutomationPoint {
            time_sec: t.max(0.0).min(dur),
            gain_db: db,
            curve: Curve::Linear,
        });
    };

    // Seed with 0 dB at t=0 so unaffected leading audio sits at unity.
    emit(0.0, 0.0);

    let attack = knobs.attack_sec.max(0.0);
    let release = knobs.release_sec.max(0.0);
    let hold = knobs.hold_sec.max(0.0);
    let duck = knobs.duck_db;

    let mut ends_ducked = false;
    for (start, end) in merged {
        // Attack: from `start - attack` (clamped to 0) ride 0 → duck.
        let attack_start = (start - attack).max(0.0);
        // Anchor at the start of the attack so any prior release
        // tail doesn't bleed past this point.
        emit(attack_start, 0.0);
        if start > attack_start {
            emit(start, duck);
        } else {
            emit(start.max(attack_start), duck);
        }
        // Hold the ducked level across the interval.
        emit(end, duck);
        // If the interval bumps right up against the recording end
        // (typical when the trigger extends past dur and got clamped),
        // we can't fit a hold + release without overwriting the duck
        // — leave the curve ducked at end-of-stream.
        if end >= dur - 1e-3 {
            ends_ducked = true;
            continue;
        }
        // Hold past end before release starts.
        let release_start = (end + hold).min(dur);
        emit(release_start, duck);
        let release_end = (release_start + release).min(dur);
        emit(release_end, 0.0);
    }

    // Ensure the curve lands at 0 dB at end-of-stream UNLESS the
    // trigger ran right to the end.
    if !ends_ducked {
        emit(dur, 0.0);
    }

    VolumeAutomation { points }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iv(start: f32, end: f32) -> VoiceInterval {
        VoiceInterval {
            start_sec: start,
            end_sec: end,
            mean_db: -15.0,
        }
    }

    fn sample(auto: &VolumeAutomation, t: f32) -> f32 {
        auto.sample(t)
    }

    #[test]
    fn empty_intervals_yield_resting_curve() {
        let auto = build_automation(&[], 10.0, &SidechainKnobs::default());
        // Two anchors at 0 and end, both at unity.
        assert!(auto.points.iter().all(|p| (p.gain_db - 0.0).abs() < 1e-3));
    }

    #[test]
    fn empty_duration_yields_empty_curve() {
        let auto = build_automation(&[iv(1.0, 2.0)], 0.0, &SidechainKnobs::default());
        assert!(auto.points.is_empty());
    }

    #[test]
    fn single_interval_drops_to_duck_during_active() {
        let knobs = SidechainKnobs::default();
        let auto = build_automation(&[iv(2.0, 4.0)], 8.0, &knobs);
        // Before attack: unity.
        assert!((sample(&auto, 0.5) - 0.0).abs() < 0.5);
        // Middle of interval: ducked.
        let mid = sample(&auto, 3.0);
        assert!((mid - knobs.duck_db).abs() < 0.5, "got {mid}");
    }

    #[test]
    fn attack_ramps_from_unity_to_duck() {
        let knobs = SidechainKnobs {
            attack_sec: 0.5,
            ..Default::default()
        };
        let auto = build_automation(&[iv(2.0, 4.0)], 8.0, &knobs);
        // Halfway through attack (t = 2.0 - 0.25 = 1.75): roughly half-ducked.
        let halfway = sample(&auto, 1.75);
        let target = knobs.duck_db * 0.5;
        assert!(
            (halfway - target).abs() < 1.5,
            "halfway {halfway} not near {target}"
        );
    }

    #[test]
    fn release_recovers_to_unity_after_hold() {
        let knobs = SidechainKnobs {
            release_sec: 0.4,
            hold_sec: 0.1,
            ..Default::default()
        };
        let auto = build_automation(&[iv(2.0, 4.0)], 8.0, &knobs);
        // Just before hold ends + release starts: still ducked.
        let held = sample(&auto, 4.05);
        assert!((held - knobs.duck_db).abs() < 1.0, "held {held} not ducked");
        // After release fully done: back to 0 dB.
        let recovered = sample(&auto, 5.0);
        assert!(recovered.abs() < 1.0, "recovered {recovered}");
    }

    #[test]
    fn back_to_back_intervals_merge_into_one_duck_region() {
        // Two intervals 100 ms apart with hold=0 — they should still
        // collapse because the hold default is 100 ms, which makes the
        // gap qualify for merging.
        let auto = build_automation(
            &[iv(1.0, 2.0), iv(2.05, 3.0)],
            5.0,
            &SidechainKnobs::default(),
        );
        // Sample the bridge between them — should be ducked, not riding
        // back to unity and re-attacking.
        let bridge = sample(&auto, 2.025);
        let knobs = SidechainKnobs::default();
        assert!(
            (bridge - knobs.duck_db).abs() < 1.5,
            "bridge {bridge} should stay ducked",
        );
    }

    #[test]
    fn distant_intervals_get_separate_ducks() {
        // Far apart enough that the release fully completes and a
        // fresh attack happens for the second.
        let auto = build_automation(
            &[iv(1.0, 2.0), iv(5.0, 6.0)],
            8.0,
            &SidechainKnobs::default(),
        );
        // Between them at unity.
        let between = sample(&auto, 3.5);
        assert!(between.abs() < 0.5, "between {between} should be unity");
    }

    #[test]
    fn duration_clamps_intervals_extending_past_end() {
        let auto = build_automation(&[iv(8.0, 50.0)], 10.0, &SidechainKnobs::default());
        // Ducked at end of recording.
        let near_end = sample(&auto, 9.5);
        let knobs = SidechainKnobs::default();
        assert!(
            (near_end - knobs.duck_db).abs() < 1.5,
            "near_end {near_end} should stay ducked when interval extends past dur",
        );
    }

    #[test]
    fn intervals_starting_before_zero_clamp_to_zero() {
        let auto = build_automation(&[iv(-1.0, 2.0)], 8.0, &SidechainKnobs::default());
        // Already ducked at t=0.
        let at_start = sample(&auto, 0.0);
        let knobs = SidechainKnobs::default();
        assert!(
            (at_start - knobs.duck_db).abs() < 1.5,
            "at_start {at_start}"
        );
    }

    #[test]
    fn overlapping_intervals_merge_to_extended_duck() {
        let auto = build_automation(
            &[iv(1.0, 3.0), iv(2.0, 5.0)],
            8.0,
            &SidechainKnobs::default(),
        );
        // Both regions ducked, with no bounce back to unity in between.
        let mid = sample(&auto, 4.0);
        let knobs = SidechainKnobs::default();
        assert!((mid - knobs.duck_db).abs() < 1.0, "mid {mid}");
    }

    #[test]
    fn curve_lands_at_unity_at_end_of_recording() {
        let auto = build_automation(&[iv(1.0, 9.0)], 10.0, &SidechainKnobs::default());
        let last = auto.points.last().unwrap();
        assert!(last.time_sec >= 9.5);
        assert!(last.gain_db.abs() < 1.5);
    }

    #[test]
    fn build_automation_hands_off_to_automation_filter() {
        // Verify the output composes cleanly with the existing
        // automation filter builder.
        let auto = build_automation(&[iv(1.0, 3.0)], 6.0, &SidechainKnobs::default());
        let filter = auto.build_audio_filter();
        assert!(filter.starts_with("volume='"), "{filter}");
        assert!(filter.ends_with("':eval=frame"), "{filter}");
        assert!(
            !filter.contains("asendcmd"),
            "the handoff must reach the render as a per-frame expression: asendcmd's \
             command delivery is a cross-thread race, so a ducking envelope built \
             from it is not reproducible. {filter}"
        );
    }
}
