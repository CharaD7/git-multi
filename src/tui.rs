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
use std::path::Path;
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

/// Detect the device (host) name and system username cross-platform.
///
/// Order: environment variables first, then the platform command
/// (`hostname` / `whoami`) via the captured-output helper with a short timeout,
/// so neither can hang the UI. On Windows `whoami` emits `DOMAIN\user`, which
/// is trimmed to the part after the backslash.
fn system_identity() -> (String, String) {
    let device = env_or_cmd(&["HOSTNAME", "HOST", "COMPUTERNAME"], "hostname");
    let username = env_or_cmd(&["USER", "LOGNAME", "USERNAME"], "whoami")
        .rsplit('\\')
        .next()
        .unwrap_or("unknown")
        .to_string();
    (device, username)
}

/// Return the first non-empty env var, otherwise the trimmed output of `cmd`
/// (captured, 5s timeout), otherwise "unknown".
fn env_or_cmd(env_keys: &[&str], cmd: &str) -> String {
    for key in env_keys {
        if let Ok(v) = std::env::var(key) {
            let v = v.trim().to_string();
            if !v.is_empty() {
                return v;
            }
        }
    }
    if let Ok(out) = crate::git::run_captured(cmd, &[], Path::new("."), &[], Duration::from_secs(5)) {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !s.is_empty() {
                return s;
            }
        }
    }
    "unknown".to_string()
}

/// Apply `[identity]` config overrides over the auto-detected device/username.
fn apply_identity_overrides(
    device: String,
    username: String,
    prefs: &crate::config::IdentityPreferences,
) -> (String, String) {
    let device = prefs
        .device
        .as_ref()
        .filter(|d| !d.trim().is_empty())
        .map(|d| d.trim().to_string())
        .unwrap_or(device);
    let username = prefs
        .username
        .as_ref()
        .filter(|u| !u.trim().is_empty())
        .map(|u| u.trim().to_string())
        .unwrap_or(username);
    (device, username)
}

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
    // ---- Cross-origin pick ----
    PickSource { filter: String, selected: usize },
    PickBrowse { selected: usize },
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
    PickTarget,
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
            Key::CtrlChar(c) => format!("Ctrl+{}", c),
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
        Binding::action(Scope::Focus(Remotes), Key::Char('v'), "Pick from remote", "Browse a remote branch and cherry-pick its commits", |s| s.open_pick_source()),
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
        Binding::doc(Scope::OverlayDoc, Key::Char('c'), "Copy / no-commit", "Toggle apply-without-commit in the pick overlays"),
        Binding::doc(Scope::OverlayDoc, Key::Char('t'), "Target branch", "Set the pick target branch"),
        Binding::doc(Scope::OverlayDoc, Key::Char('p'), "Push result", "Toggle pushing after the pick"),
        Binding::doc(Scope::OverlayDoc, Key::Char('a'), "Select all", "Toggle all commits in the pick browser"),
        Binding::doc(Scope::OverlayDoc, Key::Char('d'), "Preview diff", "Preview the selected commit's diff"),
    ]
}

struct RemoteEntry {
    name: String,
    url: String,
}

/// State for the cross-origin commit browser (`Overlay::PickBrowse`).
struct PickBrowseState {
    remote: String,
    branch: String,
    items: Vec<CommitSummary>,
    marks: Vec<bool>,
}

/// An in-progress screen transition (welcome -> playground).
struct Transition {
    start: Instant,
    duration: Duration,
}

/// The kind of a rendering animation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnimKind {
    OverlayIn,
    FocusPulse,
    PanelTransition,
    RefreshFlash,
}

/// A single active rendering animation (finished ones are dropped each tick).
struct ActiveAnim {
    start: Instant,
    duration: Duration,
    kind: AnimKind,
    /// The pane a FocusPulse/RefreshFlash applies to (None otherwise).
    focus: Option<Focus>,
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
    // ---- Cross-origin pick ----
    LoadRemoteCommits { remote: String, branch: String },
    PickCommits { specs: Vec<String>, target_branch: String, copy: bool, push_remote: Option<String> },
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
    RemoteCommits { remote: String, branch: String, commits: Vec<CommitSummary> },
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
    // Current-position indicators (branch / HEAD / upstream remote)
    current_branch: Option<String>,
    head_short: String,
    upstream_remote: Option<String>,
    ahead: usize,
    behind: usize,
    // Cross-origin pick state
    pick_browse: Option<PickBrowseState>,
    pick_target: String,
    pick_copy: bool,
    pick_push: bool,
    // Welcome screen / identity
    welcome: bool,
    welcome_start: Instant,
    welcome_button: usize,
    transition: Option<Transition>,
    transition_from_welcome: bool,
    host: String,
    username: String,
    gh_user: Option<String>,
    // Rendering animations
    anims: Vec<ActiveAnim>,
    prev_overlay_none: bool,
    prev_detail_mode: DetailMode,
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
        let (host, username) = {
            let (device, user) = system_identity();
            apply_identity_overrides(device, user, &repo.config.identity)
        };
        let gh_user = crate::github::current_user(&repo);
        let show_welcome = gui.show_welcome;
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
            current_branch: None,
            head_short: String::new(),
            upstream_remote: None,
            ahead: 0,
            behind: 0,
            pick_browse: None,
            pick_target: String::new(),
            pick_copy: false,
            pick_push: false,
            welcome: show_welcome,
            welcome_start: Instant::now(),
            welcome_button: 0,
            transition: None,
            transition_from_welcome: false,
            host,
            username,
            gh_user,
            anims: Vec::new(),
            prev_overlay_none: true,
            prev_detail_mode: DetailMode::Detail,
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
        let prev_remote_n = self.remotes.len();
        let prev_branch_n = self.branches.len();
        let prev_file_n = self.files.len();

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

        // Current-position indicators: active branch, HEAD commit, upstream
        // remote, and ahead/behind (stays fresh on every refresh/checkout).
        self.current_branch = self.repo.current_branch().ok().flatten();
        self.head_short = self
            .repo
            .head_commit()
            .ok()
            .map(|c| {
                let s = c.id().to_string();
                s[..8.min(s.len())].to_string()
            })
            .unwrap_or_default();
        self.ahead = 0;
        self.behind = 0;
        self.upstream_remote = None;
        if let Some(branch) = self.current_branch.clone() {
            if let Ok(info) = self.repo.branch_info(&branch) {
                self.ahead = info.ahead;
                self.behind = info.behind;
            }
            self.upstream_remote = self.repo.upstream_remote(&branch).ok().flatten();
        }
        if self.upstream_remote.is_none() {
            self.upstream_remote = self.repo.config.get_default_remote().cloned();
        }
        if self.pick_target.is_empty() {
            self.pick_target = self.current_branch.clone().unwrap_or_default();
        }

