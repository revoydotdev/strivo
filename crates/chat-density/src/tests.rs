use super::*;

/// A whole number of microseconds from a seconds literal, which is how a
/// test states a fixture in the unit a person thinks in.
fn secs(v: f32) -> MediaTime {
    MediaTime::from_secs_f64(f64::from(v)).expect("a non-negative fixture time")
}

#[test]
fn parse_csv_skips_header_and_handles_missing_message() {
    let csv = "time_sec,user,message\n12.5,alice,hello\n42.0,bob,";
    let evs = parse_csv_log(csv);
    assert_eq!(evs.len(), 2);
    assert_eq!(evs[0].user, "alice");
    assert_eq!(evs[0].at, secs(12.5));
    assert_eq!(evs[1].message, "");
}

#[test]
fn parse_csv_handles_numeric_first_row_as_data() {
    let csv = "5,alice,hi";
    let evs = parse_csv_log(csv);
    assert_eq!(evs.len(), 1);
    assert_eq!(evs[0].user, "alice");
}

#[test]
fn parse_csv_skips_malformed_lines() {
    let csv = "5,alice,hi\nbroken\n10,bob,";
    let evs = parse_csv_log(csv);
    assert_eq!(evs.len(), 2);
}

#[test]
fn parse_irc_skips_lines_without_privmsg() {
    let log = "PING :tmi.twitch.tv\n@badge-info=;tmi-sent-ts=1000 :nick!nick@nick.tmi.twitch.tv JOIN #chan\n";
    let evs = parse_irc_log(log, 0);
    assert!(evs.is_empty());
}

#[test]
fn parse_irc_extracts_nick_and_message_with_timestamp() {
    let log = "@badge-info=;tmi-sent-ts=1500 :alice!alice@alice.tmi.twitch.tv PRIVMSG #chan :hello world\n";
    let evs = parse_irc_log(log, 1000);
    assert_eq!(evs.len(), 1);
    assert_eq!(evs[0].user, "alice");
    assert_eq!(evs[0].message, "hello world");
    assert_eq!(evs[0].at, secs(0.5));
}

#[test]
fn parse_irc_skips_events_before_stream_start() {
    // Event ts (500ms) is before the stream-start (1000ms) — should
    // be discarded so it doesn't pile into bucket 0.
    let log = "@badge-info=;tmi-sent-ts=500 :alice!alice@alice.tmi.twitch.tv PRIVMSG #chan :hi\n";
    let evs = parse_irc_log(log, 1000);
    assert!(evs.is_empty());
}

#[test]
fn parse_irc_skips_events_with_no_timestamp() {
    let log = ":alice!alice@alice.tmi.twitch.tv PRIVMSG #chan :hi\n";
    let evs = parse_irc_log(log, 0);
    assert!(evs.is_empty(), "got {evs:?}");
}

#[test]
fn density_returns_empty_for_zero_duration() {
    let evs = vec![ChatEvent {
        at: secs(0.0),
        user: "a".into(),
        message: "".into(),
        weight: 1.0,
    }];
    assert!(compute_density(&evs, secs(0.0), secs(30.0)).is_empty());
}

#[test]
fn density_buckets_events_correctly() {
    let evs = vec![
        ChatEvent {
            at: secs(5.0),
            user: "a".into(),
            message: "".into(),
            weight: 1.0,
        },
        ChatEvent {
            at: secs(7.0),
            user: "b".into(),
            message: "".into(),
            weight: 1.0,
        },
        ChatEvent {
            at: secs(35.0),
            user: "a".into(),
            message: "".into(),
            weight: 1.0,
        },
    ];
    let pts = compute_density(&evs, secs(60.0), secs(30.0));
    assert_eq!(pts.len(), 2);
    assert_eq!(pts[0].message_count, 2);
    assert_eq!(pts[0].unique_chatters, 2);
    assert_eq!(pts[1].message_count, 1);
}

#[test]
fn density_engagement_lifts_high_unique_chatters() {
    // Bucket 0 has 10 messages from 10 unique chatters; bucket 1
    // has 10 messages from 1 chatter (bot). Engagement should
    // rank bucket 0 above bucket 1 even though message_count ties.
    let mut evs: Vec<ChatEvent> = Vec::new();
    for i in 0..10 {
        evs.push(ChatEvent {
            at: secs(i as f32),
            user: format!("user_{i}"),
            message: "".into(),
            weight: 1.0,
        });
    }
    for _ in 0..10 {
        evs.push(ChatEvent {
            at: secs(35.0),
            user: "spambot".into(),
            message: "".into(),
            weight: 1.0,
        });
    }
    let pts = compute_density(&evs, secs(60.0), secs(30.0));
    assert_eq!(pts.len(), 2);
    assert!(pts[0].engagement > pts[1].engagement);
}

