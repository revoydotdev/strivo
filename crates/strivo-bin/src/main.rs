// Crate-wide clippy allows — these flag style preferences, not bugs, and
// touching them now would be pure churn. Re-enable per-module when rewriting
// the relevant code.
#![allow(clippy::field_reassign_with_default)]
#![allow(clippy::type_complexity)]

mod cli;

use strivo_core::{config, daemon, ipc, recording};
// `plugin` is only referenced by the Creator Edition plugin registration.
#[cfg(feature = "creator")]
use strivo_core::plugin;

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser};

use crate::cli::{Command, ConfigAction, CookiePlatform, LogAction, SetupAction};

#[tokio::main]
async fn main() -> Result<()> {
    let args = cli::Args::parse();

    if let Some(ref cmd) = args.command {
        return handle_command(cmd, args.config.as_deref()).await;
    }

    run_default_webui(args).await
}

/// New default: launch the webui. Spawns the daemon in-process if it is
/// not already running, waits briefly for the IPC socket to come up, then
/// serves the SPA. Polling-then-serve keeps the entry point one command
/// for users; advanced setups still split daemon and webui across
/// processes with `strivo daemon` + `strivo serve` as before.
async fn run_default_webui(args: cli::Args) -> Result<()> {
    config::AppConfig::load(args.config.as_deref()).context("load config")?;
    let config_path = args.config.clone();

    if !ipc::is_daemon_running() {
        // Same plugin host bring-up as `Command::Daemon`. Spawned as a
        // task so the webui can run alongside in the same process; the
        // task outlives the await below and only exits when the daemon
        // shuts down.
        let daemon_config_path = config_path.clone();
        tokio::spawn(async move {
            #[allow(unused_mut)]
            let mut host = strivo_core::daemon::DaemonPluginHost::new();
            #[cfg(feature = "creator")]
            register_first_party_plugins(&mut host.registry);
            if let Err(e) = daemon::run_with_plugins_at(host, daemon_config_path.as_deref()).await {
                tracing::error!("daemon exited: {e}");
            }
        });

        // Wait up to ~3s for the daemon to bind its socket. The probe is
        // intentionally cheap; once we move past this, handle_serve will
        // fail loudly if the daemon never came up.
        for _ in 0..30 {
            if ipc::is_daemon_running() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    handle_serve("127.0.0.1:8181", None, config_path.as_deref()).await
}

async fn handle_command(cmd: &Command, config_path: Option<&std::path::Path>) -> Result<()> {
    match cmd {
        Command::Daemon => {
            // Plugins boot inside the daemon process: init_all opens
            // each plugin's SQLite DB, status_line contributions and
            // PluginRpc dispatch hang off here.
            #[allow(unused_mut)]
            let mut host = strivo_core::daemon::DaemonPluginHost::new();
            #[cfg(feature = "creator")]
            register_first_party_plugins(&mut host.registry);
            daemon::run_with_plugins_at(host, config_path).await
        }
        Command::Enable {
            daemon_only,
            envchain,
        } => handle_enable(config_path, *daemon_only, envchain.as_deref()).await,
        Command::Disable => handle_disable().await,
        Command::Status => handle_status(),
        Command::Config { action } => handle_config_command(action, config_path),
        Command::Log { action } => handle_log_command(action).await,
        Command::Search { query } => handle_search(query, config_path),
        Command::Doctor => handle_doctor().await,
        Command::Setup { action } => handle_setup(action, config_path).await,
        Command::Serve { bind, api_key } => {
            handle_serve(bind, api_key.as_deref(), config_path).await
        }
        Command::Chapter { file, every } => handle_chapter(file, *every),
        Command::Import { source } => handle_import(source, config_path),
        Command::Merge { output, sources } => handle_merge(output, sources),
        Command::Thumbnail { file, seek } => handle_thumbnail(file, *seek).await,
        Command::TwitchRewind {
            channel,
            sample_secs,
            out,
        } => handle_twitch_rewind(channel, *sample_secs, out.clone(), config_path).await,
        Command::Completions { shell } => handle_completions(*shell),
        Command::Man => handle_man(),
        Command::Pull {
            target,
            format,
            since,
            max,
            force,
            no_transcribe,
        } => {
            handle_pull(
                target,
                format.as_deref(),
                since.as_deref(),
                *max,
                *force,
                *no_transcribe,
                config_path,
            )
            .await
        }
    }
}

async fn handle_setup(action: &SetupAction, config_path: Option<&std::path::Path>) -> Result<()> {
    match action {
        SetupAction::Cookies {
            platform,
            browser,
            profile,
            keyring,
            force,
        } => {
            import_browser_cookies(
                *platform,
                browser,
                profile.as_deref(),
                keyring.as_deref(),
                *force,
                config_path,
            )
            .await
        }
    }
}

async fn import_browser_cookies(
    platform: CookiePlatform,
    browser: &str,
    profile: Option<&str>,
    keyring: Option<&str>,
    force: bool,
    config_path: Option<&std::path::Path>,
) -> Result<()> {
    use std::process::Stdio;
    use strivo_core::config::{AppConfig, PatreonConfig, YouTubeConfig};

    const BROWSERS: &[&str] = &[
        "brave", "chrome", "chromium", "edge", "firefox", "opera", "safari", "vivaldi", "whale",
    ];
    const KEYRINGS: &[&str] = &[
        "basictext",
        "gnomekeyring",
        "kwallet",
        "kwallet5",
        "kwallet6",
    ];

    let browser = browser.trim().to_ascii_lowercase();
    anyhow::ensure!(
        BROWSERS.contains(&browser.as_str()),
        "unsupported browser '{browser}'; choose one of: {}",
        BROWSERS.join(", ")
    );
    if let Some(value) = keyring {
        anyhow::ensure!(
            KEYRINGS.contains(&value),
            "unsupported keyring '{value}'; choose one of: {}",
            KEYRINGS.join(", ")
        );
    }
    anyhow::ensure!(
        profile.map_or(true, |value| !value.trim().is_empty()),
        "--profile cannot be empty"
    );

    let mut source = browser;
    if let Some(value) = keyring {
        source.push('+');
        source.push_str(value);
    }
    if let Some(value) = profile {
        source.push(':');
        source.push_str(value.trim());
    }

    let mut cfg = AppConfig::load(config_path).context("load config")?;
    let cookie_dir = AppConfig::config_dir().join("cookies");
    std::fs::create_dir_all(&cookie_dir)
        .with_context(|| format!("create {}", cookie_dir.display()))?;
    let platform_name = match platform {
        CookiePlatform::Youtube => "youtube",
        CookiePlatform::Patreon => "patreon",
    };
    let output = cookie_dir.join(format!("{platform_name}.txt"));
    anyhow::ensure!(
        force || !output.exists(),
        "{} already exists; use --force to refresh it",
        output.display()
    );

    println!("Importing {platform_name} session from {source}…");
    println!("Your browser may need to be closed briefly if its cookie database is locked.");
    let probe_url = match platform {
        CookiePlatform::Youtube => "https://www.youtube.com/feed/subscriptions",
        CookiePlatform::Patreon => "https://www.patreon.com/home",
    };
    let output_arg = output.to_string_lossy().into_owned();
    let status = tokio::process::Command::new("yt-dlp")
        .args([
            "--cookies-from-browser",
            &source,
            "--cookies",
            &output_arg,
            "--skip-download",
            "--playlist-items",
            "0",
            probe_url,
        ])
        .stdin(Stdio::null())
        .status()
        .await
        .context("run yt-dlp (install it first, then retry `strivo setup cookies`)")?;
    if !status.success() || !output.is_file() {
        // yt-dlp may create a partial jar before browser decryption or the
        // account probe fails. Do not make the next retry require --force.
        let _ = std::fs::remove_file(&output);
        anyhow::bail!(
            "browser session import failed; close the browser, confirm you are signed in, and retry"
        );
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&output, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("secure {}", output.display()))?;
    }

    #[cfg(windows)]
    {
        // Windows files inherit their parent directory's ACL — there is no
        // chmod. `icacls` ships with every Windows install, so it is used
        // in preference to pulling in the full Win32 ACL/security-descriptor
        // API just to replicate `chmod 0600`: `/inheritance:r` strips the
        // inherited ACEs (that's the world-readable gap), then
        // `/grant:r <user>:F` re-grants access solely to the account
        // running strivo, the closest equivalent to owner-only read/write.
        //
        // If this fails for any reason (icacls missing/blocked, unresolved
        // identity), the file was already written by yt-dlp — warn loudly
        // rather than silently leaving an exported session cookie readable
        // by every account on the machine.
        let user = std::env::var("USERDOMAIN")
            .ok()
            .filter(|d| !d.is_empty())
            .zip(std::env::var("USERNAME").ok())
            .map(|(domain, name)| format!("{domain}\\{name}"))
            .or_else(|| std::env::var("USERNAME").ok());
        let secured = match user {
            Some(user) => std::process::Command::new("icacls")
                .arg(&output)
                .args(["/inheritance:r", "/grant:r", &format!("{user}:F")])
                .status()
                .map(|s| s.success())
                .unwrap_or(false),
            None => false,
        };
        if !secured {
            eprintln!(
                "warning: could not restrict permissions on {}; the exported \
                 session cookie may be readable by other accounts on this \
                 machine. Move it to a private location or set its \
                 permissions manually via File Properties > Security.",
                output.display()
            );
        }
    }

    match platform {
        CookiePlatform::Youtube => {
            let current = cfg.youtube.take();
            cfg.youtube = Some(YouTubeConfig {
                client_id: current
                    .as_ref()
                    .map(|v| v.client_id.clone())
                    .unwrap_or_default(),
                client_secret: current
                    .as_ref()
                    .map(|v| v.client_secret.clone())
                    .unwrap_or_default(),
                cookies_path: Some(output.clone()),
                websub_callback_url: current.and_then(|v| v.websub_callback_url),
            });
        }
        CookiePlatform::Patreon => {
            let current = cfg.patreon.take();
            cfg.patreon = Some(PatreonConfig {
                client_id: current
                    .as_ref()
                    .map(|v| v.client_id.clone())
                    .unwrap_or_default(),
                client_secret: current
                    .as_ref()
                    .map(|v| v.client_secret.clone())
                    .unwrap_or_default(),
                poll_interval_secs: current
                    .as_ref()
                    .map(|v| v.poll_interval_secs)
                    .unwrap_or(300),
                cookies_path: Some(output.clone()),
            });
        }
    }
    cfg.save(config_path)
        .context("save cookie path to config")?;
    println!("✓ Session imported and configured at {}", output.display());
    println!("  Re-run this command with --force whenever the browser session expires.");
    Ok(())
}

fn parse_since(s: &str) -> Result<chrono::DateTime<chrono::Utc>> {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&chrono::Utc));
    }
    let (num_part, unit) = s.split_at(s.len().saturating_sub(1));
    let n: i64 = num_part
        .parse()
        .with_context(|| format!("bad --since duration: {s}"))?;
    let dur = match unit {
        "h" => chrono::Duration::hours(n),
        "d" => chrono::Duration::days(n),
        "w" => chrono::Duration::weeks(n),
        _ => anyhow::bail!("unknown --since suffix '{unit}' (use h/d/w or RFC3339)"),
    };
    Ok(chrono::Utc::now() - dur)
}

