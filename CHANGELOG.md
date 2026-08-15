# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Auto-save safety snapshots in the TUI: after ~30s idle, the current dirty state is
  committed to `refs/gitmulti/autosave` so an accidental destructive operation
  (`git reset --hard`, AI-driven cleanup, etc.) can be recovered without losing
  uncommitted work.
- TUI recovery keybinding `O` to merge the auto-saved state back into the working
  tree when available.
- Cherry-pick overlay expanded to 85% width, 5 rows with context display
  (commit author/message in Graph view, file dirty status in Files view).
- Press `d` in the cherry-pick overlay to preview the commit diff in the
  detail panel before executing the cherry-pick.
- Focus-specific `P` key: pre-fills commit SHA in Graph view, pre-fills
  dirty file path in Files view.
- Full git coverage: granular stage/unstage/restore, amend, revert, reset
  (soft/mixed/hard), cherry-pick, unified diffs (staged/unstaged/HEAD).
- GitLens features: per-line blame, file history, line history.
- Git Graph: commit DAG with branch/tag/remote ref labels, reachable from HEAD
  or all refs, with one-key cherry-pick of any node.
- CLI subcommands: `commit`, `diff`, `blame`, `log`, `graph`, `revert`, `reset`,
  `pick`, `stage`, `unstage`, `restore`, `stash`, `tag`, `reflog`, `completions`.
- Colored diff rendering in Details pane: red for deletions, green for additions,
  yellow for hunk headers, cyan for file headers.
- J/K keyboard navigation for scrolling commit details in Details pane.
- Enter on a commit in Files panel shows commit details in Details pane.
- `--json` machine-readable output for `status`, `list`, `graph`, `remote list`
  and `branch list`.
- ASCII `git log --graph` rendering in the GUI Graph panel, with `Enter` to
  cherry-pick and `D` to preview a commit diff directly from the graph.
- Integration test suite (`tests/cli.rs`).
- Keymap registry: all shortcuts live in a single table that drives dispatch,
  the `?` cheatsheet, the `Ctrl+P` command palette, and idle tips — so they can
  never drift. A unit test asserts no duplicate key bindings.
- `?` cheatsheet: scrollable reference of every shortcut grouped by scope with
  a one-line description.
- `Ctrl+P` command palette: fuzzy-search any action by name and run it, so every
  git verb is reachable without memorizing keys.
- Idle tips & hovers: after ~10s idle on a pane (configurable via
  `[gui] idle_tips / idle_tip_delay_secs / idle_previews`), a passive hint box
  shows the pane's actions and a hover preview shows the selected item's info
  (file status + recency, branch last-commit + ahead/behind, remote URL, graph
  node).
- Visualizations: `h` commit-activity heatmap (weekday × hour grid + totals),
  `G` full-screen Git Graph, `H`/`L` GitLens file/line history, `t` tags modal,
  `U` stash modal (save/apply/pop/drop), `W` worktree/branch status,
  `Y` copy HEAD sha.
- GitHub integration via `gh`: `N` contributors (colored initials avatars +
  commit counts; falls back to `git shortlog` offline) with a translucent
  profile modal, and `o` pull-request browser with list filters, a tabbed
  detail modal (milestone, labels/scope, assignees, reviewers, commits, files,
  description) and actions (merge with strategy, close/reopen, comment/review,
  checkout, open in browser, edit, labels, milestone, reviewers, assign).
- Action-gap coverage: `Enter` checks out the selected branch, Files pane
  `X`/`V` run `git rm`/`git mv`, Branches `K` rebases onto a ref, Detail `X`
  runs `git show`, PR detail `C` checks out the PR locally.
- Pick/cherry-pick across origins: `git-multi pick <sha|A..B> [--from-remote
  <r>] [--to-branch <b>] [--copy] [--push <r>]` auto-fetches the source,
  applies onto a chosen target branch, can stage without committing
  (`--copy`), and can push the result. In the TUI, `v` in the Remotes pane
  opens a remote-branch picker: browse a `remote/branch`, multi-select commits
  (Space), toggle copy/target/push (`c`/`t`/`p`), preview diffs (`d`), and
  apply (`Enter`). The cherry-pick overlay (Files `P`, graph `Enter`) now also
  supports `t`/`c`/`p`.
