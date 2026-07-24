# git-multi

A CLI tool for managing multiple Git remotes and syncing content between them. `git-multi` simplifies workflows where you need to keep multiple remotes in sync, cherry-pick changes across different upstream repositories, or manage complex multi-remote branch structures.

## Features

- **Multi-Remote Management:** Easily add, rename, and track multiple Git remotes.
- **Cross-Remote Syncing:** Sync branches and cherry-pick commits between different remotes.
- **Unified Branch View:** See local and remote branches across all configured remotes in one view.
- **File Portability:** Copy specific files between different branches or remotes without full merges.
- **PR Integration:** Quick Pull Request creation using the `gh` CLI.
- **Default Remotes:** Set a default remote for streamlined commands.
- **Full Git Coverage:** commit & amend, granular stage/unstage, revert, reset (soft/mixed/hard),
  and cherry-pick — available both in the CLI and the GUI.
- **GitLens in the GUI:** per-line blame with author/commit/date, plus file history, so you can
  see who changed what and when without leaving the terminal.
- **Git Graph in the GUI:** an interactive commit DAG with branch/tag/remote ref labels, reachable
  from HEAD or all refs, with one-key cherry-pick of any commit.
- **Diffs everywhere:** unified diffs of staged / unstaged / HEAD changes, for the whole tree or a
  single file, in both CLI and GUI.
- **Auto-save safety net in the GUI:** when you are idle for ~30s, the current dirty state is
  captured into `refs/gitmulti/autosave` (not a normal branch). If something like an AI-driven
  `git reset --hard` happens, press `O` to restore from that snapshot without losing work.

## Installation

Build from source with Cargo:

```bash
cargo install --path .
```