async fn handle_pull(
    target: &str,
    format_override: Option<&str>,
    since: Option<&str>,
    max: Option<usize>,
    force: bool,
    no_transcribe: bool,
    config_path: Option<&std::path::Path>,
) -> Result<()> {
    use strivo_core::config::{AppConfig, RecordingFormat};
    use strivo_core::platform::{Platform, PlatformKind, VodEntry};
    use strivo_core::recording::catalog::{self, CatalogPullOptions};
    use strivo_core::recording::persist::PersistDb;

    let (platform_str, channel_id) = target.split_once(':').ok_or_else(|| {
        anyhow::anyhow!("target must be `<platform>:<channel_id>`, got `{target}`")
    })?;
    let platform = match platform_str.to_lowercase().as_str() {
        "youtube" | "yt" => PlatformKind::YouTube,
        "twitch" | "tw" => PlatformKind::Twitch,
        "patreon" | "pt" => PlatformKind::Patreon,
        other => anyhow::bail!("unknown platform `{other}` (try youtube/twitch/patreon)"),
    };
    let since = since.map(parse_since).transpose()?;

    let config = AppConfig::load(config_path).context("load config")?;

    // Resolve format: per-channel override → CLI flag → global default → built-ins.
    let cli_override = format_override.map(|f| RecordingFormat {
        format: Some(f.to_string()),
        ..Default::default()
    });
    let chan_override = config
        .auto_record_channels
        .iter()
        .find(|c| c.channel_id == channel_id && c.platform == platform.to_string())
        .and_then(|c| c.format.clone());
    let resolved = RecordingFormat::resolved(
        cli_override.as_ref().or(chan_override.as_ref()),
        &config.recording.format,
    );

    let cookies_path = if matches!(platform, PlatformKind::YouTube) {
        config.youtube.as_ref().and_then(|y| y.cookies_path.clone())
    } else {
        None
    };

    let db_path = AppConfig::data_dir().join("jobs.db");
    let db = PersistDb::open(&db_path).context("open jobs.db")?;

    println!("Enumerating {platform} catalog for {channel_id}…");
    let vods: Vec<VodEntry> = match platform {
        PlatformKind::YouTube => {
            let yt_cfg = config
                .youtube
                .clone()
                .context("youtube section missing in config")?;
            let yt = strivo_core::platform::youtube::YouTubePlatform::new(
                yt_cfg.client_id,
                yt_cfg.client_secret,
                yt_cfg.cookies_path.clone(),
            );
            yt.load_stored_tokens().await.context("youtube auth")?;
            yt.fetch_channel_vods(channel_id, since, max).await?
        }
        PlatformKind::Twitch => {
            let tw_cfg = config
                .twitch
                .clone()
                .context("twitch section missing in config")?;
            let tw = strivo_core::platform::twitch::TwitchPlatform::new(
                tw_cfg.client_id,
                tw_cfg.client_secret,
            );
            tw.load_stored_tokens().await.context("twitch auth")?;
            tw.fetch_channel_vods(channel_id, since, max).await?
        }
        PlatformKind::Patreon => {
            let pt_cfg = config
                .patreon
                .clone()
                .context("patreon section missing in config")?;
            let pt = strivo_core::platform::patreon::PatreonClient::new(
                pt_cfg.client_id,
                pt_cfg.client_secret,
            );
            pt.load_stored_tokens().await.context("patreon auth")?;
            pt.fetch_channel_vods(channel_id, since, max).await?
        }
    };

    if vods.is_empty() {
        println!("No matching VODs found.");
        return Ok(());
    }
    println!("Discovered {} VOD(s).", vods.len());

    // `--no-transcribe` only gates the Crunchr tandem hook, which exists in
    // Creator Edition; in the pure-PVR build the flag is accepted but inert.
    #[cfg(not(feature = "creator"))]
    let _ = no_transcribe;

    let opts = CatalogPullOptions {
        root: config.recording_dir.clone(),
        channel_name: channel_id.to_string(),
        format: resolved,
        cookies_path,
        force,
        #[cfg(feature = "creator")]
        crunchr_auto: !no_transcribe && config.crunchr.enabled,
        #[cfg(not(feature = "creator"))]
        crunchr_auto: false,
    };

    let report = catalog::run_pull(&db, vods, &opts, None, None).await?;
    println!(
        "Done. discovered={} skipped={} downloaded={} failed={}",
        report.discovered,
        report.skipped_existing,
        report.downloaded,
        report.failed.len()
    );
    for (id, err) in &report.failed {
        eprintln!("  failed: {id} — {err}");
    }
    Ok(())
}

