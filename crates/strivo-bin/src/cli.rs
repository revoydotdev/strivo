use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser, Debug)]
#[command(
    name = "strivo",
    version,
    about = "Self-hosted live-stream PVR (web UI)"
)]
pub struct Args {
    /// Path to config file
    #[arg(short, long, global = true)]
    pub config: Option<std::path::PathBuf>,

    /// Log level (trace, debug, info, warn, error)
    #[arg(short, long, default_value = "info", global = true)]
    pub log_level: String,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Run background daemon (foreground, for systemd)
    Daemon,
    /// Install and start as a background service: a systemd user service on
    /// Linux/macOS, a Task Scheduler task (runs at sign-in) on Windows
    Enable {
        /// Run only the daemon, without serving the web UI. The default unit
        /// runs both in one process, matching what plain `strivo` does.
        #[arg(long)]
        daemon_only: bool,
        /// Wrap ExecStart in `envchain NAMESPACE` so secrets stored in the OS
        /// keyring are handed to the service and nothing else. Requires
        /// envchain on PATH.
        #[arg(long, value_name = "NAMESPACE")]
        envchain: Option<String>,
    },
    /// Stop and remove the background service installed by `enable`
    Disable,
    /// Check if daemon is running
    Status,
    /// Manage configuration
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Manage log file
    Log {
        #[command(subcommand)]
        action: LogAction,
    },
    /// Search recordings by title, channel, or platform
    Search {
        /// Search query (fuzzy match against filenames and metadata)
        query: String,
    },
    /// Pull a creator's full back-catalog (Patreon, YouTube, Twitch) and feed
    /// each episode to the recording + Crunchr pipeline.
    Pull {
        /// Target as `platform:channel_id`, e.g. `youtube:UCxxxx` or
        /// `patreon:1234567` or `twitch:7890`.
        target: String,
        /// yt-dlp format selector (default `best`). Overrides config defaults.
        #[arg(long)]
        format: Option<String>,
        /// Lower bound on `published_at` — accepts an RFC3339 timestamp or a
        /// relative offset like `30d`, `90d`, `12h`.
        #[arg(long)]
        since: Option<String>,
        /// Cap on number of episodes to pull (oldest dropped first).
        #[arg(long)]
        max: Option<usize>,
        /// Skip the dedupe index — re-download even if marked recorded.
        #[arg(long)]
        force: bool,
        /// Don't auto-tandem to Crunchr; just download.
        #[arg(long)]
        no_transcribe: bool,
    },

    /// Check that required external tools are installed
    Doctor,
    /// Guided account and credential setup
    Setup {
        #[command(subcommand)]
        action: SetupAction,
    },
    /// Run the *arr-style web UI. Talks to a running daemon over IPC.
    Serve {
        /// Bind address.
        #[arg(long, default_value = "127.0.0.1:8181")]
        bind: String,
        /// Override the API key (default: random per run; persist via `[web] api_key`).
        #[arg(long)]
        api_key: Option<String>,
    },
    /// Import auto-record channels from an OBS scene collection or a
    /// Streamlink-style stream list (M5.7).
    Import {
        #[command(subcommand)]
        source: ImportSource,
    },
    /// Repair library inconsistencies. Reports by default; only touches
    /// files when you pass --apply.
    Repair {
        #[command(subcommand)]
        what: RepairKind,
    },

