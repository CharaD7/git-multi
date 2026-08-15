use crossterm::event::{self, Event, KeyCode, KeyEventKind};
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

use crate::git::{BlameLine, CommitSummary, DiffMode, FileStatus, ResetMode};
use crate::github::{Contributor, PrDetails, PrFile, PrSummary, UserProfile};

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

#[derive(Default, Clone)]
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
    Message { text: String, is_error: bool },
    // ---- Help / palette ----
    Help { scroll: u16 },
    Palette { value: String, selected: usize, filtered: Vec<usize> },
    // ---- Visualization modals ----
    Heatmap,
    GraphFull { all: bool, scroll: u16 },
    FileHistory { path: String, selected: usize },
    LineHistory { path: String, selected: usize },
    Tags { selected: usize },
    Stash { selected: usize },
    Worktree { text: String },
    // ---- GitHub ----
    Contributors { selected: usize, offline: bool },
    Profile { login: String, loaded: bool },
    Prs { selected: usize, state: String, filter: String },
    PrDetail { number: u32, tab: PrTab },
    // ---- Generic input / confirm ----
    Prompt { title: String, value: String, action: PromptAction },
    ConfirmDangerous { title: String, prompt: String, action: DangerousAction },
}

/// Which tab of the PR detail modal is shown.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum PrTab {
    #[default]
    Overview,
    Commits,
    Files,
}

/// A text-input prompt that executes an action on Enter.
#[derive(Clone)]
enum PromptAction {
    PrComment { number: u32 },
    PrReview { number: u32, verdict: String },
    PrMergeStrategy { number: u32 },
    PrClose { number: u32 },
    PrEdit { number: u32, field: String },
    PrAddLabels { number: u32 },
    PrMilestone { number: u32 },
    PrReviewers { number: u32 },
    PrAssignees { number: u32 },
    PrFilter,
    RebaseOnto,
    ShowRef,
    GitMv { from: String },
    AddTag,
    StashSave,
}

/// A destructive action awaiting y/n confirmation.
#[derive(Clone)]
#[allow(dead_code)]
enum DangerousAction {
    GitClean,
    GitRm { path: String },
    StashDrop { index: usize },
    DeleteTag { name: String },
    PrMerge { number: u32, strategy: String, delete_branch: bool },
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Focus {
    Remotes,
    Branches,
    Files,
    Detail,
    Graph,
}

/// Normalized key representation used by the binding registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Key {
    Char(char),
    CtrlChar(char),
    Tab,
    Up,
    Down,
    Left,
    Right,
    Enter,
    Esc,
    Backspace,
    Delete,
    F(u8),
}

impl Key {
    fn label(self) -> String {
        match self {
            Key::Char(c) => c.to_string(),
            Key::CtrlChar(c) => format!("^+{}", c),
            Key::Tab => "Tab".to_string(),
            Key::Up => "↑".to_string(),
            Key::Down => "↓".to_string(),
            Key::Left => "←".to_string(),
            Key::Right => "→".to_string(),
            Key::Enter => "Enter".to_string(),
            Key::Esc => "Esc".to_string(),
            Key::Backspace => "Backspace".to_string(),
            Key::Delete => "Del".to_string(),
            Key::F(n) => format!("F{}", n),
        }
    }
}

/// Which contexts a binding is active in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Scope {
    Global,
    Focus(Focus),
    /// Documented only (text-input/confirm keys handled contextually).
    OverlayDoc,
}

/// A single shortcut: scope + key -> action. This table drives dispatch, the
/// `?` cheatsheet, the command palette, and the idle tips, so it never drifts.
#[derive(Clone, Copy)]
struct Binding {
    scope: Scope,
    key: Key,
    label: &'static str,
    desc: &'static str,
    handler: fn(&mut AppState),
    doc_only: bool,
}

impl Binding {
    const fn action(
        scope: Scope,
        key: Key,
        label: &'static str,
        desc: &'static str,
        handler: fn(&mut AppState),
    ) -> Self {
        Self { scope, key, label, desc, handler, doc_only: false }
    }
    const fn doc(scope: Scope, key: Key, label: &'static str, desc: &'static str) -> Self {
        Self { scope, key, label, desc, handler: |_| {}, doc_only: true }
    }
}

/// Normalize a crossterm key event into our `Key`, folding Shift onto the
/// character itself (terminals deliver `M` with or without the modifier flag).
fn parse_key(ev: &crossterm::event::KeyEvent) -> Option<Key> {
    use crossterm::event::KeyModifiers as KM;
    let mods = ev.modifiers;
    match ev.code {
        KeyCode::Char(c) => {
            if mods.contains(KM::CONTROL) {
                Some(Key::CtrlChar(c.to_ascii_lowercase()))
            } else if mods.contains(KM::SHIFT) {
                Some(Key::Char(c.to_ascii_uppercase()))
            } else {
                Some(Key::Char(c))
            }
        }
        KeyCode::Tab => Some(Key::Tab),
        KeyCode::Up => Some(Key::Up),
        KeyCode::Down => Some(Key::Down),
        KeyCode::Left => Some(Key::Left),
        KeyCode::Right => Some(Key::Right),
        KeyCode::Enter => Some(Key::Enter),
        KeyCode::Esc => Some(Key::Esc),
        KeyCode::Backspace => Some(Key::Backspace),
        KeyCode::Delete => Some(Key::Delete),
        KeyCode::F(n) => Some(Key::F(n)),
        _ => None,
    }
}