fn handle_completions(shell: clap_complete::Shell) -> Result<()> {
    let mut cmd = cli::Args::command();
    let name = cmd.get_name().to_string();
    clap_complete::generate(shell, &mut cmd, name, &mut std::io::stdout());
    Ok(())
}

fn handle_man() -> Result<()> {
    let cmd = cli::Args::command();
    let man = clap_mangen::Man::new(cmd);
    man.render(&mut std::io::stdout())?;
    Ok(())
}

fn handle_import(source: &cli::ImportSource, config_path: Option<&std::path::Path>) -> Result<()> {
    use strivo_core::config::import::{parse_obs_export, parse_streamlink_lines, Candidate};

    let (candidates, apply, source_path) = match source {
        cli::ImportSource::Obs { file, apply } => (parse_obs_export(file)?, *apply, file.clone()),
        cli::ImportSource::Streamlink { file, apply } => {
            (parse_streamlink_lines(file)?, *apply, file.clone())
        }
    };

    if candidates.is_empty() {
        println!("No channels discovered in {}", source_path.display());
        return Ok(());
    }

    println!("Discovered {} channel(s):", candidates.len());
    for c in &candidates {
        println!("  + {}:{}  ({})", c.platform, c.channel_id, c.channel_name);
    }

    if !apply {
        println!();
        println!("Dry-run. Pass --apply to write into config.toml.");
        return Ok(());
    }

    let mut cfg = config::AppConfig::load(config_path).context("load config")?;
    let mut added = 0usize;
    let mut skipped = 0usize;
    for c in candidates {
        let exists = cfg
            .auto_record_channels
            .iter()
            .any(|a| a.platform == c.platform && a.channel_id == c.channel_id);
        if exists {
            skipped += 1;
            continue;
        }
        cfg.auto_record_channels
            .push(Candidate::into_auto_record(c));
        added += 1;
    }
    cfg.save(config_path).context("save config")?;
    println!("Applied: {added} added, {skipped} already present.");
    Ok(())
}

fn handle_merge(output: &std::path::Path, sources: &[std::path::PathBuf]) -> Result<()> {
    use strivo_core::recording::segments::merge_segments;
    if sources.is_empty() {
        anyhow::bail!("provide at least one source file");
    }
    println!(
        "Merging {} segment(s) → {}",
        sources.len(),
        output.display()
    );
    merge_segments(sources, output)?;
    println!("ok");
    Ok(())
}

/// Debug subcommand: resolve the Twitch rewind master playlist for a live
/// channel and optionally smoke-test it by pulling a few seconds with
/// ffmpeg. Verifies the GQL → Usher → segment chain end-to-end.
async fn handle_twitch_rewind(
    channel: &str,
    sample_secs: Option<u32>,
    out: Option<std::path::PathBuf>,
    config_path: Option<&std::path::Path>,
) -> Result<()> {
    use std::sync::Arc;
    use strivo_core::stream::twitch_rewind::RewindResolver;
    use tokio::sync::RwLock;

    let config = config::AppConfig::load(config_path).context("load config")?;
    let tw_cfg = config
        .twitch
        .clone()
        .context("twitch section missing in config")?;
    let tw = strivo_core::platform::twitch::TwitchPlatform::new(
        tw_cfg.client_id.clone(),
        tw_cfg.client_secret.clone(),
    );
    tw.load_stored_tokens()
        .await
        .context("load twitch tokens (run `strivo` once to authenticate)")?;

    let (channel_id, _display_name) = tw
        .lookup_channel_id_by_login(channel)
        .await
        .context("lookup channel id")?;
    println!("channel_id = {channel_id}");

    let token = strivo_core::config::credentials::get_secret("twitch_access_token")
        .ok()
        .flatten();
    let tw_arc = Arc::new(RwLock::new(tw));
    let resolver = RewindResolver::new(tw_arc, token);
    let stream = match resolver.resolve(&channel_id).await {
        Ok(s) => s,
        Err(e) => anyhow::bail!("rewind resolve failed: {e}"),
    };
    println!("video_id = {}", stream.video_id);
    if let Some(t) = stream.broadcast_started_at {
        println!("broadcast_started_at = {t}");
    }
    println!("master_url = {}", stream.master_url);

    if let Some(secs) = sample_secs {
        let out_path = out
            .unwrap_or_else(|| std::path::PathBuf::from(format!("./rewind-sample-{channel}.mkv")));
        println!(
            "\nffmpeg smoke test: pulling first {secs}s into {}...",
            out_path.display()
        );
        let status = tokio::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "warning", "-y", "-t"])
            .arg(secs.to_string())
            .args(["-i"])
            .arg(&stream.master_url)
            .args(["-c", "copy"])
            .arg(&out_path)
            .status()
            .await
            .context("spawn ffmpeg")?;
        if !status.success() {
            anyhow::bail!("ffmpeg exited with {status}");
        }
        let size = std::fs::metadata(&out_path).map(|m| m.len()).unwrap_or(0);
        println!("ok — wrote {} bytes to {}", size, out_path.display());
    }
    Ok(())
}

async fn handle_thumbnail(file: &std::path::Path, seek: f64) -> Result<()> {
    use strivo_core::recording::thumbnail;
    if !file.exists() {
        anyhow::bail!("file does not exist: {}", file.display());
    }
    if let Some(cached) = thumbnail::cached(file) {
        println!("cached: {}", cached.display());
        return Ok(());
    }
    let path = thumbnail::extract(file, seek)
        .await
        .context("extract thumbnail")?;
    println!("wrote: {}", path.display());
    Ok(())
}

fn handle_chapter(file: &std::path::Path, every: u64) -> Result<()> {
    use strivo_core::media::probe_file;
    use strivo_core::recording::chapters::{embed_chapters, every_n_minutes};

    if !file.exists() {
        anyhow::bail!("file does not exist: {}", file.display());
    }
    // Probe the duration so the chapter set reaches the end of the file.
    let info = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(probe_file(file))
        .context("ffprobe the recording duration")?;
    let duration = info.duration_secs;
    if duration <= 0.0 {
        anyhow::bail!("ffprobe reported zero duration; cannot chapter");
    }
    let chapters = every_n_minutes(duration, every);
    if chapters.is_empty() {
        anyhow::bail!("no chapters generated (file shorter than interval?)");
    }
    println!(
        "Embedding {} chapter(s) every {} min into {}",
        chapters.len(),
        every,
        file.display()
    );
    embed_chapters(file, &chapters)?;
    println!("ok");
    Ok(())
}

