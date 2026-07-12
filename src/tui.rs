use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
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
const GRAY: Color = Color::Rgb(120, 120, 120);

#[derive(Default)]
enum Overlay {
    #[default]
    None,
    AddName { value: String },
    AddUrl { name: String, value: String },
    Message { text: String, is_error: bool },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    Remotes,
    Branches,
}

struct RemoteEntry {
    name: String,
    url: String,
}

struct AppState {
    repo: GitRepo,
    remotes: Vec<RemoteEntry>,
    remote_state: ListState,
    branches: Vec<(String, bool)>,
    branch_state: ListState,
    focus: Focus,
    overlay: Overlay,
    log: Vec<String>,
}

impl AppState {
    fn new() -> io::Result<Self> {
        let repo = GitRepo::open().map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
        let mut state = Self {
            repo,
            remotes: Vec::new(),
            remote_state: ListState::default(),
            branches: Vec::new(),
            branch_state: ListState::default(),
            focus: Focus::Remotes,
            overlay: Overlay::None,
            log: Vec::new(),
        };
        state.refresh();
        state.remote_state.select(Some(0));
        state.branch_state.select(Some(0));
        Ok(state)
    }

    fn refresh(&mut self) {
        if let Ok(list) = self.repo.list_remotes_with_urls() {
            self.remotes = list
                .into_iter()
                .map(|(name, url)| RemoteEntry { name, url })
                .collect();
        }
        if self.remotes.is_empty() {
            self.remote_state.select(None);
        } else if self.remote_state.selected().map_or(true, |i| i >= self.remotes.len()) {
            self.remote_state.select(Some(0));
        }
        self.load_branches();
    }

    fn selected_remote_name(&self) -> Option<String> {
        self.remote_state.selected().and_then(|i| self.remotes.get(i)).map(|r| r.name.clone())
    }

    fn load_branches(&mut self) {
        self.branches.clear();
        if let Some(name) = self.selected_remote_name() {
            if let Ok(list) = self.repo.list_remote_branches(&name) {
                for b in list {
                    self.branches.push((b, false));
                }
            }
        }
        if self.branches.is_empty() {
            self.branch_state.select(None);
        } else if self.branch_state.selected().map_or(true, |i| i >= self.branches.len()) {
            self.branch_state.select(Some(0));
        }
    }

    fn selected_branches(&self) -> Vec<String> {
        self.branches
            .iter()
            .filter(|(_, sel)| *sel)
            .map(|(b, _)| b.clone())
            .collect()
    }

    fn log(&mut self, line: String) {
        self.log.push(line);
        if self.log.len() > 200 {
            self.log.remove(0);
        }
    }

    fn action_fetch(&mut self) {
        if let Some(name) = self.selected_remote_name() {
            let selected = self.selected_branches();
            let result = if selected.is_empty() {
                self.log(format!("Fetching all branches from '{}'", name));
                self.repo.fetch_remote(&name)
            } else {
                self.log(format!("Fetching {:?} from '{}'", selected, name));
                self.repo.fetch_branches(&name, &selected)
            };
            match result {
                Ok(()) => {
                    self.load_branches();
                    self.log(format!("Fetched from '{}'", name));
                }
                Err(e) => self.log(format!("Fetch '{}' failed: {}", name, e)),
            }
        } else {
            self.log("No remote selected".to_string());
        }
    }

    fn action_push(&mut self) {
        if let Some(name) = self.selected_remote_name() {
            let selected = self.selected_branches();
            if selected.is_empty() {
                match self.repo.push_to_remote(&name, None) {
                    Ok(()) => self.log(format!("Pushed current branch to '{}'", name)),
                    Err(e) => self.log(format!("Push '{}' failed: {}", name, e)),
                }
            } else {
                self.log(format!("Pushing {:?} to '{}'", selected, name));
                match self.repo.push_branches(&name, &selected, false) {
                    Ok(()) => self.log(format!("Pushed to '{}'", name)),
                    Err(e) => self.log(format!("Push '{}' failed: {}", name, e)),
                }
            }
        } else {
            self.log("No remote selected".to_string());
        }
    }

    fn action_pull(&mut self) {
        if let Some(name) = self.selected_remote_name() {
            let selected = self.selected_branches();
            if selected.is_empty() {
                match self.repo.pull_from_remote(&name, None) {
                    Ok(()) => self.log(format!("Pulled current branch from '{}'", name)),
                    Err(e) => self.log(format!("Pull '{}' failed: {}", name, e)),
                }
            } else {
                self.log(format!("Pulling {:?} from '{}'", selected, name));
                match self.repo.pull_branches(&name, &selected) {
                    Ok(()) => self.log(format!("Pulled from '{}'", name)),
                    Err(e) => self.log(format!("Pull '{}' failed: {}", name, e)),
                }
            }
        } else {
            self.log("No remote selected".to_string());
        }
    }
}