/// The complete shortcut table (single source of truth).
fn bindings() -> Vec<Binding> {
    use Focus::*;
    vec![
        // ---- Navigation (Global) ----
        Binding::action(Scope::Global, Key::Tab, "Focus cycle", "Move focus between panes", cycle_focus),
        Binding::action(Scope::Global, Key::Right, "Focus cycle", "Move focus to the next pane", cycle_focus),
        Binding::action(Scope::Global, Key::Left, "Focus cycle back", "Move focus to the previous pane", cycle_focus_back),
        Binding::action(Scope::Global, Key::Up, "Move up", "Move selection up in the focused pane", move_up),
        Binding::action(Scope::Global, Key::Down, "Move down", "Move selection down in the focused pane", move_down),
        Binding::action(Scope::Global, Key::Char(' '), "Toggle", "Toggle branch multi-select", |s| s.toggle_branch_sel()),
        // ---- General (Global) ----
        Binding::action(Scope::Global, Key::Char('q'), "Quit", "Exit git-multi", |s| s.do_quit()),
        Binding::action(Scope::Global, Key::Char('r'), "Refresh", "Reload remotes/branches/files", |s| s.refresh()),
        Binding::action(Scope::Global, Key::Char('s'), "Status", "Show repo status in the Detail pane", |s| s.detail_mode = DetailMode::Status),
        Binding::action(Scope::Global, Key::Char('F'), "Files", "Show staged/unstaged files in Detail", |s| { s.detail_mode = DetailMode::Files; s.refresh(); }),
        Binding::action(Scope::Global, Key::Char('d'), "Diff", "Show unstaged diff in Detail", |s| s.detail_mode = DetailMode::DiffUnstaged),
        Binding::action(Scope::Global, Key::Char('g'), "Graph", "Open the ASCII git graph", |s| s.load_graph()),
        Binding::action(Scope::Global, Key::Char('b'), "Blame", "Blame the selected file (GitLens)", |s| s.do_blame()),
        Binding::action(Scope::Global, Key::Char('S'), "Stage/Unstage", "Toggle staged state of the selected file", |s| s.do_stage_toggle()),
        Binding::action(Scope::Global, Key::Char('v'), "Commits", "Toggle the commits view", |s| s.do_toggle_commits()),
        Binding::action(Scope::Global, Key::Char('/'), "Search", "Search branches/commits in the focused pane", |s| s.open_search()),
        Binding::action(Scope::Global, Key::Char('A'), "Amend", "Amend the last commit (new message)", |s| s.overlay = Overlay::AmendMsg { value: String::new() }),
        Binding::action(Scope::Global, Key::Char('R'), "Revert", "Revert a commit (enter sha/ref)", |s| s.overlay = Overlay::RevertCommit { value: String::new() }),
        Binding::action(Scope::Global, Key::Char('Z'), "Reset", "Reset the current branch (soft/mixed/hard)", |s| s.overlay = Overlay::ResetCommit { value: String::new(), mode: ResetMode::Mixed }),
        Binding::action(Scope::Global, Key::Char('C'), "Commit", "Create a commit (type + subject + body)", |s| s.do_commit()),
        Binding::action(Scope::Global, Key::Char('M'), "Merge/Sync", "Merge or sync across remotes", |s| s.do_merge()),
        Binding::action(Scope::Global, Key::Char('O'), "Restore auto-save", "Restore the auto-save snapshot", |s| s.do_restore_autosave()),
        // ---- Cheatsheet / palette (Global) ----
        Binding::action(Scope::Global, Key::Char('?'), "Cheatsheet", "Show all shortcuts and their meanings", |s| s.overlay = Overlay::Help { scroll: 0 }),
        Binding::action(Scope::Global, Key::CtrlChar('p'), "Command palette", "Fuzzy-search and run any action", |s| s.open_palette()),
        // ---- Visualizations (Global) ----
        Binding::action(Scope::Global, Key::Char('h'), "Heatmap", "Commit activity heatmap (weekday × hour)", |s| s.open_heatmap()),
        Binding::action(Scope::Global, Key::Char('H'), "File history", "GitLens history for the selected file", |s| s.open_file_history()),
        Binding::action(Scope::Global, Key::Char('L'), "Line history", "GitLens line history for the selected file", |s| s.open_line_history()),
        Binding::action(Scope::Global, Key::Char('G'), "Graph (full)", "Full-screen git graph", |s| s.open_graph_full()),
        Binding::action(Scope::Global, Key::Char('t'), "Tags", "Manage git tags", |s| s.open_tags()),
        Binding::action(Scope::Global, Key::Char('U'), "Stash", "Manage the git stash", |s| s.open_stash()),
        Binding::action(Scope::Global, Key::Char('N'), "Contributors", "Repo contributors (GitHub)", |s| s.open_contributors()),
        Binding::action(Scope::Global, Key::Char('o'), "Pull requests", "List and manage pull requests", |s| s.open_prs()),
        Binding::action(Scope::Global, Key::Char('W'), "Worktree status", "Ahead/behind and branch info", |s| s.open_worktree()),
        Binding::action(Scope::Global, Key::Char('Y'), "Copy SHA", "Copy the selected item's SHA", |s| s.copy_sha()),
        // ---- Remotes pane ----
        Binding::action(Scope::Focus(Remotes), Key::Char('a'), "Add remote", "Add a remote (name + URL)", |s| s.overlay = Overlay::AddName { value: String::new() }),
        Binding::action(Scope::Focus(Remotes), Key::Char('R'), "Rename remote", "Rename the selected remote", |s| s.do_rename_remote()),
        Binding::action(Scope::Focus(Remotes), Key::Char('x'), "Remove remote", "Remove the selected remote", |s| s.do_remove_remote()),
        Binding::action(Scope::Focus(Remotes), Key::Char('D'), "Set default", "Set the selected remote as default", |s| s.do_set_default()),
        Binding::action(Scope::Focus(Remotes), Key::Char('f'), "Fetch", "Fetch from the selected remote", |s| s.action_fetch()),
        Binding::action(Scope::Focus(Remotes), Key::Char('p'), "Push", "Push to the selected remote", |s| s.action_push()),
        Binding::action(Scope::Focus(Remotes), Key::Char('l'), "Pull", "Pull from the selected remote", |s| s.action_pull()),
        // ---- Branches pane ----
        Binding::action(Scope::Focus(Branches), Key::Enter, "Checkout", "Check out the selected branch", |s| s.do_checkout()),
        Binding::action(Scope::Focus(Branches), Key::Char('c'), "Create branch", "Create a branch", |s| s.overlay = Overlay::CreateBranch { step: 0, name: String::new(), base: String::new(), remote: String::new() }),
        Binding::action(Scope::Focus(Branches), Key::Char('m'), "Rename branch", "Rename the selected branch", |s| s.do_rename_branch()),
        Binding::action(Scope::Focus(Branches), Key::Char('x'), "Delete branch", "Delete the selected branch", |s| s.do_delete_branch()),
        Binding::action(Scope::Focus(Branches), Key::Char('f'), "Fetch", "Fetch from the selected remote", |s| s.action_fetch()),
        Binding::action(Scope::Focus(Branches), Key::Char('p'), "Push", "Push to the selected remote", |s| s.action_push()),
        Binding::action(Scope::Focus(Branches), Key::Char('l'), "Pull", "Pull from the selected remote", |s| s.action_pull()),
        // ---- Files pane ----
        Binding::action(Scope::Focus(Files), Key::Enter, "Open", "Open the selected file/commit", |s| s.do_enter()),
        Binding::action(Scope::Focus(Files), Key::Char('f'), "Fetch", "Fetch from the selected remote", |s| s.action_fetch()),
        Binding::action(Scope::Focus(Files), Key::Char('p'), "Push", "Push to the selected remote", |s| s.action_push()),
        Binding::action(Scope::Focus(Files), Key::Char('l'), "Pull", "Pull from the selected remote", |s| s.action_pull()),
        Binding::action(Scope::Focus(Files), Key::Char('P'), "Cherry-pick", "Cherry-pick a commit onto HEAD", |s| s.do_files_pick()),
        Binding::action(Scope::Focus(Files), Key::Char('X'), "git rm", "Remove the selected file (git rm)", |s| s.do_git_rm()),
        Binding::action(Scope::Focus(Files), Key::Char('V'), "git mv", "Rename the selected file (git mv)", |s| s.do_git_mv()),
        // ---- Branches pane ----
        Binding::action(Scope::Focus(Branches), Key::Char('K'), "Rebase onto", "Rebase the current branch onto a ref", |s| s.overlay = Overlay::Prompt { title: "Rebase onto (sha/ref)".to_string(), value: String::new(), action: PromptAction::RebaseOnto }),
        // ---- Detail pane ----
        Binding::action(Scope::Focus(Detail), Key::Enter, "Commit detail", "Show the selected commit's details", |s| s.do_commit_enter()),
        Binding::action(Scope::Focus(Detail), Key::Char('j'), "Scroll down", "Scroll commit details down", |s| s.do_commit_scroll(1)),
        Binding::action(Scope::Focus(Detail), Key::Char('k'), "Scroll up", "Scroll commit details up", |s| s.do_commit_scroll(-1)),
        Binding::action(Scope::Focus(Detail), Key::Char('X'), "Show ref", "git show an arbitrary ref", |s| s.overlay = Overlay::Prompt { title: "Show ref (git show)".to_string(), value: String::new(), action: PromptAction::ShowRef }),
        Binding::action(Scope::Focus(Detail), Key::Char('f'), "Fetch", "Fetch from the selected remote", |s| s.action_fetch()),
        Binding::action(Scope::Focus(Detail), Key::Char('p'), "Push", "Push to the selected remote", |s| s.action_push()),
        Binding::action(Scope::Focus(Detail), Key::Char('l'), "Pull", "Pull from the selected remote", |s| s.action_pull()),
        // ---- Graph focus ----
        Binding::action(Scope::Focus(Graph), Key::Char('a'), "All refs", "Toggle graph between HEAD and all refs", |s| s.do_graph_all()),
        Binding::action(Scope::Focus(Graph), Key::Char('D'), "Diff commit", "Preview the selected commit's diff", |s| s.do_graph_diff()),
        // ---- Documented overlay/input keys ----
        Binding::doc(Scope::OverlayDoc, Key::Char('y'), "Confirm", "Confirm a destructive action"),
        Binding::doc(Scope::OverlayDoc, Key::Char('n'), "Cancel", "Cancel a destructive action"),
        Binding::doc(Scope::OverlayDoc, Key::Char('1'), "Reset soft", "Reset mode: soft (in Reset overlay)"),
        Binding::doc(Scope::OverlayDoc, Key::Char('2'), "Reset mixed", "Reset mode: mixed (in Reset overlay)"),
        Binding::doc(Scope::OverlayDoc, Key::Char('3'), "Reset hard", "Reset mode: hard (in Reset overlay)"),
        Binding::doc(Scope::OverlayDoc, Key::Esc, "Close", "Close the current overlay"),
        Binding::doc(Scope::OverlayDoc, Key::Char(' '), "Cherry-pick", "Execute a cherry-pick (in the pick overlay)"),
    ]
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
    // ---- Data loads (read-only, worker-thread) ----
    LoadActivity,
    LoadContributors,
    LoadProfile { login: String },
    LoadPrs { state: String },
    LoadPrDetail { number: u32 },
    LoadPrFiles { number: u32 },
    // ---- Git action coverage ----
    GitClean,
    GitRm { path: String },
    GitMv { from: String, to: String },
    StashApply { index: usize },
    StashDrop { index: usize },
    StashSave { message: Option<String> },
    RebaseOnto { onto: String },
    // ---- PR actions ----
    PrMerge { number: u32, strategy: String, delete_branch: bool },
    PrClose { number: u32, comment: Option<String> },
    PrReopen { number: u32 },
    PrComment { number: u32, body: String },
    PrReview { number: u32, verdict: String, body: Option<String> },
    PrCheckout { number: u32 },
    PrEdit { number: u32, title: Option<String>, body: Option<String> },
    PrEditList { number: u32, flag: String, values: Vec<String> },
    PrOpenWeb { number: u32 },
    PrShow { number: u32, show_diff: bool },
    OpenUrl { url: String },
    DeleteTag { name: String },
}

/// Structured data returned from a background job, applied on the UI thread.
enum JobPayload {
    None,
    Activity(Box<[u32; 168]>),
    Contributors(Vec<Contributor>),
    Profile(UserProfile),
    Prs(Vec<PrSummary>),
    PrDetail(Box<PrDetails>),
    PrFiles(Vec<PrFile>),
}

/// Outcome of a background job, applied on the UI thread.
struct JobResult {
    message: String,
    refresh: bool,
    payload: JobPayload,
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
    pending_jobs: u32,
    autosave_pending: bool,
    quit: bool,
    // Cached data from background loads
    activity: Option<[u32; 168]>,
    contributors: Option<Vec<Contributor>>,
    contributors_offline: bool,
    profile: Option<UserProfile>,
    prs: Option<Vec<PrSummary>>,
    pr_detail: Option<PrDetails>,
    pr_files: Vec<PrFile>,
    worktree_text: String,
    // Cached local data for modals (avoid per-frame git calls)
    file_history_cache: Vec<CommitSummary>,
    line_history_cache: Vec<CommitSummary>,
    tags_cache: Vec<(String, String, String)>,
    stashes_cache: Vec<String>,
    // Idle tips / hovers
    gui: crate::config::GuiPreferences,
    tip_visible: bool,
    tip_signature: Option<(Focus, usize, DetailMode)>,
    hover_cache: Option<(Focus, usize, DetailMode, String)>,
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

        let gui = repo.config.gui.clone();
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
            pending_jobs: 0,
            autosave_pending: false,
            quit: false,
            activity: None,
            contributors: None,
            contributors_offline: false,
            profile: None,
            prs: None,
            pr_detail: None,
            pr_files: Vec::new(),
            worktree_text: String::new(),
            file_history_cache: Vec::new(),
            line_history_cache: Vec::new(),
            tags_cache: Vec::new(),
            stashes_cache: Vec::new(),
            gui,
            tip_visible: false,
            tip_signature: None,
            hover_cache: None,
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

    /// Send a job to the background worker. Jobs are queued and processed in
    /// order on a single worker thread; `pending_jobs` powers the busy flag.
    fn submit_job(&mut self, job: UiJob, _silent_when_busy: bool) {
        self.pending_jobs += 1;
        if let Err(e) = self.job_tx.send(job) {
            self.pending_jobs = self.pending_jobs.saturating_sub(1);
            self.log(format!("Failed to start background task: {}", e));
        }
    }

    fn is_busy(&self) -> bool {
        self.pending_jobs > 0
    }

