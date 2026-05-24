//! UI rendering — Ratatui draw routines for the five tabs.

use crate::app::App;
use crate::tabs;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Tabs},
    Frame,
};

const TAB_NAMES: &[&str] = &["Sessions", "Tools", "KB", "RAG", "Logs"];

pub fn draw(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),   // top tab bar
            Constraint::Min(0),      // body
            Constraint::Length(1),   // status bar
        ])
        .split(f.area());

    draw_tabs(f, app, chunks[0]);
    match app.current_tab {
        0 => tabs::sessions::draw(f, app, chunks[1]),
        1 => tabs::tools::draw(f, app, chunks[1]),
        2 => tabs::kb::draw(f, app, chunks[1]),
        3 => tabs::rag::draw(f, app, chunks[1]),
        4 => tabs::logs::draw(f, app, chunks[1]),
        _ => {}
    }
    draw_status_bar(f, app, chunks[2]);
}

fn draw_tabs(f: &mut Frame, app: &App, area: Rect) {
    let titles: Vec<Line> = TAB_NAMES.iter().enumerate().map(|(i, name)| {
        Line::from(vec![
            Span::styled(format!(" {} ", i + 1), Style::default().fg(Color::DarkGray)),
            Span::styled(*name, Style::default().fg(Color::White)),
        ])
    }).collect();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(" SAP-Automate / operator console ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)));
    let tabs = Tabs::new(titles)
        .block(block)
        .select(app.current_tab)
        .highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD))
        .divider(Span::raw("│"));
    f.render_widget(tabs, area);
}

fn draw_status_bar(f: &mut Frame, app: &App, area: Rect) {
    let uptime = app.uptime();
    let total_calls: u64 = app.tools.values().map(|s| s.invocations).sum();
    let errors: u64 = app.tools.values().map(|s| s.errors).sum();
    let status = Line::from(vec![
        Span::styled(" SAP-Automate ", Style::default().fg(Color::Black).bg(Color::Cyan)),
        Span::raw(format!(
            "  up {:>3}s | sessions {:>2} | calls {:>5} | errors {:>3} | ",
            uptime.as_secs(), app.sessions.len(), total_calls, errors,
        )),
        Span::styled("[1–5] tabs  [j/k] scroll  [q] quit", Style::default().fg(Color::DarkGray)),
    ]);
    f.render_widget(Paragraph::new(status), area);
}