async fn handle_serve(
    bind: &str,
    api_key: Option<&str>,
    config_path: Option<&std::path::Path>,
) -> Result<()> {
    let addr: std::net::SocketAddr = bind
        .parse()
        .with_context(|| format!("invalid --bind {bind}"))?;

    // Key precedence: explicit --api-key > config.toml `[web] api_key`
    // > freshly generated + saved.
    let mut cfg = config::AppConfig::load(config_path).context("load config")?;
    let api_key = if let Some(k) = api_key {
        strivo_web::auth::ApiKey(k.to_string())
    } else if let Some(k) = cfg.web.api_key.clone() {
        strivo_web::auth::ApiKey(k)
    } else {
        let generated = strivo_web::auth::ApiKey::generate();
        cfg.web.api_key = Some(generated.as_str().to_string());
        if let Err(e) = cfg.save(config_path) {
            tracing::warn!("could not persist [web] api_key to config.toml: {e}");
        }
        generated
    };

    println!(
        "strivo-web on http://{} (X-Api-Key: {})",
        addr,
        api_key.as_str()
    );
    strivo_web::serve(strivo_web::ServeConfig {
        bind: addr,
        api_key,
        config_path: config_path.map(std::path::Path::to_path_buf),
    })
    .await
    .map_err(|e| anyhow::anyhow!("{e}"))
}

async fn handle_doctor() -> Result<()> {
    // Tool presence first, then platform credentials so the user sees
    // both gates in one shot. We're already inside the #[tokio::main]
    // runtime, so the credential probes are awaited directly — spinning
    // a nested current-thread runtime here panics with "Cannot start a
    // runtime from within a runtime".
    let creds_summary = probe_platform_credentials().await;

    let tools: &[(&str, &str)] = &[
        ("ffmpeg", "recording (required)"),
        ("ffprobe", "multitrack stream inspection (required)"),
        ("mpv", "playback (required)"),
        ("streamlink", "Twitch stream resolution (required)"),
        ("yt-dlp", "YouTube/Patreon resolution (required)"),
        ("whisper", "transcription (optional, Crunchr plugin)"),
    ];
    let mut missing_required = 0;
    println!("StriVo external tool check");
    println!("{}", "-".repeat(60));
    for (bin, purpose) in tools {
        match which::which(bin) {
            Ok(path) => println!("  ok      {:<12} {}  [{}]", bin, purpose, path.display()),
            Err(_) => {
                println!("  MISSING {:<12} {}", bin, purpose);
                if purpose.contains("required") {
                    missing_required += 1;
                }
            }
        }
    }
    println!();
    if missing_required > 0 {
        // The hint has to match the machine it is printed on. It read
        // "pacman -S ..." unconditionally, which is wrong on every platform
        // except Arch and actively confusing on Windows and macOS.
        let hint = if cfg!(target_os = "windows") {
            "winget install Gyan.FFmpeg mpv.net yt-dlp.yt-dlp  (streamlink: pip install streamlink)"
        } else if cfg!(target_os = "macos") {
            "brew install ffmpeg mpv streamlink yt-dlp"
        } else {
            "your package manager, e.g. pacman -S ffmpeg mpv streamlink yt-dlp \
             or apt install ffmpeg mpv streamlink yt-dlp"
        };
        println!("{missing_required} required tool(s) missing. Install via: {hint}");
        std::process::exit(1);
    } else {
        println!("All required tools present.");
    }

    println!();
    println!("Platform credentials");
    println!("{}", "-".repeat(60));
    print!("{creds_summary}");
    Ok(())
}

/// Test each configured platform's stored credentials by attempting a
/// lightweight authenticated call. The wizard-credential-validation
/// item — surfaces stale tokens immediately rather than waiting for the
/// next monitor poll to fail.
async fn probe_platform_credentials() -> String {
    use strivo_core::config::AppConfig;
    use strivo_core::platform::Platform;

    let cfg = match AppConfig::load(None) {
        Ok(c) => c,
        Err(e) => return format!("  could not load config: {e}\n"),
    };
    let mut out = String::new();

    if let Some(ref tw) = cfg.twitch {
        let plat = strivo_core::platform::twitch::TwitchPlatform::new(
            tw.client_id.clone(),
            tw.client_secret.clone(),
        );
        match plat
            .repair_stored_token(std::time::Duration::from_secs(15 * 60))
            .await
        {
            Ok(strivo_core::platform::twitch::TwitchTokenHealth::Valid {
                expires_in_secs,
            }) => match plat.fetch_followed_channels().await {
                Ok(channels) => {
                    out.push_str(&format!(
                        "  ok      twitch       {} followed channel(s); token valid for {}m\n",
                        channels.len(),
                        expires_in_secs / 60
                    ));
                }
                Err(e) => out.push_str(&format!(
                    "  STALE   twitch       call failed: {e}\n  hint: open Settings → Platforms to re-authenticate\n"
                )),
            },
            Ok(strivo_core::platform::twitch::TwitchTokenHealth::Refreshed) => out.push_str(
                "  fixed   twitch       stale token refreshed automatically and saved\n",
            ),
            Ok(strivo_core::platform::twitch::TwitchTokenHealth::LoginRequired { reason }) => {
                out.push_str(&format!(
                    "  STALE   twitch       {reason}\n  hint: verify client_id/client_secret, then open Settings → Platforms to approve a new login\n"
                ));
            }
            Err(e) => out.push_str(&format!("  ERROR   twitch       {e}\n")),
        }
    } else {
        out.push_str("  skip    twitch       not configured\n");
    }

    if let Some(ref yt) = cfg.youtube {
        let plat = strivo_core::platform::youtube::YouTubePlatform::new(
            yt.client_id.clone(),
            yt.client_secret.clone(),
            yt.cookies_path.clone(),
        );
        match plat.load_stored_tokens().await {
            Ok(true) => match plat.fetch_followed_channels().await {
                Ok(channels) => out.push_str(&format!(
                    "  ok      youtube      {} subscription(s)\n",
                    channels.len()
                )),
                Err(e) => out.push_str(&format!(
                    "  STALE   youtube      call failed: {e}\n  hint: re-auth or refresh the cookies file\n"
                )),
            },
            Ok(false) => out.push_str("  none    youtube      no stored token (run the wizard)\n"),
            Err(e) => out.push_str(&format!("  ERROR   youtube      {e}\n")),
        }
    } else {
        out.push_str("  skip    youtube      not configured\n");
    }

    if let Some(ref pt) = cfg.patreon {
        let plat = strivo_core::platform::patreon::PatreonClient::new(
            pt.client_id.clone(),
            pt.client_secret.clone(),
        );
        match plat.load_stored_tokens().await {
            Ok(true) => match plat.fetch_pledged_creators().await {
                Ok(creators) => out.push_str(&format!(
                    "  ok      patreon      {} pledged creator(s)\n",
                    creators.len()
                )),
                Err(e) => out.push_str(&format!("  STALE   patreon      call failed: {e}\n")),
            },
            Ok(false) => out.push_str("  none    patreon      no stored token\n"),
            Err(e) => out.push_str(&format!("  ERROR   patreon      {e}\n")),
        }
    } else {
        out.push_str("  skip    patreon      not configured\n");
    }

    out
}

/// Exit code reported when the daemon is not running.
///
/// 3 is the LSB "program is not running" convention, and it is what
/// `docs/DAEMON.md` has always told operators to branch on in monitoring
/// scripts. This used to return `Ok(())` in both branches, so every such
/// script reported healthy no matter what the daemon was doing — a
/// documented contract the binary quietly did not honour.
const EXIT_DAEMON_NOT_RUNNING: i32 = 3;

