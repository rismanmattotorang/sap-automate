//! Tab 3 — Knowledge Base.  Per-collection statistics and staleness, plus
//! the RFC metadata cache row (thupalo pattern, polled from
//! `sap-cache://stats` on the live admin feed).

use crate::app::App;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table},
    Frame,
};

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(4)])
        .split(area);

    draw_collections(f, app, layout[0]);
    draw_cache(f, app, layout[1]);
}

fn draw_collections(f: &mut Frame, app: &App, area: Rect) {
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

fn draw_cache(f: &mut Frame, app: &App, area: Rect) {
    let c = app.cache;
    let ratio_style = if c.hit_ratio >= 0.8 {
        Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
    } else if c.hit_ratio >= 0.5 {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::Red)
    };

    let text = vec![
        ratatui::text::Line::from(vec![
            ratatui::text::Span::styled("RFC metadata cache ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            ratatui::text::Span::raw(format!(
                "hits={} misses={} entries={} ratio=",
                c.hits, c.misses, c.entries,
            )),
            ratatui::text::Span::styled(format!("{:.2}", c.hit_ratio), ratio_style),
        ]),
        ratatui::text::Line::from(ratatui::text::Span::styled(
            "  thupalo/sap-rfc-mcp-server pattern · resource: sap-cache://stats · tool: sap.system.cache_stats",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    let p = Paragraph::new(text).block(
        Block::default().borders(Borders::ALL).title(" Cache ")
    );
    f.render_widget(p, area);
}