    /// Drain finished background jobs onto the UI thread.
    fn pump_jobs(&mut self) {
        while let Ok(result) = self.result_rx.try_recv() {
            self.pending_jobs = self.pending_jobs.saturating_sub(1);
            if self.pending_jobs == 0 {
                self.autosave_pending = false;
            }
            self.last_activity = Instant::now();
            match result.payload {
                JobPayload::None => {}
                JobPayload::Activity(counts) => {
                    self.activity = Some(*counts);
                }
                JobPayload::Contributors(list) => {
                    self.contributors = Some(list);
                    self.contributors_offline = false;
                }
                JobPayload::Profile(p) => {
                    self.profile = Some(p);
                    if let Overlay::Profile { loaded, .. } = &mut self.overlay {
                        *loaded = true;
                    }
                }
                JobPayload::Prs(list) => {
                    self.prs = Some(list);
                    if let Overlay::Prs { .. } = &self.overlay {
                        // no-op marker; selection clamped at render
                    }
                }
                JobPayload::PrDetail(d) => {
                    self.pr_detail = Some(*d);
                    if let Overlay::PrDetail { .. } = &mut self.overlay {
                        // loaded flag lives on the cached data being present
                    }
                }
                JobPayload::PrFiles(files) => {
                    self.pr_files = files;
                }
            }
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

    // ========================================================================
    // Registry handlers (called from the binding table)
    // ========================================================================

    fn do_quit(&mut self) {
        self.quit = true;
    }

    fn toggle_branch_sel(&mut self) {
        if self.focus == Focus::Branches {
            if let Some(i) = self.branch_state.selected() {
                if let Some((_, sel)) = self.branches.get_mut(i) {
                    *sel = !*sel;
                }
            }
        }
    }

    fn do_blame(&mut self) {
        if let Some(p) = self.selected_file_path() {
            self.load_blame(&p);
        } else {
            self.log("Select a file in the Files panel first ([F]).".to_string());
        }
    }

    fn do_stage_toggle(&mut self) {
        if let Some(p) = self.selected_file_path() {
            let f = self.files.iter().find(|f| f.path == p);
            let staged = f.map(|f| f.staged != ' ').unwrap_or(false);
            if staged {
                self.do_unstage(&p);
            } else {
                self.do_stage(&p);
            }
        }
    }

    fn do_toggle_commits(&mut self) {
        self.files_show_commits = !self.files_show_commits;
        if self.files_show_commits {
            self.commit_items = self.repo.list_recent_commits(30).unwrap_or_default();
            self.filtered_commit_items = self.commit_items.clone();
            self.file_state.select(Some(0));
            self.detail_mode = DetailMode::Commit;
            self.commit_detail_scroll = 0;
        } else {
            self.detail_mode = DetailMode::Detail;
            self.commit_items.clear();
            self.filtered_commit_items.clear();
        }
    }

    fn open_search(&mut self) {
        match self.focus {
            Focus::Files if self.files_show_commits => {
                self.overlay = Overlay::SearchCommit { value: String::new() };
            }
            Focus::Branches => {
                self.overlay = Overlay::SearchBranch { value: String::new() };
            }
            _ => {}
        }
    }

    fn do_commit(&mut self) {
        self.overlay = Overlay::CommitType { value: String::new() };
    }

    fn do_merge(&mut self) {
        self.overlay = Overlay::Merge {
            step: 0,
            src_remote: String::new(),
            src_branch: String::new(),
            dest_remote: String::new(),
            dest_branch: String::new(),
        };
    }

    fn do_restore_autosave(&mut self) {
        if self.autosave_ref_exists {
            match self.repo.restore_from_autosave() {
                Ok(()) => {
                    self.refresh();
                    self.log("Restored from auto-save snapshot".to_string());
                }
                Err(e) => self.log(format!("Auto-save restore failed: {}", e)),
            }
        } else {
            self.log("No auto-save snapshot available yet".to_string());
        }
    }

    fn do_rename_remote(&mut self) {
        if let Some(name) = self.selected_remote_name() {
            self.overlay = Overlay::RenameRemote { old: name, value: String::new() };
        }
    }

    fn do_remove_remote(&mut self) {
        if let Some(name) = self.selected_remote_name() {
            self.overlay = Overlay::RemoveRemote { name };
        }
    }

    fn do_set_default(&mut self) {
        if let Some(name) = self.selected_remote_name() {
            self.log(format!("Setting default remote to '{}' ...", name));
            self.submit_job(UiJob::SetDefault { name }, false);
        }
    }

    fn do_checkout(&mut self) {
        if let Some(name) = self.selected_branch_name() {
            match self.repo.checkout_branch(&name) {
                Ok(()) => {
                    self.refresh();
                    self.log(format!("Checked out '{}'", name));
                }
                Err(e) => self.log(format!("Checkout failed: {}", e)),
            }
        }
    }

    fn do_rename_branch(&mut self) {
        if let Some(name) = self.selected_branch_name() {
            self.overlay = Overlay::RenameBranch { old: name, value: String::new() };
        }
    }

    fn do_delete_branch(&mut self) {
        if let Some(name) = self.selected_branch_name() {
            self.overlay = Overlay::DeleteBranch { name };
        }
    }

    fn do_enter(&mut self) {
        match self.focus {
            Focus::Remotes => self.action_fetch(),
            Focus::Branches => self.do_checkout(),
            Focus::Files => {
                if self.files_show_commits {
                    let commit_list = if self.filtered_commit_items.is_empty() {
                        &self.commit_items
                    } else {
                        &self.filtered_commit_items
                    };
                    if let Some(idx) = self.file_state.selected() {
                        if let Some(line) = commit_list.get(idx) {
                            let sha = line.split_whitespace().next().map(|s| s.to_string());
                            self.commit_diff_spec = sha;
                            self.detail_mode = DetailMode::Commit;
                            self.focus = Focus::Detail;
                            self.commit_detail_scroll = 0;
                        }
                    }
                } else if let Some(p) = self.selected_file_path() {
                    self.overlay = Overlay::DiffPath { value: p, mode: DiffMode::Unstaged };
                }
            }
            Focus::Graph => {
                if let Some(idx) = self.graph_state.selected() {
                    if let Some(gl) = self.graph_lines.get(idx) {
                        if gl.is_commit {
                            let short = gl.sha[..8.min(gl.sha.len())].to_string();
                            let ctx = gl.text.clone();
                            self.overlay = Overlay::CherryPick { value: short, context: ctx };
                        }
                    }
                }
            }
            Focus::Detail => {}
        }
    }

    fn do_files_pick(&mut self) {
        if self.files_show_commits {
            return;
        }
        if let Some(p) = self.selected_file_path() {
            let is_dirty = self.files.iter().any(|f| f.path == p && (f.staged != ' ' || f.unstaged != ' '));
            let head_short = self
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
            self.overlay = Overlay::CherryPick { value, context };
        }
    }

    fn do_git_rm(&mut self) {
        if let Some(p) = self.selected_file_path() {
            self.overlay = Overlay::ConfirmDangerous {
                title: "git rm".to_string(),
                prompt: format!("Remove '{}' (git rm)?", p),
                action: DangerousAction::GitRm { path: p },
            };
        }
    }

    fn do_git_mv(&mut self) {
        if let Some(p) = self.selected_file_path() {
            self.overlay = Overlay::Prompt {
                title: "git mv (destination)".to_string(),
                value: String::new(),
                action: PromptAction::GitMv { from: p },
            };
        }
    }

    fn do_commit_enter(&mut self) {
        if self.detail_mode == DetailMode::Commit {
            let commit_list = if self.filtered_commit_items.is_empty() {
                &self.commit_items
            } else {
                &self.filtered_commit_items
            };
            if let Some(idx) = self.file_state.selected() {
                if let Some(line) = commit_list.get(idx) {
                    let sha = line.split_whitespace().next().unwrap_or("").to_string();
                    if !sha.is_empty() {
                        self.commit_diff_spec = Some(sha.clone());
                        self.refresh();
                    }
                }
            }
        }
    }

    fn do_commit_scroll(&mut self, delta: i16) {
        if self.detail_mode == DetailMode::Commit {
            if delta > 0 {
                self.commit_detail_scroll = self.commit_detail_scroll.saturating_add(1);
            } else {
                self.commit_detail_scroll = self.commit_detail_scroll.saturating_sub(1);
            }
        }
    }

    fn do_graph_all(&mut self) {
        self.graph_all = !self.graph_all;
        self.load_graph();
    }

    fn do_graph_diff(&mut self) {
        if let Some(idx) = self.graph_state.selected() {
            if let Some(gl) = self.graph_lines.get(idx) {
                if gl.is_commit {
                    self.commit_diff_spec = Some(gl.sha.clone());
                    self.detail_mode = DetailMode::CommitDiff;
                    self.log(format!("Diff for {} shown in detail panel", &gl.sha[..8.min(gl.sha.len())]));
                }
            }
        }
    }

    // ========================================================================
    // Feature modals (cheatsheet, palette, visualizations, GitHub)
    // ========================================================================

    fn open_palette(&mut self) {
        self.overlay = Overlay::Palette { value: String::new(), selected: 0, filtered: Vec::new() };
    }

    fn open_heatmap(&mut self) {
        if self.activity.is_none() {
            self.submit_job(UiJob::LoadActivity, true);
        }
        self.overlay = Overlay::Heatmap;
    }

    fn open_file_history(&mut self) {
        if let Some(p) = self.selected_file_path() {
            self.file_history_cache = self.repo.file_history(&p).unwrap_or_default();
            self.overlay = Overlay::FileHistory { path: p, selected: 0 };
        } else {
            self.log("Select a file in the Files panel first.".to_string());
        }
    }

    fn open_line_history(&mut self) {
        if let Some(p) = self.selected_file_path() {
            self.line_history_cache = self.repo.line_history(&p, 1).unwrap_or_default();
            self.overlay = Overlay::LineHistory { path: p, selected: 0 };
        } else {
            self.log("Select a file in the Files panel first.".to_string());
        }
    }

    fn open_graph_full(&mut self) {
        let lines = self.repo.log_graph(self.graph_all, 400).unwrap_or_default();
        let parsed: Vec<GraphLine> = lines.iter().filter_map(|l| parse_graph_line(l)).collect();
        self.overlay = Overlay::GraphFull { all: self.graph_all, scroll: 0 };
        self.graph_lines = parsed;
    }

    fn open_tags(&mut self) {
        self.tags_cache = self.repo.tag_detail().unwrap_or_default();
        self.overlay = Overlay::Tags { selected: 0 };
    }

    fn open_stash(&mut self) {
        self.stashes_cache = self.repo.stash_list().unwrap_or_default();
        self.overlay = Overlay::Stash { selected: 0 };
    }

    fn open_worktree(&mut self) {
        self.worktree_text = self.repo.status_text().unwrap_or_else(|e| format!("Error: {}", e));
        if let Ok(Some(b)) = self.repo.current_branch() {
            if let Ok(info) = self.repo.branch_info(&b) {
                let _ = info;
            }
        }
        self.overlay = Overlay::Worktree { text: self.worktree_text.clone() };
    }

    fn copy_sha(&mut self) {
        let sha = self
            .repo
            .head_commit()
            .ok()
            .map(|c| c.id().to_string())
            .unwrap_or_default();
        if !sha.is_empty() {
            self.log(format!("Copied SHA {}", &sha[..8.min(sha.len())]));
        } else {
            self.log("No HEAD commit to copy".to_string());
        }
    }

    fn open_contributors(&mut self) {
        self.contributors = None;
        self.overlay = Overlay::Contributors { selected: 0, offline: false };
        self.submit_job(UiJob::LoadContributors, true);
    }

    fn open_prs(&mut self) {
        self.prs = None;
        let state = self.gui.pr_default_state.clone();
        self.overlay = Overlay::Prs { selected: 0, state, filter: String::new() };
        self.submit_job(UiJob::LoadPrs { state: self.gui.pr_default_state.clone() }, true);
    }

    fn open_profile(&mut self, login: String) {
        self.profile = None;
        self.overlay = Overlay::Profile { login: login.clone(), loaded: false };
        self.submit_job(UiJob::LoadProfile { login }, true);
    }

    fn open_pr_detail(&mut self, number: u32) {
        self.pr_detail = None;
        self.pr_files = Vec::new();
        self.overlay = Overlay::PrDetail { number, tab: PrTab::Overview };
        self.submit_job(UiJob::LoadPrDetail { number }, true);
        self.submit_job(UiJob::LoadPrFiles { number }, true);
    }

    /// Signature of the current selection, used to re-arm idle tips/hovers.
    fn selection_signature(&self) -> Option<(Focus, usize, DetailMode)> {
        let idx = match self.focus {
            Focus::Remotes => self.remote_state.selected(),
            Focus::Branches => self.branch_state.selected(),
            Focus::Files => self.file_state.selected(),
            Focus::Graph => self.graph_state.selected(),
            Focus::Detail => self.file_state.selected(),
        };
        idx.map(|i| (self.focus, i, self.detail_mode))
    }

    /// Per-pane idle tip lines for the currently focused pane.
    fn focus_tip(&self) -> Option<String> {
        let focus = self.focus;
        let (title, keys): (&str, Vec<&str>) = match focus {
            Focus::Remotes => ("Remotes", vec!["f fetch", "p push", "l pull", "M merge/sync", "a add", "R rename", "x remove", "D default", "Enter fetch"]),
            Focus::Branches => ("Branches", vec!["Enter checkout", "c create", "m rename", "x delete", "Space toggle", "f/p/l net"]),
            Focus::Files => ("Files", vec!["Enter diff", "S stage/unstage", "b blame", "P cherry-pick", "H history", "v commits"]),
            Focus::Detail => ("Detail", vec!["j/k scroll", "d diff", "F files", "s status", "v commits"]),
            Focus::Graph => ("Graph", vec!["a all-refs", "Enter pick", "D diff", "G full-screen"]),
        };
        let joined = keys.join("  ·  ");
        Some(format!("{}: {}", title, joined))
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
                    payload: JobPayload::None,
                });
                continue;
            }
        };
        let result = handle_job(&mut repo, job);
        let _ = result_tx.send(result);
    }
}

