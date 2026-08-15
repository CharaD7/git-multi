use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};
use std::collections::HashMap;
use std::io;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::git::{BlameLine, DiffMode, FileStatus, ResetMode};

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
    CherryPick { value: String, context: String },
    DiffPath { value: String, mode: DiffMode },
    SearchCommit { value: String },
    SearchBranch { value: String },
    Message { text: String, is_error: bool }
}

/// Which detail panel mode is active.
#[derive(Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
enum DetailMode {
    Detail,
    Status,
    Files,
    DiffStaged,
    DiffUnstaged,
    Blame,
    Graph,
    Commit,
    CommitDiff,
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
            DetailMode::CommitDiff => " Commit Diff ",
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

/// A blocking git operation handed off to the background worker thread so the
/// UI never freezes while fetch/push/pull/merge/commit run.
enum UiJob {
    Fetch { remote: String, branches: Vec<String> },
    Push { remote: String, branches: Vec<String> },
    Pull { remote: String, branches: Vec<String> },
    Merge { src_remote: String, src_branch: String, dest_remote: String, dest_branch: String },
    Commit { subject: String, body: Option<String> },
    Stage { path: String },
    Unstage { path: String },
    Amend { msg: String },
    Revert { spec: String },
    Reset { mode: ResetMode, spec: String },
    CherryPick { spec: String },
    AddRemote { name: String, url: String },
    RenameRemote { old: String, new: String },
    RemoveRemote { name: String },
    DeleteBranch { name: String },
    RenameBranch { old: String, new: String },
    CreateBranch { name: String, base: String, remote: String },
    SetDefault { name: String },
    Autosave,
}

/// Outcome of a background job, applied on the UI thread.
struct JobResult {
    message: String,
    refresh: bool,
}

/// One line of the ASCII git graph. Edge-only lines carry an empty `sha`.
#[derive(Clone)]
struct GraphLine {
    sha: String,
    text: String,
    is_commit: bool,
}

struct AppState {
    repo: crate::git::GitRepo,
    remotes: Vec<RemoteEntry>,
    remote_state: ListState,
    branches: Vec<(String, bool)>,
    branch_state: ListState,
    filtered_branches: Vec<String>,
    files: Vec<FileStatus>,
    file_state: ListState,
    focus: Focus,
    overlay: Overlay,
    log: Vec<String>,
    detail_mode: DetailMode,
    commit_msg: String,
    commit_diff_spec: Option<String>,
    files_show_commits: bool,
    commit_items: Vec<String>,
    filtered_commit_items: Vec<String>,
    commit_detail_scroll: u16,
    // Cached heavier views (refreshed on demand)
    blame: Vec<BlameLine>,
    blame_path: String,
    graph_lines: Vec<GraphLine>,
    graph_all: bool,
    graph_state: ListState,
    last_activity: Instant,
    autosave_ref_exists: bool,
    // Background worker plumbing
    job_tx: mpsc::Sender<UiJob>,
    result_rx: mpsc::Receiver<JobResult>,
    busy: bool,
}

impl AppState {
    fn new() -> io::Result<Self> {
        let repo = crate::git::GitRepo::open().map_err(|e| io::Error::other(e.to_string()))?;
        let autosave_ref_exists = repo.autosave_ref_exists() || repo.ensure_autosave_ref().is_ok();

        // Spawn the background worker thread. It opens its own GitRepo per job
        // (Repository is Send but the UI keeps its own instance for rendering).
        let (job_tx, job_rx) = mpsc::channel::<UiJob>();
        let (result_tx, result_rx) = mpsc::channel::<JobResult>();
        std::thread::spawn(move || tui_worker(job_rx, result_tx));

        let mut state = Self {
            repo,
            remotes: Vec::new(),
            remote_state: ListState::default(),
            branches: Vec::new(),
            branch_state: ListState::default(),
            filtered_branches: Vec::new(),
            files: Vec::new(),
            file_state: ListState::default(),
            focus: Focus::Remotes,
            overlay: Overlay::None,
            log: Vec::new(),
            detail_mode: DetailMode::Detail,
            commit_msg: String::new(),
            commit_diff_spec: None,
            files_show_commits: false,
            commit_items: Vec::new(),
            filtered_commit_items: Vec::new(),
            commit_detail_scroll: 0,
            blame: Vec::new(),
            blame_path: String::new(),
            graph_lines: Vec::new(),
            graph_all: false,
            graph_state: ListState::default(),
            last_activity: Instant::now(),
            autosave_ref_exists,
            job_tx,
            result_rx,
            busy: false,
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
        self.filtered_branches = self.branches.iter()
            .map(|(b, _)| b.clone())
            .collect();

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

    fn log(&mut self, line: String) {
        self.log.push(line);
        if self.log.len() > 200 {
            self.log.remove(0);
        }
    }

    /// Send a job to the background worker, if one is not already running.
    fn submit_job(&mut self, job: UiJob, silent_when_busy: bool) {
        if self.busy {
            if !silent_when_busy {
                self.log("An operation is already running".to_string());
            }
            return;
        }
        self.busy = true;
        if let Err(e) = self.job_tx.send(job) {
            self.busy = false;
            self.log(format!("Failed to start background task: {}", e));
        }
    }

    /// Drain finished background jobs onto the UI thread.
    fn pump_jobs(&mut self) {
        while let Ok(result) = self.result_rx.try_recv() {
            self.busy = false;
            self.last_activity = Instant::now();
            if result.refresh {
                self.refresh();
            }
            if !result.message.is_empty() {
                self.log(result.message);
            }
        }
    }

    fn action_fetch(&mut self) {
        if let Some(name) = self.selected_remote_name() {
            let selected = self.selected_branches();
            let branches = if selected.is_empty() {
                self.log(format!("Fetching all branches from '{}'", name));
                Vec::new()
            } else {
                self.log(format!("Fetching {:?} from '{}'", selected, name));
                selected
            };
            self.submit_job(UiJob::Fetch { remote: name, branches }, false);
        } else {
            self.log("No remote selected".to_string());
        }
    }

    fn action_push(&mut self) {
        if let Some(name) = self.selected_remote_name() {
            let selected = self.selected_branches();
            if selected.is_empty() {
                self.log(format!("Pushing current branch to '{}'", name));
            } else {
                self.log(format!("Pushing {:?} to '{}'", selected, name));
            }
            self.submit_job(UiJob::Push { remote: name, branches: selected }, false);
        } else {
            self.log("No remote selected".to_string());
        }
    }

    fn action_pull(&mut self) {
        if let Some(name) = self.selected_remote_name() {
            let selected = self.selected_branches();
            if selected.is_empty() {
                self.log(format!("Pulling current branch from '{}'", name));
            } else {
                self.log(format!("Pulling {:?} from '{}'", selected, name));
            }
            self.submit_job(UiJob::Pull { remote: name, branches: selected }, false);
        } else {
            self.log("No remote selected".to_string());
        }
    }

    fn action_merge_explicit(&mut self, src_remote: String, src_branch: String, dest_remote: String, dest_branch: String) {
        self.log(format!("Merging {}/{} into {}/{} ...", src_remote, src_branch, dest_remote, dest_branch));
        self.submit_job(UiJob::Merge { src_remote, src_branch, dest_remote, dest_branch }, false);
    }

    fn action_commit(&mut self, subject: String, body: Option<String>) {
        self.log(format!("Creating commit: {}", subject));
        self.submit_job(UiJob::Commit { subject, body }, false);
    }

    fn do_stage(&mut self, path: &str) {
        self.log(format!("Staging {}", path));
        self.submit_job(UiJob::Stage { path: path.to_string() }, false);
    }

    fn do_unstage(&mut self, path: &str) {
        self.log(format!("Unstaging {}", path));
        self.submit_job(UiJob::Unstage { path: path.to_string() }, false);
    }

    fn do_amend(&mut self, msg: String) {
        self.log("Amending last commit ...".to_string());
        self.submit_job(UiJob::Amend { msg }, false);
    }

    fn do_revert(&mut self, spec: String) {
        self.log(format!("Reverting {} ...", spec));
        self.submit_job(UiJob::Revert { spec }, false);
    }

    fn do_reset(&mut self, mode: ResetMode, spec: String) {
        self.log(format!("Resetting ({:?}) to {} ...", mode, spec));
        self.submit_job(UiJob::Reset { mode, spec }, false);
    }

    fn do_cherry_pick(&mut self, spec: String) {
        self.log(format!("Cherry-picking {} ...", spec));
        self.submit_job(UiJob::CherryPick { spec }, false);
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
        match self.repo.log_graph(self.graph_all, 300) {
            Ok(lines) => {
                self.graph_lines = lines.iter().filter_map(|l| parse_graph_line(l)).collect();
                self.detail_mode = DetailMode::Graph;
                self.graph_state.select(Some(0));
            }
            Err(e) => self.log(format!("Graph failed: {}", e)),
        }
    }
}

/// Background worker: runs blocking git operations so the UI thread never
/// freezes. Each job opens its own `GitRepo` (fresh from disk).
fn tui_worker(job_rx: mpsc::Receiver<UiJob>, result_tx: mpsc::Sender<JobResult>) {
    for job in job_rx {
        let mut repo = match crate::git::GitRepo::open() {
            Ok(r) => r,
            Err(e) => {
                let _ = result_tx.send(JobResult {
                    message: format!("Repo error: {}", e),
                    refresh: false,
                });
                continue;
            }
        };
        let result = handle_job(&mut repo, job);
        let _ = result_tx.send(result);
    }
}

fn handle_job(repo: &mut crate::git::GitRepo, job: UiJob) -> JobResult {
    match job {
        UiJob::Fetch { remote, branches } => {
            let r = if branches.is_empty() {
                repo.fetch_remote(&remote)
            } else {
                repo.fetch_branches(&remote, &branches)
            };
            match r {
                Ok(()) => JobResult { message: format!("Fetched from '{}'", remote), refresh: true },
                Err(e) => JobResult { message: format!("Fetch '{}' failed: {}", remote, e), refresh: false },
            }
        }
        UiJob::Push { remote, branches } => {
            let r = if branches.is_empty() {
                repo.push_to_remote(&remote, None)
            } else {
                repo.push_branches(&remote, &branches, false)
            };
            match r {
                Ok(()) => JobResult { message: format!("Pushed to '{}'", remote), refresh: true },
                Err(e) => JobResult { message: format!("Push '{}' failed: {}", remote, e), refresh: false },
            }
        }
        UiJob::Pull { remote, branches } => {
            let r = if branches.is_empty() {
                repo.pull_from_remote(&remote, None)
            } else {
                repo.pull_branches(&remote, &branches)
            };
            match r {
                Ok(()) => JobResult { message: format!("Pulled from '{}'", remote), refresh: true },
                Err(e) => JobResult { message: format!("Pull '{}' failed: {}", remote, e), refresh: false },
            }
        }
        UiJob::Merge { src_remote, src_branch, dest_remote, dest_branch } => {
            let src_ref = format!("refs/remotes/{}/{}", src_remote, src_branch);
            let r = repo
                .fetch_remote(&src_remote)
                .and_then(|_| repo.fetch_remote(&dest_remote))
                .and_then(|_| {
                    if repo.current_branch()?.as_deref() != Some(&dest_branch) {
                        repo.checkout_branch(&dest_branch)
                    } else {
                        Ok(())
                    }
                })
                .and_then(|_| repo.merge_and_commit(&src_ref))
                .and_then(|_| repo.push_to_remote(&dest_remote, Some(&dest_branch)));
            match r {
                Ok(()) => JobResult {
                    message: format!("Merged {}/{} into {}/{} and pushed", src_remote, src_branch, dest_remote, dest_branch),
                    refresh: true,
                },
                Err(e) => JobResult { message: format!("Merge failed: {}", e), refresh: false },
            }
        }
        UiJob::Commit { subject, body } => match repo.create_commit(&subject, body.as_deref()) {
            Ok(()) => JobResult { message: format!("Created commit: {}", subject), refresh: true },
            Err(e) => JobResult { message: format!("Commit failed: {}", e), refresh: false },
        },
        UiJob::Stage { path } => match repo.stage_file(&path) {
            Ok(()) => JobResult { message: format!("Staged: {}", path), refresh: true },
            Err(e) => JobResult { message: format!("Stage failed: {}", e), refresh: false },
        },
        UiJob::Unstage { path } => match repo.unstage_file(&path) {
            Ok(()) => JobResult { message: format!("Unstaged: {}", path), refresh: true },
            Err(e) => JobResult { message: format!("Unstage failed: {}", e), refresh: false },
        },
        UiJob::Amend { msg } => match repo.amend_commit(&msg, None) {
            Ok(()) => JobResult { message: "Amended last commit".to_string(), refresh: true },
            Err(e) => JobResult { message: format!("Amend failed: {}", e), refresh: false },
        },
        UiJob::Revert { spec } => match repo.revert_commit(&spec) {
            Ok(()) => JobResult { message: format!("Reverted {}", spec), refresh: true },
            Err(e) => JobResult { message: format!("Revert failed: {}", e), refresh: false },
        },
        UiJob::Reset { mode, spec } => match repo.reset(mode, &spec) {
            Ok(()) => JobResult { message: format!("Reset ({:?}) to {}", mode, spec), refresh: true },
            Err(e) => JobResult { message: format!("Reset failed: {}", e), refresh: false },
        },
        UiJob::CherryPick { spec } => match repo.cherry_pick_commit(&spec) {
            Ok(()) => JobResult { message: format!("Cherry-picked {}", spec), refresh: true },
            Err(e) => JobResult { message: format!("Cherry-pick failed: {}", e), refresh: false },
        },
        UiJob::AddRemote { name, url } => match repo.add_remote(&name, &url) {
            Ok(()) => JobResult { message: format!("Added remote '{}'", name), refresh: true },
            Err(e) => JobResult { message: format!("Error: {}", e), refresh: false },
        },
        UiJob::RenameRemote { old, new } => match repo.rename_remote(&old, &new) {
            Ok(()) => JobResult { message: format!("Renamed remote '{}' -> '{}'", old, new), refresh: true },
            Err(e) => JobResult { message: format!("Error: {}", e), refresh: false },
        },
        UiJob::RemoveRemote { name } => match repo.remove_remote(&name) {
            Ok(()) => JobResult { message: format!("Removed remote '{}'", name), refresh: true },
            Err(e) => JobResult { message: format!("Error: {}", e), refresh: false },
        },
        UiJob::DeleteBranch { name } => match repo.delete_local_branch(&name, false) {
            Ok(()) => JobResult { message: format!("Deleted branch '{}'", name), refresh: true },
            Err(e) => JobResult { message: format!("Error: {}", e), refresh: false },
        },
        UiJob::RenameBranch { old, new } => match repo.rename_branch(&old, &new) {
            Ok(()) => JobResult { message: format!("Renamed branch '{}' -> '{}'", old, new), refresh: true },
            Err(e) => JobResult { message: format!("Error: {}", e), refresh: false },
        },
        UiJob::CreateBranch { name, base, remote } => {
            let r = repo
                .resolve_commit_spec(&base)
                .and_then(|oid| Ok(repo.repo.find_commit(oid)?))
                .and_then(|commit| {
                    repo.repo.branch(&name, &commit, false)?;
                    Ok(())
                })
                .and_then(|_| {
                    if remote.is_empty() {
                        Ok(())
                    } else {
                        repo.push_to_remote(&remote, Some(&name))
                    }
                });
            match r {
                Ok(()) => JobResult { message: format!("Created branch '{}'", name), refresh: true },
                Err(e) => JobResult { message: format!("Error: {}", e), refresh: false },
            }
        }
        UiJob::SetDefault { name } => {
            let r = repo
                .config
                .set_default_remote(name.clone())
                .and_then(|_| repo.config.save(&repo.repo));
            match r {
                Ok(()) => JobResult { message: format!("Default remote set to '{}'", name), refresh: true },
                Err(e) => JobResult { message: format!("Error: {}", e), refresh: false },
            }
        }
        UiJob::Autosave => match repo.write_autosave_snapshot() {
            Ok(true) => JobResult { message: "[auto-save] snapshot captured".to_string(), refresh: true },
            Ok(false) => JobResult { message: String::new(), refresh: false },
            Err(_) => JobResult { message: "[auto-save] failed".to_string(), refresh: false },
        },
    }
}

/// Parse one line of `git log --graph --format=%H%x00%h%x00%an%x00%aI%x00%s%d`
/// into a displayable graph line. Edge-only lines carry an empty sha.
fn parse_graph_line(line: &str) -> Option<GraphLine> {
    let fields: Vec<&str> = line.split('\0').collect();
    let first = *fields.first()?;

    // The leading graph characters end where the 40-char full sha begins.
    let bytes = first.as_bytes();
    let mut start = None;
    let mut len = 0;
    let mut sha_start = None;
    for (i, &b) in bytes.iter().enumerate() {
        if b.is_ascii_hexdigit() {
            if start.is_none() {
                start = Some(i);
            }
            len += 1;
            if len == 40 {
                sha_start = Some(i - 39);
                break;
            }
        } else {
            start = None;
            len = 0;
        }
    }

    match sha_start {
        Some(s) => {
            let prefix = &first[..s];
            let full_sha = &first[s..s + 40];
            let short = fields.get(1).copied().unwrap_or("");
            let author = fields.get(2).copied().unwrap_or("");
            let date = fields.get(3).copied().unwrap_or("");
            let subject = fields.get(4).copied().unwrap_or("");
            let deco = fields.get(5).copied().unwrap_or("");
            let text = format!(
                "{}{} {} {} {}{}",
                prefix,
                short,
                author,
                crate::git::format_timestamp(crate::git::parse_iso_date(date)),
                subject,
                deco
            );
            Some(GraphLine { sha: full_sha.to_string(), text, is_commit: true })
        }
        None => Some(GraphLine { sha: String::new(), text: line.trim_end().to_string(), is_commit: false }),
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
        state.pump_jobs();
        if handle_events(&mut state)? {
            break;
        }
        if state.autosave_ref_exists
            && state.last_activity.elapsed() >= Duration::from_secs(30)
        {
            state.submit_job(UiJob::Autosave, true);
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

    let base = "[Tab] Focus  [↑/↓] Move  [Space] Toggle  [f] Fetch [p] Push [l] Pull  [M] Merge \
  [C] Commit  [a] Add remote  [c] Branch  [m] Rename  [x] Delete  [D] Default\n\
  [g] Git Graph  [b] Blame file  [d] Diff  [F] Files  [s] Status  [S] Stage/Unstage file  \
  [A] Amend  [R] Revert  [Z] Reset  [P] Cherry-pick (Files only)  [v] Commits  [/] Search";
    let suffix = if state.autosave_ref_exists {
        "[O] Restore auto-save  "
    } else {
        ""
    };
    let busy = if state.busy { "  [working…]" } else { "" };
    let help = format!("{}{}[r] Refresh  [q] Quit{}", base, suffix, busy);
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
    let search_query = if let Overlay::SearchBranch { value } = &state.overlay {
        value
    } else {
        ""
    };
    
    let branch_items: Vec<ListItem> = {
        if search_query.is_empty() && state.filtered_branches.is_empty() {
            state.branches
                .iter()
                .map(|(b, sel)| {
                    let mark = if *sel { "[x]" } else { "[ ]" };
                    ListItem::new(format!("{} {}", mark, b))
                })
                .collect()
        } else if search_query.is_empty() {
            state.filtered_branches.iter()
                .filter_map(|b| state.branches.iter().find(|(name, _)| name == b))
                .map(|(b, sel)| {
                    let mark = if *sel { "[x]" } else { "[ ]" };
                    ListItem::new(format!("{} {}", mark, b))
                })
                .collect()
        } else {
            state.branches.iter()
                .filter(|(b, _)| b.contains(search_query))
                .map(|(b, sel)| {
                    let mark = if *sel { "[x]" } else { "[ ]" };
                    ListItem::new(format!("{} {}", mark, b))
                })
                .collect()
        }
    };
    let title = if state.focus == Focus::Branches { " Branches (focused) " } else { " Branches " };
    let sel_count = state.selected_branches().len();
    let block = Block::default()
        .title(format!("{} [{} selected]", title, sel_count))
        .title_bottom(" [c] Create  [m] Rename  [x] Delete  [Space] Toggle ")
        .borders(Borders::ALL)
        .border_style(border_style(state.focus == Focus::Branches));
    let branch_list = List::new(branch_items)
        .block(block)
        .highlight_style(Style::default().bg(MAUVE).fg(Color::Black))
        .highlight_symbol(">> ");
    f.render_stateful_widget(branch_list, area, &mut state.branch_state.clone());
}

fn render_files(f: &mut Frame, state: &AppState, area: Rect) {
    let (items, title, count) = if state.files_show_commits {
        let search_query = if let Overlay::SearchCommit { value } = &state.overlay {
            value
        } else {
            ""
        };
        
        let commit_list: Vec<String> = if search_query.is_empty() && state.filtered_commit_items.is_empty() {
            state.commit_items.clone()
        } else if !search_query.is_empty() {
            state.commit_items.iter()
                .filter(|c| c.contains(search_query))
                .cloned()
                .collect()
        } else {
            state.filtered_commit_items.clone()
        };
        let items: Vec<ListItem> = commit_list
            .iter()
            .map(|c| ListItem::new(c.clone()))
            .collect();
        (items, " Commits ", commit_list.len())
    } else {
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
        let title = if state.files_show_commits {
            if state.focus == Focus::Files { " Commits (focused) " } else { " Commits " }
        } else if state.focus == Focus::Files { " Files (focused) " } else { " Files " };
        (items, title, state.files.len())
    };
    let block = Block::default()
        .title(format!("{} [{}]", title, count))
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
        DetailMode::Detail => string_to_lines(&build_detail(state)),
        DetailMode::Status => string_to_lines(&state
            .repo
            .status_text()
            .unwrap_or_else(|e| format!("Error: {}", e))),
        DetailMode::Files => string_to_lines(&build_files(state)),
        DetailMode::DiffStaged => {
            let diff = state.repo.diff(DiffMode::Staged, None).unwrap_or_else(|e| format!("Error: {}", e));
            diff_to_lines(diff)
        }
        DetailMode::DiffUnstaged => {
            let diff = state.repo.diff(DiffMode::Unstaged, None).unwrap_or_else(|e| format!("Error: {}", e));
            diff_to_lines(diff)
        }
        DetailMode::Blame => string_to_lines(&build_blame(state)),
        DetailMode::Graph => string_to_lines(&build_graph(state)),
        DetailMode::CommitDiff => {
            let diff = state.repo.commit_diff(state.commit_diff_spec.as_deref().unwrap_or("")).unwrap_or_else(|e| format!("Error: {}", e));
            diff_to_lines(diff)
        }
        DetailMode::Commit => {
            let items = build_commit_details(state);
            let p = Paragraph::new(items)
                .block(block)
                .style(Style::default().fg(CREAM))
                .wrap(Wrap { trim: false })
                .scroll((state.commit_detail_scroll, 0));
            f.render_widget(p, area);
            return;
        }
    };
    let p = Paragraph::new(text)
        .block(block)
        .style(Style::default().fg(CREAM))
        .wrap(Wrap { trim: false })
        .scroll((0, 0));
    f.render_widget(p, area);
    if state.detail_mode == DetailMode::Graph {
        // Re-render graph as a list for selection highlight.
        let items: Vec<ListItem> = state
            .graph_lines
            .iter()
            .map(|gl| {
                if gl.is_commit {
                    ListItem::new(format!("{}  [Enter: pick, D: diff]", gl.text))
                        .style(Style::default().fg(CREAM))
                } else {
                    ListItem::new(gl.text.clone()).style(Style::default().fg(GRAY))
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
        Overlay::AddName { value } => modal(f, 60, 4, " Add Remote ",
            &format!("Remote name:\n> {}\u{2588}", value), RED),
        Overlay::AddUrl { name, value } => modal(f, 70, 4, " Add Remote ",
            &format!("URL for '{}':\n> {}\u{2588}", name, value), RED),
        Overlay::RenameRemote { old, value } => modal(f, 60, 4, " Rename Remote ",
            &format!("Rename '{}' to:\n> {}\u{2588}", old, value), RED),
        Overlay::RemoveRemote { name } => modal(f, 60, 4, " Remove Remote ",
            &format!("Remove remote '{}'?\n\n[y] Yes  [n/Esc] Cancel", name), RED),
        Overlay::CreateBranch { step, name, base, remote } => {
            let prompt = match step {
                0 => format!("Branch name:\n> {}\u{2588}", name),
                1 => format!("Base (commit/branch):\n> {}\u{2588}", base),
                _ => format!("Push to remote (empty = local only):\n> {}\u{2588}", remote),
            };
            modal(f, 65, 4, " Create Branch ", &prompt, RED)
        }
        Overlay::DeleteBranch { name } => modal(f, 60, 4, " Delete Branch ",
            &format!("Delete local branch '{}'?\n\n[y] Yes  [n/Esc] Cancel", name), RED),
        Overlay::RenameBranch { old, value } => modal(f, 60, 4, " Rename Branch ",
            &format!("Rename '{}' to:\n> {}\u{2588}", old, value), RED),
        Overlay::Merge { step, src_remote, src_branch, dest_remote, dest_branch } => {
            let prompt = match step {
                0 => format!("Source remote:\n> {}\u{2588}", src_remote),
                1 => format!("Source branch (from {}):\n> {}\u{2588}", src_remote, src_branch),
                2 => format!("Destination remote:\n> {}\u{2588}", dest_remote),
                _ => format!("Destination branch:\n> {}\u{2588}", dest_branch),
            };
            modal(f, 65, 4, " Merge ", &prompt, VIBRANT_PINK)
        }
        Overlay::CommitType { value } => modal(f, 60, 7, " Commit Type ",
            &format!("Select commit type:\n\n[f] feat  [x] fix  [d] docs  [s] style  [r] refactor\n[T] test  [c] chore  [b] build  [p] perf\n\nOr type to filter:\n> {}\u{2588}", value), GREEN),
        Overlay::CommitMsg { value } => modal(f, 70, 4, " Commit Message ",
            &format!("Commit subject:\n> {}\u{2588}", value), GREEN),
        Overlay::CommitBody { value } => modal(f, 70, 6, " Commit Body ",
            &format!("Commit body (optional, Enter to skip):\n> {}\u{2588}", value), GREEN),
        Overlay::AmendMsg { value } => modal(f, 70, 4, " Amend last commit ",
            &format!("New message:\n> {}\u{2588}", value), YELLOW),
        Overlay::RevertCommit { value } => modal(f, 60, 4, " Revert commit ",
            &format!("Commit to revert (sha/ref):\n> {}\u{2588}", value), YELLOW),
        Overlay::ResetCommit { value, mode } => modal(f, 70, 5, " Reset ",
            &format!("Mode: [1] soft  [2] mixed  [3] hard   (current: {:?})\nTarget (sha/ref):\n> {}\u{2588}", mode, value), YELLOW),
        Overlay::DiffPath { value, mode } => modal(f, 70, 4, " Diff file ",
            &format!("Diff ({:?}) for path:\n> {}\u{2588}", mode, value), CYAN),
        Overlay::CherryPick { value, context } => {
            let ctx_line = if context.is_empty() {
                String::new()
            } else {
                format!("\n{}", context)
            };
            modal(f, 85, 6, " Cherry-pick commit ",
                &format!("Commit to cherry-pick (sha/ref):\n> {}\u{2588}{}\n\n[space] cherry-pick  [d] preview diff  [Enter] accept  [Esc] cancel", value, ctx_line), VIBRANT_PINK)
        }
Overlay::Message { text, is_error } => {
             let color = if *is_error { RED } else { GREEN };
             modal(f, 70, 4, " Message ", &format!("{}\n\n[Enter/Esc to dismiss]", text), color)
         }
         Overlay::SearchCommit { value } => {
             let prompt = if value.is_empty() {
                 "Search commits by SHA or message:\n> \u{2588}".to_string()
             } else {
                 format!("Search commits by SHA or message:\n> {}\u{2588}", value)
             };
             modal(f, 70, 5, " Search Commits ", &prompt, CYAN)
         }
         Overlay::SearchBranch { value } => {
             let prompt = if value.is_empty() {
                 "Search branches by name:\n> \u{2588}".to_string()
             } else {
                 format!("Search branches by name:\n> {}\u{2588}", value)
             };
             modal(f, 70, 5, " Search Branches ", &prompt, CYAN)
         }
         Overlay::None => {}
    }
}

fn modal(f: &mut Frame, percent_x: u16, height: u16, title: &str, text: &str, color: Color) {
    let area = centered_rect(percent_x, height, f.area());
    let m = Paragraph::new(text)
        .block(Block::default().title(title).borders(Borders::ALL).border_style(Style::default().fg(color)))
        .style(Style::default().fg(Color::White));
    f.render_widget(ratatui::widgets::Clear, area);
    f.render_widget(m, area);
}

fn border_style(focused: bool) -> Style {
    if focused {
        Style::default().fg(CYAN).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(GRAY)
    }
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
out.push_str("  [A] Amend  [R] Revert  [Z] Reset  [C] Commit\n");
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
        out.push_str("[P] on a file: cherry-pick a commit.\n");
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

fn build_graph(state: &AppState) -> String {
    // Fallback text view (the list widget also renders selection + edges).
    if state.graph_lines.is_empty() {
        return "No graph loaded. Press [g].".to_string();
    }
    state
        .graph_lines
        .iter()
        .map(|gl| gl.text.clone())
        .collect::<Vec<_>>()
        .join("\n")
}

fn string_to_lines(s: &str) -> Vec<Line<'static>> {
    s.lines()
        .map(|l| Line::from(Span::styled(l.to_string(), Style::default().fg(CREAM))))
        .collect()
}

fn diff_to_lines(diff: String) -> Vec<Line<'static>> {
    diff.lines()
        .map(|line| {
            let content = line.to_string();
            let styled = if line.starts_with('+') {
                Line::from(Span::styled(content, Style::default().fg(GREEN)))
            } else if line.starts_with('-') {
                Line::from(Span::styled(content, Style::default().fg(RED)))
            } else if line.starts_with("@@") {
                Line::from(Span::styled(content, Style::default().fg(YELLOW).add_modifier(Modifier::BOLD)))
            } else if line.starts_with("diff --git") || line.starts_with("index ") || line.starts_with("---") || line.starts_with("+++") {
                Line::from(Span::styled(content, Style::default().fg(CYAN)))
            } else if line.starts_with('\\') {
                Line::from(Span::styled(content, Style::default().fg(GRAY)))
            } else {
                Line::from(Span::styled(content, Style::default().fg(CREAM)))
            };
            styled
        })
        .collect()
}

fn build_commit_details(state: &AppState) -> Vec<Line<'static>> {
    let mut out: Vec<Line> = Vec::new();

    let sha = if state.files_show_commits {
        let commit_list = if state.filtered_commit_items.is_empty() {
            &state.commit_items
        } else {
            &state.filtered_commit_items
        };
        state.file_state.selected()
            .and_then(|i| commit_list.get(i))
            .and_then(|line| line.split_whitespace().next())
            .unwrap_or("")
    } else {
        state.commit_diff_spec.as_deref().unwrap_or("")
    };

    if sha.is_empty() {
        out.push(Line::from(Span::styled("No commit selected. Use [v] in Files panel to view commits.", Style::default().fg(CREAM))));
        return out;
    }

    match state.repo.commit_detail(sha) {
        Ok(detail) => {
            out.push(Line::from(Span::styled(format!("Commit: {}", detail.short_id), Style::default().fg(CYAN))));
            out.push(Line::from(Span::styled(format!("SHA:    {}", detail.id), Style::default().fg(GRAY))));
            out.push(Line::from(Span::styled(format!("Author: {} <{}>", detail.author, detail.author_email), Style::default().fg(CREAM))));
            out.push(Line::from(Span::styled(format!("Date:   {}", detail.author_date), Style::default().fg(GRAY))));
            out.push(Line::from(Span::styled(format!("Committer: {} <{}>", detail.committer, detail.committer_date), Style::default().fg(CREAM))));
            out.push(Line::from(Span::styled(format!("Message:\n  {}", detail.message.lines().next().unwrap_or("")), Style::default().fg(CREAM))));
            if !detail.parents.is_empty() {
                out.push(Line::from(Span::styled(format!("Parents: {}", detail.parents.join(", ")), Style::default().fg(GRAY))));
            }
        }
        Err(e) => {
            out.push(Line::from(Span::styled(format!("Error loading commit detail: {}", e), Style::default().fg(RED))));
        }
    }

    out.push(Line::from(Span::styled("\n── Diff ──", Style::default().fg(YELLOW))));
    match state.repo.commit_diff(sha) {
        Ok(diff) => {
            out.extend(diff_to_lines(diff));
        }
        Err(e) => {
            out.push(Line::from(Span::styled(format!("Error loading diff: {}", e), Style::default().fg(RED))));
        }
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
                state.last_activity = Instant::now();
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
                if key.code == KeyCode::Char('O') {
                    if state.autosave_ref_exists {
                        match state.repo.restore_from_autosave() {
                            Ok(()) => {
                                state.refresh();
                                state.log("Restored from auto-save snapshot".to_string());
                            }
                            Err(e) => state.log(format!("Auto-save restore failed: {}", e)),
                        }
                    } else {
                        state.log("No auto-save snapshot available yet".to_string());
                    }
                    return Ok(false);
                }

                match key.code {
                    KeyCode::Char('q') => return Ok(true),
                    KeyCode::Tab => cycle_focus(state),
                    KeyCode::Right => cycle_focus(state),
                    KeyCode::Left => cycle_focus_back(state),
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
                    KeyCode::Char('/') => {
                        match state.focus {
                            Focus::Files if state.files_show_commits => {
                                state.overlay = Overlay::SearchCommit { value: String::new() };
                            }
                            Focus::Branches => {
                                state.overlay = Overlay::SearchBranch { value: String::new() };
                            }
                            _ => {}
                        }
                    }
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
                                if state.files_show_commits {
                                    let commit_list = if state.filtered_commit_items.is_empty() {
                                        &state.commit_items
                                    } else {
                                        &state.filtered_commit_items
                                    };
                                    if let Some(idx) = state.file_state.selected() {
                                        if let Some(line) = commit_list.get(idx) {
                                            let sha = line.split_whitespace().next().map(|s| s.to_string());
                                            state.commit_diff_spec = sha;
                                            state.detail_mode = DetailMode::Commit;
                                            state.focus = Focus::Detail;
                                            state.commit_detail_scroll = 0;
                                        }
                                    }
                                } else if let Some(p) = state.selected_file_path() {
                                    state.overlay = Overlay::DiffPath { value: p, mode: DiffMode::Unstaged };
                                }
                            }
                            Focus::Graph => {
                                if let Some(idx) = state.graph_state.selected() {
                                    if let Some(gl) = state.graph_lines.get(idx) {
                                        if gl.is_commit {
                                            let short = gl.sha[..8.min(gl.sha.len())].to_string();
                                            let ctx = gl.text.clone();
                                            state.overlay = Overlay::CherryPick { value: short, context: ctx };
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    KeyCode::Char('v') => {
                        state.files_show_commits = !state.files_show_commits;
                        if state.files_show_commits {
                            state.commit_items = state.repo.list_recent_commits(30).unwrap_or_default();
                            state.filtered_commit_items = state.commit_items.clone();
                            state.file_state.select(Some(0));
                            state.detail_mode = DetailMode::Commit;
                            state.commit_detail_scroll = 0;
                        } else {
                            state.detail_mode = DetailMode::Detail;
                            state.commit_items.clear();
                            state.filtered_commit_items.clear();
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
                                state.log(format!("Setting default remote to '{}' ...", name));
                                state.submit_job(UiJob::SetDefault { name }, false);
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
                        KeyCode::Char('/') => {
                            state.overlay = Overlay::SearchBranch { value: String::new() };
                        }
                        _ => {}
                    },
                    Focus::Files => match key.code {
                        KeyCode::Char('f') => state.action_fetch(),
                        KeyCode::Char('p') => state.action_push(),
                        KeyCode::Char('l') => state.action_pull(),
                        KeyCode::Char('P') => {
                            if !state.files_show_commits {
                                if let Some(p) = state.selected_file_path() {
                                    let is_dirty = state.files.iter().any(|f| f.path == p && (f.staged != ' ' || f.unstaged != ' '));
                                    // Pre-fill the HEAD sha (not the file path!) so
                                    // Space cherry-picks a real commit onto HEAD.
                                    let head_short = state
                                        .repo
                                        .head_commit()
                                        .ok()
                                        .map(|c| {
                                            let s = c.id().to_string();
                                            s[..8.min(s.len())].to_string()
                                        })
                                        .unwrap_or_default();
                                    let (value, context) = if is_dirty {
                                        (head_short.clone(), format!("File: {} (dirty) — picking {}", p, head_short))
                                    } else {
                                        (String::new(), String::new())
                                    };
                                    state.overlay = Overlay::CherryPick { value, context };
                                }
                            }
                            return Ok(false);
                        }
                        KeyCode::Char('/') => {
                            if state.files_show_commits {
                                state.overlay = Overlay::SearchCommit { value: String::new() };
                            }
                        }
                        _ => {}
                    },
                    Focus::Detail => match key.code {
                        KeyCode::Char('f') => state.action_fetch(),
                        KeyCode::Char('p') => state.action_push(),
                        KeyCode::Char('l') => state.action_pull(),
                        KeyCode::Enter => {
                            if state.detail_mode == DetailMode::Commit {
                                let commit_list = if state.filtered_commit_items.is_empty() {
                                    &state.commit_items
                                } else {
                                    &state.filtered_commit_items
                                };
                                if let Some(idx) = state.file_state.selected() {
                                    if let Some(line) = commit_list.get(idx) {
                                        let sha = line.split_whitespace().next().unwrap_or("").to_string();
                                        if !sha.is_empty() {
                                            state.commit_diff_spec = Some(sha.clone());
                                            state.refresh();
                                        }
                                    }
                                }
                            }
                        }
                        KeyCode::Char('j') | KeyCode::Down => {
                            if state.detail_mode == DetailMode::Commit {
                                state.commit_detail_scroll = state.commit_detail_scroll.saturating_add(1);
                            }
                        }
                        KeyCode::Char('k') | KeyCode::Up => {
                            if state.detail_mode == DetailMode::Commit {
                                state.commit_detail_scroll = state.commit_detail_scroll.saturating_sub(1);
                            }
                        }
                        _ => {}
                    },
                    Focus::Graph => match key.code {
                        KeyCode::Char('a') => { state.graph_all = !state.graph_all; state.load_graph(); }
                        KeyCode::Char('D') => {
                            if let Some(idx) = state.graph_state.selected() {
                                if let Some(gl) = state.graph_lines.get(idx) {
                                    if gl.is_commit {
                                        state.commit_diff_spec = Some(gl.sha.clone());
                                        state.detail_mode = DetailMode::CommitDiff;
                                        state.log(format!("Diff for {} shown in detail panel", &gl.sha[..8.min(gl.sha.len())]));
                                    }
                                }
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
                        state.submit_job(UiJob::AddRemote { name: nm.clone(), url }, false);
                        state.log(format!("Adding remote '{}' ...", nm));
                        state.overlay = Overlay::None;
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
                        state.submit_job(UiJob::RenameRemote { old: o.clone(), new: new.clone() }, false);
                        state.log(format!("Renaming remote '{}' -> '{}' ...", o, new));
                        state.overlay = Overlay::None;
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
                state.submit_job(UiJob::RemoveRemote { name: nm.clone() }, false);
                state.log(format!("Removing remote '{}' ...", nm));
                state.overlay = Overlay::None;
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
                            state.log(format!("Creating branch '{}' ...", nm));
                            state.submit_job(UiJob::CreateBranch { name: nm, base: base_spec, remote: rm }, false);
                            state.overlay = Overlay::None;
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
                state.submit_job(UiJob::DeleteBranch { name: nm.clone() }, false);
                state.log(format!("Deleting branch '{}' ...", nm));
                state.overlay = Overlay::None;
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
                        state.submit_job(UiJob::RenameBranch { old: o.clone(), new: new.clone() }, false);
                        state.log(format!("Renaming branch '{}' -> '{}' ...", o, new));
                        state.overlay = Overlay::None;
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
                            state.overlay = Overlay::Message { text: "Merge started in background (see log)".to_string(), is_error: false };
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
                    if !msg.is_empty() {
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
                    state.action_commit(msg, body);
                    state.overlay = Overlay::Message { text: "Commit started (see log)".to_string(), is_error: false };
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
                // Mode keys don't collide with typing a target like `main` or `HEAD~2`.
                KeyCode::Char('1') => *mode = ResetMode::Soft,
                KeyCode::Char('2') => *mode = ResetMode::Mixed,
                KeyCode::Char('3') => *mode = ResetMode::Hard,
                KeyCode::Enter => {
                    let spec = value.trim().to_string();
                    if !spec.is_empty() {
                        let m = *mode;
                        state.do_reset(m, spec);
                        state.overlay = Overlay::Message { text: "Reset started (see log)".to_string(), is_error: false };
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
        Overlay::CherryPick { value, context: _ } => {
            match code {
                KeyCode::Char(' ') => {
                    let spec = value.trim().to_string();
                    if !spec.is_empty() {
                        state.do_cherry_pick(spec);
                        state.overlay = Overlay::Message { text: "Cherry-picked (see log)".to_string(), is_error: false };
                    } else {
                        state.overlay = Overlay::Message { text: "Enter a commit SHA/ref first".to_string(), is_error: true };
                    }
                }
                KeyCode::Enter => {
                    state.overlay = Overlay::None;
                    state.commit_diff_spec = None;
                }
                KeyCode::Char('d') => {
                    let spec = value.trim().to_string();
                    if !spec.is_empty() {
                        state.commit_diff_spec = Some(spec.clone());
                        state.detail_mode = DetailMode::CommitDiff;
                        state.overlay = Overlay::Message { text: format!("Diff for {} shown in detail panel", spec), is_error: false };
                    } else {
                        state.overlay = Overlay::Message { text: "Enter a commit SHA/ref first".to_string(), is_error: true };
                    }
                }
                KeyCode::Char(c) => { value.push(c); }
                KeyCode::Backspace => { value.pop(); }
                KeyCode::Esc => {
                    state.overlay = Overlay::None;
                    state.commit_diff_spec = None;
                }
                _ => {}
            }
        }
Overlay::Message { .. } => {
             if code == KeyCode::Enter || code == KeyCode::Esc { state.overlay = Overlay::None; }
         }
         Overlay::SearchCommit { value } => {
             match code {
                 KeyCode::Enter => {
                     let query = value.trim().to_string();
                     if !query.is_empty() {
                         state.filtered_commit_items = state.commit_items.iter()
                             .filter(|c| c.contains(&query))
                             .cloned()
                             .collect();
                     } else {
                         state.filtered_commit_items = state.commit_items.clone();
                     }
                     state.overlay = Overlay::None;
                 }
                 KeyCode::Char(c) => { value.push(c); }
                 KeyCode::Backspace => { value.pop(); }
                 KeyCode::Esc => {
                     state.overlay = Overlay::None;
                     state.filtered_commit_items = state.commit_items.clone();
                 }
                 _ => {}
             }
         }
         Overlay::SearchBranch { value } => {
             match code {
                 KeyCode::Enter => {
                     let query = value.trim().to_string();
                     if !query.is_empty() {
                         state.filtered_branches = state.branches.iter()
                             .filter(|(b, _)| b.contains(&query))
                             .map(|(b, _)| b.clone())
                             .collect();
                     } else {
                         state.filtered_branches = state.branches.iter()
                             .map(|(b, _)| b.clone())
                             .collect();
                     }
                     state.overlay = Overlay::None;
                 }
                 KeyCode::Char(c) => { value.push(c); }
                 KeyCode::Backspace => { value.pop(); }
                 KeyCode::Esc => {
                     state.overlay = Overlay::None;
                     state.filtered_branches = state.branches.iter()
                         .map(|(b, _)| b.clone())
                         .collect();
                 }
                 _ => {}
             }
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

fn cycle_focus_back(state: &mut AppState) {
    state.focus = match state.focus {
        Focus::Remotes => Focus::Graph,
        Focus::Graph => Focus::Detail,
        Focus::Detail => Focus::Files,
        Focus::Files => Focus::Branches,
        Focus::Branches => Focus::Remotes,
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
            let n = state.graph_lines.len();
            if n > 0 {
                let i = state.graph_state.selected().map(|i| (i + 1) % n).unwrap_or(0);
                state.graph_state.select(Some(i));
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
            let n = state.graph_lines.len();
            if n > 0 {
                let i = state.graph_state.selected().map(|i| if i == 0 { n - 1 } else { i - 1 }).unwrap_or(0);
                state.graph_state.select(Some(i));
            }
        }
        Focus::Detail => {}
    }
}
