//! Tab 3 — Knowledge Base.  Per-collection statistics and staleness.

use crate::app::App;
use ratatui::{
    layout::{Constraint, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Cell, Row, Table},
    Frame,
};

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let mut rows: Vec<Row> = app.kb_collections.iter().map(|(name, (points, staleness))| {
        let stale_style = if *staleness > 5.0 {
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
        } else if *staleness > 2.0 {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::Green)
        };
        Row::new(vec![
            Cell::from(name.clone()),
            Cell::from(format!("{:>9}", points)),
            Cell::from(format!("{:>4.1}%", staleness)).style(stale_style),
            Cell::from(if *staleness > 5.0 { "warning" } else { "ok" }).style(stale_style),
        ])
    }).collect();

    if rows.is_empty() {
        rows.push(Row::new(vec![Cell::from("(no KB stats yet)").style(Style::default().fg(Color::DarkGray))]));
    }

    let table = Table::new(rows, [
        Constraint::Length(16),
        Constraint::Length(12),
        Constraint::Length(10),
        Constraint::Length(10),
    ])
    .header(
        Row::new(vec!["collection", "points", "stale", "status"])
            .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
    )
    .block(Block::default().borders(Borders::ALL).title(" Knowledge Base "));
    f.render_widget(table, area);
}
