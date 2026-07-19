//! Viewguard — viewbot detection for live streams.
//!
//! Subscribes to the host's channel-monitor stream, persists viewer
//! counts as a time series, runs statistical detectors (SpikeShape,
//! PlateauVariance, BenfordDigits), and surfaces verdicts to the host.
//! The webui reads the verdicts straight from viewguard.db; previously
//! a TUI pane rendered them as well, but that path retired with the
//! TUI deletion.

use std::any::Any;
use std::collections::HashMap;
use std::path::PathBuf;

use chrono::Utc;

use strivo_core::events::DaemonEvent;
use strivo_core::platform::{ChannelEntry, PlatformKind};
use strivo_core::plugin::{DaemonEventKind, Plugin, PluginAction, PluginContext, StatusSlot};

pub mod detectors;
pub mod score;
pub mod stats;
pub mod store;

use detectors::run_all;
use score::{aggregate, AggregatedVerdict, Band};
use stats::ChannelStats;
use store::{VerdictRow, ViewguardStore};

pub struct ViewguardPlugin {
    data_dir: PathBuf,
    store: Option<ViewguardStore>,
    channels: HashMap<String, ChannelStats>,
    /// Latest aggregated verdict per channel (in-memory mirror of `verdicts`).
    verdicts: HashMap<String, AggregatedVerdict>,
    last_status: Option<String>,
}

impl Default for ViewguardPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl ViewguardPlugin {
    pub fn new() -> Self {
        Self {
            data_dir: PathBuf::new(),
            store: None,
            channels: HashMap::new(),
            verdicts: HashMap::new(),
            last_status: None,
        }
    }

    fn record_snapshot(&mut self, entries: &[ChannelEntry]) {
        let now = Utc::now();
        for entry in entries.iter().filter(|c| c.is_live) {
            let stats = self.channels.entry(entry.id.clone()).or_insert_with(|| {
                ChannelStats::new(
                    entry.id.clone(),
                    format!("{}", entry.platform),
                    entry.display_name.clone(),
                )
            });
            stats.mark_session_start(entry.started_at.unwrap_or(now));
            let viewers = entry.viewer_count.unwrap_or(0) as u32;
            if stats.push(now, viewers) {
                if let Some(s) = &self.store {
                    let _ = s.insert_sample(&entry.id, &platform_str(entry.platform), now, viewers);
                }
            }
        }
        // Run detectors on every poll for any channel with enough data.
        let channel_ids: Vec<String> = self.channels.keys().cloned().collect();
        for channel_id in channel_ids {
            let stats = self.channels.get(&channel_id).unwrap().clone();
            if stats.session_started_at.is_none() {
                continue;
            }
            let signals = run_all(&stats);
            if signals.is_empty() {
                continue;
            }
            if let Some(s) = &self.store {
                for sig in &signals {
                    let _ = s.insert_signal(
                        &channel_id,
                        sig.kind.name(),
                        sig.score,
                        sig.confidence,
                        now,
                        &serde_json::to_string(&sig.evidence).unwrap_or_else(|_| "{}".into()),
                    );
                }
            }
            let verdict = aggregate(&signals);
            if !matches!(verdict.band, Band::Clean) {
                self.last_status = Some(format!(
                    "{} → {}",
                    stats.display_name,
                    verdict.band.as_str()
                ));
            }
            self.verdicts.insert(channel_id, verdict);
        }
    }

    fn close_session(&mut self, channel_id: &str) {
        let now = Utc::now();
        let Some(stats) = self.channels.get_mut(channel_id) else {
            return;
        };
        let started_at = match stats.session_started_at {
            Some(t) => t,
            None => return,
        };
        let signals = run_all(stats);
        let verdict = aggregate(&signals);
        let contributors =
            serde_json::to_string(&verdict.contributors).unwrap_or_else(|_| "[]".into());
        if let Some(s) = &self.store {
            let _ = s.upsert_verdict(&VerdictRow {
                channel_id: channel_id.to_string(),
                stream_started_at: started_at,
                stream_ended_at: Some(now),
                final_score: verdict.final_score,
                band: verdict.band.as_str().into(),
                contributors_json: contributors,
            });
        }
        self.verdicts.insert(channel_id.to_string(), verdict);
        stats.end_session();
    }
}

fn platform_str(p: PlatformKind) -> String {
    format!("{}", p)
}

impl Plugin for ViewguardPlugin {
    fn name(&self) -> &'static str {
        "viewguard"
    }
    fn display_name(&self) -> &str {
        "Viewguard"
    }

    fn init(&mut self, ctx: &PluginContext) -> anyhow::Result<()> {
        // `init_all` already scopes data_dir to `<base>/plugins/viewguard`;
        // use it directly. The webui reads `<data>/plugins/viewguard/viewguard.db`.
        self.data_dir = ctx.data_dir.clone();
        std::fs::create_dir_all(&self.data_dir)?;
        let db_path = self.data_dir.join("viewguard.db");
        self.store = Some(ViewguardStore::open(&db_path)?);
        tracing::info!(
            plugin = "viewguard",
            db = %db_path.display(),
            "viewguard initialized",
        );
        Ok(())
    }

    fn event_filter(&self) -> Option<Vec<DaemonEventKind>> {
        Some(vec![
            DaemonEventKind::ChannelsUpdated,
            DaemonEventKind::ChannelWentLive,
            DaemonEventKind::ChannelWentOffline,
        ])
    }

    fn on_event(
        &mut self,
        event: &DaemonEvent,
        _ctx: &strivo_core::plugin::VerbContext,
    ) -> Vec<PluginAction> {
        match event {
            DaemonEvent::ChannelsUpdated(entries) => self.record_snapshot(entries),
            DaemonEvent::ChannelWentLive(entry) => {
                let now = Utc::now();
                let stats = self.channels.entry(entry.id.clone()).or_insert_with(|| {
                    ChannelStats::new(
                        entry.id.clone(),
                        platform_str(entry.platform),
                        entry.display_name.clone(),
                    )
                });
                stats.mark_session_start(entry.started_at.unwrap_or(now));
            }
            DaemonEvent::ChannelWentOffline(entry) => {
                self.close_session(&entry.id);
            }
            _ => {}
        }
        Vec::new()
    }

    fn status_line(&self) -> Option<String> {
        let suspect = self
            .verdicts
            .values()
            .filter(|v| matches!(v.band, Band::Suspect | Band::Fraudulent))
            .count();
        if suspect > 0 {
            Some(format!("viewguard: {suspect} suspect"))
        } else {
            self.last_status.clone()
        }
    }

    fn status_slot(&self) -> StatusSlot {
        StatusSlot::Tray
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