    /// Merge resume segments back into a single MKV file (M5.5).
    /// Sources are appended in the order given; the first owns the
    /// timeline. Requires mkvtoolnix.
    Merge {
        /// Output MKV path.
        #[arg(long)]
        output: std::path::PathBuf,
        /// Source MKV segments in chronological order.
        sources: Vec<std::path::PathBuf>,
    },
    /// Extract a first-frame thumbnail for a recording (M5.4 substrate).
    Thumbnail {
        /// MKV / mp4 file to thumbnail.
        file: std::path::PathBuf,
        /// Seek N seconds in before grabbing the frame so a black opening
        /// frame doesn't dominate. Default: 10s.
        #[arg(long, default_value = "10")]
        seek: f64,
    },
    /// Embed chapter markers into a recording (M5.3, requires mkvpropedit).
    Chapter {
        /// MKV file to chapter.
        file: std::path::PathBuf,
        /// Emit one chapter every N minutes (default 10). Each chapter
        /// is labeled "Part 1", "Part 2", … so semantic sources (Crunchr
        /// topics, manual splits) can override later.
        #[arg(long, default_value = "10")]
        every: u64,
    },
    /// Resolve the Twitch live-from-start (Rewind) Usher master playlist
    /// URL for a channel that is currently broadcasting. Useful for
    /// validating the GQL/Usher recon before plumbing it into the
    /// recording flow. Prints the resolved URL; pipe it to ffmpeg with
    /// `-i` to verify it pulls broadcast from t=0.
    TwitchRewind {
        /// Channel login (lowercase, no @ prefix), e.g. `xqc`.
        channel: String,
        /// Optionally save the first N seconds of broadcast to this path
        /// via ffmpeg, end-to-end smoke test.
        #[arg(long)]
        sample_secs: Option<u32>,
        /// When `--sample-secs` is set, the output file path. Defaults to
        /// `./rewind-sample-<channel>.mkv`.
        #[arg(long)]
        out: Option<std::path::PathBuf>,
    },
    /// Print shell completion script to stdout
    Completions {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
    /// Print man page (roff) to stdout
    Man,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum CookiePlatform {
    Youtube,
    Patreon,
}

#[derive(Subcommand, Debug)]
pub enum SetupAction {
    /// Import a signed-in browser session without manually juggling cookies.txt
    Cookies {
        /// Account whose browser session should be imported
        #[arg(value_enum)]
        platform: CookiePlatform,
        /// Browser to import from (brave, chrome, chromium, edge, firefox,
        /// opera, safari, vivaldi, or whale)
        #[arg(long)]
        browser: String,
        /// Optional browser profile name/path (for example "Default")
        #[arg(long)]
        profile: Option<String>,
        /// Linux Chromium keyring override (basictext, gnomekeyring, kwallet,
        /// kwallet5, or kwallet6)
        #[arg(long)]
        keyring: Option<String>,
        /// Replace an existing managed cookie file
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum RepairKind {
    /// Reconcile each recording's extension with the container actually on
    /// disk.
    ///
    /// Recording names come from a template ending in a fixed extension,
    /// but the container is whatever ffmpeg or yt-dlp produced — so a
    /// library accumulates MP4, MPEG-TS and even MP3 wearing `.mkv`.
    /// MPEG-TS is the one that actually breaks: browsers refuse it whatever
    /// the extension says, so it is remuxed rather than renamed.
    Containers {
        /// Actually modify files and the journal. Without this the command
        /// only reports what it would do.
        #[arg(long)]
        apply: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum ImportSource {
    /// Parse an OBS Studio scene-collection JSON export.
    Obs {
        /// Path to the .json export.
        file: std::path::PathBuf,
        /// Commit results to config.toml. Without this, the command
        /// only previews what would be added.
        #[arg(long)]
        apply: bool,
    },
    /// Parse a Streamlink config / streams.txt file. Any line
    /// containing a Twitch / YouTube / Patreon URL is candidate.
    Streamlink {
        /// Path to the streamlink config or stream-list.
        file: std::path::PathBuf,
        #[arg(long)]
        apply: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum ConfigAction {
    /// List all configuration values
    List,
    /// Print the config file path
    Path,
    /// Get a specific config value
    Get {
        /// Config key (recording_dir, poll_interval, transcode, filename_template,
        /// twitch.client_id, youtube.client_id, youtube.client_secret, youtube.cookies_path,
        /// patreon.client_id, patreon.client_secret, patreon.poll_interval)
        key: String,
    },
    /// Set a config value and save
    Set {
        /// Config key
        key: String,
        /// New value
        value: String,
    },
    /// Reset config to defaults (preserves platform credentials)
    Reset,
}

#[derive(Subcommand, Debug)]
pub enum LogAction {
    /// Print the log file path
    Path,
    /// Clear the log file
    Clear,
    /// Tail the log file (live, Ctrl-C to stop)
    Tail {
        /// Number of lines to show initially.
        // `-n` matches GNU `tail -n`; the natural `-l` is already taken by
        // the global `--log-level` flag, and clap's debug assertions reject
        // duplicate shorts when generating completions / the manpage.
        #[arg(short = 'n', long, default_value = "50")]
        lines: usize,
    },
}