fn handle_job(repo: &mut crate::git::GitRepo, job: UiJob) -> JobResult {
    let result = |message: String, refresh: bool| JobResult {
        message,
        refresh,
        payload: JobPayload::None,
    };
    let result_err = |message: String| JobResult {
        message,
        refresh: false,
        payload: JobPayload::None,
    };
    match job {
        UiJob::Fetch { remote, branches } => {
            let r = if branches.is_empty() {
                repo.fetch_remote(&remote)
            } else {
                repo.fetch_branches(&remote, &branches)
            };
            match r {
                Ok(()) => result(format!("Fetched from '{}'", remote), true),
                Err(e) => result_err(format!("Fetch '{}' failed: {}", remote, e)),
            }
        }
        UiJob::Push { remote, branches } => {
            let r = if branches.is_empty() {
                repo.push_to_remote(&remote, None)
            } else {
                repo.push_branches(&remote, &branches, false)
            };
            match r {
                Ok(()) => result(format!("Pushed to '{}'", remote), true),
                Err(e) => result_err(format!("Push '{}' failed: {}", remote, e)),
            }
        }
        UiJob::Pull { remote, branches } => {
            let r = if branches.is_empty() {
                repo.pull_from_remote(&remote, None)
            } else {
                repo.pull_branches(&remote, &branches)
            };
            match r {
                Ok(()) => result(format!("Pulled from '{}'", remote), true),
                Err(e) => result_err(format!("Pull '{}' failed: {}", remote, e)),
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
                Ok(()) => result(
                    format!("Merged {}/{} into {}/{} and pushed", src_remote, src_branch, dest_remote, dest_branch),
                    true,
                ),
                Err(e) => result_err(format!("Merge failed: {}", e)),
            }
        }
        UiJob::Commit { subject, body } => match repo.create_commit(&subject, body.as_deref()) {
            Ok(()) => result(format!("Created commit: {}", subject), true),
            Err(e) => result_err(format!("Commit failed: {}", e)),
        },
        UiJob::Stage { path } => match repo.stage_file(&path) {
            Ok(()) => result(format!("Staged: {}", path), true),
            Err(e) => result_err(format!("Stage failed: {}", e)),
        },
        UiJob::Unstage { path } => match repo.unstage_file(&path) {
            Ok(()) => result(format!("Unstaged: {}", path), true),
            Err(e) => result_err(format!("Unstage failed: {}", e)),
        },
        UiJob::Amend { msg } => match repo.amend_commit(&msg, None) {
            Ok(()) => result("Amended last commit".to_string(), true),
            Err(e) => result_err(format!("Amend failed: {}", e)),
        },
        UiJob::Revert { spec } => match repo.revert_commit(&spec) {
            Ok(()) => result(format!("Reverted {}", spec), true),
            Err(e) => result_err(format!("Revert failed: {}", e)),
        },
        UiJob::Reset { mode, spec } => match repo.reset(mode, &spec) {
            Ok(()) => result(format!("Reset ({:?}) to {}", mode, spec), true),
            Err(e) => result_err(format!("Reset failed: {}", e)),
        },
        UiJob::CherryPick { spec } => match repo.cherry_pick_commit(&spec) {
            Ok(()) => result(format!("Cherry-picked {}", spec), true),
            Err(e) => result_err(format!("Cherry-pick failed: {}", e)),
        },
        UiJob::AddRemote { name, url } => match repo.add_remote(&name, &url) {
            Ok(()) => result(format!("Added remote '{}'", name), true),
            Err(e) => result_err(format!("Error: {}", e)),
        },
        UiJob::RenameRemote { old, new } => match repo.rename_remote(&old, &new) {
            Ok(()) => result(format!("Renamed remote '{}' -> '{}'", old, new), true),
            Err(e) => result_err(format!("Error: {}", e)),
        },
        UiJob::RemoveRemote { name } => match repo.remove_remote(&name) {
            Ok(()) => result(format!("Removed remote '{}'", name), true),
            Err(e) => result_err(format!("Error: {}", e)),
        },
        UiJob::DeleteBranch { name } => match repo.delete_local_branch(&name, false) {
            Ok(()) => result(format!("Deleted branch '{}'", name), true),
            Err(e) => result_err(format!("Error: {}", e)),
        },
        UiJob::RenameBranch { old, new } => match repo.rename_branch(&old, &new) {
            Ok(()) => result(format!("Renamed branch '{}' -> '{}'", old, new), true),
            Err(e) => result_err(format!("Error: {}", e)),
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
                Ok(()) => result(format!("Created branch '{}'", name), true),
                Err(e) => result_err(format!("Error: {}", e)),
            }
        }
        UiJob::SetDefault { name } => {
            let r = repo
                .config
                .set_default_remote(name.clone())
                .and_then(|_| repo.config.save(&repo.repo));
            match r {
                Ok(()) => result(format!("Default remote set to '{}'", name), true),
                Err(e) => result_err(format!("Error: {}", e)),
            }
        }
        UiJob::Autosave => match repo.write_autosave_snapshot() {
            Ok(true) => result("[auto-save] snapshot captured".to_string(), true),
            Ok(false) => result(String::new(), false),
            Err(_) => result_err("[auto-save] failed".to_string()),
        },
        // ---- Data loads ----
        UiJob::LoadActivity => {
            let counts = repo.commit_activity(5000);
            match counts {
                Ok(c) => JobResult { message: String::new(), refresh: false, payload: JobPayload::Activity(Box::new(c)) },
                Err(e) => result_err(format!("Heatmap failed: {}", e)),
            }
        }
        UiJob::LoadContributors => {
            if crate::github::gh_available() {
                match crate::github::list_contributors(repo) {
                    Ok(list) => JobResult {
                        message: String::new(),
                        refresh: false,
                        payload: JobPayload::Contributors(list),
                    },
                    Err(e) => result_err(format!("Contributors failed: {}", e)),
                }
            } else {
                JobResult {
                    message: String::new(),
                    refresh: false,
                    payload: JobPayload::Contributors(crate::github::contributors_from_shortlog(repo)),
                }
            }
        }
        UiJob::LoadProfile { login } => match crate::github::user_profile(repo, &login) {
            Ok(p) => JobResult { message: String::new(), refresh: false, payload: JobPayload::Profile(p) },
            Err(e) => result_err(format!("Profile failed: {}", e)),
        },
        UiJob::LoadPrs { state } => match crate::github::list_prs(repo, &state) {
            Ok(list) => JobResult { message: String::new(), refresh: false, payload: JobPayload::Prs(list) },
            Err(e) => result_err(format!("PRs failed: {}", e)),
        },
        UiJob::LoadPrDetail { number } => match crate::github::pr_detail(repo, number) {
            Ok(d) => JobResult { message: String::new(), refresh: false, payload: JobPayload::PrDetail(Box::new(d)) },
            Err(e) => result_err(format!("PR #{} failed: {}", number, e)),
        },
        UiJob::LoadPrFiles { number } => match crate::github::pr_files(repo, number) {
            Ok(files) => JobResult {
                message: String::new(),
                refresh: false,
                payload: JobPayload::PrFiles(files),
            },
            Err(e) => result_err(format!("PR #{} files failed: {}", number, e)),
        },
        // ---- Git action coverage ----
        UiJob::GitClean => match repo.git_clean(true) {
            Ok(()) => result("Cleaned untracked files".to_string(), true),
            Err(e) => result_err(format!("git clean failed: {}", e)),
        },
        UiJob::GitRm { path } => match repo.git_rm(&path) {
            Ok(()) => result(format!("Removed '{}'", path), true),
            Err(e) => result_err(format!("git rm failed: {}", e)),
        },
        UiJob::GitMv { from, to } => match repo.git_mv(&from, &to) {
            Ok(()) => result(format!("Moved '{}' -> '{}'", from, to), true),
            Err(e) => result_err(format!("git mv failed: {}", e)),
        },
        UiJob::StashApply { index } => match repo.stash_apply(index) {
            Ok(()) => result(format!("Applied stash@{{{}}}", index), true),
            Err(e) => result_err(format!("Stash apply failed: {}", e)),
        },
        UiJob::StashDrop { index } => match repo.stash_drop(index) {
            Ok(()) => result(format!("Dropped stash@{{{}}}", index), true),
            Err(e) => result_err(format!("Stash drop failed: {}", e)),
        },
        UiJob::StashSave { message } => match repo.stash_save(message.as_deref()) {
            Ok(()) => result("Working tree stashed".to_string(), true),
            Err(e) => result_err(format!("Stash failed: {}", e)),
        },
        UiJob::RebaseOnto { onto } => {
            let branch = repo.current_branch().ok().flatten().unwrap_or_default();
            match repo.rebase_onto(&branch, &onto) {
                Ok(()) => result(format!("Rebased '{}' onto '{}'", branch, onto), true),
                Err(e) => result_err(format!("Rebase failed: {}", e)),
            }
        }
        // ---- PR actions ----
        UiJob::PrMerge { number, strategy, delete_branch } => {
            match crate::github::merge_pr(repo, number, &strategy, delete_branch) {
                Ok(msg) => result(format!("PR #{} merged: {}", number, msg), true),
                Err(e) => result_err(format!("PR merge failed: {}", e)),
            }
        }
        UiJob::PrClose { number, comment } => match crate::github::close_pr(repo, number, comment.as_deref()) {
            Ok(msg) => result(format!("PR #{} closed: {}", number, msg), true),
            Err(e) => result_err(format!("PR close failed: {}", e)),
        },
        UiJob::PrReopen { number } => match crate::github::reopen_pr(repo, number) {
            Ok(msg) => result(format!("PR #{} reopened: {}", number, msg), true),
            Err(e) => result_err(format!("PR reopen failed: {}", e)),
        },
        UiJob::PrComment { number, body } => match crate::github::comment_pr(repo, number, &body) {
            Ok(msg) => result(format!("Commented on PR #{}: {}", number, msg), true),
            Err(e) => result_err(format!("PR comment failed: {}", e)),
        },
        UiJob::PrReview { number, verdict, body } => match crate::github::review_pr(repo, number, &verdict, body.as_deref()) {
            Ok(msg) => result(format!("Reviewed PR #{} ({}): {}", number, verdict, msg), true),
            Err(e) => result_err(format!("PR review failed: {}", e)),
        },
        UiJob::PrCheckout { number } => match crate::github::checkout_pr(repo, number) {
            Ok(msg) => result(format!("Checked out PR #{}: {}", number, msg), true),
            Err(e) => result_err(format!("PR checkout failed: {}", e)),
        },
        UiJob::PrEdit { number, title, body } => {
            match crate::github::edit_pr(repo, number, title.as_deref(), body.as_deref()) {
                Ok(msg) => result(format!("Edited PR #{}: {}", number, msg), true),
                Err(e) => result_err(format!("PR edit failed: {}", e)),
            }
        }
        UiJob::PrEditList { number, flag, values } => {
            match crate::github::edit_pr_list(repo, number, "", &flag, &values) {
                Ok(msg) => result(format!("Updated PR #{}: {}", number, msg), true),
                Err(e) => result_err(format!("PR update failed: {}", e)),
            }
        }
        UiJob::PrOpenWeb { number } => match crate::github::open_pr_web(repo, number) {
            Ok(msg) => result(format!("Opened PR #{} in browser: {}", number, msg), true),
            Err(e) => result_err(format!("Open PR failed: {}", e)),
        },
        UiJob::PrShow { number, show_diff } => {
            let r = if show_diff {
                crate::github::pr_diff(repo, number)
            } else {
                crate::github::pr_files(repo, number).map(|files| {
                    files
                        .iter()
                        .map(|f| format!("{}  {}  +{}/-{}", f.path, f.status, f.additions, f.deletions))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
            };
            match r {
                Ok(msg) => result(msg, false),
                Err(e) => result_err(format!("PR view failed: {}", e)),
            }
        }
        UiJob::OpenUrl { url } => {
            let workdir = repo.workdir_public();
            let opened = if cfg!(target_os = "macos") {
                crate::git::run_captured("open", &[&url], workdir, &[], Duration::from_secs(10))
            } else {
                crate::git::run_captured("xdg-open", &[&url], workdir, &[], Duration::from_secs(10))
            };
            match opened {
                Ok(o) if o.status.success() => result(format!("Opened {}", url), false),
                _ => result_err(format!("Could not open {}", url)),
            }
        }
        UiJob::DeleteTag { name } => match repo.delete_tag(&name) {
            Ok(()) => result(format!("Deleted tag '{}'", name), true),
            Err(e) => result_err(format!("Tag delete failed: {}", e)),
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
        // Idle engine: show per-pane tips/hovers after `idle_tip_delay_secs`
        // without any keypress. Any keypress resets `last_activity`, so "idle"
        // already implies "not navigating".
        if state.gui.idle_tips
            && matches!(state.overlay, Overlay::None)
            && !state.is_busy()
            && state.last_activity.elapsed() >= Duration::from_secs(state.gui.idle_tip_delay_secs)
        {
            let sig = state.selection_signature();
            if sig != state.tip_signature {
                state.tip_signature = sig;
                state.tip_visible = true;
                state.hover_cache = None;
            }
        }
        if state.autosave_ref_exists
            && !state.autosave_pending
            && state.last_activity.elapsed() >= Duration::from_secs(30)
        {
            state.autosave_pending = true;
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
    let busy = if state.is_busy() { "  [working…]" } else { "" };
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
    if state.focus == Focus::Remotes {
        maybe_tip_and_hover(f, state, area);
    }
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
    if state.focus == Focus::Branches {
        maybe_tip_and_hover(f, state, area);
    }
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
    if state.focus == Focus::Files {
        maybe_tip_and_hover(f, state, area);
    }
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
    if state.focus == Focus::Detail || state.focus == Focus::Graph {
        maybe_tip_and_hover(f, state, area);
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
         Overlay::Help { scroll } => render_help(f, *scroll),
         Overlay::Palette { value, selected, filtered } => render_palette(f, value, *selected, filtered),
         Overlay::Heatmap => render_heatmap(f, state),
         Overlay::GraphFull { all, scroll } => render_graph_full(f, state, *all, *scroll),
         Overlay::FileHistory { path, selected } => render_file_history(f, state, path, *selected),
         Overlay::LineHistory { path, selected } => render_line_history(f, state, path, *selected),
         Overlay::Tags { selected } => render_tags(f, state, *selected),
         Overlay::Stash { selected } => render_stash(f, state, *selected),
         Overlay::Worktree { text } => {
             let big = centered_rect(70, 24, f.area());
             let m = Paragraph::new(text.as_str())
                 .block(Block::default().title(" Worktree / Branch Status ").borders(Borders::ALL).border_style(Style::default().fg(BLUE)))
                 .style(Style::default().fg(CREAM));
             f.render_widget(ratatui::widgets::Clear, big);
             f.render_widget(m, big);
         }
         Overlay::Contributors { selected, offline } => render_contributors(f, state, *selected, *offline),
         Overlay::Profile { login, loaded } => render_profile(f, state, login, *loaded),
         Overlay::Prs { selected, state: pr_state, filter } => render_prs(f, state, *selected, pr_state, filter),
         Overlay::PrDetail { number, tab } => render_pr_detail(f, state, *number, *tab),
         Overlay::Prompt { title, value, .. } => modal(f, 80, 5, title,
             &format!("{}\n> {}\u{2588}", prompt_hint(title), value), CYAN),
         Overlay::ConfirmDangerous { title, prompt, .. } => modal(f, 70, 5, title,
             &format!("{}\n\n[y] Yes  [n/Esc] Cancel", prompt), RED),
         Overlay::None => {}
    }
}

fn prompt_hint(_title: &str) -> &'static str {
    "Enter value (Esc to cancel)"
}

// ---------------------------------------------------------------------------
// Modal renderers
// ---------------------------------------------------------------------------

fn render_help(f: &mut Frame, scroll: u16) {
    let area = centered_rect(82, 40, f.area());
    let mut text = String::new();
    text.push_str("Git-multi cheatsheet  —  press ? or Esc to close\n\n");
    let groups: [(&str, Scope); 2] = [("GLOBAL", Scope::Global), ("FOCUSED", Scope::OverlayDoc)];
    for (gname, _gscope) in groups {
        text.push_str(&format!("══ {} ══\n", gname));
        for b in bindings() {
            let in_group = match b.scope {
                Scope::Global => gname == "GLOBAL",
                Scope::OverlayDoc => gname == "FOCUSED",
                _ => false,
            };
            if !in_group {
                continue;
            }
            text.push_str(&format!("  {:<14} {:<18} — {}\n", b.key.label(), b.label, b.desc));
        }
    }
    // Focus-scoped bindings grouped by pane.
    for focus in [Focus::Remotes, Focus::Branches, Focus::Files, Focus::Detail, Focus::Graph] {
        let pane_name = match focus {
            Focus::Remotes => "REMOTES",
            Focus::Branches => "BRANCHES",
            Focus::Files => "FILES",
            Focus::Detail => "DETAIL",
            Focus::Graph => "GRAPH",
        };
        text.push_str(&format!("\n══ {} ══\n", pane_name));
        for b in bindings() {
            if let Scope::Focus(f) = b.scope {
                if f == focus {
                    text.push_str(&format!("  {:<14} {:<18} — {}\n", b.key.label(), b.label, b.desc));
                }
            }
        }
    }
    text.push_str("\nTip: press ^+P to run any action by name.\n");

    let p = Paragraph::new(text)
        .block(Block::default().title(" Cheatsheet ").borders(Borders::ALL).border_style(Style::default().fg(VIBRANT_PINK)))
        .style(Style::default().fg(CREAM))
        .scroll((scroll, 0));
    f.render_widget(ratatui::widgets::Clear, area);
    f.render_widget(p, area);
}

fn render_palette(f: &mut Frame, value: &str, selected: usize, _filtered: &Vec<usize>) {
    let area = centered_rect(75, 18, f.area());
    let all = bindings();
    let filtered_indices: Vec<usize> = if value.trim().is_empty() {
        (0..all.len()).collect()
    } else {
        all.iter()
            .enumerate()
            .filter(|(_, b)| {
                let q = value.to_lowercase();
                b.label.to_lowercase().contains(&q)
                    || b.desc.to_lowercase().contains(&q)
                    || b.key.label().to_lowercase().contains(&q)
            })
            .map(|(i, _)| i)
            .collect()
    };
    let sel = selected.min(filtered_indices.len().saturating_sub(1));
    let mut text = format!("Command palette — type to filter, Enter to run, Esc to close\n> {}\u{2588}\n\n", value);
    let start = sel.saturating_sub(12);
    for (row, &idx) in filtered_indices.iter().enumerate().skip(start).take(15) {
        let b = &all[idx];
        let marker = if row == sel { ">> " } else { "   " };
        text.push_str(&format!("{} {:<4} {:<18} — {}\n", marker, b.key.label(), b.label, b.desc));
    }
    let p = Paragraph::new(text)
        .block(Block::default().title(" Command Palette ").borders(Borders::ALL).border_style(Style::default().fg(GREEN)))
        .style(Style::default().fg(CREAM));
    f.render_widget(ratatui::widgets::Clear, area);
    f.render_widget(p, area);
}

/// Dim the whole frame (frosted look) and draw a centered modal on top.
fn glass_modal(f: &mut Frame, width: u16, height: u16, title: &str, text: String, color: Color) {
    let full = f.area();
    let buf = f.buffer_mut();
    for x in full.left()..full.right() {
        for y in full.top()..full.bottom() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.bg = blend_color(cell.bg, Color::Rgb(12, 12, 22), 0.55);
            }
        }
    }
    let area = centered_rect(width, height, full);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(color));
    let p = Paragraph::new(text)
        .block(block)
        .style(Style::default().fg(CREAM).bg(Color::Rgb(25, 25, 38)))
        .wrap(Wrap { trim: false });
    f.render_widget(ratatui::widgets::Clear, area);
    f.render_widget(p, area);
}

