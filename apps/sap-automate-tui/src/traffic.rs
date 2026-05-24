//! Synthetic traffic generator.
//!
//! Drives the TUI with realistic-looking events so operators can navigate
//! the layout offline.  Phase 7 swaps this for a tokio mpsc bound to the
//! server's admin endpoint without touching `app.rs` or `ui.rs`.

use crate::app::{LogLevel, TrafficEvent};
use sap_automate_rag::LatencyBreakdown;
use std::time::Instant;

const TOOLS: &[&str] = &[
    "sap.docs.search", "sap.help.search", "abap.search",
    "sap.rfc.search", "sap.rfc.metadata", "sap.system.info",
    "sap.table.read", "sap.table.structure",
    "abap.adt.get_class", "abap.adt.get_program", "abap.adt.where_used",
];

pub struct Synthetic {
    started: Instant,
    tick: u64,
    sessions: Vec<String>,
}

impl Synthetic {
    pub fn new() -> Self {
        Self { started: Instant::now(), tick: 0, sessions: Vec::new() }
    }

    pub fn tick(&mut self) -> Option<TrafficEvent> {
        self.tick += 1;
        let phase = self.tick % 23;
        match phase {
            0 => {
                let id = format!("S-{:04x}", self.tick % 0x10000);
                self.sessions.push(id.clone());
                Some(TrafficEvent::SessionOpen { id, client: "claude-code".into(), protocol: "2025-06-18".into() })
            }
            5 if !self.sessions.is_empty() => {
                let id = self.sessions.remove(self.tick as usize % self.sessions.len());
                Some(TrafficEvent::SessionClose { id })
            }
            7 => Some(TrafficEvent::KbStat {
                collection: "sap_help".into(),
                points: 12_400 + (self.tick % 100),
                staleness_pct: 1.2,
            }),
            11 => Some(TrafficEvent::KbStat {
                collection: "abap".into(),
                points: 84_300 + (self.tick % 100),
                staleness_pct: 0.4,
            }),
            13 => Some(TrafficEvent::KbStat {
                collection: "bpmn".into(),
                points: 1_120 + (self.tick % 10),
                staleness_pct: 3.1,
            }),
            17 => Some(TrafficEvent::KbStat {
                collection: "leanix".into(),
                points: 2_980 + (self.tick % 10),
                staleness_pct: 0.9,
            }),
            19 => {
                if self.tick % 200 == 0 {
                    Some(TrafficEvent::Log {
                        level: LogLevel::Warn,
                        source: "rag".into(),
                        message: "reranker latency spike: 380μs (P95 ceiling 350μs)".into(),
                    })
                } else {
                    Some(TrafficEvent::Log {
                        level: LogLevel::Info,
                        source: "kb".into(),
                        message: format!("upsert batch ok ({} chunks)", 24 + (self.tick % 8)),
                    })
                }
            }
            _ => {
                // The common case: a tool call.
                let idx = (self.tick as usize) % TOOLS.len();
                let name = TOOLS[idx];
                // Realistic latency: 80–250μs for RAG, 200μs–1ms for ADT/RFC.
                let base = if name.starts_with("abap.") || name.starts_with("sap.rfc") || name.starts_with("sap.table") {
                    200 + (self.tick % 800)
                } else {
                    100 + (self.tick % 150)
                };
                let breakdown = if name.contains("search") || name == "sap.docs.search" {
                    Some(LatencyBreakdown {
                        dense_us: 30 + (self.tick % 20),
                        sparse_us: 30 + (self.tick % 20),
                        fusion_us: 2,
                        rerank_us: 50 + (self.tick % 30),
                        total_us: base,
                    })
                } else { None };
                Some(TrafficEvent::ToolCall {
                    name: name.into(),
                    latency_us: base,
                    error: self.tick % 197 == 0,
                    breakdown,
                })
            }
        }
    }

    pub fn uptime(&self) -> std::time::Duration { self.started.elapsed() }
}