        // Pulse the panes whose data actually changed.
        if self.remotes.len() != prev_remote_n {
            self.push_anim(AnimKind::RefreshFlash, Some(Focus::Remotes));
        }
        if self.branches.len() != prev_branch_n {
            self.push_anim(AnimKind::RefreshFlash, Some(Focus::Branches));
        }
        if self.files.len() != prev_file_n {
            self.push_anim(AnimKind::RefreshFlash, Some(Focus::Files));
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
                JobPayload::RemoteCommits { remote, branch, commits } => {
                    let marks = vec![false; commits.len()];
                    self.pick_browse = Some(PickBrowseState { remote, branch, items: commits, marks });
                    self.overlay = Overlay::PickBrowse { selected: 0 };
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

    /// Poll interval: animate the welcome/transition or any active rendering
    /// animation smoothly, otherwise relax.
    fn poll_delay(&self) -> Duration {
        if self.welcome || self.transition.is_some() || !self.anims.is_empty() {
            Duration::from_millis(16)
        } else {
            Duration::from_millis(100)
        }
    }

    /// Register a rendering animation, honoring the `[animations]` prefs.
    /// Returns whether an animation was actually started.
    fn push_anim(&mut self, kind: AnimKind, focus: Option<Focus>) -> bool {
        let p = &self.repo.config.animations;
        let enabled = match kind {
            AnimKind::OverlayIn => p.enabled && p.overlay,
            AnimKind::FocusPulse => p.enabled && p.focus,
            AnimKind::PanelTransition => p.enabled && p.panel,
            AnimKind::RefreshFlash => p.enabled && p.refresh,
        };
        if !enabled {
            return false;
        }
        let ms = anim_duration_ms(p, kind);
        self.anims.push(ActiveAnim {
            start: Instant::now(),
            duration: Duration::from_millis(ms),
            kind,
            focus,
        });
        true
    }

    /// Eased progress (0..1) of the newest active animation of `kind`.
    /// Returns 1.0 ("settled") when no animation of that kind is active.
    fn anim_progress(&self, kind: AnimKind) -> f64 {
        anim_progress_of(&self.anims, kind)
    }

    /// Fading pulse intensity (1 -> 0) for a FocusPulse/RefreshFlash on `focus`.
    fn pulse_level(&self, kind: AnimKind, focus: Focus) -> f64 {
        self.anims
            .iter()
            .filter(|a| a.kind == kind && a.focus == Some(focus))
            .map(|a| {
                let t = a.start.elapsed().as_secs_f64() / a.duration.as_secs_f64();
                (1.0 - t.clamp(0.0, 1.0)).max(0.0)
            })
            .fold(0.0, f64::max)
    }

    /// Drop finished animations.
    fn prune_anims(&mut self) {
        self.anims.retain(|a| a.start.elapsed() < a.duration);
    }

    /// Leave the welcome screen and bounce into the playground.
    fn start_tool(&mut self) {
        self.welcome = false;
        self.transition = Some(Transition {
            start: Instant::now(),
            duration: Duration::from_millis(900),
        });
        self.transition_from_welcome = true;
    }

    /// Activate the focused welcome button.
    fn activate_welcome_button(&mut self) {
        match self.welcome_button {
            0 | 1 => self.start_tool(), // Continue → / Skip intro
            2 => self.overlay = Overlay::Help { scroll: 0 },
            3 => self.open_palette(),
            4 => {
                // ✓ Don't show again
                self.repo.config.gui.show_welcome = false;
                let _ = self.repo.config.save(&self.repo.repo);
                self.start_tool();
            }
            _ => self.start_tool(),
        }
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

    // ---- Cross-origin pick ----

    /// Open the remote-branch source picker (Remotes pane `v`).
    fn open_pick_source(&mut self) {
        self.overlay = Overlay::PickSource { filter: String::new(), selected: 0 };
    }

    /// Prompt to set the pick target branch (shared by the cherry-pick overlay
    /// and the commit browser).
    fn open_pick_target_prompt(&mut self) {
        self.overlay = Overlay::Prompt {
            title: "Pick target branch (empty = current)".to_string(),
            value: self.pick_target.clone(),
            action: PromptAction::PickTarget,
        };
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
            Focus::Remotes => ("Remotes", vec!["f fetch", "p push", "l pull", "M merge/sync", "a add", "R rename", "x remove", "D default", "v pick from remote", "Enter fetch"]),
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
        // ---- Cross-origin pick ----
        UiJob::LoadRemoteCommits { remote, branch } => {
            let r = repo
                .fetch_remote(&remote)
                .and_then(|_| repo.list_remote_commits(&remote, &branch, 200));
            match r {
                Ok(commits) => JobResult {
                    message: String::new(),
                    refresh: false,
                    payload: JobPayload::RemoteCommits { remote, branch, commits },
                },
                Err(e) => result_err(format!("Could not load {}/{}: {}", remote, branch, e)),
            }
        }
        UiJob::PickCommits { specs, target_branch, copy, push_remote } => {
            let target = if target_branch.is_empty() { None } else { Some(target_branch.as_str()) };
            let r = repo.pick_commits(&specs, copy, target);
            match r {
                Ok(applied) => {
                    let verb = if copy { "Copied" } else { "Cherry-picked" };
                    let mut msg = format!("{} {} commit(s)", verb, applied.len());
                    if let Some(pr) = &push_remote {
                        let branch = if target_branch.is_empty() {
                            repo.current_branch().ok().flatten().unwrap_or_default()
                        } else {
                            target_branch.clone()
                        };
                        match repo.push_branches(pr, std::slice::from_ref(&branch), false) {
                            Ok(()) => msg.push_str(&format!(" and pushed to '{}'", pr)),
                            Err(e) => return result_err(format!("{}; push to '{}' failed: {}", msg, pr, e)),
                        }
                    }
                    result(msg, true)
                }
                Err(e) => result_err(format!("Cherry-pick failed: {}", e)),
            }
        }
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
        // Rendering-animation bookkeeping.
        state.prune_anims();
        // Overlay open -> push an OverlayIn animation.
        if state.prev_overlay_none && !matches!(state.overlay, Overlay::None) {
            state.push_anim(AnimKind::OverlayIn, None);
        }
        state.prev_overlay_none = matches!(state.overlay, Overlay::None);
        // Detail mode change -> push a PanelTransition animation.
        if state.prev_detail_mode != state.detail_mode {
            state.push_anim(AnimKind::PanelTransition, None);
            state.prev_detail_mode = state.detail_mode;
        }
        // Clear a finished transition so the playground renders normally.
        if let Some(tr) = state.transition.as_ref() {
            if tr.start.elapsed() >= tr.duration {
                state.transition = None;
                state.transition_from_welcome = false;
            }
        }
        // Idle engine: show per-pane tips/hovers after `idle_tip_delay_secs`
        // without any keypress. Any keypress resets `last_activity`, so "idle"
        // already implies "not navigating". Paused during welcome/transition.
        let in_playground = !state.welcome && state.transition.is_none();
        if in_playground
            && state.gui.idle_tips
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
        if in_playground
            && state.autosave_ref_exists
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
    if let Some(tr) = state.transition.as_ref() {
        // Dissolve + warp transition from the welcome screen into the playground.
        let elapsed = tr.start.elapsed().as_secs_f64();
        let t = (elapsed / tr.duration.as_secs_f64()).clamp(0.0, 1.0);
        if state.transition_from_welcome {
            let wipe_frac = (t * 2.0).clamp(0.0, 1.0);
            let wipe_h = (f.area().height as f64 * (1.0 - wipe_frac)) as u16;
            render_welcome(f, state, Rect::new(0, 0, f.area().width, wipe_h));
        }
        let offset = (f.area().height as f64 * (1.0 - ease_out_bounce(t))) as u16;
        render_playground(f, state, Rect::new(0, offset, f.area().width, f.area().height));
    } else if state.welcome {
        render_welcome(f, state, f.area());
    } else {
        render_playground(f, state, f.area());
    }
    render_overlay(f, state);
}

/// The main playground: identity top bar, panes, status bar, help footer.
fn render_playground(f: &mut Frame, state: &mut AppState, area: Rect) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(3),
        ])
        .split(area);

    let inner = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(18),
            Constraint::Percentage(18),
            Constraint::Percentage(20),
            Constraint::Percentage(44),
        ])
        .split(layout[1]);

    render_top_bar(f, state, layout[0]);
    render_remotes(f, state, inner[0]);
    render_branches(f, state, inner[1]);
    render_files(f, state, inner[2]);
    render_detail(f, state, inner[3]);

    render_status_bar(f, state, layout[2]);

    // Floating idle tooltip, drawn after all panes so it can overflow the
    // focused pane and use the full terminal width without truncation.
    let focused_rect = match state.focus {
        Focus::Remotes => inner[0],
        Focus::Branches => inner[1],
        Focus::Files => inner[2],
        Focus::Detail | Focus::Graph => inner[3],
    };
    render_idle_tooltip(f, state, focused_rect);

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
    f.render_widget(footer, layout[3]);
}