fn blend_color(a: Color, b: Color, t: f64) -> Color {
    let rgb = |c: Color| -> Option<(u8, u8, u8)> {
        match c {
            Color::Rgb(r, g, b) => Some((r, g, b)),
            Color::Black => Some((0, 0, 0)),
            Color::White => Some((255, 255, 255)),
            _ => None,
        }
    };
    match (rgb(a), rgb(b)) {
        (Some((r1, g1, b1)), Some((r2, g2, b2))) => {
            let mix = |x: u8, y: u8| (x as f64 * (1.0 - t) + y as f64 * t) as u8;
            Color::Rgb(mix(r1, r2), mix(g1, g2), mix(b1, b2))
        }
        _ => b,
    }
}

fn render_heatmap(f: &mut Frame, state: &AppState) {
    let area = centered_rect(80, 20, f.area());
    let mut text = String::new();
    text.push_str("Commit activity heatmap (local time, last ~5000 commits)\n\n");
    let counts: [u32; 168] = match state.activity {
        Some(c) => c,
        None => {
            text.push_str("Loading…");
            let p = Paragraph::new(text).block(Block::default().title(" Heatmap ").borders(Borders::ALL).border_style(Style::default().fg(ORANGE)))
                .style(Style::default().fg(CREAM));
            f.render_widget(ratatui::widgets::Clear, area);
            f.render_widget(p, area);
            return;
        }
    };
    let max = counts.iter().copied().max().unwrap_or(1).max(1);
    let days = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    text.push_str("         0         1         2         3\n");
    text.push_str("   0123456789012345678901234567890123456789\n");
    for (wd, day) in days.iter().enumerate() {
        text.push_str(&format!(" {} ", day));
        for hour in 0..24 {
            let idx = wd * 24 + hour;
            let c = counts[idx];
            let level = (c as f64 / max as f64).sqrt();
            let ch = if c == 0 {
                '·'
            } else if level < 0.2 {
                '░'
            } else if level < 0.4 {
                '▒'
            } else if level < 0.7 {
                '▓'
            } else {
                '█'
            };
            let _ = level;
            text.push(ch);
        }
        text.push('\n');
    }
    text.push_str("\n  · none   ░ low   ▒ med   ▓ high   █ peak   (highest hour: ");
    let mut best = (0u32, 0usize);
    for (i, &c) in counts.iter().enumerate() {
        if c > best.0 {
            best = (c, i);
        }
    }
    text.push_str(&format!("{} commits at weekday {} hour {})\n", best.0, best.1 / 24, best.1 % 24));
    text.push_str("\n[Esc] close   [r] refresh");
    let p = Paragraph::new(text)
        .block(Block::default().title(" Heatmap ").borders(Borders::ALL).border_style(Style::default().fg(ORANGE)))
        .style(Style::default().fg(CREAM));
    f.render_widget(ratatui::widgets::Clear, area);
    f.render_widget(p, area);
}