- Animated welcome screen on launch: shows your machine hostname and GitHub
  username (from `gh api user`), a typewritten blurb about the tool, and
  navigable buttons (Continue, Skip intro, `?` Cheatsheet, `Ctrl+P` Palette,
  "Don't show again"). The playground then dissolves in with a warp/bounce
  transition, and the identity is shown in a new top bar (hostname left,
  GitHub username right). Configure via `[gui] show_welcome`.

### Fixed
- Auto-save no longer stages everything into the real index (`git add -A` was
  destroying the user's granular stage/unstage state every ~30s idle). The
  snapshot now uses a throwaway index; identical snapshots are skipped.
- Auto-save restore (`O`) now restores both the index and working tree.
- Cherry-pick ranges were reversed (`HEAD~3..HEAD` picked the wrong commits);
  they now push the upper bound and hide the lower bound, applying oldest-first.
- `sync` is now correct end-to-end: it fetches both remotes, creates/uses the
  destination local branch, cherry-picks/merges/rebases, and pushes the result to
  the destination remote. The default range now means "everything the destination
  is missing" instead of erroring on `HEAD`. `--force` is honoured.
- `pull --branches` no longer merges unrelated branches into your current branch.
- `copy --files` resolves paths relative to the repo root (not the CWD), supports
  real glob patterns, and implements `--prune`.
- `pr --repo` now resolves the `owner/repo` slug from the remote URL (it previously
  passed the git remote name, which `gh` rejects), omits empty bodies/heads, and
  implements `--open`.
- `remote list` / `branch list --all` no longer show the phantom `HEAD` branch.
- `self-update` now tolerates any archive layout (the Linux tarball used to contain
  `git-multi-bin` and Windows zips a flat `.exe`), uses timeouts, and compares
  versions numerically. The Linux release tarball is now built with the correct
  binary name.
- Version string matches the crate version (was hard-coded `0.1.1`).
- The `-h` short flag on `pr --head` no longer collides with `--help`.
- Reset overlay mode keys (`1`/`2`/`3`) no longer block typing targets containing
  `s`/`m`/`h`.
- Files-panel cherry-pick pre-fills HEAD's SHA (was pre-filling the file path).
- Failed merges/cherry-picks now clean up `MERGE_HEAD`/`CHERRY_PICK_HEAD` state.
- Dates in blame/log/graph/commit views are now readable (`YYYY-MM-DD HH:MM:SS`)
  instead of raw epoch values.
- `.gitmulti/config.toml` is created on first open, not just after `init`.

### Changed
- GUI is now a five-panel layout: `Remotes | Branches | Files | Detail`,
  with an interactive Git Graph mode.
- Cherry-pick modal: `Space` triggers cherry-pick, `Enter` accepts changes.
- `P` key for cherry-pick is only available in Files panel mode.
- Details pane in Commit mode shows commit metadata + diff instead of commit list.
- `v` toggle resets commit detail scroll when leaving commit view.
- All blocking git operations (fetch/push/pull/merge/commit/stage/reset/…)
  now run on a background worker thread in the TUI; every subprocess has a hard
  timeout, disabled prompts, and captured output, so the UI can never freeze,
  hang on a credential prompt, or be corrupted by git progress output.
- Non-interactive confirmations fail with a clear `--force` hint instead of
  hanging on a TTY read.
- `sync` uses the configured `default_strategy` when `--strategy` is omitted.

## [0.2.2] - 2026-07-12

### Added
- Multi-step merge flow (4-step) in GUI and CLI.
- Commits view in the GUI Details panel.
- Commit creation in the GUI (conventional types + subject + optional body).
- Global `Space` toggle now works regardless of focus.
- Release workflow producing `.deb`, `.rpm`, `.AppImage`, `.exe`/`.msi`, `.pkg`
  assets on GitHub tag push.
- Detailed installation instructions per package type in `INSTALL.md`.

[Unreleased]: https://github.com/CharaD7/git-multi/compare/v0.2.2...HEAD
[0.2.2]: https://github.com/CharaD7/git-multi/releases/tag/v0.2.2