/// Identity bar at the top: hostname (left) and GitHub username (right),
/// both bold and colored.
fn render_top_bar(f: &mut Frame, state: &AppState, area: Rect) {
    let host = if state.host.is_empty() { "unknown".to_string() } else { state.host.clone() };
    let who = if state.username.is_empty() { "unknown".to_string() } else { state.username.clone() };
    let gh = state.gh_user.as_deref().unwrap_or("not signed in");
    let left = format!("▸ {}@{}", who, host);
    let right = gh.to_string();
    let pad = area.width.saturating_sub(2).saturating_sub(left.chars().count() as u16 + right.chars().count() as u16 + 1);
    let line = Line::from(vec![
        Span::styled(left, Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
        Span::raw(" ".repeat(pad as usize)),
        Span::styled(right, Style::default().fg(GREEN).add_modifier(Modifier::BOLD)),
    ]);
    let p = Paragraph::new(line).style(Style::default().bg(Color::Rgb(35, 35, 45)));
    f.render_widget(p, area);
}

const WELCOME_BUTTONS: [&str; 5] = [
    "Continue →",
    "Skip intro",
    "? Cheatsheet",
    "Ctrl+P Palette",
    "✓ Don't show again",
];

const WELCOME_BLURB: &str = "\
git-multi is a CLI + TUI for managing multiple Git remotes.
Fetch, push, pull, cherry-pick and sync across origins from one view.
Browse remote branches and pick commits, manage pull requests,
blame lines GitLens-style, read the commit graph, and keep an
auto-save safety net for your working tree.

Everything is one key away: press ? for the cheatsheet or
Ctrl+P to run any action by name.";

/// The animated welcome screen: pulsing title, hostname + GitHub username,
/// a typewritten blurb, and navigable buttons.
fn render_welcome(f: &mut Frame, state: &AppState, area: Rect) {
    if area.width < 40 || area.height < 12 {
        return;
    }
    let elapsed = state.welcome_start.elapsed().as_millis() as usize;

    let total = WELCOME_BLURB.chars().count();
    let typed = (elapsed / 14).min(total);
    let mut blurb: String = WELCOME_BLURB.chars().take(typed).collect();
    // Blinking cursor while the blurb is still typing.
    if typed < total && (elapsed / 400).is_multiple_of(2) {
        blurb.push('█');
    }

    let pulse = (elapsed as f64 / 600.0 * std::f64::consts::TAU).sin();
    let title_fg = if pulse > 0.0 { VIBRANT_PINK } else { CYAN };

    let inner_w = (area.width.saturating_sub(4)) as usize;
    let host = if state.host.is_empty() { "unknown".to_string() } else { state.host.clone() };
    let who = if state.username.is_empty() { "unknown".to_string() } else { state.username.clone() };
    let gh = state.gh_user.as_deref().unwrap_or("not signed in");
    let identity = format!("{}  ·  {}", host, who);
    let github_line = format!("github: {}", gh);

    // Assemble centered content.
    let wrapped = wrap_text(&blurb, inner_w);
    let mut rows: Vec<String> = vec![
        center_pad("git-multi", inner_w),
        center_pad(&identity, inner_w),
        center_pad(&github_line, inner_w),
        String::new(),
    ];
    for l in &wrapped {
        rows.push(center_pad(l, inner_w));
    }
    rows.push(String::new());

    // Button row: the focused button is highlighted.
    let btn_total: usize = WELCOME_BUTTONS.iter().map(|b| b.chars().count()).sum::<usize>()
        + (WELCOME_BUTTONS.len() - 1) * 3;
    let btn_left = if btn_total < inner_w { (inner_w - btn_total) / 2 } else { 0 };
    let mut btn_text = " ".repeat(btn_left);
    for (i, b) in WELCOME_BUTTONS.iter().enumerate() {
        if i > 0 {
            btn_text.push_str("   ");
        }
        if i == state.welcome_button {
            btn_text.push_str(&format!("▶ {} ◀", b));
        } else {
            btn_text.push_str(b);
        }
    }
    rows.push(center_pad(&btn_text, inner_w));
    rows.push(String::new());
    rows.push(center_pad("Enter: activate · Tab/↑↓: move · ?: cheatsheet · Ctrl+P: palette · q: skip intro", inner_w));

    // Vertical centering.
    let avail = (area.height as usize).saturating_sub(2);
    let top_pad = if rows.len() < avail { (avail - rows.len()) / 2 } else { 0 };

    let mut text = String::new();
    for _ in 0..top_pad {
        text.push('\n');
    }
    for l in &rows {
        text.push_str(l);
        text.push('\n');
    }

    let block = Block::default()
        .title(" git-multi — multi-remote Git control center ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(title_fg));
    let p = Paragraph::new(text)
        .block(block)
        .style(Style::default().fg(CREAM));
    f.render_widget(ratatui::widgets::Clear, area);
    f.render_widget(p, area);
}

/// Center-pad a line to `width` characters.
fn center_pad(text: &str, width: usize) -> String {
    let w = text.chars().count();
    if w >= width {
        return text.to_string();
    }
    let left = (width - w) / 2;
    format!("{}{}", " ".repeat(left), text)
}

/// Word-wrap a string to `width` columns.
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    let width = width.max(1);
    let mut current = String::new();
    for word in text.split_whitespace() {
        if !current.is_empty() && current.chars().count() + word.chars().count() + 1 > width {
            out.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// Standard ease-out-bounce curve (0..1) for the playground drop-in.
fn ease_out_bounce(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    let n1 = 7.5625;
    let d1 = 2.75;
    if t < 1.0 / d1 {
        n1 * t * t
    } else if t < 2.0 / d1 {
        let t = t - 1.5 / d1;
        n1 * t * t + 0.75
    } else if t < 2.5 / d1 {
        let t = t - 2.25 / d1;
        n1 * t * t + 0.9375
    } else {
        let t = t - 2.625 / d1;
        n1 * t * t + 0.984375
    }
}

/// Cubic ease-out (0..1) for subtle slides/wipe-ins.
fn ease_out(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t) * (1.0 - t) * (1.0 - t)
}

/// Animation duration in ms for a kind, scaled by the global `speed`.
fn anim_duration_ms(p: &crate::config::AnimationPrefs, kind: AnimKind) -> u64 {
    let base = match kind {
        AnimKind::OverlayIn => p.overlay_ms,
        AnimKind::FocusPulse => p.focus_ms,
        AnimKind::PanelTransition => p.panel_ms,
        AnimKind::RefreshFlash => p.refresh_ms,
    };
    (base as f64 * p.speed.clamp(0.1, 10.0)) as u64
}

/// Eased progress of the newest animation of `kind`: `max(t)` clamped to
/// 0..1, or **1.0 ("settled")** when no animation of that kind is active —
/// so callers render their final state instead of a blank/partial frame.
fn anim_progress_of(anims: &[ActiveAnim], kind: AnimKind) -> f64 {
    let mut found = false;
    let mut best = 0.0f64;
    for a in anims {
        if a.kind == kind {
            found = true;
            let t = a.start.elapsed().as_secs_f64() / a.duration.as_secs_f64();
            best = best.max(t.clamp(0.0, 1.0));
        }
    }
    if found {
        best
    } else {
        1.0
    }
}

/// Build the status-bar text: `◆ branch @ short_sha  ↑ remote  (+a/-b)`.
fn status_line(branch: &str, head_short: &str, remote: Option<&str>, ahead: usize, behind: usize) -> String {
    let mut line = format!("◆ {}", if branch.is_empty() { "HEAD" } else { branch });
    if !head_short.is_empty() {
        line.push_str(&format!(" @ {}", head_short));
    }
    if let Some(r) = remote.filter(|r| !r.is_empty()) {
        line.push_str(&format!("  ↑ {}", r));
    }
    if ahead > 0 || behind > 0 {
        line.push_str(&format!("  (+{}/-{})", ahead, behind));
    }
    line
}

/// Render the 1-row status bar: active branch, HEAD commit, upstream remote,
/// and ahead/behind.
fn render_status_bar(f: &mut Frame, state: &AppState, area: Rect) {
    let branch = state.current_branch.as_deref().unwrap_or("HEAD");
    let remote = state.upstream_remote.as_deref();
    let text = status_line(branch, &state.head_short, remote, state.ahead, state.behind);
    let p = Paragraph::new(text)
        .style(Style::default().fg(Color::Black).bg(GREEN));
    f.render_widget(p, area);
}

fn render_remotes(f: &mut Frame, state: &mut AppState, area: Rect) {
    let default = state.repo.config.get_default_remote().cloned();
    let upstream = state.upstream_remote.clone();
    let items: Vec<ListItem> = state
        .remotes
        .iter()
        .map(|r| {
            let is_default = default.as_deref() == Some(&r.name);
            let is_upstream = upstream.as_deref() == Some(&r.name);
            let marker = if is_default { " [default]" } else { "" };
            let head = if is_upstream { "● " } else { "  " };
            let text = format!("{}{}{}", head, r.name, marker);
            let mut style = if is_upstream {
                Style::default().fg(GREEN).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            if is_default {
                style = style.fg(CYAN);
            }
            ListItem::new(text).style(style)
        })
        .collect();
    let title = if state.focus == Focus::Remotes { " Remotes (focused) " } else { " Remotes " };
    let list = List::new(items)
        .block(Block::default().title(title).borders(Borders::ALL).border_style(border_style(state, Focus::Remotes)))
        .highlight_style(Style::default().bg(CYAN).fg(Color::Black))
        .highlight_symbol(">> ");
    f.render_stateful_widget(list, area, &mut state.remote_state);
}

fn render_branches(f: &mut Frame, state: &mut AppState, area: Rect) {
    let search_query = if let Overlay::SearchBranch { value } = &state.overlay {
        value
    } else {
        ""
    };
    
    let branch_items: Vec<ListItem> = {
        let make_item = |b: &str, sel: bool, is_current: bool| {
            let mark = if sel { "[x]" } else { "[ ]" };
            let head = if is_current { "● " } else { "  " };
            let text = format!("{}{} {}", head, mark, b);
            if is_current {
                ListItem::new(text).style(Style::default().fg(GREEN).add_modifier(Modifier::BOLD))
            } else {
                ListItem::new(text)
            }
        };
        if search_query.is_empty() && state.filtered_branches.is_empty() {
            state
                .branches
                .iter()
                .map(|(b, sel)| make_item(b, *sel, state.current_branch.as_deref() == Some(b.as_str())))
                .collect()
        } else if search_query.is_empty() {
            state.filtered_branches.iter()
                .filter_map(|b| state.branches.iter().find(|(name, _)| name == b))
                .map(|(b, sel)| make_item(b, *sel, state.current_branch.as_deref() == Some(b.as_str())))
                .collect()
        } else {
            state.branches.iter()
                .filter(|(b, _)| b.contains(search_query))
                .map(|(b, sel)| make_item(b, *sel, state.current_branch.as_deref() == Some(b.as_str())))
                .collect()
        }
    };
    let title = if state.focus == Focus::Branches { " Branches (focused) " } else { " Branches " };
    let sel_count = state.selected_branches().len();
    let block = Block::default()
        .title(format!("{} [{} selected]", title, sel_count))
        .title_bottom(" [c] Create  [m] Rename  [x] Delete  [Space] Toggle ")
        .borders(Borders::ALL)
        .border_style(border_style(state, Focus::Branches));
    let branch_list = List::new(branch_items)
        .block(block)
        .highlight_style(Style::default().bg(MAUVE).fg(Color::Black))
        .highlight_symbol(">> ");
    f.render_stateful_widget(branch_list, area, &mut state.branch_state);
}

fn render_files(f: &mut Frame, state: &mut AppState, area: Rect) {
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
        .border_style(border_style(state, Focus::Files));
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().bg(BLUE).fg(Color::Black))
        .highlight_symbol(">> ");
    f.render_stateful_widget(list, area, &mut state.file_state);
}

fn render_detail(f: &mut Frame, state: &mut AppState, area: Rect) {
    // Wipe-in: clip the content to a growing height during a PanelTransition.
    let panel_t = state.anim_progress(AnimKind::PanelTransition);
    let grow = ease_out(panel_t);
    let content_area = Rect::new(
        area.x,
        area.y,
        area.width,
        ((area.height as f64 * grow) as u16).max(1),
    );
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
            f.render_widget(p, content_area);
            return;
        }
    };
    let p = Paragraph::new(text)
        .block(block)
        .style(Style::default().fg(CREAM))
        .wrap(Wrap { trim: false })
        .scroll((0, 0));
    f.render_widget(p, content_area);
    if state.detail_mode == DetailMode::Graph {
        // Re-render graph as a list for selection highlight.
        let head = state.head_short.as_str();
        let items: Vec<ListItem> = state
            .graph_lines
            .iter()
            .map(|gl| {
                if gl.is_commit {
                    let is_head = head.len() >= 8 && gl.sha.starts_with(head);
                    let style = if is_head {
                        Style::default().fg(GREEN).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(CREAM)
                    };
                    ListItem::new(format!("{}  [Enter: pick, D: diff]", gl.text)).style(style)
                } else {
                    ListItem::new(gl.text.clone()).style(Style::default().fg(GRAY))
                }
            })
            .collect();
        let list = List::new(items)
            .block(Block::default().title(state.detail_mode.title()).borders(Borders::ALL).border_style(Style::default().fg(MAUVE)))
            .highlight_style(Style::default().bg(ORANGE).fg(Color::Black))
            .highlight_symbol(">> ");
        f.render_stateful_widget(list, content_area, &mut state.graph_state);
    }
}

fn render_overlay(f: &mut Frame, state: &AppState) {
    // Overlay fade-in + slide-up (animated entry).
    let t = state.anim_progress(AnimKind::OverlayIn);
    let fade = ease_out(t);
    let dim_target = state.repo.config.animations.dim;
    let dim = dim_target * (1.0 - fade);
    let slide = ((1.0 - fade) * 8.0) as i16;
    if dim > 0.0 {
        let full = f.area();
        let buf = f.buffer_mut();
        for x in full.left()..full.right() {
            for y in full.top()..full.bottom() {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.bg = blend_color(cell.bg, Color::Rgb(10, 10, 18), dim);
                }
            }
        }
    }
    match &state.overlay {
        Overlay::AddName { value } => modal(f, 60, 4, " Add Remote ",
            &format!("Remote name:\n> {}\u{2588}", value), RED, slide),
        Overlay::AddUrl { name, value } => modal(f, 70, 4, " Add Remote ",
            &format!("URL for '{}':\n> {}\u{2588}", name, value), RED, slide),
        Overlay::RenameRemote { old, value } => modal(f, 60, 4, " Rename Remote ",
            &format!("Rename '{}' to:\n> {}\u{2588}", old, value), RED, slide),
        Overlay::RemoveRemote { name } => modal(f, 60, 4, " Remove Remote ",
            &format!("Remove remote '{}'?\n\n[y] Yes  [n/Esc] Cancel", name), RED, slide),
        Overlay::CreateBranch { step, name, base, remote } => {
            let prompt = match step {
                0 => format!("Branch name:\n> {}\u{2588}", name),
                1 => format!("Base (commit/branch):\n> {}\u{2588}", base),
                _ => format!("Push to remote (empty = local only):\n> {}\u{2588}", remote),
            };
            modal(f, 65, 4, " Create Branch ", &prompt, RED, slide)
        }
        Overlay::DeleteBranch { name } => modal(f, 60, 4, " Delete Branch ",
            &format!("Delete local branch '{}'?\n\n[y] Yes  [n/Esc] Cancel", name), RED, slide),
        Overlay::RenameBranch { old, value } => modal(f, 60, 4, " Rename Branch ",
            &format!("Rename '{}' to:\n> {}\u{2588}", old, value), RED, slide),
        Overlay::Merge { step, src_remote, src_branch, dest_remote, dest_branch } => {
            let prompt = match step {
                0 => format!("Source remote:\n> {}\u{2588}", src_remote),
                1 => format!("Source branch (from {}):\n> {}\u{2588}", src_remote, src_branch),
                2 => format!("Destination remote:\n> {}\u{2588}", dest_remote),
                _ => format!("Destination branch:\n> {}\u{2588}", dest_branch),
            };
            modal(f, 65, 4, " Merge ", &prompt, VIBRANT_PINK, slide)
        }
        Overlay::CommitType { value } => modal(f, 60, 7, " Commit Type ",
            &format!("Select commit type:\n\n[f] feat  [x] fix  [d] docs  [s] style  [r] refactor\n[T] test  [c] chore  [b] build  [p] perf\n\nOr type to filter:\n> {}\u{2588}", value), GREEN, slide),
        Overlay::CommitMsg { value } => modal(f, 70, 4, " Commit Message ",
            &format!("Commit subject:\n> {}\u{2588}", value), GREEN, slide),
        Overlay::CommitBody { value } => modal(f, 70, 6, " Commit Body ",
            &format!("Commit body (optional, Enter to skip):\n> {}\u{2588}", value), GREEN, slide),
        Overlay::AmendMsg { value } => modal(f, 70, 4, " Amend last commit ",
            &format!("New message:\n> {}\u{2588}", value), YELLOW, slide),
        Overlay::RevertCommit { value } => modal(f, 60, 4, " Revert commit ",
            &format!("Commit to revert (sha/ref):\n> {}\u{2588}", value), YELLOW, slide),
        Overlay::ResetCommit { value, mode } => modal(f, 70, 5, " Reset ",
            &format!("Mode: [1] soft  [2] mixed  [3] hard   (current: {:?})\nTarget (sha/ref):\n> {}\u{2588}", mode, value), YELLOW, slide),
        Overlay::DiffPath { value, mode } => modal(f, 70, 4, " Diff file ",
            &format!("Diff ({:?}) for path:\n> {}\u{2588}", mode, value), CYAN, slide),
        Overlay::CherryPick { value, context } => {
            let ctx_line = if context.is_empty() {
                String::new()
            } else {
                format!("\n{}", context)
            };
            let target = if state.pick_target.is_empty() { "(current)" } else { &state.pick_target };
            let copy = if state.pick_copy { "ON" } else { "off" };
            let push = if state.pick_push { "ON" } else { "off" };
            modal(f, 85, 7, " Cherry-pick commit ",
                &format!("Commit to cherry-pick (sha/ref):\n> {}\u{2588}{}\ntarget: {}   copy (no-commit): {}   push: {}\n\n[space] cherry-pick  [d] preview diff  [t] target  [c] copy  [p] push  [Enter] accept  [Esc] cancel", value, ctx_line, target, copy, push), VIBRANT_PINK, slide)
        }
Overlay::Message { text, is_error } => {
             let color = if *is_error { RED } else { GREEN };
             modal(f, 70, 4, " Message ", &format!("{}\n\n[Enter/Esc to dismiss]", text), color, slide)
         }
         Overlay::SearchCommit { value } => {
             let prompt = if value.is_empty() {
                 "Search commits by SHA or message:\n> \u{2588}".to_string()
             } else {
                 format!("Search commits by SHA or message:\n> {}\u{2588}", value)
             };
             modal(f, 70, 5, " Search Commits ", &prompt, CYAN, slide)
         }
         Overlay::SearchBranch { value } => {
             let prompt = if value.is_empty() {
                 "Search branches by name:\n> \u{2588}".to_string()
             } else {
                 format!("Search branches by name:\n> {}\u{2588}", value)
             };
             modal(f, 70, 5, " Search Branches ", &prompt, CYAN, slide)
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
         Overlay::PickSource { filter, selected } => render_pick_source(f, state, filter, *selected),
         Overlay::PickBrowse { selected } => render_pick_browse(f, state, *selected),
         Overlay::Prompt { title, value, .. } => modal(f, 80, 5, title,
             &format!("{}\n> {}\u{2588}", prompt_hint(title), value), CYAN, slide),
         Overlay::ConfirmDangerous { title, prompt, .. } => modal(f, 70, 5, title,
             &format!("{}\n\n[y] Yes  [n/Esc] Cancel", prompt), RED, slide),
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
    text.push_str("\nTip: press Ctrl+P to run any action by name.\n");

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
#[allow(clippy::too_many_arguments)]
fn glass_modal(f: &mut Frame, width: u16, height: u16, title: &str, text: String, color: Color, slide: i16, dim: f64) {
    let full = f.area();
    let buf = f.buffer_mut();
    for x in full.left()..full.right() {
        for y in full.top()..full.bottom() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.bg = blend_color(cell.bg, Color::Rgb(12, 12, 22), dim);
            }
        }
    }
    let mut area = centered_rect(width, height, full);
    if slide > 0 {
        area.y = area.y.saturating_add(slide as u16).min(full.bottom().saturating_sub(1));
    }
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
    let t = state.anim_progress(AnimKind::OverlayIn);
    let fade = ease_out(t);
    let dim = state.repo.config.animations.dim * (1.0 - fade);
    let slide = ((1.0 - fade) * 8.0) as i16;
    glass_modal(f, 62, 16, &format!(" GitHub Profile — {} ", login), text, GREEN, slide, dim);
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
    let t = state.anim_progress(AnimKind::OverlayIn);
    let fade = ease_out(t);
    let dim = state.repo.config.animations.dim * (1.0 - fade);
    let slide = ((1.0 - fade) * 8.0) as i16;
    glass_modal(f, 88, 34, &format!(" Pull Request #{} ", number), text, VIBRANT_PINK, slide, dim);
}

/// Flatten `remote/branch` sources for the picker, sorted by remote name.
fn pick_sources(repo: &crate::git::GitRepo) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(info) = repo.list_all_branches() {
        let mut keys: Vec<&String> = info.remote.keys().collect();
        keys.sort();
        for r in keys {
            if let Some(brs) = info.remote.get(r) {
                for b in brs {
                    out.push(format!("{}/{}", r, b.name));
                }
            }
        }
    }
    out
}

