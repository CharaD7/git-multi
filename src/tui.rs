use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame,
};
use std::io;

use crate::git::GitRepo;

// Custom Palette
const VIBRANT_PINK: Color = Color::Rgb(255, 105, 180);
const CYAN: Color = Color::Rgb(0, 255, 255);
const CREAM: Color = Color::Rgb(255, 253, 208);
const RED: Color = Color::Rgb(255, 69, 58);
const MAUVE: Color = Color::Rgb(224, 176, 255);

#[derive(Default)]
enum Overlay {
    #[default]
    None,
    Input { prompt: String, value: String, action: String },
    Confirm { question: String, action: String },
    Message { text: String, is_error: bool },
}

struct AppState {
    items: Vec<String>,
    list_state: ListState,
    overlay: Overlay,
    repo: Option<GitRepo>,
}

impl AppState {
    fn new() -> Self {
        Self {
            items: vec![
                "Remotes".to_string(), 
                "Branches".to_string(), 
                "Status".to_string(),
                "Fetch".to_string(),
                "Push".to_string()
            ],
            list_state: ListState::default(),
            overlay: Overlay::None,
            repo: GitRepo::open().ok(),
        }
    }
}

// ... update run_tui and ui logic to handle overlay ...
// (I will continue with this in subsequent steps)

pub fn run_tui() -> io::Result<()> {
    let mut terminal = ratatui::init();
    let mut state = AppState::new();
    state.list_state.select(Some(0));

    loop {
        terminal.draw(|f| ui(f, &mut state))?;
        if handle_events(&mut state)? {
            break;
        }
    }

    ratatui::restore();
    Ok(())
}

fn ui(f: &mut Frame, state: &mut AppState) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(3)])
        .split(f.area());

    let inner_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(layout[0]);

    // Sidebar
    let items: Vec<ListItem> = state.items.iter().map(|i| ListItem::new(i.as_str())).collect();
    let list = List::new(items)
        .block(Block::default().title(" Navigation ").borders(Borders::ALL).border_style(Style::default().fg(VIBRANT_PINK)))
        .highlight_style(Style::default().bg(CYAN).fg(Color::Black))
        .highlight_symbol(">> ");
    f.render_stateful_widget(list, inner_layout[0], &mut state.list_state);

    // Main Content
    let content = match state.list_state.selected() {
        Some(0) => "Remote Management\n\n- Add/Rename/List remotes",
        Some(1) => "Branch Management\n\n- [n] Create branch\n- [d] Delete branch",
        Some(2) => "System Status\n\nAll systems are stable.",
        Some(3) => "Fetch Action\n\nPress [Enter] to fetch from all remotes.",
        Some(4) => "Push Action\n\nPress [Enter] to push to all remotes.",
        _ => "Select an action.",
    };
    let main_view = Paragraph::new(content)
        .block(Block::default().title(" Details ").borders(Borders::ALL).border_style(Style::default().fg(MAUVE)))
        .style(Style::default().fg(CREAM));
    f.render_widget(main_view, inner_layout[1]);

    // Footer/Help Bar
    let help_text = " [↑/↓] Navigate  [Enter] Select  [q] Quit  [f/p] Remote Action ";
    let footer = Paragraph::new(help_text)
        .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(CYAN)))
        .style(Style::default().fg(CREAM).bg(Color::Rgb(50, 50, 50)));
    f.render_widget(footer, layout[1]);

    // Render Overlay if active
    match &state.overlay {
        Overlay::Input { prompt, value, .. } => {
            let area = centered_rect(50, 3, f.area());
            let modal = Paragraph::new(format!("{}\n{}", prompt, value))
                .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(RED)));
            f.render_widget(ratatui::widgets::Clear, area);
            f.render_widget(modal, area);
        }
        Overlay::Message { text, is_error } => {
            let area = centered_rect(50, 3, f.area());
            let color = if *is_error { RED } else { VIBRANT_PINK };
            let modal = Paragraph::new(text.as_str())
                .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(color)));
            f.render_widget(ratatui::widgets::Clear, area);
            f.render_widget(modal, area);
        }
        Overlay::Confirm { question, .. } => {
            let area = centered_rect(50, 3, f.area());
            let modal = Paragraph::new(format!("{}\n[Y/N]", question))
                .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(CYAN)));
            f.render_widget(ratatui::widgets::Clear, area);
            f.render_widget(modal, area);
        }
        Overlay::None => {}
    }
}

fn centered_rect(percent_x: u16, height: u16, r: ratatui::layout::Rect) -> ratatui::layout::Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(50),
            Constraint::Length(height),
            Constraint::Percentage(50),
        ])
        .split(r);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

fn handle_events(state: &mut AppState) -> io::Result<bool> {
    if event::poll(std::time::Duration::from_millis(100))? {
        if let Event::Key(key) = event::read()? {
            if key.kind == KeyEventKind::Press {
                // Overlay handling
                match &mut state.overlay {
                    Overlay::Input { ref mut value, .. } => {
                        match key.code {
                            KeyCode::Enter => state.overlay = Overlay::None,
                            KeyCode::Char(c) => value.push(c),
                            KeyCode::Backspace => { value.pop(); }
                            KeyCode::Esc => state.overlay = Overlay::None,
                            _ => {}
                        }
                        return Ok(false);
                    }
                    Overlay::Message { .. } => {
                        if key.code == KeyCode::Enter || key.code == KeyCode::Esc {
                            state.overlay = Overlay::None;
                        }
                        return Ok(false);
                    }
                    Overlay::Confirm { .. } => {
                        if key.code == KeyCode::Char('y') {
                            /* trigger action */
                        }
                        state.overlay = Overlay::None;
                        return Ok(false);
                    }
                    Overlay::None => {}
                }

                // Normal navigation
                match key.code {
                    KeyCode::Char('q') => return Ok(true),
                    KeyCode::Down => {
                        let i = state.list_state.selected().map(|i| (i + 1) % state.items.len());
                        state.list_state.select(i);
                    }
                    KeyCode::Up => {
                        let i = state.list_state.selected().map(|i| if i == 0 { state.items.len() - 1 } else { i - 1 });
                        state.list_state.select(i);
                    }
                    KeyCode::Enter => {
                        if let Some(i) = state.list_state.selected() {
                            match i {
                                3 => { /* Fetch */ 
                                    if let Some(repo) = &state.repo {
                                        if repo.fetch_all().is_ok() {
                                            state.overlay = Overlay::Message { text: "Fetched!".to_string(), is_error: false };
                                        }
                                    }
                                }
                                4 => { /* Push */
                                    if let Some(repo) = &state.repo {
                                        if repo.push_to_all(None).is_ok() {
                                            state.overlay = Overlay::Message { text: "Pushed!".to_string(), is_error: false };
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(false)
}
