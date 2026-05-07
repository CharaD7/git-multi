use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame,
};
use std::io;

// Custom Palette
const VIBRANT_PINK: Color = Color::Rgb(255, 105, 180);
const CYAN: Color = Color::Rgb(0, 255, 255);
const CREAM: Color = Color::Rgb(255, 253, 208);
const RED: Color = Color::Rgb(255, 69, 58);
const MAUVE: Color = Color::Rgb(224, 176, 255);

struct AppState {
    items: Vec<String>,
    list_state: ListState,
}

pub fn run_tui() -> io::Result<()> {
    let mut terminal = ratatui::init();
    let mut state = AppState {
        items: vec!["Remotes".to_string(), "Branches".to_string(), "Status".to_string()],
        list_state: ListState::default(),
    };
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
        Some(0) => "Remote Management\n\n- [f] Fetch all\n- [p] Push all",
        Some(1) => "Branch Management\n\n- [n] Create new branch\n- [d] Delete selected branch",
        _ => "System Status\n\nAll systems are stable.",
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
}

fn handle_events(state: &mut AppState) -> io::Result<bool> {
    if event::poll(std::time::Duration::from_millis(100))? {
        if let Event::Key(key) = event::read()? {
            if key.kind == KeyEventKind::Press {
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
                    KeyCode::Char('f') => {
                        // Example: Trigger Fetch
                        eprintln!("Fetch triggered!"); 
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(false)
}