/// Render the cross-origin source picker (choose a `remote/branch`).
fn render_pick_source(f: &mut Frame, state: &AppState, filter: &str, selected: usize) {
    let area = centered_rect(62, 18, f.area());
    let sources = pick_sources(&state.repo);
    let filtered: Vec<&String> = if filter.trim().is_empty() {
        sources.iter().collect()
    } else {
        sources.iter().filter(|s| s.contains(filter)).collect()
    };
    let mut text = format!("Pick source — a remote branch to browse\n> {}\u{2588}\n\n", filter);
    if filtered.is_empty() {
        text.push_str("(no remote branches — run a fetch first)\n");
    }
    let sel = selected.min(filtered.len().saturating_sub(1));
    for (i, s) in filtered.iter().enumerate() {
        let marker = if i == sel { ">> " } else { "   " };
        text.push_str(&format!("{} {}\n", marker, s));
    }
    text.push_str("\n[Enter] browse commits   [Esc] close");
    let p = Paragraph::new(text)
        .block(Block::default().title(" Pick from Remote ").borders(Borders::ALL).border_style(Style::default().fg(VIBRANT_PINK)))
        .style(Style::default().fg(CREAM));
    f.render_widget(ratatui::widgets::Clear, area);
    f.render_widget(p, area);
}

