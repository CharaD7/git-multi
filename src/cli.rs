use clap::{Parser, Subcommand, ValueEnum};

/// Git Multi-Remote CLI
/// Manage multiple Git remotes and sync content between them
#[derive(Debug, Parser)]
#[command(name = "git-multi")]
#[command(author = "CharaD7")]
#[command(version = "0.1.1")]
#[command(about = "A CLI tool for managing multiple Git remotes", long_about = None)]
pub struct Cli {
    /// Increase verbosity
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Launch GUI
    #[arg(short, long)]
    pub gui: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Initialize git-multi configuration in the current repository
    Init,

    /// Manage remotes
    Remote {
        #[command(subcommand)]
        command: RemoteCommands,
    },

    /// Manage branches
    Branch {
        #[command(subcommand)]
        command: BranchCommands,
    },

    /// Fetch from remotes
    Fetch {
        /// Fetch from all remotes
        #[arg(short, long)]
        all: bool,

        /// Specific remote to fetch from
        #[arg(value_name = "REMOTE")]
        remote: Option<String>,

        /// Specific branches to fetch (repeatable)
        #[arg(long, value_name = "BRANCH", num_args = 1..)]
        branches: Vec<String>,

        /// Fetch all branches of the remote
        #[arg(long)]
        all_branches: bool,
    },

    /// Pull from remotes
    Pull {
        /// Pull from all remotes
        #[arg(short, long)]
        all: bool,

        /// Specific remote to pull from
        #[arg(value_name = "REMOTE")]
        remote: Option<String>,

        /// Specific branches to pull (repeatable)
        #[arg(long, value_name = "BRANCH", num_args = 1..)]
        branches: Vec<String>,

        /// Pull all branches of the remote
        #[arg(long)]
        all_branches: bool,
    },

    /// Push to remotes
    Push {
        /// Push to all remotes
        #[arg(short, long)]
        all: bool,

        /// Specific remote to push to
        #[arg(value_name = "REMOTE")]
        remote: Option<String>,

        /// Specific branches to push (repeatable)
        #[arg(long, value_name = "BRANCH", num_args = 1..)]
        branches: Vec<String>,

        /// Push all local branches to the remote
        #[arg(long)]
        all_branches: bool,

        /// Force push
        #[arg(short, long)]
        force: bool,
    },

    /// Checkout a branch
    Checkout {
        /// Branch name
        #[arg(value_name = "BRANCH")]
        branch: String,
        
        /// Remote to checkout from
        #[arg(short, long, value_name = "REMOTE")]
        remote: Option<String>,
        
        /// Create a new branch
        #[arg(short, long)]
        new: bool,
    },

    /// Sync content between remotes
    Sync {
        /// Source remote name
        #[arg(long, value_name = "REMOTE")]
        from_remote: String,
        
        /// Destination remote name
        #[arg(long, value_name = "REMOTE")]
        to_remote: String,
        
        /// Source branch
        #[arg(long, value_name = "BRANCH", default_value = "main")]
        from_branch: String,
        
        /// Destination branch
        #[arg(long, value_name = "BRANCH", default_value = "main")]
        to_branch: String,
        
        /// Commit range to sync (e.g., HEAD~3..HEAD, abc123..def456)
        #[arg(short, long, value_name = "RANGE", default_value = "HEAD")]
        commits: String,
        
        /// Sync strategy
        #[arg(short, long, value_enum, default_value = "cherry-pick")]
        strategy: SyncStrategy,
        
        /// Force sync (overwrite existing branch)
        #[arg(short, long)]
        force: bool,
    },

    /// Copy files from one remote/branch to another
    Copy {
        /// Source in format: REMOTE:BRANCH or just BRANCH (for local)
        #[arg(long, value_name = "FROM")]
        from: String,
        
        /// Destination in format: REMOTE:BRANCH or just BRANCH (for local) (optional)
        #[arg(long, value_name = "TO")]
        to: Option<String>,
        
        /// Files to copy (glob patterns)
        #[arg(long, value_name = "FILES", num_args = 1..)]
        files: Vec<String>,
        
        /// Delete files in destination that don't exist in source
        #[arg(short, long)]
        prune: bool,
    },