fn render_graph_full(f: &mut Frame, state: &AppState, all: bool, scroll: u16) {
    let area = centered_rect(90, 40, f.area());
    let text = state
        .graph_lines
        .iter()
        .map(|gl| gl.text.clone())
        .collect::<Vec<_>>()
        .join("\n");
    let title = if all { " Git Graph (all refs) " } else { " Git Graph " };
    let p = Paragraph::new(text)
        .block(Block::default().title(title).borders(Borders::ALL).border_style(Style::default().fg(MAUVE)))
        .style(Style::default().fg(CREAM))
        .scroll((scroll, 0));
    f.render_widget(ratatui::widgets::Clear, area);
    f.render_widget(p, area);
}

fn render_file_history(f: &mut Frame, state: &AppState, path: &str, selected: usize) {
    let area = centered_rect(78, 20, f.area());
    let items = &state.file_history_cache;
    let mut text = format!("File history for {}\n\n", path);
    let sel = selected.min(items.len().saturating_sub(1));
    for (i, c) in items.iter().enumerate() {
        let marker = if i == sel { ">> " } else { "   " };
        text.push_str(&format!("{} {}  {}  {}  {}\n", marker, c.short_id, c.author, crate::git::format_timestamp(c.author_date), c.message));
    }
    if items.is_empty() {
        text.push_str("(no history)");
    }
    let p = Paragraph::new(text)
        .block(Block::default().title(" File History (GitLens) ").borders(Borders::ALL).border_style(Style::default().fg(BLUE)))
        .style(Style::default().fg(CREAM));
    f.render_widget(ratatui::widgets::Clear, area);
    f.render_widget(p, area);
}

fn render_line_history(f: &mut Frame, state: &AppState, path: &str, selected: usize) {
    let area = centered_rect(78, 16, f.area());
    let line = 1;
    let items = &state.line_history_cache;
    let mut text = format!("Line history for {} (line {})\n\n", path, line);
    let sel = selected.min(items.len().saturating_sub(1));
    for (i, c) in items.iter().enumerate() {
        let marker = if i == sel { ">> " } else { "   " };
        text.push_str(&format!("{} {}  {}  {}\n", marker, c.short_id, c.author, c.message));
    }
    if items.is_empty() {
        text.push_str("(no history)");
    }
    let p = Paragraph::new(text)
        .block(Block::default().title(" Line History (GitLens) ").borders(Borders::ALL).border_style(Style::default().fg(BLUE)))
        .style(Style::default().fg(CREAM));
    f.render_widget(ratatui::widgets::Clear, area);
    f.render_widget(p, area);
}

fn render_tags(f: &mut Frame, state: &AppState, selected: usize) {
    let area = centered_rect(60, 16, f.area());
    let tags = &state.tags_cache;
    let mut text = format!("Tags  ({})\n\n", tags.len());
    let sel = selected.min(tags.len().saturating_sub(1));
    for (i, (name, sha, msg)) in tags.iter().enumerate() {
        let marker = if i == sel { ">> " } else { "   " };
        text.push_str(&format!("{} {}  {}  {}\n", marker, name, sha, msg));
    }
    text.push_str("\n[c] create   [x] delete (selected)   [Esc] close");
    let p = Paragraph::new(text)
        .block(Block::default().title(" Tags ").borders(Borders::ALL).border_style(Style::default().fg(GREEN)))
        .style(Style::default().fg(CREAM));
    f.render_widget(ratatui::widgets::Clear, area);
    f.render_widget(p, area);
}

fn render_stash(f: &mut Frame, state: &AppState, selected: usize) {
    let area = centered_rect(70, 16, f.area());
    let stashes = &state.stashes_cache;
    let mut text = format!("Stash  ({})\n\n", stashes.len());
    let sel = selected.min(stashes.len().saturating_sub(1));
    for (i, s) in stashes.iter().enumerate() {
        let marker = if i == sel { ">> " } else { "   " };
        text.push_str(&format!("{} {}\n", marker, s));
    }
    text.push_str("\n[s] save   [a] apply   [p] pop   [d] drop   [Esc] close");
    let p = Paragraph::new(text)
        .block(Block::default().title(" Stash ").borders(Borders::ALL).border_style(Style::default().fg(YELLOW)))
        .style(Style::default().fg(CREAM));
    f.render_widget(ratatui::widgets::Clear, area);
    f.render_widget(p, area);
}

fn render_contributors(f: &mut Frame, state: &AppState, selected: usize, _offline: bool) {
    let area = centered_rect(60, 20, f.area());
    let mut text = String::new();
    let list: Vec<Contributor> = state.contributors.clone().unwrap_or_default();
    if list.is_empty() {
        text.push_str("Loading contributors…");
    } else {
        text.push_str(&format!("Contributors ({})\n\n", list.len()));
        let sel = selected.min(list.len().saturating_sub(1));
        for (i, c) in list.iter().enumerate() {
            let marker = if i == sel { ">> " } else { "   " };
            let initials = avatar_initials(&c.login);
            text.push_str(&format!("{} [{}]  {:<20} {}  · {} commits\n", marker, initials, c.login, c.name, c.contributions));
        }
        text.push_str("\n[Enter]/[p] profile   [Esc] close");
    }
    let p = Paragraph::new(text)
        .block(Block::default().title(" Contributors ").borders(Borders::ALL).border_style(Style::default().fg(GREEN)))
        .style(Style::default().fg(CREAM));
    f.render_widget(ratatui::widgets::Clear, area);
    f.render_widget(p, area);
}

fn avatar_initials(login: &str) -> String {
    let clean: String = login
        .chars()
        .filter(|c| c.is_alphanumeric())
        .take(2)
        .collect();
    if clean.len() >= 2 {
        clean.to_uppercase()
    } else {
        login.chars().take(1).collect::<String>().to_uppercase()
    }
}

fn render_profile(f: &mut Frame, state: &AppState, login: &str, loaded: bool) {
    let mut text = String::new();
    if !loaded {
        text.push_str(&format!("Loading {}…", login));
    } else if let Some(p) = &state.profile {
        text.push_str(&format!("{} {}  [{}]\n", avatar_initials(&p.login), p.name, p.login));
        if !p.bio.is_empty() {
            text.push_str(&format!("\n  {}\n", p.bio));
        }
        if !p.location.is_empty() {
            text.push_str(&format!("\n  Location: {}\n", p.location));
        }
        text.push_str(&format!(
            "\n  Followers: {}   Following: {}   Public repos: {}\n",
            p.followers, p.following, p.public_repos
        ));
        if !p.html_url.is_empty() {
            text.push_str(&format!("\n  {}\n", p.html_url));
        }
        text.push_str("\n[b] open in browser   [Esc] close");
    } else {
        text.push_str("Profile unavailable (is gh installed & authenticated?).");
    }
    glass_modal(f, 62, 16, &format!(" GitHub Profile — {} ", login), text, GREEN);
}

fn render_prs(f: &mut Frame, state: &AppState, selected: usize, pr_state: &str, filter: &str) {
    let area = centered_rect(75, 22, f.area());
    let mut text = String::new();
    let list: Vec<PrSummary> = state.prs.clone().unwrap_or_default();
    if list.is_empty() && state.prs.is_none() {
        text.push_str("Loading pull requests…");
    } else {
        let filtered: Vec<&PrSummary> = if filter.trim().is_empty() {
            list.iter().collect()
        } else {
            list.iter().filter(|p| p.title.to_lowercase().contains(&filter.to_lowercase())).collect()
        };
        text.push_str(&format!("Pull requests  [{}]  ({})\n\n", pr_state, filtered.len()));
        let sel = selected.min(filtered.len().saturating_sub(1));
        for (i, p) in filtered.iter().enumerate() {
            let marker = if i == sel { ">> " } else { "   " };
            let badge = pr_badge(p);
            text.push_str(&format!("{} {} {}  {}  by {}\n", marker, badge, p.number, p.title, p.author));
        }
        text.push_str("\n[1] open  [2] closed  [3] merged   [/] filter   [Enter] detail   [Esc] close");
    }
    let p = Paragraph::new(text)
        .block(Block::default().title(" Pull Requests ").borders(Borders::ALL).border_style(Style::default().fg(VIBRANT_PINK)))
        .style(Style::default().fg(CREAM));
    f.render_widget(ratatui::widgets::Clear, area);
    f.render_widget(p, area);
}

fn pr_badge(p: &PrSummary) -> String {
    if p.is_draft {
        "DRAFT".to_string()
    } else {
        match p.state.as_str() {
            "OPEN" => "OPEN".to_string(),
            "MERGED" => "MERGED".to_string(),
            "CLOSED" => "CLOSED".to_string(),
            _ => p.state.clone(),
        }
    }
}

fn render_pr_detail(f: &mut Frame, state: &AppState, number: u32, tab: PrTab) {
    let mut text = String::new();
    let detail = state.pr_detail.clone();
    match (&detail, tab) {
        (None, _) => text.push_str(&format!("Loading PR #{}…", number)),
        (Some(d), PrTab::Overview) => {
            let badge = if d.is_draft { "DRAFT" } else { &d.state };
            text.push_str(&format!("#{} {}  [{}]\n", d.number, d.title, badge));
            text.push_str(&format!("  by {} · {} → {}\n", d.author, d.base, d.head));
            text.push_str(&format!("  created {} · updated {}\n", d.created_at, d.updated_at));
            if let Some(m) = &d.milestone {
                text.push_str(&format!("  milestone: {}\n", m));
            }
            if !d.labels.is_empty() {
                text.push_str(&format!("  labels/scope: {}\n", d.labels.join(", ")));
            }
            if let Some(scope) = &d.scope {
                text.push_str(&format!("  title scope: {}\n", scope));
            }
            if !d.assignees.is_empty() {
                text.push_str(&format!("  assigned to: {}\n", d.assignees.join(", ")));
            }
            if !d.reviewers.is_empty() {
                let rev: Vec<String> = d.reviewers.iter().map(|(l, s)| format!("{} ({})", l, s)).collect();
                text.push_str(&format!("  reviewers: {}\n", rev.join(", ")));
            }
            text.push_str(&format!(
                "  mergeable: {} · merge state: {}\n",
                d.mergeable, d.merge_state
            ));
            text.push_str("\n── Description ──\n");
            text.push_str(if d.body.is_empty() { "(no description)" } else { &d.body });
            text.push_str("\n\n[c] commits  [f] files  [m] merge  [x] close  [r] reopen  [k] review  [d] comment  [o] web  [E] edit  [l] labels  [s] milestone  [@] reviewers  [Esc] close");
        }
        (Some(d), PrTab::Commits) => {
            text.push_str(&format!("PR #{} — commits\n\n", d.number));
            for c in &d.commits {
                text.push_str(&format!("{}  {}  {}\n", &c.oid[..8.min(c.oid.len())], c.author, c.message));
            }
            text.push_str("\n[o] overview   [f] files   [Esc] close");
        }
        (Some(d), PrTab::Files) => {
            text.push_str(&format!("PR #{} — file changes\n\n", d.number));
            if state.pr_files.is_empty() {
                text.push_str("(loading files…)");
            }
            for f in &state.pr_files {
                text.push_str(&format!("{}  {}  +{}/-{}\n", f.status, f.path, f.additions, f.deletions));
            }
            text.push_str("\n[o] overview   [c] commits   [v] show diff in detail pane   [Esc] close");
        }
    }
    glass_modal(f, 88, 34, &format!(" Pull Request #{} ", number), text, VIBRANT_PINK);
}