/// Render the commit browser for picking/copying across origins.
fn render_pick_browse(f: &mut Frame, state: &AppState, selected: usize) {
    let area = centered_rect(88, 26, f.area());
    let mut text = String::new();
    if let Some(pb) = &state.pick_browse {
        let target = if state.pick_target.is_empty() { "(current)" } else { &state.pick_target };
        let copy = if state.pick_copy { "ON" } else { "off" };
        let push = if state.pick_push { "ON" } else { "off" };
        text.push_str(&format!(
            "Pick from {}/{}    target: {}    copy (no-commit): {}    push: {}\n\n",
            pb.remote, pb.branch, target, copy, push
        ));
        let sel = selected.min(pb.items.len().saturating_sub(1));
        for (i, c) in pb.items.iter().enumerate() {
            let mark = if pb.marks.get(i).copied().unwrap_or(false) { "[x]" } else { "[ ]" };
            let marker = if i == sel { ">> " } else { "   " };
            text.push_str(&format!(
                "{} {} {}  {}  {}  {}\n",
                marker,
                mark,
                c.short_id,
                c.author,
                crate::git::format_timestamp(c.author_date),
                c.message
            ));
        }
        text.push_str("\n[Space] select  [a] all  [c] copy  [t] target  [p] push  [d] diff  [Enter] apply  [Esc] close");
    } else {
        text.push_str("Loading commits…");
    }
    let p = Paragraph::new(text)
        .block(Block::default().title(" Pick Commits ").borders(Borders::ALL).border_style(Style::default().fg(VIBRANT_PINK)))
        .style(Style::default().fg(CREAM));
    f.render_widget(ratatui::widgets::Clear, area);
    f.render_widget(p, area);
}

