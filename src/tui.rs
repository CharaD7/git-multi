use crossterm::event::{self, Event, KeyCode};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Style},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use std::io;

// Custom Palette
const VIBRANT_PINK: Color = Color::Rgb(255, 105, 180);
const MAUVE: Color = Color::Rgb(224, 176, 255);
const CYAN: Color = Color::Rgb(0, 255, 255);
const CREAM: Color = Color::Rgb(255, 253, 208);
const RED: Color = Color::Rgb(255, 69, 58);

pub fn run_tui() -> io::Result<()> {
    let mut terminal = ratatui::init();
    let mut should_quit = false;

    while !should_quit {
        terminal.draw(|f| ui(f))?;
        should_quit = handle_events()?;
    }

    ratatui::restore();
    Ok(())
}

fn ui(f: &mut Frame) {
    let area = f.area();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(VIBRANT_PINK))
        .title(" git-multi TUI ")
        .title_style(Style::default().fg(CYAN).bold());

    let layout = Layout::default()
        .direction(ratatui::layout::Direction::Vertical)
        .constraints([
            Constraint::Percentage(80),
            Constraint::Percentage(20),
        ])
        .split(area);

    let main_content = Paragraph::new("Welcome to git-multi!\n\nStatus: [Operational]\n\nPress 'q' to quit.")
        .style(Style::default().fg(CREAM))
        .alignment(Alignment::Center)
        .block(block);
    f.render_widget(main_content, layout[0]);

    let footer = Paragraph::new("System Status: All systems stable.")
        .style(Style::default().fg(MAUVE))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(RED)));
    f.render_widget(footer, layout[1]);
}
fn handle_events() -> io::Result<bool> {
    if event::poll(std::time::Duration::from_millis(100))? {
        if let Event::Key(key) = event::read()? {
            if key.kind == event::KeyEventKind::Press && key.code == KeyCode::Char('q') {
                return Ok(true);
            }
        }
    }
    Ok(false)
}
