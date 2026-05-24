//! Tab 4 — RAG pipeline.  Per-layer latency breakdown from the live
//! `LatencyBreakdown` plumbed through in Phase 3.

use crate::app::App;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Cell, Gauge, Paragraph, Row, Table},
    Frame,
};

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(6), Constraint::Min(0)])
        .split(area);

    draw_latency_gauge(f, app, chunks[0]);
    draw_per_tool_breakdown(f, app, chunks[1]);
}

/// A coarse "where in the 80 ms budget are we?" gauge.  Computed against
/// the most recent breakdown of any RAG tool.
fn draw_latency_gauge(f: &mut Frame, app: &App, area: Rect) {
    let latest = app.tools.values()
        .filter_map(|t| t.last_breakdown.as_ref())
        .max_by_key(|b| b.total_us);
    let (total, dense, sparse, fusion, rerank) = match latest {
        Some(b) => (b.total_us, b.dense_us, b.sparse_us, b.fusion_us, b.rerank_us),
        None => (0, 0, 0, 0, 0),
    };

    let budget_us: u64 = 80_000;
    let pct = ((total as f64 / budget_us as f64) * 100.0).min(100.0) as u16;
    let gauge_style = if pct < 40 {
        Style::default().fg(Color::Green)
    } else if pct < 80 {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Length(3)])
        .split(area);

    let gauge = Gauge::default()
        .block(Block::default().borders(Borders::ALL).title(" P95 latency budget (80 ms gate) "))
        .gauge_style(gauge_style)
        .percent(pct)
        .label(format!("{:>5} μs / {} ms gate ({}%)", total, budget_us / 1000, pct));
    f.render_widget(gauge, chunks[0]);

    let breakdown = Paragraph::new(format!(
        "dense {:>5} μs   |   sparse {:>5} μs   |   fusion {:>4} μs   |   rerank {:>5} μs",
        dense, sparse, fusion, rerank,
    )).block(Block::default().borders(Borders::ALL).title(" Layer breakdown (latest) "));
    f.render_widget(breakdown, chunks[1]);
}

fn draw_per_tool_breakdown(f: &mut Frame, app: &App, area: Rect) {
    let mut rows: Vec<Row> = app.tools.values()
        .filter(|t| t.last_breakdown.is_some())
        .map(|t| {
            let b = t.last_breakdown.as_ref().unwrap();
            Row::new(vec![
                Cell::from(t.name.clone()),
                Cell::from(format!("{:>5}", b.dense_us)),
                Cell::from(format!("{:>5}", b.sparse_us)),
                Cell::from(format!("{:>4}", b.fusion_us)),
                Cell::from(format!("{:>5}", b.rerank_us)),
                Cell::from(format!("{:>5}", b.total_us)).style(
                    if b.total_us > 80_000 { Style::default().fg(Color::Red).add_modifier(Modifier::BOLD) }
                    else if b.total_us > 40_000 { Style::default().fg(Color::Yellow) }
                    else { Style::default().fg(Color::Green) }
                ),
            ])
        }).collect();
    if rows.is_empty() {
        rows.push(Row::new(vec![Cell::from("(no RAG tool traffic yet)").style(Style::default().fg(Color::DarkGray))]));
    }
    let table = Table::new(rows, [
        Constraint::Length(26),
        Constraint::Length(8),
        Constraint::Length(8),
        Constraint::Length(6),
        Constraint::Length(8),
        Constraint::Length(8),
    ])
    .header(
        Row::new(vec!["tool", "dense μs", "sparse μs", "fuse μs", "rerank μs", "total μs"])
            .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
    )
    .block(Block::default().borders(Borders::ALL).title(" RAG pipeline — per-tool layer breakdown "));
    f.render_widget(table, area);
}