pub fn run_tui() -> io::Result<()> {
    let mut terminal = ratatui::init();
    let mut state = match AppState::new() {
        Ok(s) => s,
        Err(e) => {
            ratatui::restore();
            return Err(e);
        }
    };

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
        .constraints([Constraint::Percentage(26), Constraint::Percentage(32), Constraint::Percentage(42)])
        .split(layout[0]);

    // Sidebar: list of remotes
    let default = state.repo.config.get_default_remote().cloned();
    let items: Vec<ListItem> = state
        .remotes
        .iter()
        .map(|r| {
            let marker = if default.as_deref() == Some(&r.name) {
                " [default]"
            } else {
                ""
            };
            ListItem::new(format!("{}{}", r.name, marker))
        })
        .collect();
    let remote_title = if state.focus == Focus::Remotes {
        " Remotes (focused) "
    } else {
        " Remotes "
    };
    let list = List::new(items)
        .block(Block::default().title(remote_title).borders(Borders::ALL).border_style(border_style(state.focus == Focus::Remotes)))
        .highlight_style(Style::default().bg(CYAN).fg(Color::Black))
        .highlight_symbol(">> ");
    f.render_stateful_widget(list, inner_layout[0], &mut state.remote_state);

    // Middle: branches of selected remote (multi-select)
    let branch_items: Vec<ListItem> = state
        .branches
        .iter()
        .map(|(b, sel)| {
            let mark = if *sel { "[x]" } else { "[ ]" };
            ListItem::new(format!("{} {}", mark, b))
        })
        .collect();
    let branch_title = if state.focus == Focus::Branches {
        " Branches (focused) "
    } else {
        " Branches "
    };
    let sel_count = state.selected_branches().len();
    let branch_block = Block::default()
        .title(format!("{} [{} selected]", branch_title, sel_count))
        .borders(Borders::ALL)
        .border_style(border_style(state.focus == Focus::Branches));
    let branch_list = List::new(branch_items)
        .block(branch_block)
        .highlight_style(Style::default().bg(MAUVE).fg(Color::Black))
        .highlight_symbol(">> ");
    f.render_stateful_widget(branch_list, inner_layout[1], &mut state.branch_state);

    // Right: details + log
    let detail = build_detail(state);
    let main_view = Paragraph::new(detail)
        .block(Block::default().title(" Remote Details ").borders(Borders::ALL).border_style(Style::default().fg(MAUVE)))
        .style(Style::default().fg(CREAM));
    f.render_widget(main_view, inner_layout[2]);

    // Footer
    let help_text = " [Tab] Focus  [↑/↓] Move  [Space] Toggle branch  [f/Enter] Fetch  [p] Push  [l] Pull  [a] Add  [r] Refresh  [q] Quit ";
    let footer = Paragraph::new(help_text)
        .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(CYAN)))
        .style(Style::default().fg(CREAM).bg(Color::Rgb(50, 50, 50)));
    f.render_widget(footer, layout[1]);

    // Overlay
    match &state.overlay {
        Overlay::AddName { value } => {
            let area = centered_rect(60, 3, f.area());
            let modal = Paragraph::new(format!("Remote name:\n> {}\u{2588}", value))
                .block(Block::default().title(" Add Remote ").borders(Borders::ALL).border_style(Style::default().fg(RED)));
            f.render_widget(ratatui::widgets::Clear, area);
            f.render_widget(modal, area);
        }
        Overlay::AddUrl { name, value } => {
            let area = centered_rect(70, 3, f.area());
            let modal = Paragraph::new(format!("URL for '{}':\n> {}\u{2588}", name, value))
                .block(Block::default().title(" Add Remote ").borders(Borders::ALL).border_style(Style::default().fg(RED)));
            f.render_widget(ratatui::widgets::Clear, area);
            f.render_widget(modal, area);
        }
        Overlay::Message { text, is_error } => {
            let area = centered_rect(60, 4, f.area());
            let color = if *is_error { RED } else { VIBRANT_PINK };
            let modal = Paragraph::new(format!("{}\n\n[Enter/Esc to dismiss]", text))
                .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(color)));
            f.render_widget(ratatui::widgets::Clear, area);
            f.render_widget(modal, area);
        }
        Overlay::None => {}
    }
}

fn border_style(focused: bool) -> Style {
    if focused {
        Style::default().fg(CYAN)
    } else {
        Style::default().fg(GRAY)
    }
}