    /// Create a Pull Request
    Pr {
        /// Remote to create PR on
        #[arg(value_name = "REMOTE")]
        remote: String,
        
        /// Base branch
        #[arg(short, long, value_name = "BRANCH", default_value = "main")]
        base: String,
        
        /// Head branch (current branch if not specified)
        #[arg(short, long, value_name = "BRANCH")]
        head: Option<String>,
        
        /// PR title
        #[arg(short, long, value_name = "TITLE")]
        title: String,
        
        /// PR description
        #[arg(short, long, value_name = "DESCRIPTION")]
        description: Option<String>,
        
        /// Open PR in browser after creation
        #[arg(short, long)]
        open: bool,
    },

    /// Merge a branch from one remote into the current local branch and
    /// optionally push the result to another remote.
    Merge {
        /// Source remote name
        #[arg(long, value_name = "REMOTE")]
        from_remote: String,

        /// Source branch
        #[arg(long, value_name = "BRANCH", default_value = "main")]
        from_branch: String,

        /// Local branch to merge into (defaults to current branch)
        #[arg(long, value_name = "BRANCH")]
        to_branch: Option<String>,

        /// Remote to push the merged result to
        #[arg(long, value_name = "REMOTE")]
        to_remote: Option<String>,

        /// Push the merged result after a clean merge
        #[arg(short, long)]
        push: bool,
    },

    /// Set default remote
    Use {
        /// Remote name to use as default
        #[arg(value_name = "REMOTE")]
        remote: String,
    },

    /// Show status of remotes and branches
    Status,

    /// List all remotes with their branches
    List,

    /// Create a commit with message (subject + optional body)
    Commit {
        /// Commit subject
        #[arg(value_name = "SUBJECT")]
        subject: String,

        /// Optional commit body
        #[arg(short, long, value_name = "BODY")]
        body: Option<String>,

        /// Amend the previous commit instead of creating a new one
        #[arg(short, long)]
        amend: bool,
    },

    /// Show a diff (staged, unstaged, or against HEAD)
    Diff {
        /// What to diff against
        #[arg(value_name = "WHAT", default_value = "unstaged")]
        what: String,

        /// Restrict to a path
        #[arg(value_name = "PATH")]
        path: Option<String>,
    },

    /// Show GitLens-style blame for a file
    Blame {
        /// File to blame
        #[arg(value_name = "PATH")]
        path: String,

        /// Blame at this commit instead of the working tree
        #[arg(short, long, value_name = "COMMIT")]
        commit: Option<String>,
    },

    /// Show GitLens-style file history
    Log {
        /// File to show history for (defaults to full repo log)
        #[arg(value_name = "PATH")]
        path: Option<String>,

        /// Number of entries
        #[arg(short, long, default_value_t = 20)]
        count: usize,
    },

    /// Show the commit DAG (Git Graph)
    Graph {
        /// Include all refs (branches + remotes), not just HEAD
        #[arg(short, long)]
        all: bool,

        /// Maximum number of commits to show
        #[arg(short, long, default_value_t = 100)]
        limit: usize,
    },

    /// Revert a commit by creating a new revert commit
    Revert {
        /// Commit to revert (sha or ref)
        #[arg(value_name = "COMMIT")]
        commit: String,
    },

    /// Reset the current branch
    Reset {
        /// Reset mode: soft | mixed | hard
        #[arg(value_name = "MODE", default_value = "mixed")]
        mode: String,

        /// Target (sha or ref, defaults to HEAD)
        #[arg(value_name = "TARGET", default_value = "HEAD")]
        target: String,
    },

    /// Cherry-pick a single commit onto the current HEAD
    Pick {
        /// Commit to cherry-pick (sha or ref)
        #[arg(value_name = "COMMIT")]
        commit: String,
    },

