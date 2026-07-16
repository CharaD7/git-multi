use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};
use std::collections::HashMap;
use std::io;
use std::time::Duration;

use crate::git::{BlameLine, CommitGraph, DiffMode, FileStatus, ResetMode};

// Custom Palette
const VIBRANT_PINK: Color = Color::Rgb(255, 105, 180);
const CYAN: Color = Color::Rgb(0, 255, 255);
const CREAM: Color = Color::Rgb(255, 253, 208);
const RED: Color = Color::Rgb(255, 69, 58);
const MAUVE: Color = Color::Rgb(224, 176, 255);
const GRAY: Color = Color::Rgb(120, 120, 120);
const GREEN: Color = Color::Rgb(120, 255, 160);
const YELLOW: Color = Color::Rgb(255, 209, 102);
const BLUE: Color = Color::Rgb(120, 180, 255);
const ORANGE: Color = Color::Rgb(255, 159, 64);

#[derive(Default, Clone, PartialEq, Eq)]
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
    Merge { step: u8, src_remote: String, src_branch: String, dest_remote: String, dest_branch: String },
    CommitType { value: String },
    CommitMsg { value: String },
    CommitBody { value: String },
    AmendMsg { value: String },
    RevertCommit { value: String },
    ResetCommit { value: String, mode: ResetMode },
    DiffPath { value: String, mode: DiffMode },
    CherryPick { value: String },
    Message { text: String, is_error: bool },
}

/// Which detail panel mode is active.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DetailMode {
    Detail,
    Status,
    Files,
    DiffStaged,
    DiffUnstaged,
    Blame,
    Graph,
    Commit,
}

impl DetailMode {
    fn title(self) -> &'static str {
        match self {
            DetailMode::Detail => " Details ",
            DetailMode::Status => " Status ",
            DetailMode::Files => " Files (staged/unstaged) ",
            DetailMode::DiffStaged => " Diff (staged) ",
            DetailMode::DiffUnstaged => " Diff (unstaged) ",
            DetailMode::Blame => " Blame (GitLens) ",
            DetailMode::Graph => " Git Graph ",
            DetailMode::Commit => " Commit ",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    Remotes,
    Branches,
    Files,
    Detail,
    Graph,
}

struct RemoteEntry {
    name: String,
    url: String,
}

struct AppState {
    repo: crate::git::GitRepo,
    remotes: Vec<RemoteEntry>,
    remote_state: ListState,
    branches: Vec<(String, bool)>,
    branch_state: ListState,
    files: Vec<FileStatus>,
    file_state: ListState,
    focus: Focus,
    overlay: Overlay,
    log: Vec<String>,
    detail_mode: DetailMode,
    commit_msg: String,
    // Cached heavier views (refreshed on demand)
    blame: Vec<BlameLine>,
    blame_path: String,
    graph: Option<CommitGraph>,
    graph_all: bool,
    graph_state: ListState,
}

impl AppState {
    fn new() -> io::Result<Self> {
        let repo = crate::git::GitRepo::open().map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
        let mut state = Self {
            repo,
            remotes: Vec::new(),
            remote_state: ListState::default(),
            branches: Vec::new(),
            branch_state: ListState::default(),
            files: Vec::new(),
            file_state: ListState::default(),
            focus: Focus::Remotes,
            overlay: Overlay::None,
            log: Vec::new(),
            detail_mode: DetailMode::Detail,
            commit_msg: String::new(),
            blame: Vec::new(),
            blame_path: String::new(),
            graph: None,
            graph_all: false,
            graph_state: ListState::default(),
        };
        state.refresh();
        state.remote_state.select(Some(0));
        state.branch_state.select(Some(0));
        Ok(state)
    }

