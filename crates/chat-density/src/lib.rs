//! strivo-chat-density — chat-density-derived audience retention.
//!
//! Promotes the second marketplace catalog entry (twitch-chat-density)
//! from "coming soon" to "ships today". Provides the canonical
//! audience-retention signal the iter 13 Heatmap plugin reserved a
//! slot for via the `x.chat_density` capability extension.
//!
//! Two ingestion paths:
//!
//!   * [`parse_irc_log`] reads a Twitch IRC dump line by line. Each
//!     `PRIVMSG #channel :message` line becomes a [`ChatEvent`] when a
//!     `@badge-info=...;tmi-sent-ts=<ms>` tag block is present (the
//!     standard Twitch chatty-formatted dump shape).
//!   * [`parse_csv_log`] reads a simple CSV (`time_sec,user,message`
//!     header optional) for non-Twitch sources or hand-edited logs.
//!
//! **Time is integer microseconds**, [`MediaTime`], the single time
//! representation of the Creator edition. It used to be `f32` seconds, which
//! lost precision twice over: `(event_ts - start) as f32 / 1000.0` rounds
//! once in the subtraction and again in the division, and the drop of an
//! event before the recorded stream start (below) was a silent `continue`
//! that the model now makes a type outcome — a `MediaTime` cannot be
//! negative, so "before the start" has no value to construct.
//!
//! Both produce a `Vec<ChatEvent>` the analyzer then folds:
//!
//!   * [`compute_density`] buckets events into `bucket_secs` windows,
//!     counts messages, deduplicates unique chatters, computes a
//!     weighted score that boosts buckets with high unique-chatter
//!     turnover (broader engagement), and normalises both density and
//!     score to [0, 1] across the recording.
//!
//! All pure — no IO, no clock; tests feed canned strings.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use strivo_editor::time::MediaTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatEvent {
    /// The event's media time, in whole microseconds from the stream start.
    pub at: MediaTime,
    pub user: String,
    /// Free-form message text. Kept so downstream filters can keyword-
    /// match (raids, sponsor mentions, etc.); not used by the analyzer.
    #[serde(default)]
    pub message: String,
    /// Relative weight. Defaults to 1.0; channel bots and unverified
    /// accounts can be down-weighted by the caller before computing.
    #[serde(default = "default_weight")]
    pub weight: f32,
}

fn default_weight() -> f32 {
    1.0
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct DensityPoint {
    /// The media time this window opens at, in whole microseconds.
    pub bucket_start: MediaTime,
    pub message_count: u32,
    pub unique_chatters: u32,
    /// Normalised [0,1] message-density (per bucket vs the loudest one).
    pub density: f32,
    /// Normalised engagement: leans on unique chatters so a bot spam
    /// doesn't dominate. Fused as 0.6·density + 0.4·unique-density.
    pub engagement: f32,
}

/// Parse a Twitch IRC dump. Recognises both `tmi-sent-ts` epoch-ms
/// tags and an explicit `time_sec=` annotation for non-Twitch
/// sources. `stream_start_ts_ms` is the epoch-ms timestamp the stream
/// started — events are emitted with `time_sec = (event_ts - start)/1000`.
pub fn parse_irc_log(log: &str, stream_start_ts_ms: u64) -> Vec<ChatEvent> {
    let mut out: Vec<ChatEvent> = Vec::new();
    for line in log.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Anatomy: @<tags> :<nick>!<...>@<...>.tmi.twitch.tv PRIVMSG #<channel> :<message>
        if !line.contains("PRIVMSG") {
            continue;
        }
        let tags = if line.starts_with('@') {
            let end = line.find(' ').unwrap_or(line.len());
            &line[1..end]
        } else {
            ""
        };
        let ts_ms = tag_value(tags, "tmi-sent-ts").and_then(|v| v.parse::<u64>().ok());
        let nick = parse_nick(line).unwrap_or_default();
        let message = parse_message(line).unwrap_or_default();
        if nick.is_empty() {
            continue;
        }
        // No usable timestamp, or one before the recorded start: the event
        // has no media coordinate at all, so it is skipped rather than piled
        // into bucket 0 where it would read as "at the very beginning".
        let Some(at) = media_at(ts_ms, stream_start_ts_ms) else {
            continue;
        };
        out.push(ChatEvent {
            at,
            user: nick,
            message,
            weight: 1.0,
        });
    }
    out
}

