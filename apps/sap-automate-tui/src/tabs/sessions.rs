//! Tab 1 — Sessions.  Live MCP sessions with client identity, protocol,
//! tools-called counter, and last-activity age.

use crate::app::App;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Cell, Row, Table},
    Frame,
};

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let mut rows: Vec<Row> = app.sessions.values().map(|s| {
        let age = s.last_activity.elapsed().as_secs();
        Row::new(vec![
            Cell::from(s.id.clone()),
            Cell::from(s.client.clone()),
            Cell::from(s.protocol.clone()),
            Cell::from(format!("{}", s.tools_called)),
            Cell::from(format!("{}s", age)),
        ])
    }).collect();
    if rows.is_empty() {
        rows.push(Row::new(vec![Cell::from("(no active sessions)").style(Style::default().fg(Color::DarkGray))]));
    }
    let table = Table::new(rows, [
        Constraint::Length(8),
        Constraint::Length(22),
        Constraint::Length(14),
        Constraint::Length(8),
        Constraint::Length(8),
    ])
    .header(
        Row::new(vec!["id", "client", "protocol", "calls", "idle"])
            .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
    )
    .block(Block::default().borders(Borders::ALL).title(" Sessions "));
    f.render_widget(table, area);
}

use ratatui::layout::Constraint;
