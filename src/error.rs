use thiserror::Error;

#[derive(Debug, Error)]
#[allow(dead_code)]
pub enum GitMultiError {
    #[error("Git error: {0}")]
    GitError(#[from] git2::Error),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Config error: {0}")]
    ConfigError(String),

    #[error("Toml serialization error: {0}")]
    TomlSerializeError(#[from] toml::ser::Error),

    #[error("Toml deserialization error: {0}")]
    TomlDeserializeError(#[from] toml::de::Error),

    #[error("Dialoguer error: {0}")]
    DialoguerError(#[from] dialoguer::Error),

    #[error("Remote '{0}' not found")]
    RemoteNotFound(String),

    #[error("Remote '{0}' already exists")]
    RemoteAlreadyExists(String),

    #[error("Branch '{0}' not found")]
    BranchNotFound(String),

    #[error("No remotes configured")]
    NoRemotesConfigured,

    #[error("Destination branch '{0}' already exists. Use --force to overwrite.")]
    BranchExists(String),

    #[error("Sync failed: {0}")]
    SyncError(String),

    #[error("No commits to sync in range: {0}")]
    NoCommitsInRange(String),

    #[error("Conflict detected during sync. Resolve manually.")]
    SyncConflict,

    #[error("User cancelled operation")]
    UserCancelled,
}

pub type Result<T> = std::result::Result<T, GitMultiError>;
