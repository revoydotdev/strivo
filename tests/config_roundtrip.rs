//! Integration coverage for `AppConfig` persistence semantics:
//! round-trip, atomic save + rotating backup, and recovery from a
//! malformed TOML by falling back to `.backup`.
#![allow(clippy::field_reassign_with_default)]

use std::fs;
use strivo_core::config::{AppConfig, AutoDownloadUploadsEntry, ThemeRef};
use tempfile::TempDir;

fn seeded_config(recording_dir: std::path::PathBuf) -> AppConfig {
    let mut cfg = AppConfig::default();
    cfg.recording_dir = recording_dir;
    cfg.poll_interval_secs = 42;
    cfg.theme = ThemeRef::Named("neon".into());
    cfg
}

#[test]
fn load_creates_default_when_missing() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.toml");
    assert!(!path.exists());

    let cfg = AppConfig::load(Some(&path)).expect("load should create defaults");
    assert!(path.exists(), "default config must be persisted");
    assert_eq!(cfg.config_path.as_deref(), Some(path.as_path()));
}

#[test]
fn save_then_load_roundtrip() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.toml");

    let seeded = seeded_config(tmp.path().join("recordings"));
    seeded.save(Some(&path)).expect("save");

    let loaded = AppConfig::load(Some(&path)).expect("load");
    assert_eq!(loaded.poll_interval_secs, 42);
    assert_eq!(loaded.recording_dir, seeded.recording_dir);
}

#[test]
fn auto_download_uploads_roundtrips() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.toml");

    let mut seeded = seeded_config(tmp.path().join("recordings"));
    seeded.auto_download_uploads.push(AutoDownloadUploadsEntry {
        platform: "Twitch".into(),
        channel_id: "123".into(),
        channel_name: "some_streamer".into(),
    });
    seeded.save(Some(&path)).expect("save");

    let loaded = AppConfig::load(Some(&path)).expect("load");
    assert_eq!(loaded.auto_download_uploads.len(), 1);
    assert_eq!(loaded.auto_download_uploads[0].channel_id, "123");
    assert_eq!(loaded.auto_download_uploads[0].platform, "Twitch");
}

#[test]
fn config_without_auto_download_uploads_still_parses() {
    // An existing config.toml written before this field existed must keep
    // loading — `#[serde(default)]` is the contract under test.
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.toml");
    fs::write(
        &path,
        "recording_dir = \"/tmp/recs\"\npoll_interval_secs = 60\n",
    )
    .unwrap();

    let loaded = AppConfig::load(Some(&path)).expect("load");
    assert!(loaded.auto_download_uploads.is_empty());
}

#[test]
fn save_rotates_previous_into_backup() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.toml");
    let backup = {
        let mut s = path.clone().into_os_string();
        s.push(".backup");
        std::path::PathBuf::from(s)
    };

    let mut cfg = seeded_config(tmp.path().join("recs1"));
    cfg.save(Some(&path)).unwrap();
    let first = fs::read_to_string(&path).unwrap();

    cfg.poll_interval_secs = 999;
    cfg.save(Some(&path)).unwrap();

    assert!(backup.exists(), ".backup must exist after second save");
    let backup_contents = fs::read_to_string(&backup).unwrap();
    assert_eq!(
        backup_contents, first,
        ".backup must hold the prior known-good contents"
    );
}

#[test]
fn load_recovers_from_malformed_toml_via_backup() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.toml");

    // Two saves -> a valid `.backup` now exists alongside the live file.
    let cfg = seeded_config(tmp.path().join("recs"));
    cfg.save(Some(&path)).unwrap();
    cfg.save(Some(&path)).unwrap();

    // Now corrupt the live file.
    fs::write(&path, "this is not ][ valid { toml").unwrap();

    let recovered = AppConfig::load(Some(&path)).expect("must recover, not panic");
    assert_eq!(
        recovered.poll_interval_secs, 42,
        "recovery must restore backed-up values"
    );
}

#[test]
fn theme_accepts_legacy_string_form() {
    let src = r##"
recording_dir = "/tmp/x"
poll_interval_secs = 60
theme = "tokyo-night"
"##;
    let cfg: AppConfig = toml::from_str(src).expect("parse legacy form");
    assert_eq!(cfg.theme.name(), "tokyo-night");
    assert!(cfg.theme.colors().is_empty());
}

#[test]
fn theme_accepts_rich_table_with_overrides() {
    let src = r##"
recording_dir = "/tmp/x"
poll_interval_secs = 60
[theme]
name = "neon"
[theme.colors]
primary = "#00FF00"
[theme.ansi]
red = "#FF5555"
"##;
    let cfg: AppConfig = toml::from_str(src).expect("parse rich form");
    assert_eq!(cfg.theme.name(), "neon");
    assert_eq!(
        cfg.theme.colors().get("primary"),
        Some(&"#00FF00".to_string())
    );
    assert_eq!(cfg.theme.ansi().get("red"), Some(&"#FF5555".to_string()));
}

#[test]
fn load_falls_back_to_defaults_when_no_backup() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.toml");
    fs::write(&path, "garbage { toml").unwrap();

    // No `.backup`; loader quarantines the bad file and returns defaults
    // rather than exploding.
    let recovered = AppConfig::load(Some(&path)).expect("must fall back to defaults");
    assert!(recovered.twitch.is_none());
}

#[test]
fn reset_to_defaults_preserves_credentials() {
    use strivo_core::config::{ScheduleEntry, TwitchConfig};

    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.toml");
    let mut cfg = seeded_config(tmp.path().join("recordings"));
    cfg.config_path = Some(path.clone());
    cfg.twitch = Some(TwitchConfig {
        client_id: "kept".into(),
        client_secret: "kept-secret".into(),
    });
    cfg.schedule.push(ScheduleEntry {
        channel: "twitch:foo".into(),
        cron: "0 20 * * *".into(),
        duration: "1h".into(),
    });
    cfg.poll_interval_secs = 7777;

    cfg.reset_to_defaults();

    // Credentials survive.
    assert_eq!(cfg.twitch.as_ref().unwrap().client_id, "kept");
    // Path memory survives so save() still works.
    assert_eq!(cfg.config_path.as_deref(), Some(path.as_path()));
    // Everything else reverts.
    assert!(cfg.schedule.is_empty());
    assert_ne!(cfg.poll_interval_secs, 7777);
}
