//! Full-screen picker: arrow keys / j k to move, space to toggle.

use std::io;

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row as TRow, Table, TableState};
use ratatui::Frame;

use crate::picker::Row;
use diskdiet::util::human_bytes;

/// Returns `None` if the user quit.
pub fn pick(rows: &mut [Row], title: &str) -> io::Result<Option<()>> {
    if rows.is_empty() {
        return Ok(Some(()));
    }
    let mut terminal = ratatui::init();
    let mut state = TableState::default().with_selected(Some(0));
    let result = (|| -> io::Result<Option<()>> {
        loop {
            terminal.draw(|f| draw(f, rows, &mut state, title))?;
            let Event::Key(key) = event::read()? else {
                continue;
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }
            let last = rows.len() - 1;
            let cur = state.selected().unwrap_or(0);
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => return Ok(None),
                KeyCode::Enter => return Ok(Some(())),
                KeyCode::Down | KeyCode::Char('j') => state.select(Some((cur + 1).min(last))),
                KeyCode::Up | KeyCode::Char('k') => state.select(Some(cur.saturating_sub(1))),
                KeyCode::PageDown => state.select(Some((cur + 15).min(last))),
                KeyCode::PageUp => state.select(Some(cur.saturating_sub(15))),
                KeyCode::Home | KeyCode::Char('g') => state.select(Some(0)),
                KeyCode::End | KeyCode::Char('G') => state.select(Some(last)),
                KeyCode::Char(' ') | KeyCode::Char('x') => {
                    rows[cur].selected = !rows[cur].selected;
                    state.select(Some((cur + 1).min(last)));
                }
                KeyCode::Char('a') => rows.iter_mut().for_each(|r| r.selected = true),
                KeyCode::Char('n') => rows.iter_mut().for_each(|r| r.selected = false),
                _ => {}
            }
        }
    })();
    ratatui::restore();
    result
}

fn draw(f: &mut Frame, rows: &[Row], state: &mut TableState, title: &str) {
    let [top, body, bottom] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(2),
    ])
    .areas(f.area());

    let (n, bytes) = rows
        .iter()
        .filter(|r| r.selected)
        .fold((0, 0u64), |(n, b), r| (n + 1, b + r.size));
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!(" {title} "),
                Style::new().add_modifier(Modifier::BOLD),
            ),
            Span::raw(" · Selected: "),
            Span::styled(
                format!("{n} item(s), ~{}", human_bytes(bytes)),
                Style::new().fg(Color::Green).add_modifier(Modifier::BOLD),
            ),
        ])),
        top,
    );

    let table_rows = rows.iter().map(|r| {
        let size_style = if r.size >= 1 << 30 {
            Style::new().fg(Color::Red).add_modifier(Modifier::BOLD)
        } else if r.size >= 100 << 20 {
            Style::new().fg(Color::Yellow)
        } else {
            Style::new()
        };
        let mut label = vec![Span::raw(r.label.clone())];
        if let Some(note) = &r.note {
            let color = if note.contains('⚠') {
                Color::Yellow
            } else {
                Color::DarkGray
            };
            label.push(Span::styled(format!("  {note}"), Style::new().fg(color)));
        }
        TRow::new(vec![
            Cell::from(if r.selected { "[x]" } else { "[ ]" }).style(if r.selected {
                Style::new().fg(Color::Green).add_modifier(Modifier::BOLD)
            } else {
                Style::new()
            }),
            Cell::from(r.kind.clone()),
            Cell::from(r.age.clone()).style(Style::new().fg(Color::DarkGray)),
            Cell::from(format!("{:>10}", r.size_label)).style(size_style),
            Cell::from(Line::from(label)),
        ])
    });
    let table = Table::new(
        table_rows,
        [
            Constraint::Length(3),
            Constraint::Length(13),
            Constraint::Length(6),
            Constraint::Length(10),
            Constraint::Fill(1),
        ],
    )
    .header(
        TRow::new(vec!["sel", "type", "age", "      size", "path"])
            .style(Style::new().fg(Color::DarkGray)),
    )
    .block(Block::default().borders(Borders::TOP | Borders::BOTTOM))
    .row_highlight_style(
        Style::new()
            .bg(Color::Rgb(50, 50, 60))
            .add_modifier(Modifier::BOLD),
    )
    .highlight_symbol("▶ ");
    f.render_stateful_widget(table, body, state);

    f.render_widget(
        Paragraph::new(Line::from(
            " ↑/↓ or j/k move · space toggle · a all · n none · PgUp/PgDn · enter confirm · q quit",
        ))
        .style(Style::new().fg(Color::DarkGray)),
        bottom,
    );
}
