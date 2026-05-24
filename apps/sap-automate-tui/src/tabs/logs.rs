//! Tab 5 — Logs.  Ring-buffer tail of structured log events.

use crate::app::{App, LogLevel};
use ratatui::{
    layout::{Constraint, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let lines: Vec<Line> = app.logs.iter().rev().take((area.height as usize).saturating_sub(2)).map(|l| {
        let (lvl_label, lvl_color) = match l.level {
            LogLevel::Info => ("INFO ", Color::Green),
            LogLevel::Warn => ("WARN ", Color::Yellow),
            LogLevel::Error => ("ERROR", Color::Red),
        };
        let age_secs = l.at.elapsed().as_secs();
        Line::from(vec![
            Span::styled(format!("{:>3}s ", age_secs), Style::default().fg(Color::DarkGray)),
            Span::styled(lvl_label, Style::default().fg(lvl_color).add_modifier(Modifier::BOLD)),
            Span::raw(" "),
            Span::styled(format!("[{}]", l.source), Style::default().fg(Color::Cyan)),
            Span::raw(" "),
            Span::raw(l.message.clone()),
        ])
    }).collect();

    let p = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(" Logs (newest first) "))
        .wrap(Wrap { trim: false });
    f.render_widget(p, area);

    // dummy use of Constraint to keep import warning silent on some toolchains
    let _ = Constraint::Length(0);
}