For pre-built packages (`.deb`, `.rpm`, `.AppImage`, `.exe`/`.msi`, `.pkg`) with
step-by-step extract / install / run instructions for every platform, see
[`INSTALL.md`](./INSTALL.md) and the assets attached to each
[GitHub release](https://github.com/CharaD7/git-multi/releases).

## Usage

### GUI Mode
Launch the terminal user interface:
```bash
git-multi --gui
```
The GUI is a five-panel terminal UI: **Remotes | Branches | Files | Detail**, with an
interactive **Git Graph** mode. The remote, branch, and file lists auto-refresh on demand
(`r`), so changes made inside or outside the app show up when you refresh.

Navigation & global keys:
- `Tab` (or `←`/`→`) — cycle focus: Remotes → Branches → Files → Detail → Graph
- `↑` / `↓` — move the selection within the focused panel
- `Space` — toggle multi-select of a branch (branches show `[x]`/`[ ]`)
- `f` / `Enter` — fetch the selected remote (+ selected branches)
- `p` — push, `l` — pull the selected remote (+ selected branches)
- `M` — merge across remotes (4-step flow: source remote, source branch, dest remote, dest branch)
- `C` — create a commit (select type, enter subject, optional body)
- `A` — amend the last commit (enter a new message)
- `R` — revert a commit (enter a sha/ref)
- `Z` — reset current branch (soft/mixed/hard, then a target)
- `P` — cherry-pick a single commit onto HEAD
- `g` — Git Graph (commit DAG with branch/tag/remote labels; `a` toggles all-refs; `Enter` cherry-picks the selected commit)
- `b` — Blame the selected file (GitLens: author, commit, date per line)
- `d` — Diff (unstaged), `F` — Files panel, `s` — Status view, `v` — Commits view
- `S` — on a file: stage if unstaged, unstage if staged
- `O` — restore from auto-save (merge the hidden safety snapshot back into the working tree)
- `r` — force refresh, `q` — quit

Remote actions (when the **Remotes** panel is focused):
- `a` — add a remote (name, then URL)
- `R` — rename the selected remote
- `x` / `Delete` — remove the selected remote (confirm with `y`)
- `D` — set the selected remote as default

Branch actions (when the **Branches** panel is focused):
- `c` — create a branch (name, base, optional remote to push to)
- `m` — rename the selected branch
- `x` / `Delete` — delete the selected local branch (confirm with `y`)

With no branch selected, fetch/pull target all branches and push targets the current branch.

**Merge (GUI, `M`):** A 4-step interactive flow:
1. Source remote (e.g., `upstream`)
2. Source branch (e.g., `main`)
3. Destination remote (e.g., `origin`)
4. Destination branch (e.g., `main`)

**Commits (GUI, `v`):** View recent commits in the Details panel.

**Create Commit (GUI, `C`):** A 3-step flow for conventional commits:
1. Select type (`feat`, `fix`, `docs`, `style`, `refactor`, `test`, `chore`, `build`, `perf`)
2. Enter commit subject
3. Optional body (press Enter to skip)

> The GUI runs the same git subprocesses as the CLI, so it honours your
> `~/.ssh/config` host aliases (e.g. `github.com-personal`).

### Initialization

Initialize `git-multi` in your current Git repository:

```bash
git-multi init
```

### Remote Management

```bash
# Add a new remote
git-multi remote add upstream https://github.com/user/repo.git

# List all configured remote names
git-multi remote list-names

# Set a default remote
git-multi remote set-default upstream

# Set a primary remote
git-multi remote set-primary upstream

# Rename a remote
git-multi remote rename old-name new-name

# Remove a remote
git-multi remote remove upstream

# List all remotes
git-multi remote list --urls
```

### Branch Management

```bash
# List all local and remote branches
git-multi branch list --all

# Create a new branch locally
git-multi branch create feature-branch --base main --checkout

# Create a new branch locally and on multiple remotes
git-multi branch create feat/uat --remotes origin upstream backup --checkout

# Rename a local branch
git-multi branch rename old-branch new-branch

# Delete a local branch
git-multi branch delete feature-branch
```

### Syncing and Moving Content

```bash
# Sync changes from one remote branch to another
git-multi sync --from-remote upstream --to-remote origin --from-branch main --to-branch main

# Copy specific files from another branch
git-multi copy --from dev-branch --files src/main.rs src/utils.rs
```

### Merging across remotes

Merge a branch from one remote into the current local branch, optionally pushing the
result to another remote:

```bash
# Merge upstream/main into the current branch
git-multi merge --from-remote upstream --from-branch main

# Merge and push the result to origin
git-multi merge --from-remote upstream --from-branch main --to-remote origin --push
```

### Standard Git Operations (Enhanced)

```bash
# Fetch from all remotes
git-multi fetch --all

# Fetch from a specific remote (the current branch is used for push/pull)
git-multi fetch upstream

# Fetch specific branches from a remote
git-multi fetch upstream --branches main dev release

# Fetch every branch of a remote
git-multi fetch upstream --all-branches

# Push current branch to all remotes
git-multi push --all

# Push current branch to a specific remote
git-multi push upstream

# Push multiple branches to a specific remote
git-multi push upstream --branches main dev

# Push every local branch to a specific remote (optionally --force)
git-multi push upstream --all-branches
git-multi push upstream --all-branches --force

# Pull into the current branch from a specific remote
git-multi pull upstream

# Pull multiple branches from a specific remote
git-multi pull upstream --branches main dev

# Pull every branch of a remote
git-multi pull upstream --all-branches
```

### Committing, staging, and history

```bash
# Create a commit (stages all changes, then commits)
git-multi commit "feat: add retry logic"

# Create a commit with a body
git-multi commit "fix: null deref" --body "Guard the optional before unwrap."

# Amend the previous commit
git-multi commit "fix: null deref (amended)" --amend

# Stage / unstage individual files
git-multi stage src/main.rs          # not yet exposed as a subcommand; use the GUI (S) or `git`
git-multi unstage src/main.rs

# Diffs
git-multi diff unstaged              # working tree vs index
git-multi diff staged                # index vs HEAD
git-multi diff head                  # working tree vs HEAD
git-multi diff unstaged src/main.rs

# Revert / reset / cherry-pick
git-multi revert a1b2c3d4
git-multi reset mixed HEAD~1
git-multi reset hard a1b2c3d4
git-multi pick a1b2c3d4              # cherry-pick a single commit
```

### GitLens (blame & file history)

```bash
# Per-line blame for a file (author, commit, date, summary)
git-multi blame src/main.rs
git-multi blame src/main.rs --commit HEAD~2

# File history (commits that touched a file)
git-multi log src/main.rs
```

### Git Graph

```bash
# Commit DAG from HEAD
git-multi graph --limit 50

# Commit DAG including all branches and remote refs (with labels)
git-multi graph --all --limit 200
```

All of the above (plus granular stage/unstage, amend, revert, reset, and cherry-pick) are
also available directly inside the GUI — see [GUI Mode](#gui-mode).

### Auto-save safety net

When you are working in the TUI and stop typing for about **30 seconds**, `git-multi`
automatically takes a snapshot of your dirty working tree and stores it in a hidden
git ref: `refs/gitmulti/autosave`.

Key points:
- The auto-saved state is **not** a normal branch and does **not** appear in
  `git log main`. It only lives in the `refs/gitmulti/` namespace.
- The snapshot is only created if the repo actually has uncommitted changes.
- If an AI-assisted command or any other operation destroys your working tree
  (for example, `git reset --hard`), you can recover by pressing **`O`** in the
  TUI. That merges the auto-saved files back into the current working tree.
- Auto-save is **opt-out**. To disable it, set `auto_save = false` under
  `[gitmulti]` in `.gitmulti/config.toml`.

This is intended as a last-resort safety net, not a replacement for real commits.
When you are happy with your work, commit it normally so it becomes part of your
branch history.

## Configuration

`git-multi` stores its configuration in `.gitmulti/config.toml` within your repository. This file tracks default remotes, sync preferences, and metadata for each remote.

## License

MIT