fn handle_status() -> Result<()> {
    if ipc::is_daemon_running() {
        println!("StriVo daemon is running");
        let pid_path = ipc::pid_path();
        if let Ok(pid) = std::fs::read_to_string(&pid_path) {
            println!("PID: {}", pid.trim());
        }
        println!("Socket: {}", ipc::socket_path().display());
        Ok(())
    } else {
        println!("StriVo daemon is not running");
        println!("Start with: strivo daemon");
        println!("Or enable as service: strivo enable");
        std::process::exit(EXIT_DAEMON_NOT_RUNNING);
    }
}

/// Find an executable on PATH. systemd requires an absolute path for the
/// first token of ExecStart, so a bare name will not do.
#[cfg(unix)]
fn resolve_on_path(name: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(name))
            .find(|candidate| candidate.is_file())
    })
}

/// Render the systemd user unit.
///
/// Split out from [`handle_enable`] so the unit text can be asserted on
/// without writing to the user's systemd directory or shelling out.
///
/// Two deliberate choices worth stating, because both differ from what this
/// generator emitted before:
///
/// * `ExecStart` runs the binary with **no subcommand**, which starts the
///   daemon and serves the web UI in one process — what plain `strivo` does,
///   and what a user actually wants running. The old unit ran `daemon` alone,
///   so an enabled service left nothing serving the SPA on 8181. `--daemon-only`
///   restores the previous shape for split setups.
/// * `Restart=always` with `StartLimitIntervalSec=0`, rather than
///   `Restart=on-failure` with systemd's default start limit. A PVR that gives
///   up permanently after five quick restarts is worse than one that keeps
///   trying: credentials may live in a keyring that is not unlocked until
///   desktop login, so early-boot starts legitimately fail for a while.
#[cfg(unix)]
fn render_systemd_unit(
    exec_quoted: &str,
    config_arg: &str,
    daemon_only: bool,
    envchain: Option<&str>,
) -> Result<String> {
    let description = if daemon_only {
        "StriVo Live Stream PVR Daemon"
    } else {
        "StriVo Live Stream PVR (daemon + web UI)"
    };
    let subcommand = if daemon_only { " daemon" } else { "" };

    let exec_start = match envchain {
        Some(namespace) => {
            anyhow::ensure!(
                !namespace.trim().is_empty(),
                "--envchain needs a namespace, e.g. --envchain mistral"
            );
            let bin = resolve_on_path("envchain").context(
                "--envchain was requested but envchain is not on PATH; install it or drop the flag",
            )?;
            format!(
                "{} {} {}{}{}",
                systemd_quote(bin.as_os_str()),
                namespace.trim(),
                exec_quoted,
                config_arg,
                subcommand
            )
        }
        None => format!("{exec_quoted}{config_arg}{subcommand}"),
    };

    Ok(format!(
        "[Unit]\n\
         Description={description}\n\
         Documentation=https://github.com/revoydotdev/strivo\n\
         After=network-online.target\n\
         Wants=network-online.target\n\
         # Credentials may come from a keyring that is locked until desktop\n\
         # login, so a boot-time start can fail for a while. Retry forever\n\
         # rather than letting systemd give up on the service permanently.\n\
         StartLimitIntervalSec=0\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={exec_start}\n\
         Restart=always\n\
         RestartSec=5\n\
         TimeoutStopSec=30\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n"
    ))
}

#[cfg(unix)]
async fn handle_enable(
    config_path: Option<&std::path::Path>,
    daemon_only: bool,
    envchain: Option<&str>,
) -> Result<()> {
    let exe = std::env::current_exe()?;
    let config_arg = config_path
        .map(|path| format!(" --config {}", systemd_quote(path.as_os_str())))
        .unwrap_or_default();
    let unit_content = render_systemd_unit(
        &systemd_quote(exe.as_os_str()),
        &config_arg,
        daemon_only,
        envchain,
    )?;

    let systemd_dir = dirs_home().join(".config/systemd/user");
    std::fs::create_dir_all(&systemd_dir)?;
    let unit_path = systemd_dir.join("strivo.service");
    std::fs::write(&unit_path, unit_content)?;
    println!("Wrote {}", unit_path.display());

    let status = std::process::Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status()?;
    if !status.success() {
        anyhow::bail!("systemctl daemon-reload failed");
    }

    let status = std::process::Command::new("systemctl")
        .args(["--user", "enable", "--now", "strivo.service"])
        .status()?;
    if !status.success() {
        anyhow::bail!("systemctl enable --now failed");
    }

    println!("StriVo daemon enabled and started");
    Ok(())
}

#[cfg(unix)]
fn systemd_quote(value: &std::ffi::OsStr) -> String {
    let value = value.to_string_lossy();
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(unix)]
async fn handle_disable() -> Result<()> {
    let status = std::process::Command::new("systemctl")
        .args(["--user", "disable", "--now", "strivo.service"])
        .status()?;
    if !status.success() {
        eprintln!("Warning: systemctl disable --now may have failed");
    }

    let unit_path = dirs_home().join(".config/systemd/user/strivo.service");
    if unit_path.exists() {
        std::fs::remove_file(&unit_path)?;
        println!("Removed {}", unit_path.display());
    }

    let _ = std::process::Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status();

    println!("StriVo daemon disabled");
    Ok(())
}

// Windows has no systemd-user-service concept. The closest equivalent that
// doesn't require an elevated SCM install (Windows Services live in a
// privileged, machine-wide registry and need `sc.exe create` run as
// Administrator, which would make `strivo enable` silently fail or prompt
// UAC for what is, on Linux/macOS, an unprivileged per-user action) is a
// Task Scheduler task registered under `/sc onlogon`: it starts the daemon
// automatically at user sign-in, same as the systemd `default.target` user
// unit does, without requiring admin rights. It does not get systemd's
// `Restart=on-failure` crash-recovery — Task Scheduler's failure-restart
// policy is a machine-task-only feature — so a crashed daemon stays down
// until the next sign-in. That gap is called out below rather than hidden.
#[cfg(windows)]
const WINDOWS_TASK_NAME: &str = "StriVo";

/// Build the `/tr` (task run) command line for `schtasks /create`, quoting
/// the exe path and optional `--config` value so paths with spaces survive
/// Task Scheduler's own re-tokenizing of the string.
#[cfg(windows)]
fn windows_task_run_command(
    exe: &std::path::Path,
    config_path: Option<&std::path::Path>,
) -> String {
    let mut tr = format!("\"{}\" daemon", exe.display());
    if let Some(path) = config_path {
        tr.push_str(&format!(" --config \"{}\"", path.display()));
    }
    tr
}

#[cfg(windows)]
async fn handle_enable(
    config_path: Option<&std::path::Path>,
    _daemon_only: bool,
    _envchain: Option<&str>,
) -> Result<()> {
    let exe = std::env::current_exe()?;
    let tr = windows_task_run_command(&exe, config_path);

    let status = std::process::Command::new("schtasks")
        .args([
            "/create",
            "/tn",
            WINDOWS_TASK_NAME,
            "/tr",
            &tr,
            "/sc",
            "onlogon",
            "/rl",
            "limited",
            "/f",
        ])
        .status()
        .context("failed to run schtasks (Task Scheduler unavailable?)")?;
    if !status.success() {
        anyhow::bail!("schtasks /create failed");
    }

    let status = std::process::Command::new("schtasks")
        .args(["/run", "/tn", WINDOWS_TASK_NAME])
        .status()?;
    if !status.success() {
        anyhow::bail!(
            "schtasks /run failed to start the daemon now (it will still start at next sign-in)"
        );
    }

    println!("StriVo daemon registered as a Task Scheduler task and started");
    println!(
        "Note: unlike the systemd unit on Linux/macOS, this task does not \
         auto-restart on crash — it (re)starts only at sign-in. Run \
         `strivo daemon` directly for a supervised long-running session."
    );
    Ok(())
}