#[test]
fn density_clamps_to_unit_range() {
    let evs = (0..50)
        .map(|i| ChatEvent {
            at: secs(i as f32),
            user: format!("u{}", i % 5),
            message: "".into(),
            weight: 1.0,
        })
        .collect::<Vec<_>>();
    let pts = compute_density(&evs, secs(60.0), secs(30.0));
    for p in &pts {
        assert!((0.0..=1.0).contains(&p.density));
        assert!((0.0..=1.0).contains(&p.engagement));
    }
}

/// A CSV row whose first column is not a real position on a timeline.
///
/// This is where the negative-time case went: it used to be an event that
/// reached the analyzer carrying `-5.0` and was dropped there by an
/// `if ev.time_sec < 0.0 { continue }` guard. A `MediaTime` cannot be
/// negative, so the row is now refused at the parse boundary instead,
/// and a good row either side of it is unaffected.
#[test]
fn a_csv_row_that_is_not_a_coordinate_is_refused_at_the_boundary() {
    let csv = "-5,a,before\nNaN,b,not-a-number\ninf,c,infinite\n10,d,ok";
    let evs = parse_csv_log(csv);
    assert_eq!(
        evs.iter().map(|e| e.user.as_str()).collect::<Vec<_>>(),
        vec!["d"],
        "only the one row that names a real media coordinate survives"
    );
    assert_eq!(evs[0].at, secs(10.0));
}

/// An IRC event stamped before the recorded stream start.
///
/// The other half of the silent drop the plan names: the event has no
/// media coordinate, so it is dropped where that is decided and cannot
/// be mistaken for an event at the very beginning of the stream.
#[test]
fn an_event_before_the_stream_start_has_no_media_coordinate() {
    let before = parse_irc_log(
        "@badge-info=;tmi-sent-ts=500 :a!a@a PRIVMSG #c :early\n",
        1_000,
    );
    assert!(before.is_empty(), "got {before:?}");
    let after = parse_irc_log(
        "@badge-info=;tmi-sent-ts=1000 :a!a@a PRIVMSG #c :on-time\n",
        1_000,
    );
    assert_eq!(after.len(), 1);
    assert_eq!(
        after[0].at,
        MediaTime::ZERO,
        "the first frame, not a late row"
    );
}

/// A long capture keeps sub-millisecond placement.
///
/// The float version computed `(ms - start) as f32 / 1000.0`, and an
/// `f32` cannot hold a millisecond offset past about 16 777 ms exactly,
/// so two events a millisecond apart landed on the same coordinate.
#[test]
fn a_long_capture_keeps_millisecond_precision() {
    let late = 20_000_000_u64;
    let log = format!(
        "@badge-info=;tmi-sent-ts={} :a!a@a PRIVMSG #c :one\n             @badge-info=;tmi-sent-ts={} :b!b@b PRIVMSG #c :two\n",
        late,
        late + 1
    );
    let evs = parse_irc_log(&log, 0);
    assert_eq!(evs.len(), 2);
    assert_eq!(evs[0].at, secs(20_000.0));
    assert_eq!(
        evs[1].at.micros() - evs[0].at.micros(),
        1_000,
        "a millisecond apart is a millisecond apart after five hours"
    );
}

#[test]
fn density_respects_event_weight() {
    // Two messages in bucket 0 with weight 0.5 each → message_count
    // floored to 1 (sum = 1.0 → as u32 = 1).
    let evs = vec![
        ChatEvent {
            at: secs(1.0),
            user: "a".into(),
            message: "".into(),
            weight: 0.5,
        },
        ChatEvent {
            at: secs(2.0),
            user: "b".into(),
            message: "".into(),
            weight: 0.5,
        },
    ];
    let pts = compute_density(&evs, secs(30.0), secs(30.0));
    assert_eq!(pts.len(), 1);
    assert_eq!(pts[0].message_count, 1);
    assert_eq!(pts[0].unique_chatters, 2);
}

#[test]
fn density_unique_chatters_dedupes_users() {
    let evs = vec![
        ChatEvent {
            at: secs(1.0),
            user: "alice".into(),
            message: "".into(),
            weight: 1.0,
        },
        ChatEvent {
            at: secs(2.0),
            user: "alice".into(),
            message: "".into(),
            weight: 1.0,
        },
        ChatEvent {
            at: secs(3.0),
            user: "alice".into(),
            message: "".into(),
            weight: 1.0,
        },
    ];
    let pts = compute_density(&evs, secs(30.0), secs(30.0));
    assert_eq!(pts[0].message_count, 3);
    assert_eq!(pts[0].unique_chatters, 1);
}
