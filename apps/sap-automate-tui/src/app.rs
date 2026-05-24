//! TUI state machine.
//!
//! Holds the current tab, scroll offset, and observed traffic events.
//! `App::observe()` is the single ingestion point — synthetic feed now,
//! real admin endpoint in Phase 7.

use sap_automate_rag::LatencyBreakdown;
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct ToolStat {
    pub name: String,
    pub invocations: u64,
    pub errors: u64,
    /// Latency samples in microseconds.  Bounded ring (last N).
    pub samples_us: VecDeque<u64>,
    /// Last-seen LatencyBreakdown (RAG tools only).
    pub last_breakdown: Option<LatencyBreakdown>,
}

impl ToolStat {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.into(),
            invocations: 0,
            errors: 0,
            samples_us: VecDeque::with_capacity(120),
            last_breakdown: None,
        }
    }

    pub fn record(&mut self, latency_us: u64, error: bool, breakdown: Option<LatencyBreakdown>) {
        self.invocations += 1;
        if error { self.errors += 1; }
        if self.samples_us.len() == 120 { self.samples_us.pop_front(); }
        self.samples_us.push_back(latency_us);
        if breakdown.is_some() { self.last_breakdown = breakdown; }
    }

    pub fn percentile(&self, q: f64) -> u64 {
        if self.samples_us.is_empty() { return 0; }
        let mut v: Vec<u64> = self.samples_us.iter().copied().collect();
        v.sort_unstable();
        v[((v.len() as f64 * q) as usize).min(v.len() - 1)]
    }
}

#[derive(Clone)]
pub struct SessionRow {
    pub id: String,
    pub client: String,
    pub protocol: String,
    pub tools_called: u64,
    pub last_activity: Instant,
}

#[derive(Clone)]
pub struct LogEntry {
    pub at: Instant,
    pub level: LogLevel,
    pub source: String,
    pub message: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LogLevel { Info, Warn, Error }

#[derive(Clone)]
pub enum TrafficEvent {
    ToolCall { name: String, latency_us: u64, error: bool, breakdown: Option<LatencyBreakdown> },
    SessionOpen { id: String, client: String, protocol: String },
    SessionClose { id: String },
    Log { level: LogLevel, source: String, message: String },
    KbStat { collection: String, points: u64, staleness_pct: f64 },
}

pub struct App {
    pub current_tab: usize,
    pub scroll: HashMap<usize, u16>,
    pub tools: HashMap<String, ToolStat>,
    pub sessions: HashMap<String, SessionRow>,
    pub logs: VecDeque<LogEntry>,
    pub kb_collections: HashMap<String, (u64, f64)>,
    pub started_at: Instant,
}

impl App {
    pub fn new() -> Self {
        Self {
            current_tab: 0,
            scroll: HashMap::new(),
            tools: HashMap::new(),
            sessions: HashMap::new(),
            logs: VecDeque::with_capacity(200),
            kb_collections: HashMap::new(),
            started_at: Instant::now(),
        }
    }

    pub fn set_tab(&mut self, n: usize) { if n < 5 { self.current_tab = n; } }
    pub fn next_tab(&mut self) { self.current_tab = (self.current_tab + 1) % 5; }
    pub fn prev_tab(&mut self) { self.current_tab = (self.current_tab + 4) % 5; }
    pub fn scroll_down(&mut self) {
        let v = self.scroll.entry(self.current_tab).or_insert(0);
        *v = v.saturating_add(1);
    }
    pub fn scroll_up(&mut self) {
        let v = self.scroll.entry(self.current_tab).or_insert(0);
        *v = v.saturating_sub(1);
    }

    pub fn observe(&mut self, ev: TrafficEvent) {
        match ev {
            TrafficEvent::ToolCall { name, latency_us, error, breakdown } => {
                let stat = self.tools.entry(name.clone()).or_insert_with(|| ToolStat::new(&name));
                stat.record(latency_us, error, breakdown);
            }
            TrafficEvent::SessionOpen { id, client, protocol } => {
                self.sessions.insert(id.clone(), SessionRow {
                    id, client, protocol, tools_called: 0,
                    last_activity: Instant::now(),
                });
            }
            TrafficEvent::SessionClose { id } => {
                self.sessions.remove(&id);
            }
            TrafficEvent::Log { level, source, message } => {
                if self.logs.len() == 200 { self.logs.pop_front(); }
                self.logs.push_back(LogEntry { at: Instant::now(), level, source, message });
            }
            TrafficEvent::KbStat { collection, points, staleness_pct } => {
                self.kb_collections.insert(collection, (points, staleness_pct));
            }
        }
    }

    pub fn uptime(&self) -> Duration { self.started_at.elapsed() }
}