#[cfg(windows)]
async fn handle_disable() -> Result<()> {
    let status = std::process::Command::new("schtasks")
        .args(["/end", "/tn", WINDOWS_TASK_NAME])
        .status();
    if status.map(|s| !s.success()).unwrap_or(true) {
        eprintln!("Warning: schtasks /end may have failed (task may not be running)");
    }

    let status = std::process::Command::new("schtasks")
        .args(["/delete", "/tn", WINDOWS_TASK_NAME, "/f"])
        .status()?;
    if !status.success() {
        eprintln!("Warning: schtasks /delete may have failed (task may not exist)");
    }

    println!("StriVo daemon disabled");
    Ok(())
}

#[cfg(not(any(unix, windows)))]
async fn handle_enable(
    _config_path: Option<&std::path::Path>,
    _daemon_only: bool,
    _envchain: Option<&str>,
) -> Result<()> {
    anyhow::bail!("`strivo enable` has no service-manager integration on this platform; run `strivo daemon` directly")
}

#[cfg(not(any(unix, windows)))]
async fn handle_disable() -> Result<()> {
    anyhow::bail!(
        "`strivo enable` has no service-manager integration on this platform; nothing to disable"
    )
}

#[cfg(unix)]
fn dirs_home() -> std::path::PathBuf {
    directories::UserDirs::new()
        .map(|d| d.home_dir().to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("~"))
}

fn handle_config_command(
    action: &ConfigAction,
    config_path: Option<&std::path::Path>,
) -> Result<()> {
    match action {
        ConfigAction::Path => {
            let path = config_path
                .map(|p| p.to_path_buf())
                .unwrap_or_else(config::AppConfig::config_path);
            println!("{}", path.display());
        }
        ConfigAction::List => {
            let cfg = config::AppConfig::load(config_path)?;
            println!("recording_dir = {:?}", cfg.recording_dir.display());
            println!("poll_interval_secs = {}", cfg.poll_interval_secs);
            println!("recording.transcode = {}", cfg.recording.transcode);
            println!(
                "recording.filename_template = {:?}",
                cfg.recording.filename_template
            );
            println!("theme = {:?}", cfg.theme.name());
            if let Some(ref tw) = cfg.twitch {
                println!("twitch.client_id = {:?}", tw.client_id);
                println!("twitch.client_secret = \"****\"");
            } else {
                println!("twitch = <not configured>");
            }
            if let Some(ref yt) = cfg.youtube {
                println!("youtube.client_id = {:?}", yt.client_id);
                println!("youtube.client_secret = \"****\"");
                if let Some(ref cp) = yt.cookies_path {
                    println!("youtube.cookies_path = {:?}", cp.display());
                }
            } else {
                println!("youtube = <not configured>");
            }
            if let Some(ref pa) = cfg.patreon {
                println!("patreon.client_id = {:?}", pa.client_id);
                println!("patreon.client_secret = \"****\"");
                println!("patreon.poll_interval = {}", pa.poll_interval_secs);
            } else {
                println!("patreon = <not configured>");
            }
            if !cfg.auto_record_channels.is_empty() {
                println!();
                println!("auto_record_channels:");
                for entry in &cfg.auto_record_channels {
                    println!(
                        "  {} / {} ({})",
                        entry.platform, entry.channel_name, entry.channel_id
                    );
                }
            }
            if !cfg.schedule.is_empty() {
                println!();
                println!("schedule:");
                for entry in &cfg.schedule {
                    println!(
                        "  {} | cron: {} | duration: {}",
                        entry.channel, entry.cron, entry.duration
                    );
                }
            }
        }
        ConfigAction::Get { key } => {
            let cfg = config::AppConfig::load(config_path)?;
            let value = config_get(&cfg, key)?;
            println!("{value}");
        }
        ConfigAction::Set { key, value } => {
            let mut cfg = config::AppConfig::load(config_path)?;
            config_set(&mut cfg, key, value)?;
            cfg.save(config_path)?;
            println!("Set {key} = {value}");
        }
        ConfigAction::Reset => {
            let old = config::AppConfig::load(config_path)?;
            let mut cfg = config::AppConfig::default();
            cfg.twitch = old.twitch;
            cfg.youtube = old.youtube;
            cfg.patreon = old.patreon;
            cfg.auto_record_channels = old.auto_record_channels;
            cfg.schedule = old.schedule;
            cfg.save(config_path)?;
            println!("Config reset to defaults (credentials preserved)");
        }
    }
    Ok(())
}

fn config_get(cfg: &config::AppConfig, key: &str) -> Result<String> {
    match key {
        "recording_dir" => Ok(cfg.recording_dir.to_string_lossy().to_string()),
        "poll_interval" | "poll_interval_secs" => Ok(cfg.poll_interval_secs.to_string()),
        "transcode" | "recording.transcode" => Ok(cfg.recording.transcode.to_string()),
        "filename_template" | "recording.filename_template" => {
            Ok(cfg.recording.filename_template.clone())
        }
        "twitch.client_id" => cfg
            .twitch
            .as_ref()
            .map(|t| t.client_id.clone())
            .ok_or_else(|| anyhow::anyhow!("Twitch not configured")),
        "twitch.client_secret" => cfg
            .twitch
            .as_ref()
            .map(|t| t.client_secret.clone())
            .ok_or_else(|| anyhow::anyhow!("Twitch not configured")),
        "youtube.client_id" => cfg
            .youtube
            .as_ref()
            .map(|y| y.client_id.clone())
            .ok_or_else(|| anyhow::anyhow!("YouTube not configured")),
        "youtube.client_secret" => cfg
            .youtube
            .as_ref()
            .map(|y| y.client_secret.clone())
            .ok_or_else(|| anyhow::anyhow!("YouTube not configured")),
        "youtube.cookies_path" => cfg
            .youtube
            .as_ref()
            .and_then(|y| y.cookies_path.as_ref())
            .map(|p| p.to_string_lossy().to_string())
            .ok_or_else(|| anyhow::anyhow!("YouTube cookies path not set")),
        "patreon.client_id" => cfg
            .patreon
            .as_ref()
            .map(|p| p.client_id.clone())
            .ok_or_else(|| anyhow::anyhow!("Patreon not configured")),
        "patreon.client_secret" => cfg
            .patreon
            .as_ref()
            .map(|p| p.client_secret.clone())
            .ok_or_else(|| anyhow::anyhow!("Patreon not configured")),
        "patreon.poll_interval" | "patreon.poll_interval_secs" => cfg
            .patreon
            .as_ref()
            .map(|p| p.poll_interval_secs.to_string())
            .ok_or_else(|| anyhow::anyhow!("Patreon not configured")),
        "theme" => Ok(cfg.theme.name().to_string()),
        _ => Err(anyhow::anyhow!(
            "Unknown key: {key}\n\nValid keys:\n  \
             recording_dir, poll_interval, transcode, filename_template, theme,\n  \
             twitch.client_id, twitch.client_secret,\n  \
             youtube.client_id, youtube.client_secret, youtube.cookies_path,\n  \
             patreon.client_id, patreon.client_secret, patreon.poll_interval"
        )),
    }
}

