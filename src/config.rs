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
    #[serde(default)]
    pub gui: GuiPreferences,
    #[serde(default)]
    pub identity: IdentityPreferences,
    #[serde(default)]
    pub animations: AnimationPrefs,
}

/// Optional display-name overrides for the identity shown in the welcome
/// screen and the playground top bar.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IdentityPreferences {
    /// Override the detected device (host) name.
    pub device: Option<String>,
    /// Override the detected system username.
    pub username: Option<String>,
}

/// TUI rendering-animation preferences (`[animations]`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnimationPrefs {
    /// Master switch — when false, every animation renders instantly.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Global duration multiplier (0.5 = half, 2.0 = double).
    #[serde(default = "default_speed")]
    pub speed: f64,
    /// Max background-dim intensity for the overlay fade.
    #[serde(default = "default_dim")]
    pub dim: f64,
    /// Modal open animation (slide + fade).
    #[serde(default = "default_true")]
    pub overlay: bool,
    #[serde(default = "default_overlay_ms")]
    pub overlay_ms: u64,
    /// Focus-change border pulse.
    #[serde(default = "default_true")]
    pub focus: bool,
    #[serde(default = "default_focus_ms")]
    pub focus_ms: u64,
    /// Detail panel view transitions (wipe-in).
    #[serde(default = "default_true")]
    pub panel: bool,
    #[serde(default = "default_panel_ms")]
    pub panel_ms: u64,
    /// Pane-refresh border pulse.
    #[serde(default = "default_true")]
    pub refresh: bool,
    #[serde(default = "default_refresh_ms")]
    pub refresh_ms: u64,
}

impl Default for AnimationPrefs {
    fn default() -> Self {
        Self {
            enabled: true,
            speed: 1.0,
            dim: 0.35,
            overlay: true,
            overlay_ms: 200,
            focus: true,
            focus_ms: 180,
            panel: true,
            panel_ms: 200,
            refresh: true,
            refresh_ms: 150,
        }
    }
}

fn default_speed() -> f64 {
    1.0
}

fn default_dim() -> f64 {
    0.35
}

fn default_overlay_ms() -> u64 {
    200
}

fn default_focus_ms() -> u64 {
    180
}

fn default_panel_ms() -> u64 {
    200
}

fn default_refresh_ms() -> u64 {
    150
}

/// GUI behaviour preferences (idle tips, previews, GitHub integration).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuiPreferences {
    /// Show per-pane action tips after idling on a pane.
    #[serde(default = "default_true")]
    pub idle_tips: bool,
    /// Seconds of idle before tips/previews appear.
    #[serde(default = "default_idle_delay")]
    pub idle_tip_delay_secs: u64,
    /// Show hover previews (heat bars, branch info, ...) when idle.
    #[serde(default = "default_true")]
    pub idle_previews: bool,
    /// GitHub backend: "auto" (gh CLI) or "gh".
    #[serde(default = "default_github")]
    pub github: String,
    /// Fallback for contributor data when gh is unavailable.
    #[serde(default = "default_fallback")]
    pub contributors_fallback: String,
    /// Default PR state filter in the PRs modal ("open", "closed", "merged").
    #[serde(default = "default_pr_state")]
    pub pr_default_state: String,
    /// Show the animated welcome screen on launch.
    #[serde(default = "default_true")]
    pub show_welcome: bool,
}

impl Default for GuiPreferences {
    fn default() -> Self {
        Self {
            idle_tips: true,
            idle_tip_delay_secs: 10,
            idle_previews: true,
            github: "auto".to_string(),
            contributors_fallback: "shortlog".to_string(),
            pr_default_state: "open".to_string(),
            show_welcome: true,
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_idle_delay() -> u64 {
    10
}

fn default_github() -> String {
    "auto".to_string()
}

fn default_fallback() -> String {
    "shortlog".to_string()
}

fn default_pr_state() -> String {
    "open".to_string()
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
            let mut config = Self::default();
            config.reconcile_with_git(repo)?;
            // Persist the reconciled config so the file exists on first open
            // (matches the README, which documents `.gitmulti/config.toml`).
            if config_path.parent().is_some() {
                config.save(repo)?;
            }
            return Ok(config);
        }

        let config_content = fs::read_to_string(&config_path)?;
        let mut config: Config = toml::from_str(&config_content)
            .map_err(GitMultiError::TomlDeserializeError)?;

        // Keep the git-multi remote set in sync with the actual git remotes
        // so `list-names`, the GUI list, and default-remote tracking are accurate.
        config.reconcile_with_git(repo)?;

        Ok(config)
    }

    /// Add any git remotes missing from the config and drop config entries
    /// whose git remote no longer exists. Also repairs the default remote.
    pub fn reconcile_with_git(&mut self, repo: &Repository) -> Result<()> {
        let git_remotes = repo.remotes()?;
        let git_names: Vec<String> = git_remotes
            .iter()
            .flatten()
            .map(|s| s.to_string())
            .collect();

        for name in &git_names {
            if !self.remotes.contains_key(name) {
                let url = repo
                    .find_remote(name)
                    .ok()
                    .and_then(|r| r.url().map(|u| u.to_string()))
                    .unwrap_or_default();
                self.remotes.insert(
                    name.clone(),
                    RemoteConfig {
                        url,
                        push_url: None,
                        fetch_refspecs: vec!["+refs/heads/*:refs/remotes/{}/*".to_string()],
                        push_refspecs: vec!["refs/heads/*:refs/heads/*".to_string()],
                        tags: vec![".*".to_string()],
                        is_primary: self.remotes.is_empty(),
                    },
                );
            }
        }

        // Remove config entries whose git remote no longer exists.
        let stale: Vec<String> = self
            .remotes
            .keys()
            .filter(|n| !git_names.iter().any(|g| g == *n))
            .cloned()
            .collect();
        for name in stale {
            self.remotes.remove(&name);
        }

        // Ensure the default remote still points at an existing remote.
        if self.default_remote.as_deref().is_none_or(|d| !git_names.iter().any(|g| g == d)) {
            self.default_remote = git_names.first().cloned();
        }

        Ok(())
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
        // repo.workdir() returns the repo root for non-bare repos (.git is inside it).
        // repo.path() returns the .git directory itself, which is NOT what the README documents.
        let root = repo.workdir()
            .unwrap_or_else(|| repo.path()); // bare repo fallback

        Ok(root.join(CONFIG_DIR).join(CONFIG_FILE))
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
    let mut config = Config::default();
    config.reconcile_with_git(repo)?;
    config.save(repo)?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_prefs_default_and_override() {
        let d = IdentityPreferences::default();
        assert!(d.device.is_none());
        assert!(d.username.is_none());

        let parsed: Config = toml::from_str("[identity]\ndevice = \"MyPC\"\nusername = \"alice\"\n").unwrap();
        assert_eq!(parsed.identity.device.as_deref(), Some("MyPC"));
        assert_eq!(parsed.identity.username.as_deref(), Some("alice"));
    }

    #[test]
    fn animation_prefs_partial_table_falls_back() {
        let cfg: Config = toml::from_str("[animations]\nenabled = false\noverlay_ms = 350\n").unwrap();
        let a = &cfg.animations;
        assert!(!a.enabled);
        assert_eq!(a.overlay_ms, 350);
        assert_eq!(a.focus_ms, 180); // default
        assert!((a.speed - 1.0).abs() < 1e-9);
    }
}