/// Draw a passive idle tip box at the bottom of a pane.
fn draw_pane_tip(f: &mut Frame, area: Rect, text: &str) {
    let width = area.width.saturating_sub(2);
    if width < 10 {
        return;
    }
    let tip_area = Rect::new(area.x + 1, area.bottom().saturating_sub(3).max(area.y + 1), width.min(text.len() as u16), 1);
    let p = Paragraph::new(text)
        .block(Block::default().borders(Borders::NONE).style(Style::default().fg(Color::Black).bg(YELLOW)))
        .style(Style::default().fg(Color::Black).bg(YELLOW));
    f.render_widget(p, tip_area);
}

/// Build a hover preview for the focused pane's selected item (idle previews).
fn hover_text(state: &AppState) -> Option<String> {
    if !state.gui.idle_previews {
        return None;
    }
    match state.focus {
        Focus::Files => {
            let p = state.selected_file_path()?;
            let f = state.files.iter().find(|f| f.path == p)?;
            let mut s = format!("{}  {}|{}", p, f.staged, f.unstaged);
            // heat bar from blame recency, when blame already loaded for this file
            if state.blame_path == p && !state.blame.is_empty() {
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
                let oldest = state.blame.iter().map(|b| b.epoch).filter(|&e| e > 0).min().unwrap_or(now);
                let span = (now - oldest).max(1);
                let recent = state.blame.iter().filter(|b| b.epoch > now - span / 3).count();
                let pct = recent as f64 / state.blame.len().max(1) as f64;
                s.push_str(&format!("  [recently-edited {:.0}%]", pct * 100.0));
            }
            Some(s)
        }
        Focus::Branches => {
            let b = state.selected_branch_name()?;
            let info = state.repo.branch_info(&b).unwrap_or_default();
            let mut s = if info.name.is_empty() { b } else { info.name.clone() };
            if !info.subject.is_empty() {
                s.push_str(&format!("  — {}", info.subject));
            }
            if info.ahead > 0 || info.behind > 0 {
                s.push_str(&format!("  (ahead {} / behind {})", info.ahead, info.behind));
            }
            Some(s)
        }
        Focus::Remotes => {
            let r = state.remote_state.selected().and_then(|i| state.remotes.get(i))?;
            Some(format!("{}: {}", r.name, r.url))
        }
        Focus::Graph => {
            let idx = state.graph_state.selected()?;
            let gl = state.graph_lines.get(idx)?;
            if gl.is_commit {
                Some(gl.text.to_string())
            } else {
                None
            }
        }
        Focus::Detail => None,
    }
}

/// Draw the passive idle tip (bottom) and hover preview (top) inside a pane
/// when the idle engine has armed them for the focused pane.
fn maybe_tip_and_hover(f: &mut Frame, state: &AppState, area: Rect) {
    if !state.tip_visible {
        return;
    }
    if !matches!(state.overlay, Overlay::None) {
        return;
    }
    if let Some(tip) = state.focus_tip() {
        draw_pane_tip(f, area, &tip);
    }
    if let Some(hover) = hover_text(state) {
        let width = area.width.saturating_sub(2).min(hover.len() as u16).max(10);
        let hover_area = Rect::new(area.x + 1, area.y + 1, width, 1);
        let p = Paragraph::new(hover)
            .style(Style::default().fg(CREAM).bg(Color::Rgb(45, 45, 60)));
        f.render_widget(p, hover_area);
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
                state.tip_visible = false;
                if handle_overlay(state, key) {
                    return Ok(false);
                }
                if let Some(k) = parse_key(&key) {
                    dispatch(state, k);
                    if state.quit {
                        return Ok(true);
                    }
                }
            }
        }
    }
    Ok(false)
}