fn config_set(cfg: &mut config::AppConfig, key: &str, value: &str) -> Result<()> {
    match key {
        "recording_dir" => {
            cfg.recording_dir = std::path::PathBuf::from(value);
        }
        "poll_interval" | "poll_interval_secs" => {
            cfg.poll_interval_secs = value
                .parse()
                .map_err(|_| anyhow::anyhow!("Invalid integer: {value}"))?;
            if cfg.poll_interval_secs < 15 {
                anyhow::bail!("Poll interval must be >= 15 seconds");
            }
        }
        "transcode" | "recording.transcode" => {
            cfg.recording.transcode = match value {
                "true" | "on" | "1" | "yes" => true,
                "false" | "off" | "0" | "no" => false,
                _ => anyhow::bail!("Invalid boolean: {value} (use true/false/on/off)"),
            };
        }
        "filename_template" | "recording.filename_template" => {
            cfg.recording.filename_template = value.to_string();
        }
        "twitch.client_id" => {
            if let Some(ref mut tw) = cfg.twitch {
                tw.client_id = value.to_string();
            } else {
                cfg.twitch = Some(config::TwitchConfig {
                    client_id: value.to_string(),
                    client_secret: String::new(),
                });
            }
        }
        "twitch.client_secret" => {
            if let Some(ref mut tw) = cfg.twitch {
                tw.client_secret = value.to_string();
            } else {
                cfg.twitch = Some(config::TwitchConfig {
                    client_id: String::new(),
                    client_secret: value.to_string(),
                });
            }
        }
        "youtube.client_id" => {
            if let Some(ref mut yt) = cfg.youtube {
                yt.client_id = value.to_string();
            } else {
                cfg.youtube = Some(config::YouTubeConfig {
                    client_id: value.to_string(),
                    client_secret: String::new(),
                    cookies_path: None,
                    websub_callback_url: None,
                });
            }
        }
        "youtube.client_secret" => {
            if let Some(ref mut yt) = cfg.youtube {
                yt.client_secret = value.to_string();
            } else {
                cfg.youtube = Some(config::YouTubeConfig {
                    client_id: String::new(),
                    client_secret: value.to_string(),
                    cookies_path: None,
                    websub_callback_url: None,
                });
            }
        }
        "youtube.cookies_path" => {
            if let Some(ref mut yt) = cfg.youtube {
                yt.cookies_path = Some(std::path::PathBuf::from(value));
            } else {
                cfg.youtube = Some(config::YouTubeConfig {
                    client_id: String::new(),
                    client_secret: String::new(),
                    cookies_path: Some(std::path::PathBuf::from(value)),
                    websub_callback_url: None,
                });
            }
        }
        "youtube.websub_callback_url" => {
            if let Some(ref mut yt) = cfg.youtube {
                yt.websub_callback_url = Some(value.to_string());
            } else {
                cfg.youtube = Some(config::YouTubeConfig {
                    client_id: String::new(),
                    client_secret: String::new(),
                    cookies_path: None,
                    websub_callback_url: Some(value.to_string()),
                });
            }
        }
        "patreon.client_id" => {
            if let Some(ref mut pa) = cfg.patreon {
                pa.client_id = value.to_string();
            } else {
                cfg.patreon = Some(config::PatreonConfig {
                    client_id: value.to_string(),
                    client_secret: String::new(),
                    poll_interval_secs: 300,
                    cookies_path: None,
                });
            }
        }
        "patreon.client_secret" => {
            if let Some(ref mut pa) = cfg.patreon {
                pa.client_secret = value.to_string();
            } else {
                cfg.patreon = Some(config::PatreonConfig {
                    client_id: String::new(),
                    client_secret: value.to_string(),
                    poll_interval_secs: 300,
                    cookies_path: None,
                });
            }
        }
        "patreon.poll_interval" | "patreon.poll_interval_secs" => {
            let secs: u64 = value
                .parse()
                .map_err(|_| anyhow::anyhow!("Invalid integer: {value}"))?;
            if let Some(ref mut pa) = cfg.patreon {
                pa.poll_interval_secs = secs;
            } else {
                cfg.patreon = Some(config::PatreonConfig {
                    client_id: String::new(),
                    client_secret: String::new(),
                    poll_interval_secs: secs,
                    cookies_path: None,
                });
            }
        }
        "patreon.cookies_path" => {
            if let Some(ref mut pa) = cfg.patreon {
                pa.cookies_path = Some(std::path::PathBuf::from(value));
            } else {
                cfg.patreon = Some(config::PatreonConfig {
                    client_id: String::new(),
                    client_secret: String::new(),
                    poll_interval_secs: 300,
                    cookies_path: Some(std::path::PathBuf::from(value)),
                });
            }
        }
        "theme" => {
            cfg.theme.set_name(value.to_string());
        }
        _ => {
            anyhow::bail!(
                "Unknown key: {key}\n\nValid keys:\n  \
                 recording_dir, poll_interval, transcode, filename_template, theme,\n  \
                 twitch.client_id, twitch.client_secret,\n  \
                 youtube.client_id, youtube.client_secret, youtube.cookies_path,\n  \
                 patreon.client_id, patreon.client_secret, patreon.poll_interval"
            );
        }
    }
    Ok(())
}

async fn handle_log_command(action: &LogAction) -> Result<()> {
    let log_path = config::AppConfig::state_dir().join("strivo.log");

    match action {
        LogAction::Path => {
            println!("{}", log_path.display());
        }
        LogAction::Clear => {
            if log_path.exists() {
                std::fs::write(&log_path, "")?;
                println!("Log cleared: {}", log_path.display());
            } else {
                println!("No log file found at {}", log_path.display());
            }
        }
        LogAction::Tail { lines } => {
            tail_log(&log_path, *lines).await?;
        }
    }
    Ok(())
}

async fn tail_log(path: &std::path::Path, initial_lines: usize) -> Result<()> {
    use tokio::io::AsyncBufReadExt;

    if !path.exists() {
        println!("No log file at {}. Start StriVo first.", path.display());
        return Ok(());
    }

    let content = tokio::fs::read_to_string(path).await?;
    let all_lines: Vec<&str> = content.lines().collect();
    let start = all_lines.len().saturating_sub(initial_lines);
    for line in &all_lines[start..] {
        println!("{line}");
    }

    println!("--- tailing {} (Ctrl-C to stop) ---", path.display());

    let mut last_len = content.len() as u64;
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(250));

    loop {
        interval.tick().await;

        let meta = match tokio::fs::metadata(path).await {
            Ok(m) => m,
            Err(_) => continue,
        };

        let current_len = meta.len();
        if current_len <= last_len {
            if current_len < last_len {
                last_len = 0;
                println!("--- log file truncated ---");
            }
            continue;
        }

        let file = tokio::fs::File::open(path).await?;
        let mut reader = tokio::io::BufReader::new(file);

        use tokio::io::AsyncSeekExt;
        reader.seek(std::io::SeekFrom::Start(last_len)).await?;

        let mut line = String::new();
        loop {
            line.clear();
            let n = reader.read_line(&mut line).await?;
            if n == 0 {
                break;
            }
            print!("{line}");
        }

        last_len = current_len;
    }
}