    /// Reload remotes/branches/files while preserving selection.
    fn refresh(&mut self) {
        let prev_remote = self.remote_state.selected();
        let prev_branch = self.branch_state.selected();
        let prev_file = self.file_state.selected();
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

        if let Ok(status) = self.repo.working_status() {
            self.files = status;
        }
        if self.files.is_empty() {
            self.file_state.select(None);
        } else {
            let i = prev_file.map(|i| i.min(self.files.len() - 1)).unwrap_or(0);
            self.file_state.select(Some(i));
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

    fn selected_file_path(&self) -> Option<String> {
        self.file_state
            .selected()
            .and_then(|i| self.files.get(i))
            .map(|f| f.path.clone())
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

    fn action_merge_explicit(&mut self, src_remote: String, src_branch: String, dest_remote: String, dest_branch: String) {
        let src_ref = format!("refs/remotes/{}/{}", src_remote, src_branch);
        let result = self
            .repo
            .fetch_remote(&src_remote)
            .and_then(|_| self.repo.fetch_remote(&dest_remote))
            .and_then(|_| self.repo.checkout_branch(&dest_branch))
            .and_then(|_| self.repo.merge_and_commit(&src_ref))
            .and_then(|_| self.repo.push_to_remote(&dest_remote, Some(&dest_branch)));

        match result {
            Ok(()) => {
                self.refresh();
                self.log(format!(
                    "Merged {}/{} into {}/{} and pushed",
                    src_remote, src_branch, dest_remote, dest_branch
                ));
            }
            Err(e) => self.log(format!("Merge failed: {}", e)),
        }
    }

    fn action_commit(&mut self, subject: String, body: Option<&str>) {
        match self.repo.create_commit(&subject, body) {
            Ok(_) => {
                self.refresh();
                self.log(format!("Created commit: {}", subject));
            }
            Err(e) => self.log(format!("Commit failed: {}", e)),
        }
    }

    fn do_stage(&mut self, path: &str) {
        match self.repo.stage_file(path) {
            Ok(()) => {
                self.refresh();
                self.log(format!("Staged: {}", path));
            }
            Err(e) => self.log(format!("Stage failed: {}", e)),
        }
    }

    fn do_unstage(&mut self, path: &str) {
        match self.repo.unstage_file(path) {
            Ok(()) => {
                self.refresh();
                self.log(format!("Unstaged: {}", path));
            }
            Err(e) => self.log(format!("Unstage failed: {}", e)),
        }
    }

    fn do_amend(&mut self, msg: String) {
        match self.repo.amend_commit(&msg, None) {
            Ok(()) => {
                self.refresh();
                self.log("Amended last commit".to_string());
            }
            Err(e) => self.log(format!("Amend failed: {}", e)),
        }
    }

    fn do_revert(&mut self, spec: String) {
        match self.repo.revert_commit(&spec) {
            Ok(()) => {
                self.refresh();
                self.log(format!("Reverted {}", spec));
            }
            Err(e) => self.log(format!("Revert failed: {}", e)),
        }
    }

    fn do_reset(&mut self, mode: ResetMode, spec: String) {
        match self.repo.reset(mode, &spec) {
            Ok(()) => {
                self.refresh();
                self.log(format!("Reset ({:?}) to {}", mode, spec));
            }
            Err(e) => self.log(format!("Reset failed: {}", e)),
        }
    }

    fn do_cherry_pick(&mut self, spec: String) {
        match self.repo.cherry_pick_commit(&spec) {
            Ok(()) => {
                self.refresh();
                self.log(format!("Cherry-picked {}", spec));
            }
            Err(e) => self.log(format!("Cherry-pick failed: {}", e)),
        }
    }

    fn load_blame(&mut self, path: &str) {
        match self.repo.blame_file(path, None) {
            Ok(b) => {
                self.blame = b;
                self.blame_path = path.to_string();
                self.detail_mode = DetailMode::Blame;
            }
            Err(e) => self.log(format!("Blame failed: {}", e)),
        }
    }

    fn load_graph(&mut self) {
        match self.repo.commit_graph(self.graph_all, 300) {
            Ok(g) => {
                self.graph = Some(g);
                self.detail_mode = DetailMode::Graph;
                self.graph_state.select(Some(0));
            }
            Err(e) => self.log(format!("Graph failed: {}", e)),
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

    let inner = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(18),
            Constraint::Percentage(18),
            Constraint::Percentage(20),
            Constraint::Percentage(44),
        ])
        .split(layout[0]);

    render_remotes(f, state, inner[0]);
    render_branches(f, state, inner[1]);
    render_files(f, state, inner[2]);
    render_detail(f, state, inner[3]);

    let help = "[Tab] Focus  [↑/↓] Move  [Space] Toggle  [f] Fetch [p] Push [l] Pull  [M] Merge \
[C] Commit  [a] Add remote  [c] Branch  [m] Rename  [x] Delete  [D] Default\n\
[g] Git Graph  [b] Blame file  [d] Diff  [F] Files  [s] Status  [S] Stage/Unstage file  \
[A] Amend  [R] Revert  [Z] Reset  [P] Cherry-pick  [v] Commits  [r] Refresh  [q] Quit";
    let footer = Paragraph::new(help)
        .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(CYAN)))
        .style(Style::default().fg(CREAM).bg(Color::Rgb(50, 50, 50)));
    f.render_widget(footer, layout[1]);

    render_overlay(f, state);
}

fn render_remotes(f: &mut Frame, state: &AppState, area: Rect) {
    let default = state.repo.config.get_default_remote().cloned();
    let items: Vec<ListItem> = state
        .remotes
        .iter()
        .map(|r| {
            let marker = if default.as_deref() == Some(&r.name) { " [default]" } else { "" };
            ListItem::new(format!("{}{}", r.name, marker))
        })
        .collect();
    let title = if state.focus == Focus::Remotes { " Remotes (focused) " } else { " Remotes " };
    let list = List::new(items)
        .block(Block::default().title(title).borders(Borders::ALL).border_style(border_style(state.focus == Focus::Remotes)))
        .highlight_style(Style::default().bg(CYAN).fg(Color::Black))
        .highlight_symbol(">> ");
    f.render_stateful_widget(list, area, &mut state.remote_state.clone());
}

fn render_branches(f: &mut Frame, state: &AppState, area: Rect) {
    let branch_items: Vec<ListItem> = state
        .branches
        .iter()
        .map(|(b, sel)| {
            let mark = if *sel { "[x]" } else { "[ ]" };
            ListItem::new(format!("{} {}", mark, b))
        })
        .collect();
    let title = if state.focus == Focus::Branches { " Branches (focused) " } else { " Branches " };
    let sel_count = state.selected_branches().len();
    let block = Block::default()
        .title(format!("{} [{} selected]", title, sel_count))
        .borders(Borders::ALL)
        .border_style(border_style(state.focus == Focus::Branches));
    let branch_list = List::new(branch_items)
        .block(block)
        .highlight_style(Style::default().bg(MAUVE).fg(Color::Black))
        .highlight_symbol(">> ");
    f.render_stateful_widget(branch_list, area, &mut state.branch_state.clone());
}

fn render_files(f: &mut Frame, state: &AppState, area: Rect) {
    let items: Vec<ListItem> = state
        .files
        .iter()
        .map(|f| {
            let staged = if f.staged == ' ' { " ".to_string() } else { format!("{}", f.staged) };
            let un = if f.unstaged == ' ' { " ".to_string() } else { format!("{}", f.unstaged) };
            let style = if f.staged != ' ' {
                Style::default().fg(GREEN)
            } else if f.unstaged != ' ' {
                Style::default().fg(YELLOW)
            } else {
                Style::default().fg(GRAY)
            };
            ListItem::new(format!("{}|{}  {}", staged, un, f.path)).style(style)
        })
        .collect();
    let title = if state.focus == Focus::Files { " Files (focused) " } else { " Files " };
    let block = Block::default()
        .title(format!("{} [{} changed]", title, state.files.len()))
        .borders(Borders::ALL)
        .border_style(border_style(state.focus == Focus::Files));
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().bg(BLUE).fg(Color::Black))
        .highlight_symbol(">> ");
    f.render_stateful_widget(list, area, &mut state.file_state.clone());
}

fn render_detail(f: &mut Frame, state: &mut AppState, area: Rect) {
    let block = Block::default()
        .title(state.detail_mode.title())
        .borders(Borders::ALL)
        .border_style(Style::default().fg(MAUVE));
    let text = match state.detail_mode {
        DetailMode::Detail => build_detail(state),
        DetailMode::Status => state
            .repo
            .status_text()
            .unwrap_or_else(|e| format!("Error: {}", e)),
        DetailMode::Files => build_files(state),
        DetailMode::DiffStaged => state
            .repo
            .diff(DiffMode::Staged, None)
            .unwrap_or_else(|e| format!("Error: {}", e)),
        DetailMode::DiffUnstaged => state
            .repo
            .diff(DiffMode::Unstaged, None)
            .unwrap_or_else(|e| format!("Error: {}", e)),
        DetailMode::Blame => build_blame(state),
        DetailMode::Graph => build_graph(state),
        DetailMode::Commit => build_commit(state),
    };
    let p = Paragraph::new(text)
        .block(block)
        .style(Style::default().fg(CREAM))
        .wrap(Wrap { trim: false })
        .scroll((0, 0));
    f.render_widget(p, area);
    if state.detail_mode == DetailMode::Graph {
        // Re-render graph as a list for selection highlight.
        let items: Vec<ListItem> = graph_lines(state)
            .into_iter()
            .map(|(s, hot)| {
                if hot {
                    ListItem::new(s).style(Style::default().fg(YELLOW).add_modifier(Modifier::BOLD))
                } else {
                    ListItem::new(s).style(Style::default().fg(CREAM))
                }
            })
            .collect();
        let list = List::new(items)
            .block(Block::default().title(state.detail_mode.title()).borders(Borders::ALL).border_style(Style::default().fg(MAUVE)))
            .highlight_style(Style::default().bg(ORANGE).fg(Color::Black))
            .highlight_symbol(">> ");
        f.render_stateful_widget(list, area, &mut state.graph_state.clone());
    }
}

fn render_overlay(f: &mut Frame, state: &AppState) {
    match &state.overlay {
        Overlay::AddName { value } => modal(f, 60, 3, " Add Remote ",
            &format!("Remote name:\n> {}\u{2588}", value), RED),
        Overlay::AddUrl { name, value } => modal(f, 70, 3, " Add Remote ",
            &format!("URL for '{}':\n> {}\u{2588}", name, value), RED),
        Overlay::RenameRemote { old, value } => modal(f, 60, 3, " Rename Remote ",
            &format!("Rename '{}' to:\n> {}\u{2588}", old, value), RED),
        Overlay::RemoveRemote { name } => modal(f, 60, 4, " Remove Remote ",
            &format!("Remove remote '{}'?\n\n[y] Yes  [n/Esc] Cancel", name), RED),
        Overlay::CreateBranch { step, name, base, remote } => {
            let prompt = match step {
                0 => format!("Branch name:\n> {}\u{2588}", name),
                1 => format!("Base (commit/branch):\n> {}\u{2588}", base),
                _ => format!("Push to remote (empty = local only):\n> {}\u{2588}", remote),
            };
            modal(f, 65, 3, " Create Branch ", &prompt, RED)
        }
        Overlay::DeleteBranch { name } => modal(f, 60, 4, " Delete Branch ",
            &format!("Delete local branch '{}'?\n\n[y] Yes  [n/Esc] Cancel", name), RED),
        Overlay::RenameBranch { old, value } => modal(f, 60, 3, " Rename Branch ",
            &format!("Rename '{}' to:\n> {}\u{2588}", old, value), RED),
        Overlay::Merge { step, src_remote, src_branch, dest_remote, dest_branch } => {
            let prompt = match step {
                0 => format!("Source remote:\n> {}\u{2588}", src_remote),
                1 => format!("Source branch (from {}/{}):\n> {}\u{2588}", src_remote, src_remote, src_branch),
                2 => format!("Destination remote:\n> {}\u{2588}", dest_remote),
                _ => format!("Destination branch:\n> {}\u{2588}", dest_branch),
            };
            modal(f, 65, 3, " Merge ", &prompt, VIBRANT_PINK)
        }
        Overlay::CommitType { value } => modal(f, 60, 5, " Commit Type ",
            &format!("Select commit type:\n\n[f] feat  [x] fix  [d] docs  [s] style  [r] refactor\n[T] test  [c] chore  [b] build  [p] perf\n\nOr type to filter:\n> {}\u{2588}", value), GREEN),
        Overlay::CommitMsg { value } => modal(f, 70, 3, " Commit Message ",
            &format!("Commit subject:\n> {}\u{2588}", value), GREEN),
        Overlay::CommitBody { value } => modal(f, 70, 5, " Commit Body ",
            &format!("Commit body (optional, Enter to skip):\n> {}\u{2588}", value), GREEN),
        Overlay::AmendMsg { value } => modal(f, 70, 3, " Amend last commit ",
            &format!("New message:\n> {}\u{2588}", value), YELLOW),
        Overlay::RevertCommit { value } => modal(f, 60, 3, " Revert commit ",
            &format!("Commit to revert (sha/ref):\n> {}\u{2588}", value), YELLOW),
        Overlay::ResetCommit { value, mode } => modal(f, 65, 3, " Reset ",
            &format!("Reset ({:?}) to (sha/ref):\n> {}\u{2588}", mode, value), YELLOW),
        Overlay::DiffPath { value, mode } => modal(f, 70, 3, " Diff file ",
            &format!("Diff ({:?}) for path:\n> {}\u{2588}", mode, value), CYAN),
        Overlay::CherryPick { value } => modal(f, 70, 3, " Cherry-pick commit ",
            &format!("Commit to cherry-pick (sha/ref):\n> {}\u{2588}", value), VIBRANT_PINK),
        Overlay::Message { text, is_error } => {
            let color = if *is_error { RED } else { GREEN };
            modal(f, 70, 4, " Message ", &format!("{}\n\n[Enter/Esc to dismiss]", text), color)
        }
        Overlay::None => {}
    }
}

fn modal(f: &mut Frame, percent_x: u16, height: u16, title: &str, text: &str, color: Color) {
    let area = centered_rect(percent_x, height, f.area());
    let m = Paragraph::new(text)
        .block(Block::default().title(title).borders(Borders::ALL).border_style(Style::default().fg(color)));
    f.render_widget(ratatui::widgets::Clear, area);
    f.render_widget(m, area);
}

fn border_style(focused: bool) -> Style {
    if focused { Style::default().fg(CYAN) } else { Style::default().fg(GRAY) }
}

fn build_detail(state: &AppState) -> String {
    let mut out = String::new();
    match state.remote_state.selected().and_then(|i| state.remotes.get(i)) {
        Some(r) => {
            let default = state.repo.config.get_default_remote().cloned();
            let default_mark = if default.as_deref() == Some(&r.name) { " [default]" } else { "" };
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
                for b in &selected { out.push_str(&format!("  - {}\n", b)); }
            }
            out.push_str("\nRemote actions:\n");
            out.push_str("  [a] Add   [R] Rename   [x] Remove   [D] Set default\n");
            out.push_str("  [f]/[Enter] Fetch   [p] Push   [l] Pull   [M] Merge\n");
        }
        None => { out.push_str("No remotes configured.\n\nPress [a] to add a remote."); }
    }
    out.push_str("\nBranch actions (focus Branches):\n");
    out.push_str("  [c] Create   [m] Rename   [x] Delete   [Space] toggle\n");
    out.push_str("\nGit features (focus Detail / Files):\n");
    out.push_str("  [g] Git Graph  [b] Blame  [d] Diff  [F] Files  [s] Status\n");
    out.push_str("  [A] Amend  [R] Revert  [Z] Reset  [P] Cherry-pick  [C] Commit\n");
    out.push_str("\nLog:\n");
    let start = state.log.len().saturating_sub(10);
    for line in &state.log[start..] { out.push_str(&format!("  {}\n", line)); }
    out
}

