# git-multi

A CLI tool for managing multiple Git remotes and syncing content between them. `git-multi` simplifies workflows where you need to keep multiple remotes in sync, cherry-pick changes across different upstream repositories, or manage complex multi-remote branch structures.

## Features

- **Multi-Remote Management:** Easily add, rename, and track multiple Git remotes.
- **Cross-Remote Syncing:** Sync branches and cherry-pick commits between different remotes.
- **Unified Branch View:** See local and remote branches across all configured remotes in one view.
- **File Portability:** Copy specific files between different branches or remotes without full merges.
- **PR Integration:** Quick Pull Request creation using the `gh` CLI.
- **Default Remotes:** Set a default remote for streamlined commands.

## Installation

```bash
cargo install --path .
```

## Usage

### GUI Mode
Launch the terminal user interface:
```bash
git-multi --gui
```
The GUI is a three-panel terminal UI: **Remotes | Branches | Details**, with the Details
panel doubling as a live **Status** view. The remote and branch lists auto-refresh in real
time (every ~200ms), so changes made inside or outside the app show up immediately.

Navigation & global keys:
- `Tab` (or `←`/`→`) — switch focus between the Remotes and Branches panels
- `↑` / `↓` — move the selection within the focused panel
- `Space` — toggle multi-select of a branch (branches show `[x]`/`[ ]`)
- `f` / `Enter` — fetch the selected remote (+ selected branches)
- `p` — push, `l` — pull the selected remote (+ selected branches)
- `M` (Shift+M) — merge a branch from one remote into the current branch and push to the selected remote
- `s` — toggle the Status view (remotes, local branches, working tree)
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

## Configuration

`git-multi` stores its configuration in `.gitmulti/config.toml` within your repository. This file tracks default remotes, sync preferences, and metadata for each remote.

## License

MIT
