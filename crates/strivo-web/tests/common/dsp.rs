//! Reading a decoded signal, so a mixer assertion is about the media and never
//! about a file size or an exit code.
//!
//! ## Why the tolerance is measured, not chosen
//!
//! "The preview's gain matches the render's within a stated tolerance" is only
//! a claim if the tolerance is derived from something. The render path is
//! lossless in, lossy out: a sine is encoded to AAC, concatenated, and decoded
//! back to signed 16-bit PCM before it is measured, so the measured ratio
//! between two renders is the requested ratio **plus** the codec's own
//! round-trip error. A hand-picked `0.5 dB` would either be too tight on a
//! quiet fixture or so loose it would pass a control that was dropped.
//!
//! So the tolerance is **calibrated per run** by
//! [`round_trip_error_db`], and every level assertion in the mixer suite is
//! `|measured - predicted| <= FLOOR_DB + TOLERANCE_K * round_trip_error_db`.
//! Both constants are derived rather than chosen:
//!
//! * [`FLOOR_DB`] is half of the last digit `invoke::track_filter` prints. It
//!   spells a track gain as `format!("volume={:.2}dB", gain_db)`, so the gain the
//!   executor runs can differ from the gain the plan holds by at most 0.005 dB.
//! * [`TOLERANCE_K`] covers the codec, and the multiplier is the sum of the two
//!   independent round trips the compared renders each carry.
//!
//! The failure direction of a collapsed calibration is deliberate. If a future
//! host made the round trip exact, the measured error would be 0, the tolerance
//! would fall to [`FLOOR_DB`], and the suite would get **stricter** — never
//! laxer. A tolerance that can only tighten is safe to leave in; one that can
//! silently widen is not.

use std::path::Path;
use std::process::Command;

/// The sample format every measurement decodes to. Fixed, so two renders are
/// directly comparable: 48 kHz mono signed 16-bit little-endian.
const SAMPLE_RATE: u32 = 48_000;

/// Half of the last digit `invoke::track_filter` prints when it spells a track
/// gain, and so the most the executed gain can differ from the plan's.
pub const FLOOR_DB: f64 = 0.005;

/// How many measured round trips of codec error the tolerance allows on top of
/// [`FLOOR_DB`].
///
/// Two renders being compared each carry one independent round trip, so the
/// difference between them carries at most the sum of the two; one times the
/// measured error is the exact budget and the factor above one exists for the
/// part of it that is not stationary. A two-second window's RMS over a tone is
/// an estimate rather than a constant, and `asendcmd` automation is sampled on
/// a 50 ms grid rather than per sample, so the reading wobbles by a small
/// fraction of a dB even when the encoder is exact.
pub const TOLERANCE_K: f64 = 3.0;

/// Decode `path`'s first audio stream to signed 16-bit mono PCM at
/// [`SAMPLE_RATE`].
///
/// Panics on a decode failure or an empty result: a fixture that will not
/// decode cannot be measured, and reporting that as "no difference" is the
/// vacuous pass this file exists to prevent.
pub fn pcm(path: &Path) -> Vec<i16> {
    let out = Command::new("ffmpeg")
        .args(["-v", "error", "-i", &path.to_string_lossy()])
        .args([
            "-map",
            "0:a",
            "-f",
            "s16le",
            "-acodec",
            "pcm_s16le",
            "-ar",
            &SAMPLE_RATE.to_string(),
            "-ac",
            "1",
            "-",
        ])
        .output()
        .expect("ffmpeg runs");
    assert!(
        out.status.success(),
        "decoding the audio of {} failed: {}",
        path.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !out.stdout.is_empty(),
        "{} decoded to no audio at all, so any level measured from it would be a \
         reading of nothing",
        path.display()
    );
    let (pairs, _) = out.stdout.as_chunks::<2>();
    pairs.iter().map(|pair| i16::from_le_bytes(*pair)).collect()
}

/// Root-mean-square of a whole signal, in dBFS.
///
/// dBFS rather than a linear ratio because every number in this task is a gain
/// in dB, and a dB-domain comparison is the one the plan is written in.
#[must_use]
pub fn rms_dbfs(samples: &[i16]) -> f64 {
    assert!(
        !samples.is_empty(),
        "an empty signal has no level; a measurement of it would be a reading of nothing"
    );
    let sum: f64 = samples
        .iter()
        .map(|s| {
            let v = f64::from(*s) / 32_768.0;
            v * v
        })
        .sum();
    let rms = (sum / samples.len() as f64).sqrt();
    assert!(
        rms > 0.0,
        "the signal is digital silence, so its level is -inf and no dB difference \
         against it is a number"
    );
    20.0 * rms.log10()
}

/// RMS over one window of samples starting at `start_secs`.
///
/// A window, not the whole signal: an automation curve or a fade means the
/// level is not constant over the file, and a whole-file RMS would report the
/// average of two states as if it were one.
#[must_use]
pub fn window_rms_dbfs(samples: &[i16], start_secs: f64, secs: f64) -> f64 {
    let rate = SAMPLE_RATE as f64;
    let from = (start_secs * rate).round().max(0.0) as usize;
    let len = (secs * rate).round().max(1.0) as usize;
    assert!(
        len as f64 >= rate / 20.0,
        "a {secs}s window is too short to be an RMS estimate at {SAMPLE_RATE} Hz"
    );
    let to = from + len;
    assert!(
        to <= samples.len(),
        "the window {start_secs}s..{}s runs past the end of a signal that decodes to \
         {} samples ({:.3}s), so a level read from it would be a truncated reading",
        start_secs + secs,
        samples.len(),
        samples.len() as f64 / rate
    );
    rms_dbfs(&samples[from..to])
}