/// Look up a key in the binding registry. Focus-scoped bindings take
/// precedence over global ones (e.g. `R` = rename remote in Remotes, but
/// revert elsewhere).
fn dispatch(state: &mut AppState, key: Key) {
    let focus = state.focus;
    let bindings = bindings();
    for b in &bindings {
        if b.doc_only {
            continue;
        }
        if let Scope::Focus(f) = b.scope {
            if f == focus && b.key == key {
                (b.handler)(state);
                return;
            }
        }
    }
    for b in &bindings {
        if b.doc_only {
            continue;
        }
        if b.scope == Scope::Global && b.key == key {
            (b.handler)(state);
            return;
        }
    }
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
         // ---- Help / palette ----
         Overlay::Help { scroll } => {
             match code {
                 KeyCode::Up | KeyCode::Char('k') => { *scroll = scroll.saturating_sub(1); }
                 KeyCode::Down | KeyCode::Char('j') => { *scroll = scroll.saturating_add(1); }
                 KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') => state.overlay = Overlay::None,
                 _ => {}
             }
         }
         Overlay::Palette { value, selected, filtered } => {
             let all = bindings();
             let refresh_filter = |filtered: &mut Vec<usize>, value: &str| {
                 *filtered = if value.trim().is_empty() {
                     (0..all.len()).collect()
                 } else {
                     all.iter().enumerate()
                         .filter(|(_, b)| {
                             let q = value.to_lowercase();
                             b.label.to_lowercase().contains(&q)
                                 || b.desc.to_lowercase().contains(&q)
                                 || b.key.label().to_lowercase().contains(&q)
                         })
                         .map(|(i, _)| i)
                         .collect()
                 };
             };
             match code {
                 KeyCode::Enter => {
                     let idx = if filtered.is_empty() {
                         refresh_filter(filtered, value);
                         filtered.first().copied()
                     } else {
                         let sel = (*selected).min(filtered.len().saturating_sub(1));
                         filtered.get(sel).copied()
                     };
                     if let Some(bi) = idx {
                         if let Some(b) = all.get(bi) {
                             if !b.doc_only {
                                 let handler = b.handler;
                                 state.overlay = Overlay::None;
                                 handler(state);
                             }
                         }
                     }
                 }
                 KeyCode::Up | KeyCode::Char('k') => {
                     let n = filtered.len();
                     if n > 0 { *selected = (*selected).saturating_sub(1) % n; }
                 }
                 KeyCode::Down | KeyCode::Char('j') => {
                     let n = filtered.len();
                     if n > 0 { *selected = (*selected + 1) % n; }
                 }
                 KeyCode::Char(c) => { value.push(c); refresh_filter(filtered, value); }
                 KeyCode::Backspace => { value.pop(); refresh_filter(filtered, value); }
                 KeyCode::Esc => state.overlay = Overlay::None,
                 _ => {}
             }
         }
         // ---- Visualization modals ----
         Overlay::Heatmap => {
             match code {
                 KeyCode::Char('r') => state.submit_job(UiJob::LoadActivity, true),
                 KeyCode::Esc | KeyCode::Char('q') => state.overlay = Overlay::None,
                 _ => {}
             }
         }
         Overlay::GraphFull { all, scroll } => {
             match code {
                 KeyCode::Char('a') => {
                     *all = !*all;
                     state.graph_all = *all;
                     let lines = state.repo.log_graph(*all, 400).unwrap_or_default();
                     state.graph_lines = lines.iter().filter_map(|l| parse_graph_line(l)).collect();
                     *scroll = 0;
                 }
                 KeyCode::Up | KeyCode::Char('k') => { *scroll = scroll.saturating_sub(1); }
                 KeyCode::Down | KeyCode::Char('j') => { *scroll = scroll.saturating_add(1); }
                 KeyCode::Esc | KeyCode::Char('q') => state.overlay = Overlay::None,
                 _ => {}
             }
         }
         Overlay::FileHistory { path, selected } => {
             match code {
                 KeyCode::Up | KeyCode::Char('k') => { *selected = selected.saturating_sub(1); }
                 KeyCode::Down | KeyCode::Char('j') => { *selected = selected.saturating_add(1); }
                 KeyCode::Esc | KeyCode::Char('q') => state.overlay = Overlay::None,
                 _ => { let _ = path; }
             }
         }
         Overlay::LineHistory { path, selected } => {
             match code {
                 KeyCode::Up | KeyCode::Char('k') => { *selected = selected.saturating_sub(1); }
                 KeyCode::Down | KeyCode::Char('j') => { *selected = selected.saturating_add(1); }
                 KeyCode::Esc | KeyCode::Char('q') => state.overlay = Overlay::None,
                 _ => { let _ = path; }
             }
         }
         Overlay::Tags { selected } => {
             match code {
                 KeyCode::Up | KeyCode::Char('k') => { *selected = selected.saturating_sub(1); }
                 KeyCode::Down | KeyCode::Char('j') => { *selected = selected.saturating_add(1); }
                 KeyCode::Char('c') => {
                     state.overlay = Overlay::Prompt { title: "Create Tag (name)".to_string(), value: String::new(), action: PromptAction::AddTag };
                 }
                 KeyCode::Char('x') => {
                     if let Some((name, _, _)) = state.repo.tag_detail().unwrap_or_default().get(*selected) {
                         let name = name.clone();
                         state.overlay = Overlay::ConfirmDangerous { title: "Delete Tag".to_string(), prompt: format!("Delete tag '{}'?", name), action: DangerousAction::DeleteTag { name } };
                     }
                 }
                 KeyCode::Esc | KeyCode::Char('q') => state.overlay = Overlay::None,
                 _ => {}
             }
         }
         Overlay::Stash { selected } => {
             match code {
                 KeyCode::Up | KeyCode::Char('k') => { *selected = selected.saturating_sub(1); }
                 KeyCode::Down | KeyCode::Char('j') => { *selected = selected.saturating_add(1); }
                 KeyCode::Char('s') => {
                     state.overlay = Overlay::Prompt { title: "Stash (message)".to_string(), value: String::new(), action: PromptAction::StashSave };
                 }
                 KeyCode::Char('a') => {
                     let i = *selected;
                     state.submit_job(UiJob::StashApply { index: i }, false);
                     state.overlay = Overlay::None;
                 }
                 KeyCode::Char('p') => {
                     state.submit_job(UiJob::StashApply { index: 0 }, false);
                     state.overlay = Overlay::None;
                 }
                 KeyCode::Char('d') => {
                     let i = *selected;
                     state.overlay = Overlay::ConfirmDangerous { title: "Drop Stash".to_string(), prompt: format!("Drop stash@{{{}}}?", i), action: DangerousAction::StashDrop { index: i } };
                 }
                 KeyCode::Esc | KeyCode::Char('q') => state.overlay = Overlay::None,
                 _ => {}
             }
         }
         Overlay::Worktree { .. } => {
             if matches!(code, KeyCode::Esc | KeyCode::Char('q')) {
                 state.overlay = Overlay::None;
             }
         }
         // ---- GitHub ----
         Overlay::Contributors { selected, offline } => {
             match code {
                 KeyCode::Up | KeyCode::Char('k') => { *selected = selected.saturating_sub(1); }
                 KeyCode::Down | KeyCode::Char('j') => { *selected = selected.saturating_add(1); }
                 KeyCode::Enter | KeyCode::Char('p') => {
                     if let Some(list) = &state.contributors {
                         if let Some(c) = list.get(*selected) {
                             let login = c.login.clone();
                             state.open_profile(login);
                         }
                     }
                 }
                 KeyCode::Char('r') => {
                     *offline = false;
                     state.contributors = None;
                     state.submit_job(UiJob::LoadContributors, true);
                 }
                 KeyCode::Esc | KeyCode::Char('q') => state.overlay = Overlay::None,
                 _ => {}
             }
         }
         Overlay::Profile { login, loaded } => {
             match code {
                 KeyCode::Char('b') => {
                     let url = state
                         .profile
                         .as_ref()
                         .map(|p| p.html_url.clone())
                         .filter(|u| !u.is_empty())
                         .unwrap_or_else(|| format!("https://github.com/{}", login));
                     state.submit_job(UiJob::OpenUrl { url }, false);
                 }
                 KeyCode::Esc | KeyCode::Char('q') => state.overlay = Overlay::None,
                 _ => { let _ = loaded; }
             }
         }
         Overlay::Prs { selected, state: pr_state, filter } => {
             match code {
                 KeyCode::Up | KeyCode::Char('k') => { *selected = selected.saturating_sub(1); }
                 KeyCode::Down | KeyCode::Char('j') => { *selected = selected.saturating_add(1); }
                 KeyCode::Char('1') => { *pr_state = "open".to_string(); state.prs = None; state.submit_job(UiJob::LoadPrs { state: "open".to_string() }, true); }
                 KeyCode::Char('2') => { *pr_state = "closed".to_string(); state.prs = None; state.submit_job(UiJob::LoadPrs { state: "closed".to_string() }, true); }
                 KeyCode::Char('3') => { *pr_state = "merged".to_string(); state.prs = None; state.submit_job(UiJob::LoadPrs { state: "merged".to_string() }, true); }
                 KeyCode::Char('/') => {
                     state.overlay = Overlay::Prompt { title: "Filter PRs".to_string(), value: filter.clone(), action: PromptAction::PrFilter };
                 }
                 KeyCode::Enter => {
                     if let Some(list) = &state.prs {
                         let f = filter.trim().to_lowercase();
                         let matches: Vec<&PrSummary> = list.iter().filter(|p| f.is_empty() || p.title.to_lowercase().contains(&f)).collect();
                         if let Some(p) = matches.get(*selected) {
                             let num = p.number;
                             state.open_pr_detail(num);
                         }
                     }
                 }
                 KeyCode::Esc | KeyCode::Char('q') => state.overlay = Overlay::None,
                 _ => {}
             }
         }
         Overlay::PrDetail { number, tab } => {
             match code {
                 KeyCode::Tab | KeyCode::Char('n') => {
                     *tab = match tab {
                         PrTab::Overview => PrTab::Commits,
                         PrTab::Commits => PrTab::Files,
                         PrTab::Files => PrTab::Overview,
                     };
                 }
                 KeyCode::Char('c') => *tab = PrTab::Commits,
                 KeyCode::Char('f') => *tab = PrTab::Files,
                 KeyCode::Char('v') => {
                     let num = *number;
                     state.submit_job(UiJob::PrShow { number: num, show_diff: true }, false);
                     state.overlay = Overlay::None;
                 }
                 KeyCode::Char('m') => {
                     state.overlay = Overlay::Prompt { title: format!("Merge PR #{}", number).to_string(), value: "squash".to_string(), action: PromptAction::PrMergeStrategy { number: *number } };
                 }
                 KeyCode::Char('x') => {
                     state.overlay = Overlay::Prompt { title: format!("Close PR #{} (comment?)", number).to_string(), value: String::new(), action: PromptAction::PrClose { number: *number } };
                 }
                 KeyCode::Char('r') => {
                     let num = *number;
                     state.submit_job(UiJob::PrReopen { number: num }, false);
                     state.overlay = Overlay::None;
                 }
                 KeyCode::Char('k') => {
                     state.overlay = Overlay::Prompt { title: format!("Review PR #{} (approve/changes/comment)", number).to_string(), value: "approve".to_string(), action: PromptAction::PrReview { number: *number, verdict: "approve".to_string() } };
                 }
                 KeyCode::Char('d') => {
                     state.overlay = Overlay::Prompt { title: format!("Comment on PR #{}", number).to_string(), value: String::new(), action: PromptAction::PrComment { number: *number } };
                 }
                 KeyCode::Char('o') | KeyCode::Char('w') => {
                     let num = *number;
                     state.submit_job(UiJob::PrOpenWeb { number: num }, false);
                     state.overlay = Overlay::None;
                 }
                 KeyCode::Char('E') => {
                     state.overlay = Overlay::Prompt { title: format!("Edit PR #{} (title)", number).to_string(), value: String::new(), action: PromptAction::PrEdit { number: *number, field: "title".to_string() } };
                 }
                 KeyCode::Char('l') => {
                     state.overlay = Overlay::Prompt { title: format!("Add labels to PR #{}", number).to_string(), value: String::new(), action: PromptAction::PrAddLabels { number: *number } };
                 }
                 KeyCode::Char('s') => {
                     state.overlay = Overlay::Prompt { title: format!("Set milestone for PR #{}", number).to_string(), value: String::new(), action: PromptAction::PrMilestone { number: *number } };
                 }
                 KeyCode::Char('@') => {
                     state.overlay = Overlay::Prompt { title: format!("Request reviewers for PR #{}", number).to_string(), value: String::new(), action: PromptAction::PrReviewers { number: *number } };
                 }
                 KeyCode::Char('=') => {
                     state.overlay = Overlay::Prompt { title: format!("Assign users to PR #{}", number).to_string(), value: String::new(), action: PromptAction::PrAssignees { number: *number } };
                 }
                 KeyCode::Char('C') => {
                     let num = *number;
                     state.submit_job(UiJob::PrCheckout { number: num }, false);
                     state.overlay = Overlay::None;
                 }
                 KeyCode::Esc | KeyCode::Char('q') => state.overlay = Overlay::None,
                 _ => {}
             }
         }
         // ---- Generic prompt / confirm ----
         Overlay::Prompt { title: _, value, action } => {
             let action = action.clone();
             let input = value.trim().to_string();
             match code {
                 KeyCode::Enter => {
                     let action_ref = action;
                     match action_ref {
                         PromptAction::PrComment { number } => {
                             state.submit_job(UiJob::PrComment { number, body: input }, false);
                         }
                         PromptAction::PrReview { number, verdict } => {
                             let v = if input.trim().is_empty() { verdict } else { input.trim().to_string() };
                             state.submit_job(UiJob::PrReview { number, verdict: v, body: None }, false);
                         }
                         PromptAction::PrMergeStrategy { number } => {
                             let strategy = if input.trim().is_empty() { "squash".to_string() } else { input.trim().to_string() };
                             state.overlay = Overlay::ConfirmDangerous { title: format!("Merge PR #{}", number), prompt: format!("Merge PR #{} with '{}'?", number, strategy), action: DangerousAction::PrMerge { number, strategy, delete_branch: false } };
                             return true;
                         }
                         PromptAction::PrClose { number } => {
                             let comment = if input.trim().is_empty() { None } else { Some(input.trim().to_string()) };
                             state.submit_job(UiJob::PrClose { number, comment }, false);
                         }
                         PromptAction::PrEdit { number, field } => {
                             if field == "body" {
                                 state.submit_job(UiJob::PrEdit { number, title: None, body: Some(input) }, false);
                             } else {
                                 state.submit_job(UiJob::PrEdit { number, title: Some(input), body: None }, false);
                             }
                         }
                         PromptAction::PrAddLabels { number } => {
                             state.submit_job(UiJob::PrEditList { number, flag: "--add-label".to_string(), values: vec![input] }, false);
                         }
                         PromptAction::PrMilestone { number } => {
                             state.submit_job(UiJob::PrEditList { number, flag: "--milestone".to_string(), values: vec![input] }, false);
                         }
                         PromptAction::PrReviewers { number } => {
                             state.submit_job(UiJob::PrEditList { number, flag: "--add-reviewer".to_string(), values: vec![input] }, false);
                         }
                         PromptAction::PrAssignees { number } => {
                             state.submit_job(UiJob::PrEditList { number, flag: "--add-assignee".to_string(), values: vec![input] }, false);
                         }
                         PromptAction::PrFilter => {
                             if let Overlay::Prs { filter, .. } = &mut state.overlay {
                                 *filter = input;
                             }
                         }
                         PromptAction::RebaseOnto => {
                             state.submit_job(UiJob::RebaseOnto { onto: input }, false);
                         }
                         PromptAction::ShowRef => {
                             let text = state.repo.git_show(&input).unwrap_or_else(|e| format!("Error: {}", e));
                             state.overlay = Overlay::Message { text, is_error: false };
                             return true;
                         }
                         PromptAction::GitMv { from } => {
                             state.submit_job(UiJob::GitMv { from, to: input }, false);
                         }
                         PromptAction::AddTag => {
                             let name = input.trim().to_string();
                             if !name.is_empty() {
                                 match state.repo.create_tag(&name, "HEAD", None) {
                                     Ok(()) => {
                                         state.tags_cache = state.repo.tag_detail().unwrap_or_default();
                                         state.log(format!("Created tag '{}'", name));
                                     }
                                     Err(e) => state.log(format!("Tag create failed: {}", e)),
                                 }
                             }
                         }
                         PromptAction::StashSave => {
                             let msg = if input.trim().is_empty() { None } else { Some(input.trim().to_string()) };
                             state.submit_job(UiJob::StashSave { message: msg }, false);
                         }
                     }
                     state.overlay = Overlay::None;
                 }
                 KeyCode::Char(c) => value.push(c),
                 KeyCode::Backspace => { value.pop(); }
                 KeyCode::Esc => state.overlay = Overlay::None,
                 _ => {}
             }
         }
         Overlay::ConfirmDangerous { title: _, prompt: _, action } => {
             if matches!(code, KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter) {
                 let action = action.clone();
                 match action {
                     DangerousAction::GitClean => state.submit_job(UiJob::GitClean, false),
                     DangerousAction::GitRm { path } => state.submit_job(UiJob::GitRm { path }, false),
                     DangerousAction::StashDrop { index } => state.submit_job(UiJob::StashDrop { index }, false),
                     DangerousAction::DeleteTag { name } => state.submit_job(UiJob::DeleteTag { name }, false),
                     DangerousAction::PrMerge { number, strategy, delete_branch } => {
                         state.submit_job(UiJob::PrMerge { number, strategy, delete_branch }, false);
                     }
                 }
                 state.overlay = Overlay::None;
             } else if matches!(code, KeyCode::Char('n') | KeyCode::Esc) {
                 state.overlay = Overlay::None;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};

    #[test]
    fn no_duplicate_bindings_per_scope() {
        let mut seen = std::collections::HashSet::new();
        for b in bindings() {
            if b.doc_only {
                continue;
            }
            assert!(
                seen.insert((b.scope, b.key)),
                "duplicate binding key={:?} scope={:?} ({})",
                b.key,
                b.scope,
                b.label
            );
        }
    }

    #[test]
    fn parse_key_normalizes_shift_and_ctrl() {
        let shift_m = crossterm::event::KeyEvent::new(KeyCode::Char('m'), KeyModifiers::SHIFT);
        assert_eq!(parse_key(&shift_m), Some(Key::Char('M')));
        let ctrl_p = crossterm::event::KeyEvent::new(KeyCode::Char('P'), KeyModifiers::CONTROL);
        assert_eq!(parse_key(&ctrl_p), Some(Key::CtrlChar('p')));
        let plain_g = crossterm::event::KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE);
        assert_eq!(parse_key(&plain_g), Some(Key::Char('g')));
        let enter = crossterm::event::KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(parse_key(&enter), Some(Key::Enter));
    }

    #[test]
    fn avatar_initials_uppercased() {
        assert_eq!(avatar_initials("chara7"), "CH");
        assert_eq!(avatar_initials("a"), "A");
        assert_eq!(avatar_initials("abc"), "AB");
    }
}