/// The y coordinate of the focused list's selected row inside a pane, accounting
/// for the top border and any scroll offset. Used to anchor the idle tooltip.
fn selected_row_y(pane_y: u16, selected: usize, offset: usize) -> u16 {
    pane_y + 1 + (selected.saturating_sub(offset)) as u16
}

/// Compute the tooltip box for the given cursor row/column, clamped to the
/// terminal. Places the box just below the cursor, flipping above it when it
/// would overflow the bottom edge.
fn tooltip_rect(cursor: (u16, u16), content_width: u16, content_height: u16, term: Rect) -> Rect {
    let right = term.right();
    let bottom = term.bottom();
    let mut width = content_width.min(right.saturating_sub(cursor.0)).max(8);
    let x = cursor.0.clamp(term.x, right.saturating_sub(width));
    // shrink width again if clamping pushed x but width no longer fits
    width = width.min(right.saturating_sub(x));
    let mut y = cursor.1.saturating_add(1);
    if y.saturating_add(content_height) > bottom {
        y = cursor.1.saturating_sub(content_height);
    }
    y = y.clamp(term.y, bottom.saturating_sub(content_height).max(term.y));
    Rect::new(x, y, width, content_height)
}

/// Estimate the number of rows a wrapped line occupies in `width` columns.
fn wrap_height(line: &str, width: u16) -> u16 {
    let width = width.max(1) as usize;
    let chars = line.chars().count();
    chars.div_ceil(width).max(1) as u16
}