fn build_files(state: &AppState) -> String {
    let mut out = String::new();
    if state.files.is_empty() {
        out.push_str("No uncommitted changes.\n");
    } else {
        out.push_str("Staged | Unstaged | Path\n");
        out.push_str("---------------------------\n");
        for f in &state.files {
            out.push_str(&format!("  {}   |   {}    | {}\n", f.staged, f.unstaged, f.path));
        }
        out.push_str("\n[S] on a file: stage if unstaged, unstage if staged.\n");
        out.push_str("[Enter] on a file: open its diff.\n");
    }
    out
}

fn build_blame(state: &AppState) -> String {
    let mut out = String::new();
    if state.blame.is_empty() {
        out.push_str(&format!("No blame data for '{}'.\n", state.blame_path));
    } else {
        out.push_str(&format!("Blame: {}\n", state.blame_path));
        out.push_str("────────────────────────────────────────────────────────\n");
        for b in &state.blame {
            out.push_str(&format!(
                "{:>5}  {}  {:<18} {:.8}  {}\n",
                b.line, b.author, b.date, b.commit, b.summary
            ));
        }
    }
    out
}

fn graph_lines(state: &AppState) -> Vec<(String, bool)> {
    let mut lines = Vec::new();
    let Some(graph) = &state.graph else {
        lines.push(("No graph loaded. Press [g].".to_string(), false));
        return lines;
    };
    let sel = state.graph_state.selected().unwrap_or(0);
    for (i, n) in graph.nodes.iter().enumerate() {
        let refs: String = n
            .refs
            .iter()
            .map(|r| {
                let c = match r.kind {
                    crate::git::RefKind::Local => GREEN,
                    crate::git::RefKind::Remote => BLUE,
                    crate::git::RefKind::Tag => YELLOW,
                    crate::git::RefKind::Other => GRAY,
                };
                let _ = c;
                format!(" {}", r.name)
            })
            .collect();
        let line = format!(
            "* {:.8} {} {}{}",
            n.id,
            n.author,
            n.message.lines().next().unwrap_or(""),
            refs
        );
        lines.push((line, i == sel));
    }
    if !graph.detached_refs.is_empty() {
        lines.push(("── detached refs ──".to_string(), false));
        for r in &graph.detached_refs {
            lines.push((format!("  {} ({:?})", r.name, r.kind), false));
        }
    }
    lines
}

