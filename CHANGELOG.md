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
- Full git coverage: granular stage/unstage/restore, amend, revert, reset
  (soft/mixed/hard), cherry-pick, unified diffs (staged/unstaged/HEAD).
- GitLens features: per-line blame, file history, line history.
- Git Graph: commit DAG with branch/tag/remote ref labels, reachable from HEAD
  or all refs, with one-key cherry-pick of any node.
- CLI subcommands: `commit`, `diff`, `blame`, `log`, `graph`, `revert`, `reset`,
  `pick`, `stage`, `unstage`, `restore`.

### Changed
- GUI is now a five-panel layout: `Remotes | Branches | Files | Detail`,
  with an interactive Git Graph mode.

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