fn build_detail(state: &AppState) -> String {
    let mut out = String::new();
    match state.remote_state.selected().and_then(|i| state.remotes.get(i)) {
        Some(r) => {
            let default = state.repo.config.get_default_remote().cloned();
            let default_mark = if default.as_deref() == Some(&r.name) {
                " [default]"
            } else {
                ""
            };
            out.push_str(&format!("Remote: {}{}\n", r.name, default_mark));
            out.push_str(&format!("URL:    {}\n", r.url));

            if let Some(branch) = state.repo.current_branch().ok().flatten() {
                out.push_str(&format!("Current branch: {}\n", branch));
            }

            let selected = state.selected_branches();
            if selected.is_empty() {
                out.push_str("\nTarget: all branches (or current branch for push/pull)\n");
            } else {
                out.push_str(&format!("\nTarget branches ({}):\n", selected.len()));
                for b in &selected {
                    out.push_str(&format!("  - {}\n", b));
                }
            }

            out.push_str("\nActions:\n");
            out.push_str("  [f]/[Enter] Fetch    [p] Push    [l] Pull\n");
            out.push_str("  [Space] toggle branch selection\n");
            out.push_str("  [a] Add remote       [r] Refresh\n");
        }
        None => {
            out.push_str("No remotes configured.\n\nPress [a] to add a remote.");
        }
    }

    out.push_str("\nLog:\n");
    let start = state.log.len().saturating_sub(10);
    for line in &state.log[start..] {
        out.push_str(&format!("  {}\n", line));
    }
    out
}

fn centered_rect(percent_x: u16, height: u16, r: Rect) -> Rect {
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
                match &mut state.overlay {
                    Overlay::AddName { value } => {
                        match key.code {
                            KeyCode::Enter => {
                                let name = value.trim().to_string();
                                if !name.is_empty() {
                                    state.overlay = Overlay::AddUrl { name, value: String::new() };
                                }
                            }
                            KeyCode::Char(c) => value.push(c),
                            KeyCode::Backspace => {
                                value.pop();
                            }
                            KeyCode::Esc => state.overlay = Overlay::None,
                            _ => {}
                        }
                        return Ok(false);
                    }
                    Overlay::AddUrl { name, value } => {
                        match key.code {
                            KeyCode::Enter => {
                                let url = value.trim().to_string();
                                if !url.is_empty() {
                                    let nm = name.clone();
                                    match state.repo.add_remote(&nm, &url) {
                                        Ok(()) => {
                                            state.refresh();
                                            state.log(format!("Added remote '{}'", nm));
                                            state.overlay = Overlay::Message {
                                                text: format!("Added remote '{}'", nm),
                                                is_error: false,
                                            };
                                        }
                                        Err(e) => {
                                            state.overlay = Overlay::Message {
                                                text: format!("Error: {}", e),
                                                is_error: true,
                                            };
                                        }
                                    }
                                }
                            }
                            KeyCode::Char(c) => value.push(c),
                            KeyCode::Backspace => {
                                value.pop();
                            }
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
                    Overlay::None => {}
                }

                // Normal navigation / actions
                match key.code {
                    KeyCode::Char('q') => return Ok(true),
                    KeyCode::Tab | KeyCode::Right | KeyCode::Left => {
                        state.focus = match state.focus {
                            Focus::Remotes => Focus::Branches,
                            Focus::Branches => Focus::Remotes,
                        };
                    }
                    KeyCode::Down => match state.focus {
                        Focus::Remotes => {
                            if !state.remotes.is_empty() {
                                let i = state.remote_state.selected().map(|i| (i + 1) % state.remotes.len());
                                state.remote_state.select(i);
                                state.load_branches();
                            }
                        }
                        Focus::Branches => {
                            if !state.branches.is_empty() {
                                let i = state.branch_state.selected().map(|i| (i + 1) % state.branches.len());
                                state.branch_state.select(i);
                            }
                        }
                    },
                    KeyCode::Up => match state.focus {
                        Focus::Remotes => {
                            if !state.remotes.is_empty() {
                                let i = state.remote_state.selected().map(|i| {
                                    if i == 0 {
                                        state.remotes.len() - 1
                                    } else {
                                        i - 1
                                    }
                                });
                                state.remote_state.select(i);
                                state.load_branches();
                            }
                        }
                        Focus::Branches => {
                            if !state.branches.is_empty() {
                                let i = state.branch_state.selected().map(|i| {
                                    if i == 0 {
                                        state.branches.len() - 1
                                    } else {
                                        i - 1
                                    }
                                });
                                state.branch_state.select(i);
                            }
                        }
                    },
                    KeyCode::Char(' ') => {
                        if state.focus == Focus::Branches {
                            if let Some(i) = state.branch_state.selected() {
                                if let Some((_, sel)) = state.branches.get_mut(i) {
                                    *sel = !*sel;
                                }
                            }
                        }
                    }
                    KeyCode::Char('r') => state.refresh(),
                    KeyCode::Char('a') => {
                        state.overlay = Overlay::AddName { value: String::new() };
                    }
                    KeyCode::Char('f') => state.action_fetch(),
                    KeyCode::Char('p') => state.action_push(),
                    KeyCode::Char('l') => state.action_pull(),
                    KeyCode::Enter => state.action_fetch(),
                    _ => {}
                }
            }
        }
    }
    Ok(false)
}
