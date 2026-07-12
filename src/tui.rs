use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame,
};
use std::collections::HashMap;
use std::io;
use std::time::Duration;

use crate::git::GitRepo;

// Custom Palette
const VIBRANT_PINK: Color = Color::Rgb(255, 105, 180);
const CYAN: Color = Color::Rgb(0, 255, 255);
const CREAM: Color = Color::Rgb(255, 253, 208);
const RED: Color = Color::Rgb(255, 69, 58);
const MAUVE: Color = Color::Rgb(224, 176, 255);
const GRAY: Color = Color::Rgb(120, 120, 120);
const GREEN: Color = Color::Rgb(120, 255, 160);

#[derive(Default)]
enum Overlay {
    #[default]
    None,
    AddName { value: String },
    AddUrl { name: String, value: String },
    RenameRemote { old: String, value: String },
    RemoveRemote { name: String },
    CreateBranch { step: u8, name: String, base: String, remote: String },
    DeleteBranch { name: String },
    RenameBranch { old: String, value: String },
    MergeRemote { value: String },
    MergeBranch { src_remote: String, value: String },
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
    status_mode: bool,
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
            status_mode: false,
        };
        state.refresh();
        state.remote_state.select(Some(0));
        state.branch_state.select(Some(0));
        Ok(state)
    }

    /// Reload remotes/branches while preserving the current selection and
    /// any branch multi-select toggles. Called on a timer for live updates.
    fn refresh(&mut self) {
        let prev_remote = self.remote_state.selected();
        let prev_branch = self.branch_state.selected();
        let prev_sel: HashMap<String, bool> =
            self.branches.iter().map(|(n, s)| (n.clone(), *s)).collect();

        if let Ok(list) = self.repo.list_remotes_with_urls() {
            self.remotes = list
                .into_iter()
                .map(|(name, url)| RemoteEntry { name, url })
                .collect();
        }
        if self.remotes.is_empty() {
            self.remote_state.select(None);
        } else {
            let i = prev_remote.map(|i| i.min(self.remotes.len() - 1)).unwrap_or(0);
            self.remote_state.select(Some(i));
        }

        self.branches.clear();
        if let Ok(names) = self.repo.local_branch_names() {
            for n in names {
                let sel = *prev_sel.get(&n).unwrap_or(&false);
                self.branches.push((n, sel));
            }
        }
        if self.branches.is_empty() {
            self.branch_state.select(None);
        } else {
            let i = prev_branch.map(|i| i.min(self.branches.len() - 1)).unwrap_or(0);
            self.branch_state.select(Some(i));
        }
    }

    fn selected_remote_name(&self) -> Option<String> {
        self.remote_state
            .selected()
            .and_then(|i| self.remotes.get(i))
            .map(|r| r.name.clone())
    }

    fn selected_branch_name(&self) -> Option<String> {
        self.branch_state
            .selected()
            .and_then(|i| self.branches.get(i))
            .map(|(n, _)| n.clone())
    }

    fn selected_branches(&self) -> Vec<String> {
        self.branches
            .iter()
            .filter(|(_, sel)| *sel)
            .map(|(b, _)| b.clone())
            .collect()
    }

    fn select_remote_by_name(&mut self, name: &str) {
        if let Some(i) = self.remotes.iter().position(|r| r.name == name) {
            self.remote_state.select(Some(i));
        }
    }

    fn select_branch_by_name(&mut self, name: &str) {
        if let Some(i) = self.branches.iter().position(|(n, _)| n == name) {
            self.branch_state.select(Some(i));
        }
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
                    self.refresh();
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

    fn action_merge(&mut self, src_remote: String, src_branch: String) {
        let target = self.selected_remote_name();
        let cur = match self.repo.current_branch() {
            Ok(Some(b)) => b,
            _ => {
                self.log("No current branch to merge into".to_string());
                return;
            }
        };
        let src_ref = format!("refs/remotes/{}/{}", src_remote, src_branch);
        let result = self
            .repo
            .fetch_remote(&src_remote)
            .and_then(|_| {
                if let Some(t) = &target {
                    self.repo.fetch_remote(t)
                } else {
                    Ok(())
                }
            })
            .and_then(|_| self.repo.checkout_branch(&cur))
            .and_then(|_| self.repo.merge_and_commit(&src_ref))
            .and_then(|_| {
                if let Some(t) = &target {
                    self.repo.push_to_remote(t, Some(&cur))
                } else {
                    Ok(())
                }
            });

        match result {
            Ok(()) => {
                self.refresh();
                self.log(format!(
                    "Merged {}/{} into {} and pushed to {:?}",
                    src_remote, src_branch, cur, target
                ));
            }
            Err(e) => self.log(format!("Merge failed: {}", e)),
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
        // Refresh is on-demand (lazygit-style): state is loaded at startup,
        // after every mutating action, when switching focus, and via `r`.
        // No tight timer poll, so the UI stays responsive.
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

    // Remotes panel
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

    // Branches panel (local branches, multi-select)
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

    // Details / Status panel
    let detail = if state.status_mode {
        state.repo.status_text().unwrap_or_else(|e| format!("Error: {}", e))
    } else {
        build_detail(state)
    };
    let detail_title = if state.status_mode { " Status " } else { " Details " };
    let main_view = Paragraph::new(detail)
        .block(Block::default().title(detail_title).borders(Borders::ALL).border_style(Style::default().fg(MAUVE)))
        .style(Style::default().fg(CREAM));
    f.render_widget(main_view, inner_layout[2]);

    // Footer
    let help_text = " [Tab] Focus  [↑/↓] Move  [Space] Toggle  [a] Add  [R] Rename remote  [x] Remove  [D] Default  [c] Create  [m] Rename branch  [f/Enter] Fetch  [p] Push  [l] Pull  [M] Merge  [s] Status  [r] Refresh  [q] Quit ";
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
        Overlay::RenameRemote { old, value } => {
            let area = centered_rect(60, 3, f.area());
            let modal = Paragraph::new(format!("Rename '{}' to:\n> {}\u{2588}", old, value))
                .block(Block::default().title(" Rename Remote ").borders(Borders::ALL).border_style(Style::default().fg(RED)));
            f.render_widget(ratatui::widgets::Clear, area);
            f.render_widget(modal, area);
        }
        Overlay::RemoveRemote { name } => {
            let area = centered_rect(60, 4, f.area());
            let modal = Paragraph::new(format!("Remove remote '{}'?\n\n[y] Yes  [n/Esc] Cancel", name))
                .block(Block::default().title(" Remove Remote ").borders(Borders::ALL).border_style(Style::default().fg(RED)));
            f.render_widget(ratatui::widgets::Clear, area);
            f.render_widget(modal, area);
        }
        Overlay::CreateBranch { step, name, base, remote } => {
            let (title, prompt) = match step {
                0 => (" Create Branch ", format!("Branch name:\n> {}\u{2588}", name)),
                1 => (" Create Branch ", format!("Base (commit/branch):\n> {}\u{2588}", base)),
                _ => (" Create Branch ", format!("Push to remote (empty = local only):\n> {}\u{2588}", remote)),
            };
            let area = centered_rect(65, 3, f.area());
            let modal = Paragraph::new(prompt)
                .block(Block::default().title(title).borders(Borders::ALL).border_style(Style::default().fg(RED)));
            f.render_widget(ratatui::widgets::Clear, area);
            f.render_widget(modal, area);
        }
        Overlay::DeleteBranch { name } => {
            let area = centered_rect(60, 4, f.area());
            let modal = Paragraph::new(format!("Delete local branch '{}'?\n\n[y] Yes  [n/Esc] Cancel", name))
                .block(Block::default().title(" Delete Branch ").borders(Borders::ALL).border_style(Style::default().fg(RED)));
            f.render_widget(ratatui::widgets::Clear, area);
            f.render_widget(modal, area);
        }
        Overlay::RenameBranch { old, value } => {
            let area = centered_rect(60, 3, f.area());
            let modal = Paragraph::new(format!("Rename '{}' to:\n> {}\u{2588}", old, value))
                .block(Block::default().title(" Rename Branch ").borders(Borders::ALL).border_style(Style::default().fg(RED)));
            f.render_widget(ratatui::widgets::Clear, area);
            f.render_widget(modal, area);
        }
        Overlay::MergeRemote { value } => {
            let area = centered_rect(60, 3, f.area());
            let modal = Paragraph::new(format!("Merge from remote:\n> {}\u{2588}", value))
                .block(Block::default().title(" Merge ").borders(Borders::ALL).border_style(Style::default().fg(VIBRANT_PINK)));
            f.render_widget(ratatui::widgets::Clear, area);
            f.render_widget(modal, area);
        }
        Overlay::MergeBranch { src_remote, value } => {
            let area = centered_rect(65, 3, f.area());
            let modal = Paragraph::new(format!("Merge branch from '{}':\n> {}\u{2588}", src_remote, value))
                .block(Block::default().title(" Merge ").borders(Borders::ALL).border_style(Style::default().fg(VIBRANT_PINK)));
            f.render_widget(ratatui::widgets::Clear, area);
            f.render_widget(modal, area);
        }
        Overlay::Message { text, is_error } => {
            let area = centered_rect(60, 4, f.area());
            let color = if *is_error { RED } else { GREEN };
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

            out.push_str("\nRemote actions:\n");
            out.push_str("  [a] Add   [R] Rename   [x] Remove   [D] Set default\n");
            out.push_str("  [f]/[Enter] Fetch   [p] Push   [l] Pull   [M] Merge\n");
        }
        None => {
            out.push_str("No remotes configured.\n\nPress [a] to add a remote.");
        }
    }

    out.push_str("\nBranch actions (focus Branches):\n");
    out.push_str("  [c] Create   [m] Rename   [x] Delete   [Space] toggle\n");

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
    if event::poll(Duration::from_millis(100))? {
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
                                            state.select_remote_by_name(&nm);
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
                    Overlay::RenameRemote { old, value } => {
                        match key.code {
                            KeyCode::Enter => {
                                let new = value.trim().to_string();
                                if !new.is_empty() {
                                    let o = old.clone();
                                    match state.repo.rename_remote(&o, &new) {
                                        Ok(()) => {
                                            state.refresh();
                                            state.select_remote_by_name(&new);
                                            state.log(format!("Renamed remote '{}' -> '{}'", o, new));
                                            state.overlay = Overlay::Message {
                                                text: format!("Renamed remote '{}' -> '{}'", o, new),
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
                            KeyCode::Backspace => { value.pop(); },
                            KeyCode::Esc => state.overlay = Overlay::None,
                            _ => {}
                        }
                        return Ok(false);
                    }
                    Overlay::RemoveRemote { name } => {
                        if matches!(key.code, KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter) {
                            let nm = name.clone();
                            match state.repo.remove_remote(&nm) {
                                Ok(()) => {
                                    state.refresh();
                                    state.log(format!("Removed remote '{}'", nm));
                                    state.overlay = Overlay::Message {
                                        text: format!("Removed remote '{}'", nm),
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
                        } else if matches!(key.code, KeyCode::Char('n') | KeyCode::Esc) {
                            state.overlay = Overlay::None;
                        }
                        return Ok(false);
                    }
                    Overlay::CreateBranch { step, name, base, remote } => {
                        match key.code {
                            KeyCode::Enter => match *step {
                                0 => {
                                    let n = name.trim().to_string();
                                    if !n.is_empty() {
                                        *step = 1;
                                        base.clear();
                                        if let Ok(Some(b)) = state.repo.current_branch() {
                                            base.push_str(&b);
                                        } else {
                                            base.push_str("main");
                                        }
                                    }
                                }
                                1 => {
                                    *step = 2;
                                    remote.clear();
                                    let rname = state
                                        .remote_state
                                        .selected()
                                        .and_then(|i| state.remotes.get(i))
                                        .map(|x| x.name.clone());
                                    if let Some(r) = rname {
                                        remote.push_str(&r);
                                    }
                                }
                                2 => {
                                    let nm = name.trim().to_string();
                                    let base_spec = if base.trim().is_empty() {
                                        state.repo.current_branch().ok().flatten().unwrap_or_else(|| "main".to_string())
                                    } else {
                                        base.trim().to_string()
                                    };
                                    let rm = remote.trim().to_string();
                                    if !nm.is_empty() {
                                        let res = state
                                            .repo
                                            .resolve_commit_spec(&base_spec)
                                            .and_then(|oid| Ok(state.repo.repo.find_commit(oid)?))
                                            .and_then(|commit| {
                                                state.repo.repo.branch(&nm, &commit, false)?;
                                                Ok(())
                                            })
                                            .and_then(|_| {
                                                if rm.is_empty() {
                                                    Ok(())
                                                } else {
                                                    state.repo.push_to_remote(&rm, Some(&nm))
                                                }
                                            });
                                        match res {
                                            Ok(()) => {
                                                state.refresh();
                                                state.select_branch_by_name(&nm);
                                                state.log(format!("Created branch '{}'", nm));
                                                state.overlay = Overlay::Message {
                                                    text: format!("Created branch '{}'", nm),
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
                                _ => {}
                            },
                            KeyCode::Char(c) => match *step {
                                0 => name.push(c),
                                1 => base.push(c),
                                2 => remote.push(c),
                                _ => {}
                            },
                            KeyCode::Backspace => match *step {
                                0 => {
                                    name.pop();
                                }
                                1 => {
                                    base.pop();
                                }
                                2 => {
                                    remote.pop();
                                }
                                _ => {}
                            },
                            KeyCode::Esc => state.overlay = Overlay::None,
                            _ => {}
                        }
                        return Ok(false);
                    }
                    Overlay::DeleteBranch { name } => {
                        if matches!(key.code, KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter) {
                            let nm = name.clone();
                            match state.repo.delete_local_branch(&nm, false) {
                                Ok(()) => {
                                    state.refresh();
                                    state.log(format!("Deleted branch '{}'", nm));
                                    state.overlay = Overlay::Message {
                                        text: format!("Deleted branch '{}'", nm),
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
                        } else if matches!(key.code, KeyCode::Char('n') | KeyCode::Esc) {
                            state.overlay = Overlay::None;
                        }
                        return Ok(false);
                    }
                    Overlay::RenameBranch { old, value } => {
                        match key.code {
                            KeyCode::Enter => {
                                let new = value.trim().to_string();
                                if !new.is_empty() {
                                    let o = old.clone();
                                    match state.repo.rename_branch(&o, &new) {
                                        Ok(()) => {
                                            state.refresh();
                                            state.select_branch_by_name(&new);
                                            state.log(format!("Renamed branch '{}' -> '{}'", o, new));
                                            state.overlay = Overlay::Message {
                                                text: format!("Renamed branch '{}' -> '{}'", o, new),
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
                            KeyCode::Backspace => { value.pop(); },
                            KeyCode::Esc => state.overlay = Overlay::None,
                            _ => {}
                        }
                        return Ok(false);
                    }
                    Overlay::MergeRemote { value } => {
                        match key.code {
                            KeyCode::Enter => {
                                let r = value.trim().to_string();
                                if !r.is_empty() {
                                    state.overlay = Overlay::MergeBranch { src_remote: r, value: String::new() };
                                }
                            }
                            KeyCode::Char(c) => value.push(c),
                            KeyCode::Backspace => { value.pop(); },
                            KeyCode::Esc => state.overlay = Overlay::None,
                            _ => {}
                        }
                        return Ok(false);
                    }
                    Overlay::MergeBranch { src_remote, value } => {
                        match key.code {
                            KeyCode::Enter => {
                                let b = value.trim().to_string();
                                if !b.is_empty() {
                                    let sr = src_remote.clone();
                                    state.action_merge(sr, b);
                                    state.overlay = Overlay::Message {
                                        text: "Merge complete (see log)".to_string(),
                                        is_error: false,
                                    };
                                }
                            }
                            KeyCode::Char(c) => value.push(c),
                            KeyCode::Backspace => { value.pop(); },
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
                // Shift+M => merge (works regardless of focus)
                if key.modifiers.contains(KeyModifiers::SHIFT) && key.code == KeyCode::Char('m') {
                    state.overlay = Overlay::MergeRemote { value: String::new() };
                    return Ok(false);
                }

                match key.code {
                    KeyCode::Char('q') => return Ok(true),
                    KeyCode::Tab | KeyCode::Right | KeyCode::Left => {
                        state.focus = match state.focus {
                            Focus::Remotes => Focus::Branches,
                            Focus::Branches => Focus::Remotes,
                        };
                        // Reload context so the focused panel reflects current state.
                        state.refresh();
                    }
                    KeyCode::Down => match state.focus {
                        Focus::Remotes => {
                            if !state.remotes.is_empty() {
                                let i = state.remote_state.selected().map(|i| (i + 1) % state.remotes.len());
                                state.remote_state.select(i);
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
                    KeyCode::Char('s') => state.status_mode = !state.status_mode,
                    KeyCode::Char('a') => {
                        state.overlay = Overlay::AddName { value: String::new() };
                    }
                    KeyCode::Char('c') => {
                        state.overlay = Overlay::CreateBranch {
                            step: 0,
                            name: String::new(),
                            base: String::new(),
                            remote: String::new(),
                        };
                    }
                    KeyCode::Char('m') => {
                        if let Some(name) = state.selected_branch_name() {
                            state.overlay = Overlay::RenameBranch { old: name, value: String::new() };
                        }
                    }
                    KeyCode::Char('f') => state.action_fetch(),
                    KeyCode::Char('p') => state.action_push(),
                    KeyCode::Char('l') => state.action_pull(),
                    KeyCode::Enter => state.action_fetch(),
                    _ => {}
                }

                // Focus-specific single-key actions
                match state.focus {
                    Focus::Remotes => match key.code {
                        KeyCode::Char('R') => {
                            if let Some(name) = state.selected_remote_name() {
                                state.overlay = Overlay::RenameRemote { old: name, value: String::new() };
                            }
                        }
                        KeyCode::Char('x') | KeyCode::Delete => {
                            if let Some(name) = state.selected_remote_name() {
                                state.overlay = Overlay::RemoveRemote { name };
                            }
                        }
                        KeyCode::Char('D') => {
                            if let Some(name) = state.selected_remote_name() {
                                if state.repo.config.set_default_remote(name.clone()).is_ok() {
                                    let _ = state.repo.config.save(&state.repo.repo);
                                    state.refresh();
                                    state.log(format!("Default remote set to '{}'", name));
                                }
                            }
                        }
                        _ => {}
                    },
                    Focus::Branches => match key.code {
                        KeyCode::Char('x') | KeyCode::Delete => {
                            if let Some(name) = state.selected_branch_name() {
                                state.overlay = Overlay::DeleteBranch { name };
                            }
                        }
                        _ => {}
                    },
                }
            }
        }
    }
    Ok(false)
}