    /// Stage a file (or all changes with ".")
    Stage {
        /// Path to stage (use "." for everything)
        #[arg(value_name = "PATH", default_value = ".")]
        path: String,
    },

    /// Unstage a file (keep working-tree contents)
    Unstage {
        /// Path to unstage
        #[arg(value_name = "PATH")]
        path: String,
    },

    /// Discard working-tree changes for a file (restore from index/HEAD)
    Restore {
        /// Path to restore
        #[arg(value_name = "PATH")]
        path: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum RemoteCommands {
    /// Add a new remote
    Add {
        /// Remote name
        #[arg(value_name = "NAME")]
        name: String,
        
        /// Remote URL
        #[arg(value_name = "URL")]
        url: String,
        
        /// Set as default remote
        #[arg(short, long)]
        default: bool,
    },

    /// Remove a remote
    Remove {
        /// Remote name
        #[arg(value_name = "NAME")]
        name: String,
        
        /// Force removal without confirmation
        #[arg(short, long)]
        force: bool,
    },

    /// List all remotes
    List {
        /// Show URLs
        #[arg(short, long)]
        urls: bool,
    },

    /// Rename a remote
    Rename {
        /// Current remote name
        #[arg(value_name = "OLD_NAME")]
        old_name: String,
        
        /// New remote name
        #[arg(value_name = "NEW_NAME")]
        new_name: String,
    },

    /// Show remote details
    Show {
        /// Remote name
        #[arg(value_name = "NAME")]
        name: String,
    },

    /// List all configured remote names
    ListNames {},
    
    /// Set default remote
    SetDefault {
        /// Remote name
        #[arg(value_name = "NAME")]
        name: String,
    },

    /// Set primary remote
    SetPrimary {
        /// Remote name
        #[arg(value_name = "NAME")]
        name: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum BranchCommands {
    /// List branches
    List {
        /// Show all branches (including remote)
        #[arg(short, long)]
        all: bool,
        
        /// Show branches for a specific remote
        #[arg(short, long, value_name = "REMOTE")]
        remote: Option<String>,
    },

    /// Delete a branch
    Delete {
        /// Branch name
        #[arg(value_name = "BRANCH")]
        branch: String,
        
        /// Force deletion
        #[arg(short, long)]
        force: bool,
        
        /// Delete from remote
        #[arg(short, long)]
        remote: bool,
    },

    /// Create a new branch
    Create {
        /// Create a new branch
        #[arg(value_name = "BRANCH")]
        branch: String,

        /// Base branch
        #[arg(short, long, value_name = "BASE", default_value = "main")]
        base: String,

        /// Remotes to create the branch on
        #[arg(short, long, value_name = "REMOTES", num_args = 1..)]
        remotes: Option<Vec<String>>,

        /// Checkout the new branch
        #[arg(short, long)]
        checkout: bool,
        },

    /// Rename a local branch
    Rename {
        /// Current branch name
        #[arg(value_name = "OLD_NAME")]
        old_name: String,

        /// New branch name
        #[arg(value_name = "NEW_NAME")]
        new_name: String,
    },
}

/// Sync strategy for syncing content between remotes
#[derive(Debug, Clone, Copy, ValueEnum, Default)]
pub enum SyncStrategy {
    /// Cherry-pick individual commits
    #[default]
    CherryPick,
    /// Merge branches
    Merge,
    /// Rebase onto target branch
    Rebase,
}

impl std::fmt::Display for SyncStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SyncStrategy::CherryPick => write!(f, "cherry-pick"),
            SyncStrategy::Merge => write!(f, "merge"),
            SyncStrategy::Rebase => write!(f, "rebase"),
        }
    }
}

impl std::str::FromStr for SyncStrategy {
    type Err = String;
    
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "cherry-pick" | "cherrypick" | "pick" => Ok(SyncStrategy::CherryPick),
            "merge" => Ok(SyncStrategy::Merge),
            "rebase" => Ok(SyncStrategy::Rebase),
            _ => Err(format!("Unknown strategy: {}", s)),
        }
    }
}