fn handle_search(query: &str, config_path: Option<&std::path::Path>) -> Result<()> {
    let config = config::AppConfig::load(config_path)?;
    let recordings = recording::scan::scan_existing_recordings(&config);

    if recordings.is_empty() {
        println!("No recordings found in {}", config.recording_dir.display());
        return Ok(());
    }

    let query_lower = query.to_lowercase();
    let query_parts: Vec<&str> = query_lower.split_whitespace().collect();

    // Fuzzy match: each query part must either be a substring or fuzzy-match a word
    let mut scored: Vec<(usize, &_)> = recordings
        .iter()
        .filter_map(|rec| {
            let haystack = format!(
                "{} {} {} {}",
                rec.channel_name,
                rec.stream_title.as_deref().unwrap_or(""),
                rec.platform,
                rec.output_path.to_string_lossy(),
            )
            .to_lowercase();

            let mut total_score: usize = 0;
            for part in &query_parts {
                if haystack.contains(part) {
                    // Exact substring match — best score
                    total_score += 0;
                } else if fuzzy_subsequence(part, &haystack) {
                    // Subsequence match (letters appear in order)
                    total_score += 1;
                } else {
                    // Try Levenshtein against individual words
                    let words: Vec<&str> = haystack.split_whitespace().collect();
                    let best = words
                        .iter()
                        .map(|w| levenshtein(part, w))
                        .min()
                        .unwrap_or(usize::MAX);
                    let threshold = (part.len() / 3).max(1); // allow ~33% edits
                    if best <= threshold {
                        total_score += best;
                    } else {
                        return None; // this query part doesn't match at all
                    }
                }
            }
            Some((total_score, rec))
        })
        .collect();

    // Sort by score (lower = better match)
    scored.sort_by_key(|(score, _)| *score);

    if scored.is_empty() {
        println!("No recordings matching \"{query}\"");
        return Ok(());
    }

    println!(
        "{:<20} {:<10} {:<12} {:<10} Title",
        "Channel", "Platform", "Date", "Size"
    );
    println!("{}", "─".repeat(80));

    for (_, rec) in &scored {
        let date = rec
            .started_at
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d")
            .to_string();
        let title = rec.stream_title.as_deref().unwrap_or("(untitled)");
        let title_display: String = title.chars().take(40).collect();
        println!(
            "{:<20} {:<10} {:<12} {:<10} {}",
            truncate_str(&rec.channel_name, 19),
            rec.platform,
            date,
            rec.format_size(),
            title_display,
        );
    }

    println!("\n{} result(s)", scored.len());
    Ok(())
}

use strivo_core::search::{fuzzy_subsequence, levenshtein};

fn truncate_str(s: &str, max: usize) -> String {
    if s.chars().count() > max {
        s.chars().take(max.saturating_sub(1)).collect::<String>() + "…"
    } else {
        s.to_string()
    }
}

/// Register every first-party plugin into the daemon's registry. Creator
/// Edition only — the pure-PVR build ships no plugins, so the daemon runs
/// with an empty registry and the plugin RPC/event paths are no-ops.
#[cfg(feature = "creator")]
fn register_first_party_plugins(registry: &mut plugin::registry::PluginRegistry) {
    registry.register(Box::new(strivo_plugins::crunchr::CrunchrPlugin::new()));
    registry.register(Box::new(strivo_plugins::artifacts::ArtifactPlugin::new()));
    registry.register(Box::new(strivo_plugins::archiver::ArchiverPlugin::new()));
    registry.register(Box::new(strivo_plugins::insights::InsightsPlugin::new()));
    registry.register(Box::new(strivo_plugins::editor::EditorPlugin::new()));
    registry.register(Box::new(strivo_plugins::viewguard::ViewguardPlugin::new()));
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// The generated unit must run the daemon AND the web UI. Emitting a
    /// daemon-only unit was the earlier bug: `strivo enable` produced a
    /// service that left nothing listening on 8181, so an "enabled" install
    /// had no usable frontend.
    #[test]
    fn default_unit_serves_the_web_ui_too() {
        let unit = render_systemd_unit("\"/usr/bin/strivo\"", "", false, None).unwrap();
        assert!(unit.contains("ExecStart=\"/usr/bin/strivo\"\n"));
        assert!(!unit.contains("strivo\" daemon"));
        assert!(unit.contains("Description=StriVo Live Stream PVR (daemon + web UI)"));
    }

    /// Durability: give-up-forever is the wrong failure mode for a PVR.
    #[test]
    fn default_unit_restarts_forever() {
        let unit = render_systemd_unit("\"/usr/bin/strivo\"", "", false, None).unwrap();
        assert!(unit.contains("Restart=always"));
        assert!(unit.contains("StartLimitIntervalSec=0"));
        assert!(!unit.contains("Restart=on-failure"));
        assert!(unit.contains("WantedBy=default.target"));
    }

    #[test]
    fn daemon_only_restores_the_split_layout() {
        let unit = render_systemd_unit("\"/usr/bin/strivo\"", "", true, None).unwrap();
        assert!(unit.contains("ExecStart=\"/usr/bin/strivo\" daemon\n"));
        assert!(unit.contains("Description=StriVo Live Stream PVR Daemon"));
    }

    #[test]
    fn config_override_survives_into_exec_start() {
        let unit = render_systemd_unit(
            "\"/usr/bin/strivo\"",
            " --config \"/etc/strivo.toml\"",
            false,
            None,
        )
        .unwrap();
        assert!(unit.contains("ExecStart=\"/usr/bin/strivo\" --config \"/etc/strivo.toml\"\n"));
    }

    #[test]
    fn envchain_namespace_must_not_be_blank() {
        assert!(render_systemd_unit("\"/usr/bin/strivo\"", "", false, Some("  ")).is_err());
    }

    /// Only meaningful where envchain is actually installed; elsewhere the
    /// call is expected to fail with a clear message rather than emit a unit
    /// whose ExecStart systemd cannot resolve.
    #[test]
    fn envchain_wraps_exec_start_with_an_absolute_path() {
        let rendered = render_systemd_unit("\"/usr/bin/strivo\"", "", false, Some("mistral"));
        match resolve_on_path("envchain") {
            Some(bin) => {
                let unit = rendered.expect("envchain present, so rendering must succeed");
                let expected =
                    format!("ExecStart={} mistral \"/usr/bin/strivo\"\n", systemd_quote(bin.as_os_str()));
                assert!(unit.contains(&expected), "got:\n{unit}");
                assert!(bin.is_absolute());
            }
            None => assert!(rendered.is_err()),
        }
    }

    #[test]
    fn systemd_quote_preserves_custom_config_paths() {
        let quoted = systemd_quote(std::ffi::OsStr::new("/tmp/Strivo configs/custom.toml"));
        assert_eq!(quoted, "\"/tmp/Strivo configs/custom.toml\"");
    }

    #[test]
    fn systemd_quote_escapes_quotes_and_backslashes() {
        let quoted = systemd_quote(std::ffi::OsStr::new("/tmp/a\\b\"c"));
        assert_eq!(quoted, "\"/tmp/a\\\\b\\\"c\"");
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    #[test]
    fn windows_task_run_command_quotes_exe_and_omits_config_when_absent() {
        let cmd = windows_task_run_command(
            std::path::Path::new(r"C:\Program Files\StriVo\strivo.exe"),
            None,
        );
        assert_eq!(cmd, r#""C:\Program Files\StriVo\strivo.exe" daemon"#);
    }

    #[test]
    fn windows_task_run_command_appends_quoted_config_path() {
        let cmd = windows_task_run_command(
            std::path::Path::new(r"C:\StriVo\strivo.exe"),
            Some(std::path::Path::new(r"C:\Users\me\My Config\strivo.toml")),
        );
        assert_eq!(
            cmd,
            r#""C:\StriVo\strivo.exe" daemon --config "C:\Users\me\My Config\strivo.toml""#
        );
    }
}
