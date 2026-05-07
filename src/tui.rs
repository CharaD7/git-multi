use crossterm::event::{self, Event, KeyCode};
use ratatui::{
    layout::Alignment,
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use std::io;

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
    let title = Paragraph::new("git-multi TUI (Press 'q' to quit)")
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, area);
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
