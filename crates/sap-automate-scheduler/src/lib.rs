//! Proactive scheduler (paper §IX-B P4).
//!
//! Declared SAP-typical monitoring jobs:
//!   - every Monday morning: summarise the ATC findings created in the
//!     last seven days and post to a Teams channel
//!   - every quarter: rank LeanIX applications by years-to-EOL and flag
//!     those within four years
//!   - hourly during business hours: scan transports stuck in QA
//!
//! Each job is declared in `./scheduler.toml`; the scheduler binds them
//! to MCP tool invocations (and optionally to messaging channels via
//! `sap-automate-channels`).
//!
//! Cron support is intentionally minimal — we only need a few cadence
//! patterns (per-N-minutes, hourly, daily-at-HH, weekly-on-DAY).

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
#[allow(unused_imports)]
use tracing::{debug, info, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledJob {
    pub name: String,
    pub schedule: Schedule,
    /// MCP tool to invoke when the job fires.
    pub tool: String,
    #[serde(default)]
    pub arguments: serde_json::Value,
    /// Optional channel destination (e.g. "teams:#fin-ops").
    #[serde(default)]
    pub channel: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool { true }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Schedule {
    /// Fire every N seconds.  Useful for tests and rapid prototypes.
    EveryNSeconds { secs: u64 },
    /// Fire every N minutes.
    EveryNMinutes { mins: u64 },
    /// Hourly at the given minute.
    Hourly { minute: u32 },
    /// Daily at HH:MM local time.
    Daily { hour: u32, minute: u32 },
    /// Weekly on a given weekday (1=Mon..7=Sun) at HH:MM.
    Weekly { weekday: u32, hour: u32, minute: u32 },
    /// Quarterly on the Nth day of the quarter.
    Quarterly { day: u32, hour: u32, minute: u32 },
}

impl Schedule {
    /// Compute the time-until-next-fire from `now_secs_since_epoch`.
    ///
    /// Deterministic; uses only seconds-since-epoch arithmetic so we
    /// don't pull in chrono.  Approximates day/week alignment using
    /// fixed-86400-second days and 604800-second weeks, which is good
    /// enough for sub-day cadence accuracy.
    pub fn next_in(&self, now_secs: u64) -> Duration {
        match self {
            Schedule::EveryNSeconds { secs } => Duration::from_secs(*secs),
            Schedule::EveryNMinutes { mins } => Duration::from_secs(*mins * 60),
            Schedule::Hourly { minute } => {
                let m = (*minute as u64).min(59);
                let now_min = (now_secs / 60) % 60;
                let delta = if now_min < m { m - now_min } else { 60 - (now_min - m) };
                Duration::from_secs(delta * 60)
            }
            Schedule::Daily { hour, minute } => {
                let target_secs = (*hour as u64 * 3600 + *minute as u64 * 60) % 86400;
                let now_in_day = now_secs % 86400;
                let delta = if now_in_day < target_secs {
                    target_secs - now_in_day
                } else {
                    86400 - (now_in_day - target_secs)
                };
                Duration::from_secs(delta)
            }
            Schedule::Weekly { weekday, hour, minute } => {
                // 1970-01-01 (Thu) → weekday math.  Treat epoch as
                // weekday 4 (Thursday); shift accordingly.
                let now_day = (now_secs / 86400 + 3) % 7 + 1; // 1..7
                let target = (*weekday).clamp(1, 7) as u64;
                let target_secs = (*hour as u64 * 3600 + *minute as u64 * 60) % 86400;
                let now_in_day = now_secs % 86400;
                let mut day_offset = if now_day <= target { target - now_day } else { 7 - (now_day - target) };
                // If we're on the target day but past the hour, push by a week.
                if day_offset == 0 && now_in_day >= target_secs {
                    day_offset = 7;
                }
                Duration::from_secs(day_offset * 86400 + target_secs.saturating_sub(now_in_day))
            }
            Schedule::Quarterly { day, hour: _, minute: _ } => {
                // 90 days for the pilot — kept rough to avoid chrono.
                // Production swaps for a real calendar.
                let _ = day;
                Duration::from_secs(90 * 86400)
            }
        }
    }
}

#[async_trait::async_trait]
pub trait JobExecutor: Send + Sync + 'static {
    /// Invoke a tool with arguments and return whatever the user-facing
    /// channel should post.  Errors become structured log entries.
    async fn invoke(&self, job: &ScheduledJob) -> Result<String, String>;
}

#[derive(Debug, Default)]
pub struct ScheduleReport {
    pub job: String,
    pub fired_at_ms: u64,
    pub duration_ms: u64,
    pub ok: bool,
    pub summary: String,
}

#[derive(Debug, Default)]
pub struct SchedulerStats {
    pub jobs_declared: usize,
    pub fires: u64,
    pub errors: u64,
    pub last: Option<ScheduleReport>,
}

pub struct Scheduler {
    jobs: Vec<ScheduledJob>,
    executor: Arc<dyn JobExecutor>,
    stats: Mutex<SchedulerStats>,
}

impl Scheduler {
    pub fn new(jobs: Vec<ScheduledJob>, executor: Arc<dyn JobExecutor>) -> Self {
        let stats = SchedulerStats { jobs_declared: jobs.len(), ..SchedulerStats::default() };
        Self { jobs, executor, stats: Mutex::new(stats) }
    }

    /// Parse a `scheduler.toml` file.  Expects `[[jobs]]` array tables.
    pub fn parse_config(toml_text: &str) -> Result<Vec<ScheduledJob>, String> {
        #[derive(Deserialize)]
        struct Wrapper { #[serde(default)] jobs: Vec<ScheduledJob> }
        let w: Wrapper = toml::from_str(toml_text).map_err(|e| e.to_string())?;
        Ok(w.jobs)
    }

    pub async fn fire_once(&self, idx: usize) -> Option<ScheduleReport> {
        let job = self.jobs.get(idx)?.clone();
        if !job.enabled { return None; }
        let t0 = std::time::Instant::now();
        let fired_at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64).unwrap_or(0);
        let (ok, summary) = match self.executor.invoke(&job).await {
            Ok(s) => (true, s),
            Err(e) => (false, e),
        };
        let report = ScheduleReport {
            job: job.name.clone(),
            fired_at_ms,
            duration_ms: t0.elapsed().as_millis() as u64,
            ok, summary,
        };
        let mut s = self.stats.lock().await;
        s.fires += 1;
        if !ok { s.errors += 1; }
        s.last = Some(ScheduleReport {
            job: report.job.clone(), fired_at_ms: report.fired_at_ms,
            duration_ms: report.duration_ms, ok: report.ok,
            summary: report.summary.clone(),
        });
        Some(report)
    }

    /// Convenience: fire every enabled job once, immediately, in order.
    /// Used by the demo + tests; production runs the cadence loop.
    pub async fn fire_all_now(&self) -> Vec<ScheduleReport> {
        let mut out = Vec::new();
        for i in 0..self.jobs.len() {
            if let Some(r) = self.fire_once(i).await { out.push(r); }
        }
        out
    }

    pub async fn stats(&self) -> SchedulerStats {
        let s = self.stats.lock().await;
        SchedulerStats {
            jobs_declared: s.jobs_declared, fires: s.fires, errors: s.errors,
            last: s.last.as_ref().map(|r| ScheduleReport {
                job: r.job.clone(), fired_at_ms: r.fired_at_ms,
                duration_ms: r.duration_ms, ok: r.ok, summary: r.summary.clone(),
            }),
        }
    }

    pub fn jobs(&self) -> &[ScheduledJob] { &self.jobs }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockExec;
    #[async_trait::async_trait]
    impl JobExecutor for MockExec {
        async fn invoke(&self, job: &ScheduledJob) -> Result<String, String> {
            Ok(format!("mock invocation of {} ({})", job.name, job.tool))
        }
    }

    #[tokio::test]
    async fn parse_config_and_fire() {
        let toml = r#"
            [[jobs]]
            name = "atc_weekly"
            tool = "sap.docs.search"
            channel = "teams:#fin-ops"
            arguments = { query = "ATC findings last week" }
            schedule = { kind = "weekly", weekday = 1, hour = 8, minute = 0 }

            [[jobs]]
            name = "eol_quarterly"
            tool = "kb.global_query"
            arguments = { query = "applications years-to-EOL" }
            schedule = { kind = "quarterly", day = 1, hour = 9, minute = 0 }
        "#;
        let jobs = Scheduler::parse_config(toml).unwrap();
        assert_eq!(jobs.len(), 2);
        let s = Scheduler::new(jobs, Arc::new(MockExec));
        let reports = s.fire_all_now().await;
        assert_eq!(reports.len(), 2);
        assert!(reports.iter().all(|r| r.ok));
        let stats = s.stats().await;
        assert_eq!(stats.fires, 2);
        assert_eq!(stats.errors, 0);
    }

    #[test]
    fn hourly_next_in_aligns_to_the_minute() {
        // At 00:30, an hourly-at-:15 schedule should fire in 45 minutes.
        let now_secs = 30 * 60;
        let d = Schedule::Hourly { minute: 15 }.next_in(now_secs);
        assert_eq!(d.as_secs(), 45 * 60);
    }

    #[test]
    fn daily_next_in_skips_past_target() {
        // At 09:00, a daily-at-08:00 schedule should fire in 23 hours.
        let now_secs = 9 * 3600;
        let d = Schedule::Daily { hour: 8, minute: 0 }.next_in(now_secs);
        assert_eq!(d.as_secs(), 23 * 3600);
    }
}

// `tracing` macros are imported above; silence dead-code lint for now.