/// The media time of an event that arrived `offset_ms` epoch-milliseconds
/// after the recorded stream start, or `None` when it has no media
/// coordinate: before the start, or an offset no capture reaches.
fn media_at(offset_ms: Option<u64>, start_ms: u64) -> Option<MediaTime> {
    let offset = offset_ms?.checked_sub(start_ms)?;
    MediaTime::from_millis(i64::try_from(offset).ok()?).ok()
}

fn tag_value<'a>(tags: &'a str, key: &str) -> Option<&'a str> {
    for pair in tags.split(';') {
        let mut it = pair.splitn(2, '=');
        if let (Some(k), Some(v)) = (it.next(), it.next()) {
            if k == key {
                return Some(v);
            }
        }
    }
    None
}

fn parse_nick(line: &str) -> Option<String> {
    // Two shapes:
    //  ":nick!user@host.tmi.twitch.tv PRIVMSG ..."
    //  "@tags ... :nick!user@host.tmi.twitch.tv PRIVMSG ..."
    let priv_idx = line.find("PRIVMSG")?;
    let before = &line[..priv_idx];
    let last_colon = before.rfind(':')?;
    let prefix = &before[last_colon + 1..];
    let bang = prefix.find('!')?;
    Some(prefix[..bang].to_string())
}

fn parse_message(line: &str) -> Option<String> {
    // Locate the SECOND `:` (after PRIVMSG #channel) — that's the
    // start of the message body.
    let priv_idx = line.find("PRIVMSG")?;
    let after = &line[priv_idx..];
    let colon = after.find(" :")?;
    Some(after[colon + 2..].to_string())
}

/// Parse a `time_sec,user,message` CSV. Header row optional (detected
/// by the first column being non-numeric). Empty / malformed lines
/// are skipped silently.
pub fn parse_csv_log(csv: &str) -> Vec<ChatEvent> {
    let mut out: Vec<ChatEvent> = Vec::new();
    for (i, raw) in csv.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.splitn(3, ',').collect();
        if parts.len() < 2 {
            continue;
        }
        // Header detection: first row's first column not a number.
        let secs: f64 = match parts[0].trim().parse() {
            Ok(n) => n,
            Err(_) if i == 0 => continue, // header row
            Err(_) => continue,
        };
        // A row whose first column is not a real position on a timeline —
        // negative, NaN, infinite — is skipped, as a malformed row is.
        let Ok(at) = MediaTime::from_secs_f64(secs) else {
            continue;
        };
        let user = parts[1].trim();
        if user.is_empty() {
            continue;
        }
        let message = parts
            .get(2)
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        out.push(ChatEvent {
            at,
            user: user.to_string(),
            message,
            weight: 1.0,
        });
    }
    out
}

/// Bucket events and compute density / engagement curves.
///
/// `duration` and `bucket` are media times in whole microseconds, so the
/// window index is an integer division rather than a float floor: two events
/// in the same window can no longer land either side of it because of a
/// rounding step. A zero or sub-microsecond `bucket` yields nothing, which
/// is the only way the division can be undefined.
pub fn compute_density(
    events: &[ChatEvent],
    duration: MediaTime,
    bucket: MediaTime,
) -> Vec<DensityPoint> {
    if duration.micros() <= 0 || bucket.micros() <= 0 {
        return Vec::new();
    }
    let n = (duration.micros() / bucket.micros()
        + i64::from(duration.micros() % bucket.micros() != 0)) as usize;
    if n == 0 {
        return Vec::new();
    }
    let mut counts = vec![0.0_f32; n];
    let mut chatters: Vec<BTreeSet<String>> = vec![BTreeSet::new(); n];
    for ev in events {
        let b = (ev.at.micros() / bucket.micros()) as usize;
        if b < n {
            counts[b] += ev.weight.max(0.0);
            chatters[b].insert(ev.user.clone());
        }
    }
    let unique_counts: Vec<f32> = chatters.iter().map(|s| s.len() as f32).collect();
    let max_count = counts.iter().copied().fold(0.0_f32, f32::max).max(1.0);
    let max_unique = unique_counts
        .iter()
        .copied()
        .fold(0.0_f32, f32::max)
        .max(1.0);
    (0..n)
        .map(|i| {
            let density = counts[i] / max_count;
            let unique_norm = unique_counts[i] / max_unique;
            let engagement = 0.6 * density + 0.4 * unique_norm;
            DensityPoint {
                bucket_start: MediaTime::from_micros(i as i64 * bucket.micros())
                    .unwrap_or(MediaTime::ZERO),
                message_count: counts[i] as u32,
                unique_chatters: chatters[i].len() as u32,
                density,
                engagement: engagement.clamp(0.0, 1.0),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests;