fn build_graph(state: &AppState) -> String {
    // Fallback text view (the list widget also renders selection).
    graph_lines(state)
        .into_iter()
        .map(|(s, _)| s)
        .collect::<Vec<_>>()
        .join("\n")
}

fn build_commit(state: &AppState) -> String {
    let mut out = String::new();
    if let Ok(commits) = state.repo.list_recent_commits(30) {
        out.push_str("Recent commits:\n\n");
        for c in commits { out.push_str(&format!("  {}\n", c)); }
    } else {
        out.push_str("Unable to load commits\n");
    }
    out
}

fn centered_rect(percent_x: u16, height: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Length(height), Constraint::Percentage(50)])
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
                if handle_overlay(state, key) {
                    return Ok(false);
                }

                // Global shortcuts (work regardless of focus).
                if (key.modifiers.contains(KeyModifiers::SHIFT) && key.code == KeyCode::Char('m')) || key.code == KeyCode::Char('M') {
                    state.overlay = Overlay::Merge { step: 0, src_remote: String::new(), src_branch: String::new(), dest_remote: String::new(), dest_branch: String::new() };
                    return Ok(false);
                }
                if (key.modifiers.contains(KeyModifiers::SHIFT) && key.code == KeyCode::Char('c')) || key.code == KeyCode::Char('C') {
                    state.overlay = Overlay::CommitType { value: String::new() };
                    return Ok(false);
                }

                match key.code {
                    KeyCode::Char('q') => return Ok(true),
                    KeyCode::Tab | KeyCode::Right | KeyCode::Left => cycle_focus(state),
                    KeyCode::Down => move_down(state),
                    KeyCode::Up => move_up(state),
                    KeyCode::Char(' ') => {
                        if state.focus == Focus::Branches {
                            if let Some(i) = state.branch_state.selected() {
                                if let Some((_, sel)) = state.branches.get_mut(i) { *sel = !*sel; }
                            }
                        }
                    }
                    KeyCode::Char('r') => state.refresh(),
                    KeyCode::Char('g') => state.load_graph(),
                    KeyCode::Char('s') => state.detail_mode = DetailMode::Status,
                    KeyCode::Char('F') => { state.detail_mode = DetailMode::Files; state.refresh(); }
                    KeyCode::Char('d') => state.detail_mode = DetailMode::DiffUnstaged,
                    KeyCode::Char('v') => state.detail_mode = DetailMode::Commit,
                    KeyCode::Char('b') => {
                        if let Some(p) = state.selected_file_path() {
                            state.load_blame(&p);
                        } else {
                            state.log("Select a file in the Files panel first ([F]).".to_string());
                        }
                    }
                    KeyCode::Char('A') => { state.overlay = Overlay::AmendMsg { value: String::new() }; return Ok(false); }
                    KeyCode::Char('R') => { state.overlay = Overlay::RevertCommit { value: String::new() }; return Ok(false); }
                    KeyCode::Char('Z') => { state.overlay = Overlay::ResetCommit { value: String::new(), mode: ResetMode::Mixed }; return Ok(false); }
                    KeyCode::Char('P') => { state.overlay = Overlay::CherryPick { value: String::new() }; return Ok(false); }
                    KeyCode::Char('S') => {
                        if let Some(p) = state.selected_file_path() {
                            let f = state.files.iter().find(|f| f.path == p);
                            let staged = f.map(|f| f.staged != ' ').unwrap_or(false);
                            if staged { state.do_unstage(&p); } else { state.do_stage(&p); }
                        }
                    }
                    KeyCode::Enter => {
                        match state.focus {
                            Focus::Remotes => state.action_fetch(),
                            Focus::Files => {
                                if let Some(p) = state.selected_file_path() {
                                    state.overlay = Overlay::DiffPath { value: p, mode: DiffMode::Unstaged };
                                }
                            }
                            Focus::Graph => {
                                if let Some(graph) = &state.graph {
                                    if let Some(idx) = state.graph_state.selected() {
                                        if let Some(n) = graph.nodes.get(idx) {
                                            state.overlay = Overlay::CherryPick { value: n.short_id.clone() };
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }

                match state.focus {
                    Focus::Remotes => match key.code {
                        KeyCode::Char('a') => { state.overlay = Overlay::AddName { value: String::new() }; }
                        KeyCode::Char('R') => {
                            if let Some(name) = state.selected_remote_name() { state.overlay = Overlay::RenameRemote { old: name, value: String::new() }; }
                        }
                        KeyCode::Char('x') | KeyCode::Delete => {
                            if let Some(name) = state.selected_remote_name() { state.overlay = Overlay::RemoveRemote { name }; }
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
                        KeyCode::Char('f') => state.action_fetch(),
                        KeyCode::Char('p') => state.action_push(),
                        KeyCode::Char('l') => state.action_pull(),
                        _ => {}
                    },
                    Focus::Branches => match key.code {
                        KeyCode::Char('c') => { state.overlay = Overlay::CreateBranch { step: 0, name: String::new(), base: String::new(), remote: String::new() }; }
                        KeyCode::Char('m') => {
                            if let Some(name) = state.selected_branch_name() { state.overlay = Overlay::RenameBranch { old: name, value: String::new() }; }
                        }
                        KeyCode::Char('x') | KeyCode::Delete => {
                            if let Some(name) = state.selected_branch_name() { state.overlay = Overlay::DeleteBranch { name }; }
                        }
                        KeyCode::Char('f') => state.action_fetch(),
                        KeyCode::Char('p') => state.action_push(),
                        KeyCode::Char('l') => state.action_pull(),
                        _ => {}
                    },
                    Focus::Files => match key.code {
                        KeyCode::Char('f') => state.action_fetch(),
                        KeyCode::Char('p') => state.action_push(),
                        KeyCode::Char('l') => state.action_pull(),
                        _ => {}
                    },
                    Focus::Detail => match key.code {
                        KeyCode::Char('f') => state.action_fetch(),
                        KeyCode::Char('p') => state.action_push(),
                        KeyCode::Char('l') => state.action_pull(),
                        _ => {}
                    },
                    Focus::Graph => match key.code {
                        KeyCode::Char('a') => { state.graph_all = !state.graph_all; state.load_graph(); }
                        _ => {}
                    },
                }
            }
        }
    }
    Ok(false)
}

/// Returns true if an overlay consumed the event.
fn handle_overlay(state: &mut AppState, key: crossterm::event::KeyEvent) -> bool {
    let code = key.code;
    match &mut state.overlay {
        Overlay::None => return false,
        Overlay::AddName { value } => {
            match code {
                KeyCode::Enter => {
                    let name = value.trim().to_string();
                    if !name.is_empty() { state.overlay = Overlay::AddUrl { name, value: String::new() }; }
                }
                KeyCode::Char(c) => value.push(c),
                KeyCode::Backspace => { value.pop(); }
                KeyCode::Esc => state.overlay = Overlay::None,
                _ => {}
            }
        }
        Overlay::AddUrl { name, value } => {
            match code {
                KeyCode::Enter => {
                    let url = value.trim().to_string();
                    if !url.is_empty() {
                        let nm = name.clone();
                        match state.repo.add_remote(&nm, &url) {
                            Ok(()) => {
                                state.refresh();
                                state.select_remote_by_name(&nm);
                                state.log(format!("Added remote '{}'", nm));
                                state.overlay = Overlay::Message { text: format!("Added remote '{}'", nm), is_error: false };
                            }
                            Err(e) => { state.overlay = Overlay::Message { text: format!("Error: {}", e), is_error: true }; }
                        }
                    }
                }
                KeyCode::Char(c) => value.push(c),
                KeyCode::Backspace => { value.pop(); }
                KeyCode::Esc => state.overlay = Overlay::None,
                _ => {}
            }
        }
        Overlay::RenameRemote { old, value } => {
            match code {
                KeyCode::Enter => {
                    let new = value.trim().to_string();
                    if !new.is_empty() {
                        let o = old.clone();
                        match state.repo.rename_remote(&o, &new) {
                            Ok(()) => {
                                state.refresh();
                                state.select_remote_by_name(&new);
                                state.log(format!("Renamed remote '{}' -> '{}'", o, new));
                                state.overlay = Overlay::Message { text: format!("Renamed remote '{}' -> '{}'", o, new), is_error: false };
                            }
                            Err(e) => { state.overlay = Overlay::Message { text: format!("Error: {}", e), is_error: true }; }
                        }
                    }
                }
                KeyCode::Char(c) => value.push(c),
                KeyCode::Backspace => { value.pop(); }
                KeyCode::Esc => state.overlay = Overlay::None,
                _ => {}
            }
        }
        Overlay::RemoveRemote { name } => {
            if matches!(code, KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter) {
                let nm = name.clone();
                match state.repo.remove_remote(&nm) {
                    Ok(()) => {
                        state.refresh();
                        state.log(format!("Removed remote '{}'", nm));
                        state.overlay = Overlay::Message { text: format!("Removed remote '{}'", nm), is_error: false };
                    }
                    Err(e) => { state.overlay = Overlay::Message { text: format!("Error: {}", e), is_error: true }; }
                }
            } else if matches!(code, KeyCode::Char('n') | KeyCode::Esc) {
                state.overlay = Overlay::None;
            }
        }
        Overlay::CreateBranch { step, name, base, remote } => {
            match code {
                KeyCode::Enter => match *step {
                    0 => {
                        let n = name.trim().to_string();
                        if !n.is_empty() {
                            *step = 1;
                            base.clear();
                            if let Ok(Some(b)) = state.repo.current_branch() { base.push_str(&b); }
                            else { base.push_str("main"); }
                        }
                    }
                    1 => { *step = 2; remote.clear(); }
                    2 => {
                        let nm = name.trim().to_string();
                        let base_spec = if base.trim().is_empty() {
                            state.repo.current_branch().ok().flatten().unwrap_or_else(|| "main".to_string())
                        } else { base.trim().to_string() };
                        let rm = remote.trim().to_string();
                        if !nm.is_empty() {
                            let res = state.repo.resolve_commit_spec(&base_spec)
                                .and_then(|oid| Ok(state.repo.repo.find_commit(oid)?))
                                .and_then(|commit| { state.repo.repo.branch(&nm, &commit, false)?; Ok(()) })
                                .and_then(|_| if rm.is_empty() { Ok(()) } else { state.repo.push_to_remote(&rm, Some(&nm)) });
                            match res {
                                Ok(()) => {
                                    state.refresh();
                                    state.select_branch_by_name(&nm);
                                    state.log(format!("Created branch '{}'", nm));
                                    state.overlay = Overlay::Message { text: format!("Created branch '{}'", nm), is_error: false };
                                }
                                Err(e) => { state.overlay = Overlay::Message { text: format!("Error: {}", e), is_error: true }; }
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
                    0 => { name.pop(); }
                    1 => { base.pop(); }
                    2 => { remote.pop(); }
                    _ => {}
                },
                KeyCode::Esc => state.overlay = Overlay::None,
                _ => {}
            }
        }
        Overlay::DeleteBranch { name } => {
            if matches!(code, KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter) {
                let nm = name.clone();
                match state.repo.delete_local_branch(&nm, false) {
                    Ok(()) => {
                        state.refresh();
                        state.log(format!("Deleted branch '{}'", nm));
                        state.overlay = Overlay::Message { text: format!("Deleted branch '{}'", nm), is_error: false };
                    }
                    Err(e) => { state.overlay = Overlay::Message { text: format!("Error: {}", e), is_error: true }; }
                }
            } else if matches!(code, KeyCode::Char('n') | KeyCode::Esc) {
                state.overlay = Overlay::None;
            }
        }
        Overlay::RenameBranch { old, value } => {
            match code {
                KeyCode::Enter => {
                    let new = value.trim().to_string();
                    if !new.is_empty() {
                        let o = old.clone();
                        match state.repo.rename_branch(&o, &new) {
                            Ok(()) => {
                                state.refresh();
                                state.select_branch_by_name(&new);
                                state.log(format!("Renamed branch '{}' -> '{}'", o, new));
                                state.overlay = Overlay::Message { text: format!("Renamed branch '{}' -> '{}'", o, new), is_error: false };
                            }
                            Err(e) => { state.overlay = Overlay::Message { text: format!("Error: {}", e), is_error: true }; }
                        }
                    }
                }
                KeyCode::Char(c) => value.push(c),
                KeyCode::Backspace => { value.pop(); }
                KeyCode::Esc => state.overlay = Overlay::None,
                _ => {}
            }
        }
        Overlay::Merge { step, src_remote, src_branch, dest_remote, dest_branch } => {
            match code {
                KeyCode::Enter => match *step {
                    0 => {
                        if !src_remote.trim().is_empty() {
                            *step = 1;
                            src_branch.clear();
                            if let Ok(Some(b)) = state.repo.current_branch() { src_branch.push_str(&b); }
                        }
                    }
                    1 => {
                        if !src_branch.trim().is_empty() {
                            *step = 2;
                            dest_remote.clear();
                        }
                    }
                    2 => {
                        if !dest_remote.trim().is_empty() {
                            *step = 3;
                            dest_branch.clear();
                            if let Ok(Some(b)) = state.repo.current_branch() { dest_branch.push_str(&b); }
                        }
                    }
                    3 => {
                        if !dest_branch.trim().is_empty() {
                            let sr = src_remote.clone();
                            let sb = src_branch.clone();
                            let dr = dest_remote.clone();
                            let db = dest_branch.clone();
                            state.action_merge_explicit(sr, sb, dr, db);
                            state.overlay = Overlay::Message { text: "Merge complete (see log)".to_string(), is_error: false };
                        }
                    }
                    _ => {}
                },
                KeyCode::Char(c) => match *step {
                    0 => src_remote.push(c),
                    1 => src_branch.push(c),
                    2 => dest_remote.push(c),
                    3 => dest_branch.push(c),
                    _ => {}
                },
                KeyCode::Backspace => match *step {
                    0 => { src_remote.pop(); }
                    1 => { src_branch.pop(); }
                    2 => { dest_remote.pop(); }
                    3 => { dest_branch.pop(); }
                    _ => {}
                },
                KeyCode::Esc => state.overlay = Overlay::None,
                _ => {}
            }
        }
        Overlay::CommitType { value } => {
            match code {
                KeyCode::Char('f') => { *value = "feat:".to_string(); }
                KeyCode::Char('x') => { *value = "fix:".to_string(); }
                KeyCode::Char('d') => { *value = "docs:".to_string(); }
                KeyCode::Char('s') => { *value = "style:".to_string(); }
                KeyCode::Char('r') => { *value = "refactor:".to_string(); }
                KeyCode::Char('T') => { *value = "test:".to_string(); }
                KeyCode::Char('c') => { *value = "chore:".to_string(); }
                KeyCode::Char('b') => { *value = "build:".to_string(); }
                KeyCode::Char('p') => { *value = "perf:".to_string(); }
                KeyCode::Enter => {
                    let msg = value.trim();
                    if !msg.is_empty() && !msg.ends_with(':') {
                        state.overlay = Overlay::CommitMsg { value: msg.to_string() };
                    }
                }
                KeyCode::Char(c) => { value.push(c); }
                KeyCode::Backspace => { value.pop(); }
                KeyCode::Esc => state.overlay = Overlay::None,
                _ => {}
            }
        }
        Overlay::CommitMsg { value } => {
            match code {
                KeyCode::Enter => {
                    state.commit_msg = value.trim().to_string();
                    state.overlay = Overlay::CommitBody { value: String::new() };
                }
                KeyCode::Char(c) => { value.push(c); }
                KeyCode::Backspace => { value.pop(); }
                KeyCode::Esc => state.overlay = Overlay::None,
                _ => {}
            }
        }
        Overlay::CommitBody { value } => {
            match code {
                KeyCode::Enter => {
                    let msg = state.commit_msg.clone();
                    let body = if value.trim().is_empty() { None } else { Some(value.trim().to_string()) };
                    state.action_commit(msg, body.as_deref());
                    state.overlay = Overlay::Message { text: "Commit created (see log)".to_string(), is_error: false };
                }
                KeyCode::Char(c) => { value.push(c); }
                KeyCode::Backspace => { value.pop(); }
                KeyCode::Esc => state.overlay = Overlay::None,
                _ => {}
            }
        }
        Overlay::AmendMsg { value } => {
            match code {
                KeyCode::Enter => {
                    let msg = value.trim().to_string();
                    if !msg.is_empty() {
                        state.do_amend(msg);
                        state.overlay = Overlay::Message { text: "Amended (see log)".to_string(), is_error: false };
                    }
                }
                KeyCode::Char(c) => { value.push(c); }
                KeyCode::Backspace => { value.pop(); }
                KeyCode::Esc => state.overlay = Overlay::None,
                _ => {}
            }
        }
        Overlay::RevertCommit { value } => {
            match code {
                KeyCode::Enter => {
                    let spec = value.trim().to_string();
                    if !spec.is_empty() {
                        state.do_revert(spec);
                        state.overlay = Overlay::Message { text: "Reverted (see log)".to_string(), is_error: false };
                    }
                }
                KeyCode::Char(c) => { value.push(c); }
                KeyCode::Backspace => { value.pop(); }
                KeyCode::Esc => state.overlay = Overlay::None,
                _ => {}
            }
        }
        Overlay::ResetCommit { value, mode } => {
            match code {
                KeyCode::Char('s') => *mode = ResetMode::Soft,
                KeyCode::Char('m') => *mode = ResetMode::Mixed,
                KeyCode::Char('h') => *mode = ResetMode::Hard,
                KeyCode::Enter => {
                    let spec = value.trim().to_string();
                    if !spec.is_empty() {
                        let m = *mode;
                        state.do_reset(m, spec);
                        state.overlay = Overlay::Message { text: "Reset complete (see log)".to_string(), is_error: false };
                    }
                }
                KeyCode::Char(c) => value.push(c),
                KeyCode::Backspace => { value.pop(); }
                KeyCode::Esc => state.overlay = Overlay::None,
                _ => {}
            }
        }
        Overlay::DiffPath { value, mode } => {
            match code {
                KeyCode::Enter => {
                    let path = value.trim().to_string();
                    if !path.is_empty() {
                        match state.repo.diff(*mode, Some(&path)) {
                            Ok(d) => {
                                state.detail_mode = DetailMode::Detail;
                                state.log(format!("Diff for {}:\n{}", path, d));
                                state.overlay = Overlay::Message { text: format!("Diff shown for {}", path), is_error: false };
                            }
                            Err(e) => { state.overlay = Overlay::Message { text: format!("Error: {}", e), is_error: true }; }
                        }
                    }
                }
                KeyCode::Char(c) => value.push(c),
                KeyCode::Backspace => { value.pop(); }
                KeyCode::Esc => state.overlay = Overlay::None,
                _ => {}
            }
        }
        Overlay::CherryPick { value } => {
            match code {
                KeyCode::Enter => {
                    let spec = value.trim().to_string();
                    if !spec.is_empty() {
                        state.do_cherry_pick(spec);
                        state.overlay = Overlay::Message { text: "Cherry-picked (see log)".to_string(), is_error: false };
                    }
                }
                KeyCode::Char(c) => { value.push(c); }
                KeyCode::Backspace => { value.pop(); }
                KeyCode::Esc => state.overlay = Overlay::None,
                _ => {}
            }
        }
        Overlay::Message { .. } => {
            if code == KeyCode::Enter || code == KeyCode::Esc { state.overlay = Overlay::None; }
        }
    }
    true
}

fn cycle_focus(state: &mut AppState) {
    state.focus = match state.focus {
        Focus::Remotes => Focus::Branches,
        Focus::Branches => Focus::Files,
        Focus::Files => Focus::Detail,
        Focus::Detail => Focus::Graph,
        Focus::Graph => Focus::Remotes,
    };
    state.refresh();
}

fn move_down(state: &mut AppState) {
    match state.focus {
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
        Focus::Files => {
            if !state.files.is_empty() {
                let i = state.file_state.selected().map(|i| (i + 1) % state.files.len());
                state.file_state.select(i);
            }
        }
        Focus::Graph => {
            if let Some(graph) = &state.graph {
                let n = graph.nodes.len();
                if n > 0 {
                    let i = state.graph_state.selected().map(|i| (i + 1) % n).unwrap_or(0);
                    state.graph_state.select(Some(i));
                }
            }
        }
        Focus::Detail => {}
    }
}

fn move_up(state: &mut AppState) {
    match state.focus {
        Focus::Remotes => {
            if !state.remotes.is_empty() {
                let i = state.remote_state.selected().map(|i| if i == 0 { state.remotes.len() - 1 } else { i - 1 });
                state.remote_state.select(i);
            }
        }
        Focus::Branches => {
            if !state.branches.is_empty() {
                let i = state.branch_state.selected().map(|i| if i == 0 { state.branches.len() - 1 } else { i - 1 });
                state.branch_state.select(i);
            }
        }
        Focus::Files => {
            if !state.files.is_empty() {
                let i = state.file_state.selected().map(|i| if i == 0 { state.files.len() - 1 } else { i - 1 });
                state.file_state.select(i);
            }
        }
        Focus::Graph => {
            if let Some(graph) = &state.graph {
                let n = graph.nodes.len();
                if n > 0 {
                    let i = state.graph_state.selected().map(|i| if i == 0 { n - 1 } else { i - 1 }).unwrap_or(0);
                    state.graph_state.select(Some(i));
                }
            }
        }
        Focus::Detail => {}
    }
}
