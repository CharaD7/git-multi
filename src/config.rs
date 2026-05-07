use crate::error::{GitMultiError, Result};
use git2::Repository;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self};

const CONFIG_DIR: &str = ".gitmulti";
const CONFIG_FILE: &str = "config.toml";

/// Configuration for a single remote
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteConfig {
    pub url: String,
    #[serde(default)]
    pub push_url: Option<String>,
    #[serde(default)]
    pub fetch_refspecs: Vec<String>,
    #[serde(default)]
    pub push_refspecs: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub is_primary: bool,
}

/// Main configuration structure
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub remotes: HashMap<String, RemoteConfig>,
    #[serde(default)]
    pub default_remote: Option<String>,
    #[serde(default)]
    pub sync_preferences: SyncPreferences,
}

/// Sync preferences for default behaviors
#[derive(Debug, Serialize, Deserialize)]
pub struct SyncPreferences {
    #[serde(default = "default_sync_strategy")]
    pub default_strategy: String,
    #[serde(default)]
    pub auto_fetch: bool,
    #[serde(default)]
    pub auto_push: bool,
}

impl Default for SyncPreferences {
    fn default() -> Self {
        Self {
            default_strategy: default_sync_strategy(),
            auto_fetch: true,
            auto_push: false,
        }
    }
}

fn default_sync_strategy() -> String {
    "cherry-pick".to_string()
}

impl Config {
    /// Load config from the repository's .gitmulti/config.toml
    pub fn load(repo: &Repository) -> Result<Self> {
        let config_path = Self::get_config_path(repo)?;
        
        if !config_path.exists() {
            return Ok(Self::default());
        }

        let config_content = fs::read_to_string(&config_path)?;
        let config: Config = toml::from_str(&config_content)
            .map_err(GitMultiError::TomlDeserializeError)?;

        Ok(config)
    }

    /// Save config to the repository's .gitmulti/config.toml
    pub fn save(&self, repo: &Repository) -> Result<()> {
        let config_path = Self::get_config_path(repo)?;
        
        // Create .gitmulti directory if it doesn't exist
        if let Some(dir) = config_path.parent() {
            fs::create_dir_all(dir)?;
        }

        let config_content = toml::to_string(self)
            .map_err(GitMultiError::TomlSerializeError)?;
        
        fs::write(&config_path, config_content)?;
        
        Ok(())
    }

    /// Get the path to the config file
    pub fn get_config_path(repo: &Repository) -> Result<std::path::PathBuf> {
        let repo_path = repo.path();
        
        Ok(repo_path.join(CONFIG_DIR).join(CONFIG_FILE))
    }

    /// Add a remote to the config
    pub fn add_remote(&mut self, name: String, url: String) -> Result<()> {
        if self.remotes.contains_key(&name) {
            return Err(GitMultiError::RemoteAlreadyExists(name));
        }

        let config = RemoteConfig {
            url,
            push_url: None,
            fetch_refspecs: vec!["+refs/heads/*:refs/remotes/{}/*".to_string()],
            push_refspecs: vec!["refs/heads/*:refs/heads/*".to_string()],
            tags: vec![".*".to_string()],
            is_primary: self.remotes.is_empty(),
        };

        self.remotes.insert(name.clone(), config);
        
        // Set as default if it's the first remote
        if self.default_remote.is_none() {
            self.default_remote = Some(name);
        }

        Ok(())
    }

    /// Remove a remote from the config
    pub fn remove_remote(&mut self, name: &str) -> Result<()> {
        if !self.remotes.contains_key(name) {
            return Err(GitMultiError::RemoteNotFound(name.to_string()));
        }

        self.remotes.remove(name);
        
        // Update default remote if needed
        if self.default_remote.as_deref() == Some(name) {
            self.default_remote = self.remotes.keys().next().cloned();
        }

        Ok(())
    }

    /// Get remote config by name
    pub fn get_remote(&self, name: &str) -> Result<&RemoteConfig> {
        self.remotes.get(name)
            .ok_or_else(|| GitMultiError::RemoteNotFound(name.to_string()))
    }

    /// Get all remote names
    pub fn get_remote_names(&self) -> Vec<&String> {
        self.remotes.keys().collect()
    }

    /// Set default remote
    pub fn set_default_remote(&mut self, name: String) -> Result<()> {
        if !self.remotes.contains_key(&name) {
            return Err(GitMultiError::RemoteNotFound(name));
        }
        self.default_remote = Some(name);
        Ok(())
    }

    /// Get default remote name
    pub fn get_default_remote(&self) -> Option<&String> {
        self.default_remote.as_ref()
    }

    /// Mark a remote as primary
    pub fn set_primary_remote(&mut self, name: &str) -> Result<()> {
        if !self.remotes.contains_key(name) {
            return Err(GitMultiError::RemoteNotFound(name.to_string()));
        }
        
        // Clear primary flag from all remotes
        for config in self.remotes.values_mut() {
            config.is_primary = false;
        }
        
        // Set as primary
        if let Some(config) = self.remotes.get_mut(name) {
            config.is_primary = true;
        }
        
        Ok(())
    }

    /// Get primary remote
    pub fn get_primary_remote(&self) -> Option<(&String, &RemoteConfig)> {
        self.remotes.iter()
            .find(|(_, config)| config.is_primary)
    }
}

/// Initialize a new git-multi configuration in a repository
pub fn init_config(repo: &Repository) -> Result<Config> {
    let config = Config::default();
    config.save(repo)?;
    Ok(config)
}
