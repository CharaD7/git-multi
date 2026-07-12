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
The GUI shows an interactive **list of remotes** (with the default marked) and, for the
selected remote, a **list of its branches** you can multi-select. From here you can:
- **Add a remote**: press `a`, type the remote name, press Enter, type the URL, press Enter.
- **Switch focus** between the remotes and branches panels with `Tab` (or `←`/`→`).
- **Select a remote / move**: use `↑`/`↓`.
- **Multi-select branches**: focus the branches panel and press `Space` to toggle each branch.
  With no branch selected, actions target all branches (or the current branch for push/pull).
- **Fetch / Push / Pull** the selected remote and chosen branches: press `f` (or Enter), `p`, or `l`.
- **Refresh** the lists with `r`, and **quit** with `q`.

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
```

### Syncing and Moving Content

```bash
# Sync changes from one remote branch to another
git-multi sync --from-remote upstream --to-remote origin --from-branch main --to-branch main

# Copy specific files from another branch
git-multi copy --from dev-branch --files src/main.rs src/utils.rs
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
