//! Tab 2 — Tools.  Tool catalogue with invocation counts, P50/P95/P99
//! latencies, and error rates.

use crate::app::App;
use ratatui::{
    layout::{Constraint, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Cell, Row, Table},
    Frame,
};

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let mut tools: Vec<_> = app.tools.values().collect();
    tools.sort_by(|a, b| b.invocations.cmp(&a.invocations));

    let rows: Vec<Row> = tools.iter().map(|t| {
        let err_pct = if t.invocations > 0 {
            t.errors as f64 / t.invocations as f64 * 100.0
        } else { 0.0 };
        let err_style = if err_pct > 1.0 {
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
        } else if err_pct > 0.0 {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::Green)
        };
        Row::new(vec![
            Cell::from(t.name.clone()),
            Cell::from(format!("{}", t.invocations)),
            Cell::from(format!("{:>5}", t.percentile(0.50))).style(Style::default().fg(Color::Gray)),
            Cell::from(format!("{:>5}", t.percentile(0.95))).style(Style::default().fg(Color::White)),
            Cell::from(format!("{:>5}", t.percentile(0.99))).style(Style::default().fg(Color::DarkGray)),
            Cell::from(format!("{:>5.1}%", err_pct)).style(err_style),
        ])
    }).collect();

    let table = Table::new(rows, [
        Constraint::Length(28),
        Constraint::Length(9),
        Constraint::Length(8),
        Constraint::Length(8),
        Constraint::Length(8),
        Constraint::Length(8),
    ])
    .header(
        Row::new(vec!["tool", "invocations", "P50 μs", "P95 μs", "P99 μs", "errors"])
            .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
    )
    .block(Block::default().borders(Borders::ALL).title(" Tools "));
    f.render_widget(table, area);
}
