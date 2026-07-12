use clap::{Parser, Subcommand, ValueEnum};

/// Git Multi-Remote CLI
/// Manage multiple Git remotes and sync content between them
#[derive(Debug, Parser)]
#[command(name = "git-multi")]
#[command(author = "CharaD7")]
#[command(version = "0.1.0")]
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
    },

    /// Pull from remotes
    Pull {
        /// Pull from all remotes
        #[arg(short, long)]
        all: bool,
        
        /// Specific remote to pull from
        #[arg(value_name = "REMOTE")]
        remote: Option<String>,
        
        /// Branch to pull
        #[arg(short, long, value_name = "BRANCH")]
        branch: Option<String>,
    },

    /// Push to remotes
    Push {
        /// Push to all remotes
        #[arg(short, long)]
        all: bool,
        
        /// Specific remote to push to
        #[arg(value_name = "REMOTE")]
        remote: Option<String>,
        
        /// Branch to push
        #[arg(short, long, value_name = "BRANCH")]
        branch: Option<String>,
        
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