/// The dB `attenuated` reads below `reference`.
///
/// Positive means `attenuated` is quieter, which is the sign convention every
/// assertion in the mixer suite uses: a control that applies -6 dB must read as
/// a **positive** 6.
#[must_use]
pub fn delta_db(reference: &[i16], attenuated: &[i16]) -> f64 {
    rms_dbfs(reference) - rms_dbfs(attenuated)
}

/// The dB difference over one window in each signal.
#[must_use]
pub fn window_delta_db(reference: &[i16], attenuated: &[i16], at_secs: f64, secs: f64) -> f64 {
    window_rms_dbfs(reference, at_secs, secs) - window_rms_dbfs(attenuated, at_secs, secs)
}

/// The dB error this host's codec path introduces on its own.
///
/// Measured, not assumed. The caller supplies two renders that differ only by
/// the codec, and the gap between them is the noise every level assertion is
/// measured against.
///
/// Asserts non-vacuously in the safe direction. A silent calibration render
/// would otherwise report `0.0` and collapse every tolerance to [`FLOOR_DB`];
/// that is caught and named, because a fixture at -60 dBFS has a round-trip
/// error that is a broken fixture rather than a noise floor. A merely tight
/// calibration is *not* caught, and deliberately so: it makes the suite
/// stricter, which is the direction a wrong tolerance should fail.
pub fn round_trip_error_db(baseline: &[i16], resent: &[i16]) -> f64 {
    let level = rms_dbfs(baseline);
    assert!(
        level > -60.0,
        "the calibration render is quieter than -60 dBFS, so its round-trip error \
         is not a noise floor but a broken fixture"
    );
    let error = delta_db(baseline, resent).abs();
    assert!(
        error.is_finite() && error < 12.0,
        "the calibration round-trip error is {error} dB, which is not a codec \
         round-trip; a tolerance derived from it would be meaningless"
    );
    error
}

/// The tolerance every level assertion in this file uses.
#[must_use]
pub fn tolerance_db(round_trip: f64) -> f64 {
    FLOOR_DB + TOLERANCE_K * round_trip
}

/// Whether a measured dB is within tolerance of a predicted one.
///
/// One function so the two call sites cannot drift apart, and so a failing
/// assertion can print one number rather than a re-derived expression.
#[must_use]
pub fn within(measured: f64, predicted: f64, round_trip: f64) -> bool {
    (measured - predicted).abs() <= tolerance_db(round_trip)
}

/// The dB the **codec path** on this host costs, measured rather than assumed.
///
/// Two successive AAC generations of the same signal: `one` is the source
/// encoded once, `two` is `one` encoded again. The gap between them is one
/// round trip's worth of codec error, which is the noise every gain assertion
/// is measured against.
///
/// A **copy** would have measured nothing — two identical files differ by
/// exactly 0 dB, so a copy-based calibration is a tolerance collapsed to zero
/// dressed up as a measurement. That was the first version of this helper and it
/// is recorded here because "the calibration ran" and "the calibration measured
/// something" are different claims.
#[must_use]
pub fn calibrate_codec_round_trip(source: &Path, dir: &Path) -> f64 {
    let one = dir.join("codec-gen1.mkv");
    let two = dir.join("codec-gen2.mkv");
    for (from, to) in [(source, &one), (one.as_path(), &two)] {
        let status = Command::new("ffmpeg")
            .args(["-y", "-v", "error", "-i", &from.to_string_lossy()])
            .args(["-c:v", "copy", "-c:a", "aac"])
            .arg(to)
            .status()
            .expect("ffmpeg runs");
        assert!(
            status.success(),
            "calibration encode {} -> {} failed",
            from.display(),
            to.display()
        );
    }
    round_trip_error_db(&pcm(&one), &pcm(&two))
}

/// The integrated loudness `ffmpeg`'s own `loudnorm` reads off a rendered
/// artifact, in LUFS.
///
/// The tool's measurement rather than this file's arithmetic, because the claim
/// being tested is that normalisation is *measure-then-apply*: the only honest
/// witness is the measurement the two passes are built on, read back off the
/// finished file.
#[must_use]
pub fn measure_lufs(path: &Path) -> f64 {
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-nostats", "-i", &path.to_string_lossy()])
        .args([
            "-af",
            "loudnorm=I=-70:LRA=50:TP=0:print_format=json",
            "-f",
            "null",
            "-",
        ])
        .output()
        .expect("ffmpeg runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let value = stderr
        .lines()
        .find_map(|line| line.trim().strip_prefix("\"input_i\""))
        .and_then(|rest| rest.split(':').nth(1))
        .map(|v| v.trim().trim_matches(',').trim_matches('"'))
        .unwrap_or_else(|| {
            panic!(
                "loudnorm reported no input_i for {}: {stderr}",
                path.display()
            )
        })
        .parse::<f64>()
        .unwrap_or_else(|e| panic!("loudnorm reported an unreadable input_i: {e}"));
    assert!(
        value > -70.5,
        "loudnorm read {value} LUFS, which is its own -70 floor rather than a \
         measurement of this artifact"
    );
    value
}
