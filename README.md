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

### Initialization

Initialize `git-multi` in your current Git repository:

```bash
git-multi init
```

### Remote Management

```bash
# Add a new remote
git-multi remote add upstream https://github.com/user/repo.git

# List all remotes
git-multi remote list --urls

# Show details of a specific remote
git-multi remote show upstream
```

### Branch Management

```bash
# List all local and remote branches
git-multi branch list --all

# Create a new branch from a base
git-multi branch create feature-branch --base main --checkout
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

# Push current branch to all remotes
git-multi push --all
```

## Configuration

`git-multi` stores its configuration in `.gitmulti/config.toml` within your repository. This file tracks default remotes, sync preferences, and metadata for each remote.

## License

MIT