/// Draw the passive idle tooltip: a floating, bordered box anchored below the
/// focused pane's selected row, combining the pane's action hints with the
/// selected item's hover preview. Uses the full terminal width when possible,
/// so long hints are never truncated to the pane's width.
fn render_idle_tooltip(f: &mut Frame, state: &AppState, pane: Rect) {
    if !state.tip_visible {
        return;
    }
    if !matches!(state.overlay, Overlay::None) {
        return;
    }
    let mut lines: Vec<String> = Vec::new();
    if let Some(tip) = state.focus_tip() {
        lines.push(tip);
    }
    if let Some(hover) = hover_text(state) {
        lines.push(String::new());
        lines.push(hover);
    }
    if lines.is_empty() {
        return;
    }

    // Anchor below the selected row; fall back to the pane's top row when the
    // focused pane has no item list selection (e.g. Detail modes).
    let (selected, offset) = match state.focus {
        Focus::Remotes => (state.remote_state.selected(), state.remote_state.offset()),
        Focus::Branches => (state.branch_state.selected(), state.branch_state.offset()),
        Focus::Files => (state.file_state.selected(), state.file_state.offset()),
        Focus::Graph => (state.graph_state.selected(), state.graph_state.offset()),
        Focus::Detail => (None, 0),
    };
    let cursor_y = selected
        .map(|i| selected_row_y(pane.y, i, offset))
        .unwrap_or(pane.y + 1);
    let cursor_x = pane.x + 2;

    let term = f.area();
    let max_width = term.width.saturating_sub(2);
    let width = lines
        .iter()
        .map(|l| l.chars().count().min(max_width as usize) as u16)
        .max()
        .unwrap_or(max_width)
        .min(max_width)
        .max(20);
    let height = lines.iter().map(|l| wrap_height(l, width)).sum::<u16>() + 2; // + borders

    let area = tooltip_rect((cursor_x, cursor_y), width, height, term);

    let accent = match state.focus {
        Focus::Remotes => CYAN,
        Focus::Branches => MAUVE,
        Focus::Files => BLUE,
        Focus::Detail | Focus::Graph => ORANGE,
    };
    let text = lines.join("\n");
    let tip = Paragraph::new(text)
        .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(accent)))
        .style(Style::default().fg(CREAM).bg(Color::Rgb(30, 30, 45)))
        .wrap(Wrap { trim: false });
    f.render_widget(tip, area);
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

 fn modal(f: &mut Frame, percent_x: u16, height: u16, title: &str, text: &str, color: Color, slide: i16) {
     let mut area = centered_rect(percent_x, height, f.area());
     if slide > 0 {
         area.y = area.y.saturating_add(slide as u16).min(f.area().bottom().saturating_sub(1));
     }
     let m = Paragraph::new(text)
         .block(Block::default().title(title).borders(Borders::ALL).border_style(Style::default().fg(color)))
         .style(Style::default().fg(Color::White));
     f.render_widget(ratatui::widgets::Clear, area);
     f.render_widget(m, area);
 }

fn border_style(state: &AppState, pane: Focus) -> Style {
    let focused = state.focus == pane;
    let mut s = if focused {
        Style::default().fg(CYAN).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(GRAY)
    };
    // Brighten the border while a focus-change or refresh pulse targets this pane.
    let pulse = state
        .pulse_level(AnimKind::FocusPulse, pane)
        .max(state.pulse_level(AnimKind::RefreshFlash, pane));
    if pulse > 0.0 {
        let b = (120.0 + 135.0 * pulse) as u8;
        s = s.fg(Color::Rgb(b, b, 255)).add_modifier(Modifier::BOLD);
    }
    s
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
    if event::poll(state.poll_delay())? {
        if let Event::Key(key) = event::read()? {
            if key.kind == KeyEventKind::Press {
                state.last_activity = Instant::now();
                state.tip_visible = false;
                if handle_overlay(state, key) {
                    return Ok(false);
                }
                if state.welcome {
                    handle_welcome_key(state, &key);
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

/// Key handling for the welcome screen (buttons + direct shortcuts).
fn handle_welcome_key(state: &mut AppState, key: &crossterm::event::KeyEvent) {
    let Some(k) = parse_key(key) else { return };
    match k {
        Key::Char('?') => state.overlay = Overlay::Help { scroll: 0 },
        Key::CtrlChar('p') => state.open_palette(),
        Key::Char('q') | Key::Esc => state.start_tool(),
        Key::Tab | Key::Right | Key::Down => {
            state.welcome_button = (state.welcome_button + 1) % WELCOME_BUTTONS.len();
        }
        Key::Left | Key::Up => {
            state.welcome_button = (state.welcome_button + WELCOME_BUTTONS.len() - 1) % WELCOME_BUTTONS.len();
        }
        Key::Enter | Key::Char(' ') => state.activate_welcome_button(),
        _ => {}
    }
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
                        let target = state.pick_target.clone();
                        let copy = state.pick_copy;
                        let push_remote = if state.pick_push {
                            state.repo.config.get_default_remote().cloned()
                        } else {
                            None
                        };
                        state.submit_job(UiJob::PickCommits { specs: vec![spec], target_branch: target, copy, push_remote }, false);
                        state.overlay = Overlay::Message { text: "Cherry-pick started (see log)".to_string(), is_error: false };
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
                KeyCode::Char('t') => {
                    state.open_pick_target_prompt();
                    return true;
                }
                KeyCode::Char('c') => {
                    state.pick_copy = !state.pick_copy;
                }
                KeyCode::Char('p') => {
                    state.pick_push = !state.pick_push;
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
         // ---- Cross-origin pick ----
         Overlay::PickSource { filter, selected } => {
             match code {
                 KeyCode::Esc | KeyCode::Char('q') => state.overlay = Overlay::None,
                 KeyCode::Up | KeyCode::Char('k') => {
                     let n = pick_sources(&state.repo).len();
                     if n > 0 { *selected = (*selected).saturating_sub(1) % n; }
                 }
                 KeyCode::Down | KeyCode::Char('j') => {
                     let n = pick_sources(&state.repo).len();
                     if n > 0 { *selected = (*selected + 1) % n; }
                 }
                 KeyCode::Enter => {
                     let q = filter.clone();
                     let sources = pick_sources(&state.repo);
                     let filtered: Vec<&String> = if q.trim().is_empty() {
                         sources.iter().collect()
                     } else {
                         sources.iter().filter(|s| s.contains(&q)).collect()
                     };
                     if let Some(src) = filtered.get(*selected) {
                         let (remote, branch) = match src.split_once('/') {
                             Some((r, b)) => (r.to_string(), b.to_string()),
                             None => return true,
                         };
                         state.overlay = Overlay::Message { text: format!("Loading {}/{}…", remote, branch), is_error: false };
                         state.submit_job(UiJob::LoadRemoteCommits { remote, branch }, true);
                     }
                 }
                 KeyCode::Char(c) => { filter.push(c); }
                 KeyCode::Backspace => { filter.pop(); }
                 _ => {}
             }
         }
         Overlay::PickBrowse { selected } => {
             match code {
                 KeyCode::Up | KeyCode::Char('k') => { *selected = selected.saturating_sub(1); }
                 KeyCode::Down | KeyCode::Char('j') => { *selected = selected.saturating_add(1); }
                 KeyCode::Char(' ') => {
                     if let Some(pb) = &mut state.pick_browse {
                         if let Some(m) = pb.marks.get_mut(*selected) {
                             *m = !*m;
                         }
                     }
                 }
                 KeyCode::Char('a') => {
                     if let Some(pb) = &mut state.pick_browse {
                         let all = pb.marks.iter().all(|m| *m);
                         for m in pb.marks.iter_mut() {
                             *m = !all;
                         }
                     }
                 }
                 KeyCode::Char('c') => { state.pick_copy = !state.pick_copy; }
                 KeyCode::Char('t') => {
                     state.open_pick_target_prompt();
                     return true;
                 }
                 KeyCode::Char('p') => { state.pick_push = !state.pick_push; }
                 KeyCode::Char('d') => {
                     if let Some(pb) = &state.pick_browse {
                         if let Some(c) = pb.items.get(*selected) {
                             state.commit_diff_spec = Some(c.id.clone());
                             state.detail_mode = DetailMode::CommitDiff;
                         }
                     }
                 }
                 KeyCode::Enter => {
                     let specs: Vec<String> = if let Some(pb) = &state.pick_browse {
                         // Items are newest-first; apply oldest-first.
                         pb.items
                             .iter()
                             .enumerate()
                             .filter(|(i, _)| pb.marks.get(*i).copied().unwrap_or(false))
                             .map(|(_, c)| c.id.clone())
                             .rev()
                             .collect()
                     } else {
                         Vec::new()
                     };
                     if specs.is_empty() {
                         state.overlay = Overlay::Message { text: "Select at least one commit (Space)".to_string(), is_error: true };
                     } else {
                         let target = state.pick_target.clone();
                         let copy = state.pick_copy;
                         let push_remote = if state.pick_push {
                             state.repo.config.get_default_remote().cloned()
                         } else {
                             None
                         };
                         state.submit_job(UiJob::PickCommits { specs, target_branch: target, copy, push_remote }, false);
                         state.overlay = Overlay::Message { text: "Pick started (see log)".to_string(), is_error: false };
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
                         PromptAction::PickTarget => {
                             state.pick_target = input.trim().to_string();
                             // Return to the commit browser if that's where the
                             // prompt was opened from.
                             if state.pick_browse.is_some() {
                                 state.overlay = Overlay::PickBrowse { selected: 0 };
                             } else {
                                 state.overlay = Overlay::None;
                             }
                             return true;
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
    state.push_anim(AnimKind::FocusPulse, Some(state.focus));
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
    state.push_anim(AnimKind::FocusPulse, Some(state.focus));
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

    #[test]
    fn selected_row_y_accounts_for_border_and_scroll() {
        // First row selected, no scroll -> just below the top border.
        assert_eq!(selected_row_y(5, 0, 0), 6);
        // Scrolled so the selected item is the 4th visible row
        // (index 7 with offset 3 hides rows 0..2).
        assert_eq!(selected_row_y(5, 7, 3), 10);
    }

    #[test]
    fn tooltip_rect_places_below_cursor() {
        let term = Rect::new(0, 0, 120, 40);
        let r = tooltip_rect((10, 5), 50, 3, term);
        assert_eq!(r.x, 10);
        assert_eq!(r.y, 6); // just below the cursor
        assert_eq!(r.width, 50);
        assert_eq!(r.height, 3);
    }

    #[test]
    fn tooltip_rect_flips_above_when_no_room_below() {
        let term = Rect::new(0, 0, 120, 40);
        // Cursor near the bottom: below would overflow, so flip above.
        let r = tooltip_rect((10, 38), 50, 4, term);
        assert_eq!(r.y, 34); // 38 - 4, above the cursor
        assert!(r.bottom() <= term.bottom());
    }

    #[test]
    fn tooltip_rect_clamps_to_terminal() {
        let term = Rect::new(0, 0, 60, 20);
        // Cursor at the far right: width shrinks to fit.
        let r = tooltip_rect((58, 3), 50, 3, term);
        assert!(r.right() <= term.right());
        assert!(r.width <= 60);
        // Cursor at the bottom edge: y is clamped into the terminal.
        let r2 = tooltip_rect((5, 19), 20, 2, term);
        assert!(r2.y < term.bottom());
        assert!(r2.bottom() <= term.bottom());
    }

    #[test]
    fn wrap_height_estimates_rows() {
        assert_eq!(wrap_height("hello", 10), 1);
        assert_eq!(wrap_height("hello world", 5), 3); // 11 chars / 5 cols
        assert_eq!(wrap_height("", 5), 1);
    }

    #[test]
    fn status_line_formats() {
        assert_eq!(status_line("main", "a1b2c3d4", Some("origin"), 0, 0), "◆ main @ a1b2c3d4  ↑ origin");
        assert_eq!(status_line("main", "a1b2c3d4", Some("origin"), 3, 1), "◆ main @ a1b2c3d4  ↑ origin  (+3/-1)");
        assert_eq!(status_line("", "", None, 0, 0), "◆ HEAD");
        assert_eq!(status_line("feature", "abcd1234", None, 5, 0), "◆ feature @ abcd1234  (+5/-0)");
    }

    #[test]
    fn branch_head_marker_flag() {
        // A helper mirroring the closure logic used in render_branches.
        let flag = |b: &str, current: Option<&str>| current == Some(b);
        assert!(flag("main", Some("main")));
        assert!(!flag("dev", Some("main")));
        assert!(!flag("main", None));
    }

    #[test]
    fn identity_overrides_replace_detection() {
        let none = crate::config::IdentityPreferences::default();
        assert_eq!(apply_identity_overrides("box".into(), "bob".into(), &none), ("box".into(), "bob".into()));

        let mut prefs = crate::config::IdentityPreferences {
            device: Some("  MyPC  ".to_string()),
            username: Some("alice".to_string()),
        };
        assert_eq!(apply_identity_overrides("box".into(), "bob".into(), &prefs), ("MyPC".into(), "alice".into()));

        // Empty override strings fall back to detection.
        prefs.device = Some("   ".to_string());
        assert_eq!(apply_identity_overrides("box".into(), "bob".into(), &prefs), ("box".into(), "alice".into()));
    }

    #[test]
    fn whoami_windows_domain_trimmed() {
        // Windows whoami returns DOMAIN\user; we keep the part after the backslash.
        let raw = "CORP\\jdoe";
        let user = raw.rsplit('\\').next().unwrap_or("unknown");
        assert_eq!(user, "jdoe");
    }

    #[test]
    fn ease_out_bounds_and_monotonic() {
        assert!((ease_out(0.0) - 0.0).abs() < 1e-9);
        assert!((ease_out(1.0) - 1.0).abs() < 1e-9);
        assert!(ease_out(0.5) > 0.5); // ease-out accelerates early
        assert!(ease_out(0.25) < ease_out(0.75));
    }

    #[test]
    fn anim_duration_scales_with_speed() {
        let p = crate::config::AnimationPrefs::default();
        assert_eq!(anim_duration_ms(&p, AnimKind::OverlayIn), 200);
        assert_eq!(anim_duration_ms(&p, AnimKind::RefreshFlash), 150);
        let fast = crate::config::AnimationPrefs { speed: 2.0, ..crate::config::AnimationPrefs::default() };
        assert_eq!(anim_duration_ms(&fast, AnimKind::FocusPulse), 360);
    }

    #[test]
    fn anim_progress_settles_to_one_when_idle() {
        // No animation -> "settled" (1.0), so the Detail pane renders at full
        // height and the overlay dim stays off instead of blanking/dimming.
        assert_eq!(anim_progress_of(&[], AnimKind::PanelTransition), 1.0);
        assert_eq!(anim_progress_of(&[], AnimKind::OverlayIn), 1.0);
    }

    #[test]
    fn anim_progress_reports_flight_and_expired() {
        let dur = Duration::from_millis(200);
        let mid = ActiveAnim {
            start: Instant::now() - Duration::from_millis(100),
            duration: dur,
            kind: AnimKind::PanelTransition,
            focus: None,
        };
        let t = anim_progress_of(&[mid], AnimKind::PanelTransition);
        assert!((t - 0.5).abs() < 0.05, "expected ~0.5, got {}", t);

        let expired = ActiveAnim {
            start: Instant::now() - Duration::from_millis(500),
            duration: dur,
            kind: AnimKind::PanelTransition,
            focus: None,
        };
        assert_eq!(anim_progress_of(&[expired], AnimKind::PanelTransition), 1.0);
    }
}
