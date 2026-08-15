use crate::config::Config;
use crate::error::{GitMultiError, Result};
use git2::{BranchType, Repository};
use std::collections::HashMap;
use std::fs;
use std::io::{self, Read};
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

pub use glob::Pattern as GlobPattern;

/// Wrapper around git2::Repository with additional functionality
pub struct GitRepo {
    pub repo: Repository,
    pub config: Config,
}

#[allow(dead_code)]
impl GitRepo {
    /// Open a git repository in the current directory
    pub fn open() -> Result<Self> {
        let repo = Repository::open_from_env()?;
        let config = Config::load(&repo)?;
        Ok(Self { repo, config })
    }

    /// Open a git repository at a specific path
    pub fn open_at(path: &std::path::Path) -> Result<Self> {
        let repo = Repository::open(path)?;
        let config = Config::load(&repo)?;
        Ok(Self { repo, config })
    }

    /// Initialize a new git-multi configuration
    pub fn init() -> Result<Self> {
        let repo = Repository::open_from_env()?;
        let config = crate::config::init_config(&repo)?;
        Ok(Self { repo, config })
    }

    /// Add a remote to both git config and git-multi config
    pub fn add_remote(&mut self, name: &str, url: &str) -> Result<()> {
        // Add to git config
        self.repo.remote(name, url)?;
        
        // Add to git-multi config
        self.config.add_remote(name.to_string(), url.to_string())?;
        self.config.save(&self.repo)?;
        
        Ok(())
    }

    /// Remove a remote from both git config and git-multi config
    pub fn remove_remote(&mut self, name: &str) -> Result<()> {
        // Remove from git config
        self.repo.remote_delete(name)?;
        
        // Remove from git-multi config
        self.config.remove_remote(name)?;
        self.config.save(&self.repo)?;
        
        Ok(())
    }

    /// Get a git2 Remote object
    pub fn get_remote(&self, name: &str) -> Result<git2::Remote<'_>> {
        self.repo.find_remote(name)
            .map_err(|_| GitMultiError::RemoteNotFound(name.to_string()))
    }

    /// List all remotes (from git config)
    pub fn list_remotes(&self) -> Result<Vec<String>> {
        let remote_names = self.repo.remotes()?;
        Ok(remote_names.iter().flatten().map(|s| s.to_string()).collect())
    }

    /// List all remotes with their URLs
    pub fn list_remotes_with_urls(&self) -> Result<Vec<(String, String)>> {
        let remote_names = self.repo.remotes()?;
        let mut remotes = Vec::new();
        
        for name in remote_names.iter().flatten() {
            let remote = self.repo.find_remote(name)?;
            let url = remote.url().unwrap_or("unknown").to_string();
            remotes.push((name.to_string(), url));
        }
        
        Ok(remotes)
    }

    /// Fetch from a specific remote using the system git binary
    /// (libgit2 does not honour ~/.ssh/config Host aliases)
    pub fn fetch_remote(&self, name: &str) -> Result<()> {
        let workdir = self.repo.workdir().unwrap_or_else(|| self.repo.path());
        git_run_net_str(workdir, &["fetch", name])?;
        Ok(())
    }

    /// Fetch from all remotes, running the fetches concurrently. Fetches that
    /// fail are collected and reported together instead of aborting the batch.
    pub fn fetch_all(&self) -> Result<Vec<String>> {
        let remote_names = self.repo.remotes()?;
        let workdir = self.repo.workdir().unwrap_or_else(|| self.repo.path());
        let names: Vec<String> = remote_names.iter().flatten().map(|s| s.to_string()).collect();

        let results: Vec<(String, Result<()>)> = names
            .into_iter()
            .map(|name| {
                let wd = workdir.to_path_buf();
                std::thread::spawn(move || {
                    let name_for_thread = name.clone();
                    let r = git_run_net_str(&wd, &["fetch", &name]);
                    (name_for_thread, r.map(|_| ()))
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|h| h.join().unwrap_or_else(|_| (String::new(), Err(GitMultiError::SyncError("fetch thread panicked".to_string())))))
            .collect();

        let mut fetched = Vec::new();
        let mut errors = Vec::new();
        for (name, res) in results {
            match res {
                Ok(()) => fetched.push(name),
                Err(e) => errors.push(format!("{}: {}", name, e)),
            }
        }
        if !errors.is_empty() {
            return Err(GitMultiError::SyncError(format!(
                "{} of {} fetch(es) failed: {}",
                errors.len(),
                fetched.len() + errors.len(),
                errors.join("; ")
            )));
        }
        Ok(fetched)
    }

    /// Checkout a branch
    pub fn checkout_branch(&self, branch_name: &str) -> Result<()> {
        let branch = self.repo.find_branch(branch_name, BranchType::Local)?;
        let commit_oid = branch.get().target().ok_or_else(|| GitMultiError::GitError(git2::Error::from_str("Branch has no target")))?;
        let commit_obj = self.repo.find_object(commit_oid, None)?;
        
        self.repo.checkout_tree(&commit_obj, None)?;
        self.repo.set_head(&format!("refs/heads/{}", branch_name))?;
        
        Ok(())
    }

    /// Checkout a branch from a specific remote
    pub fn checkout_remote_branch(&self, remote_name: &str, branch_name: &str) -> Result<()> {
        let _remote = self.repo.find_remote(remote_name)?;
        let ref_name = format!("refs/remotes/{}/{}", remote_name, branch_name);
        
        // Fetch the remote branch
        self.fetch_remote(remote_name)?;
        
        // Get the remote reference
        let remote_ref = self.repo.find_reference(&ref_name)?;
        let commit_oid = remote_ref.target().ok_or_else(|| GitMultiError::GitError(git2::Error::from_str("Remote reference has no target")))?;
        let commit_obj = self.repo.find_object(commit_oid, None)?;
        
        // Checkout the commit
        self.repo.checkout_tree(&commit_obj, None)?;
        
        // Create a local branch tracking the remote
        let local_branch_name = branch_name;
        let commit = self.repo.find_commit(commit_oid)?;
        let mut branch = self.repo.branch(local_branch_name, &commit, false)?;
        branch.set_upstream(Some(&ref_name))?;
        
        self.repo.set_head(&format!("refs/heads/{}", local_branch_name))?;
        
        Ok(())
    }

    /// List all branches (local + remote)
    pub fn list_all_branches(&self) -> Result<BranchesInfo> {
        let mut info = BranchesInfo::default();
        
        // Local branches
        for branch_res in self.repo.branches(Some(BranchType::Local))? {
            let (branch, _) = branch_res?;
            let name = branch.name()?.unwrap_or("").to_string();
            let is_head = branch.is_head();
            info.local.push(BranchInfo { name, is_head, upstream: None });
        }
        
        // Remote branches
        let remote_names = self.repo.remotes()?;
        for remote_name in remote_names.iter().flatten() {
            let _remote = self.repo.find_remote(remote_name)?;
            // Note: We should fetch or use cached remote branches from refs/remotes/
            let remote_ref_prefix = format!("refs/remotes/{}/", remote_name);
            for reference in self.repo.references()? {
                let reference = reference?;
                if reference.is_remote() {
                    let ref_name = reference.name().unwrap_or("");
                    if let Some(branch_name) = ref_name.strip_prefix(&remote_ref_prefix) {
                        // Skip the symbolic default-branch ref (refs/remotes/<remote>/HEAD).
                        if branch_name == "HEAD" {
                            continue;
                        }
                        info.remote.entry(remote_name.to_string()).or_default().push(
                            BranchInfo { 
                                name: branch_name.to_string(),
                                is_head: false,
                                upstream: Some(remote_name.to_string())
                            });
                    }
                }
            }
        }
        
        Ok(info)
    }

    /// List branches for a specific remote
    pub fn list_remote_branches(&self, remote_name: &str) -> Result<Vec<String>> {
        let mut branches = Vec::new();
        let remote_ref_prefix = format!("refs/remotes/{}/", remote_name);
        
        for reference in self.repo.references()? {
            let reference = reference?;
            if reference.is_remote() {
                let ref_name = reference.name().unwrap_or("");
                if let Some(branch_name) = ref_name.strip_prefix(&remote_ref_prefix) {
                    if branch_name == "HEAD" {
                        continue;
                    }
                    branches.push(branch_name.to_string());
                }
            }
        }
        
        Ok(branches)
    }

    /// Get current branch name
    pub fn current_branch(&self) -> Result<Option<String>> {
        let head = self.repo.head()?;
        // Strip refs/heads/ prefix to return short branch name
        let name = head.shorthand().map(|s| s.to_string());
        Ok(name)
    }

    /// Get current HEAD commit
    pub fn head_commit(&self) -> Result<git2::Commit<'_>> {
        let head = self.repo.head()?;
        let commit = head.peel_to_commit()?;
        Ok(commit)
    }

    /// Cherry-pick commits onto a local `target_branch` for the commit range
    /// `commit_range` (e.g. "HEAD~3..HEAD", "..HEAD", or a single ref/sha).
    ///
    /// The target local branch must already exist (use [`GitRepo::ensure_local_branch`]
    /// to create it first). Commits are applied oldest-first.
    pub fn cherry_pick_range(
        &self,
        commit_range: &str,
        target_branch: &str,
    ) -> Result<Vec<String>> {
        let mut picked_commits = Vec::new();

        if self.repo.find_branch(target_branch, BranchType::Local).is_err() {
            return Err(GitMultiError::SyncError(format!(
                "Local branch '{}' does not exist; create it before syncing",
                target_branch
            )));
        }
        if self.current_branch()?.as_deref() != Some(target_branch) {
            self.checkout_branch(target_branch)?;
        }

        // For A..B we pick commits reachable from B but not from A, so the
        // revwalk pushes B and hides A. A single spec picks everything on B
        // that is not already reachable from the target branch.
        let (start, end) = self.parse_commit_range(commit_range, Some(target_branch))?;

        let mut revwalk = self.repo.revwalk()?;
        revwalk.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::TIME)?;
        revwalk.push(end)?;
        if let Some(start_oid) = start {
            revwalk.hide(start_oid)?;
        }

        let commits: Vec<git2::Commit> = revwalk
            .filter_map(|oid_result| {
                let oid = oid_result.ok()?;
                self.repo.find_commit(oid).ok()
            })
            .collect();

        // Apply oldest first (the walk emits newest-first).
        for commit in commits.iter().rev() {
            let commit_sha = commit.id().to_string();
            let mut options = git2::CherrypickOptions::new();

            if let Err(e) = self.repo.cherrypick(commit, Some(&mut options)) {
                self.repo.cleanup_state()?;
                return Err(GitMultiError::GitError(e));
            }

            if self.repo.index()?.has_conflicts() {
                self.repo.cleanup_state()?;
                return Err(GitMultiError::SyncConflict);
            }

            let result = (|| -> Result<()> {
                let signature = self.repo.signature()?;
                let tree_oid = self.repo.index()?.write_tree()?;
                let tree = self.repo.find_tree(tree_oid)?;
                let parent = self.head_commit()?;
                let parents = [&parent];
                self.repo.commit(
                    Some("HEAD"),
                    &signature,
                    &signature,
                    &format!("Cherry-pick: {}", commit.summary().unwrap_or("")),
                    &tree,
                    &parents,
                )?;
                Ok(())
            })();
            if let Err(e) = result {
                self.repo.cleanup_state()?;
                return Err(e);
            }
            picked_commits.push(commit_sha);
        }

        Ok(picked_commits)
    }

    /// Ensure a local branch named `name` exists, creating it from `base_ref`
    /// (a local or remote-tracking ref) when it is missing.
    pub fn ensure_local_branch(&self, name: &str, base_ref: &str) -> Result<()> {
        if self.repo.find_branch(name, BranchType::Local).is_ok() {
            return Ok(());
        }
        let commit = self.repo.find_reference(base_ref)?.peel_to_commit()?;
        self.repo.branch(name, &commit, false)?;
        Ok(())
    }

    /// Parse a commit range into `(start, end)`:
    ///
    /// * `A..B` / `..B` — `start` is the resolved `A` (or `None` for an empty
    ///   left side) and `end` is `B`.
    /// * a single spec — `end` is the resolved commit and `start` is the
    ///   merge-base with `target_branch` (if the target is an ancestor of
    ///   `end`), so a bare `sync` picks exactly the commits the destination
    ///   branch is missing.
    pub fn parse_commit_range(
        &self,
        range: &str,
        target_branch: Option<&str>,
    ) -> Result<(Option<git2::Oid>, git2::Oid)> {
        let range = range.trim();
        if range.contains("..") {
            let parts: Vec<&str> = range.split("..").map(|s| s.trim()).collect();
            if parts.len() > 2 {
                return Err(GitMultiError::SyncError(format!(
                    "Invalid commit range: {} (expected e.g. HEAD~3..HEAD)",
                    range
                )));
            }
            let start = if parts[0].is_empty() {
                None
            } else {
                Some(self.resolve_commit_spec(parts[0])?)
            };
            let end = self.resolve_commit_spec(parts[1])?;
            Ok((start, end))
        } else {
            let end = self.resolve_commit_spec(range)?;
            let start = match target_branch {
                Some(tb) => self
                    .repo
                    .find_branch(tb, BranchType::Local)
                    .ok()
                    .and_then(|b| b.get().target())
                    .and_then(|t| self.repo.merge_base(t, end).ok()),
                None => None,
            };
            Ok((start, end))
        }
    }

    /// Resolve a commit specification (branch name, tag, SHA, or relative ref)
    /// using `git rev-parse`, which handles shorthands, `HEAD~n`, tags, etc.
    pub fn resolve_commit_spec(&self, spec: &str) -> Result<git2::Oid> {
        if let Ok(obj) = self.repo.revparse_single(spec) {
            if let Ok(commit) = obj.peel_to_commit() {
                return Ok(commit.id());
            }
        }

        // Fallback for bare SHAs that may not peel to a commit directly.
        if spec.len() >= 7 && spec.chars().all(|c| c.is_ascii_hexdigit()) {
            if let Ok(oid) = git2::Oid::from_str(spec) {
                if self.repo.find_object(oid, None).is_ok() {
                    return Ok(oid);
                }
            }
        }

        Err(GitMultiError::SyncError(format!("Could not resolve commit spec: {}", spec)))
    }

    /// Merge a branch into current branch
    pub fn merge_branch(&self, branch_name: &str) -> Result<()> {
        let branch = self.repo.find_branch(branch_name, BranchType::Local)?;
        let commit_oid = branch.get().target().ok_or_else(|| {
            GitMultiError::SyncError(format!("Branch {} has no target", branch_name))
        })?;
        let annotated_commit = self.repo.find_annotated_commit(commit_oid)?;

        let mut options = git2::MergeOptions::default();
        options.fail_on_conflict(true);

        if let Err(e) = self.repo.merge(&[&annotated_commit], Some(&mut options), None) {
            self.repo.cleanup_state()?;
            return Err(GitMultiError::GitError(e));
        }
        if self.repo.index()?.has_conflicts() {
            self.repo.cleanup_state()?;
            return Err(GitMultiError::SyncConflict);
        }

        Ok(())
    }

    /// Rebase current branch onto another branch
    pub fn rebase_branch(&self, onto_branch: &str) -> Result<()> {
        let onto = self.repo.find_branch(onto_branch, BranchType::Local)?;
        let onto_oid = onto.get().target().ok_or_else(|| {
            GitMultiError::SyncError(format!("Branch {} has no target", onto_branch))
        })?;
        let onto_annotated = self.repo.find_annotated_commit(onto_oid)?;
        
        let mut options = git2::RebaseOptions::default();
        let mut rebase = self.repo.rebase(Some(&onto_annotated), None, None, Some(&mut options))?;
        
        while let Some(op_res) = rebase.next() {
            let op = op_res?;
            if op.kind() == Some(git2::RebaseOperationType::Pick) {
                rebase.commit(None, &self.repo.signature()?, None)?;
            }
        }
        
        rebase.finish(Some(&self.repo.signature()?))?;
        
        Ok(())
    }

    /// Push to a specific remote using the system git binary
    /// (libgit2 does not honour ~/.ssh/config Host aliases)
    pub fn push_to_remote(
        &self,
        remote_name: &str,
        branch_name: Option<&str>,
    ) -> Result<()> {
        let workdir = self.workdir();

        // Default to the current branch when no branch is specified.
        let branch = match branch_name {
            Some(b) => b.to_string(),
            None => self.current_branch()?
                .ok_or_else(|| GitMultiError::SyncError(
                    "Cannot determine current branch to push".to_string()
                ))?,
        };

        git_run_net_str(workdir, &["push", remote_name, &branch])?;
        Ok(())
    }

    /// Push to all remotes
    pub fn push_to_all(&self, branch_name: Option<&str>) -> Result<Vec<String>> {
        let remote_names = self.repo.remotes()?;
        let mut pushed = Vec::new();
        
        for name in remote_names.iter().flatten() {
            self.push_to_remote(name, branch_name)?;
            pushed.push(name.to_string());
        }
        
        Ok(pushed)
    }

    /// Pull from a specific remote using the system git binary
    /// (libgit2 does not honour ~/.ssh/config Host aliases)
    pub fn pull_from_remote(
        &self,
        remote_name: &str,
        branch_name: Option<&str>,
    ) -> Result<()> {
        let workdir = self.workdir();

        // Default to the current branch when no branch is specified.
        let branch = branch_name
            .map(|b| b.to_string())
            .or_else(|| self.current_branch().ok().flatten())
            .unwrap_or_default();

        let mut args: Vec<String> = vec!["pull".to_string(), remote_name.to_string()];
        if !branch.is_empty() {
            args.push(branch);
        }
        let cargs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

        git_run_net_str(workdir, &cargs)?;
        Ok(())
    }

    /// Pull from all remotes
    pub fn pull_from_all(&self, branch_name: Option<&str>) -> Result<Vec<String>> {
        let remote_names = self.repo.remotes()?;
        let mut pulled = Vec::new();
        
        for name in remote_names.iter().flatten() {
            self.pull_from_remote(name, branch_name)?;
            pulled.push(name.to_string());
        }
        
        Ok(pulled)
    }

    /// Fetch specific branches from a remote using the system git binary.
    pub fn fetch_branches(&self, remote_name: &str, branches: &[String]) -> Result<()> {
        if branches.is_empty() {
            return self.fetch_remote(remote_name);
        }

        let workdir = self.workdir();
        let mut args: Vec<String> = vec!["fetch".to_string(), remote_name.to_string()];
        for b in branches {
            args.push(b.clone());
        }
        let cargs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

        git_run_net_str(workdir, &cargs)?;
        Ok(())
    }

    /// Push specific branches to a remote using the system git binary.
    pub fn push_branches(&self, remote_name: &str, branches: &[String], force: bool) -> Result<()> {
        let workdir = self.workdir();

        for branch in branches {
            let mut args: Vec<String> = vec!["push".to_string(), remote_name.to_string()];
            if force {
                args.push("--force".into());
            }
            args.push(branch.clone());
            let cargs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

            git_run_net_str(workdir, &cargs)?;
        }
        Ok(())
    }

    /// Pull specific branches from a remote using the system git binary.
    ///
    /// To avoid accidentally merging one branch into another, only the branch
    /// matching the current HEAD is merged via `git pull`; every other branch
    /// is fetched instead (which updates its remote-tracking ref) so a request
    /// like `pull --branches dev feature` can never fast-forward a merge into
    /// the branch you are currently on.
    pub fn pull_branches(&self, remote_name: &str, branches: &[String]) -> Result<()> {
        let workdir = self.workdir();
        let current = self.current_branch().ok().flatten().unwrap_or_default();

        for branch in branches {
            if branch == &current {
                git_run_net_str(workdir, &["pull", remote_name, branch])?;
            } else {
                git_run_net_str(workdir, &["fetch", remote_name, branch])?;
            }
        }
        Ok(())
    }

    /// List local branch names.
    pub fn local_branch_names(&self) -> Result<Vec<String>> {
        let mut names = Vec::new();
        for branch_res in self.repo.branches(Some(BranchType::Local))? {
            let (branch, _) = branch_res?;
            if let Some(name) = branch.name()? {
                names.push(name.to_string());
            }
        }
        Ok(names)
    }

    /// Rename a git remote in both git config and the git-multi config.
    pub fn rename_remote(&mut self, old: &str, new: &str) -> Result<()> {
        let remote = self.repo.find_remote(old)?;
        let url = remote.url().unwrap_or("").to_string();
        self.repo.remote_delete(old)?;
        self.repo.remote(new, &url)?;

        if let Some(rc) = self.config.remotes.remove(old) {
            self.config.remotes.insert(new.to_string(), rc);
        }
        if self.config.get_default_remote().is_some_and(|d| d == old) {
            self.config.set_default_remote(new.to_string())?;
        }
        self.config.save(&self.repo)?;
        Ok(())
    }

    /// Rename a local branch.
    pub fn rename_branch(&self, old: &str, new: &str) -> Result<()> {
        let mut branch = self.repo.find_branch(old, BranchType::Local)?;
        branch.rename(new, false)?;
        Ok(())
    }

    /// Delete a local branch.
    pub fn delete_local_branch(&self, name: &str, force: bool) -> Result<()> {
        let mut branch = self.repo.find_branch(name, BranchType::Local)?;
        branch.delete()?;
        let _ = force;
        Ok(())
    }

    /// Rebase the given local branch onto a ref (using the system git binary
    /// so interactive rebase prompts are avoided and a timeout applies).
    pub fn rebase_onto(&self, branch: &str, onto_ref: &str) -> Result<()> {
        let workdir = self.workdir();
        if self.current_branch()?.as_deref() != Some(branch) {
            self.checkout_branch(branch)?;
        }
        git_run_str(workdir, &["rebase", onto_ref])?;
        Ok(())
    }

    /// Fetch a remote ref and merge it into the current branch, creating a
    /// merge commit when the merge is clean. Used for cross-remote merges.
    pub fn merge_and_commit(&self, src_ref: &str) -> Result<()> {
        let reference = self.repo.find_reference(src_ref)?;
        let oid = reference.target().ok_or_else(|| {
            GitMultiError::SyncError(format!("Reference {} has no target", src_ref))
        })?;
        let src_commit = self.repo.find_commit(oid)?;
        let head = self.head_commit()?;

        // Nothing to do when the source is already part of history.
        if head.id() == oid || self.repo.graph_descendant_of(head.id(), oid)? {
            return Ok(());
        }

        let annotated = self.repo.find_annotated_commit(oid)?;

        let mut opts = git2::MergeOptions::default();
        opts.fail_on_conflict(true);
        if let Err(e) = self.repo.merge(&[&annotated], Some(&mut opts), None) {
            self.repo.cleanup_state()?;
            return Err(GitMultiError::GitError(e));
        }

        if self.repo.index()?.has_conflicts() {
            self.repo.cleanup_state()?;
            return Err(GitMultiError::SyncConflict);
        }

        let result = (|| -> Result<()> {
            let signature = self.repo.signature()?;
            let tree_oid = self.repo.index()?.write_tree()?;
            let tree = self.repo.find_tree(tree_oid)?;
            let parents = [&head, &src_commit];

            self.repo.commit(
                Some("HEAD"),
                &signature,
                &signature,
                &format!("Merge {}", src_ref),
                &tree,
                &parents,
            )?;
            Ok(())
        })();
        // Clear any MERGE_HEAD / ORIG_HEAD state left behind by libgit2.
        let _ = self.repo.cleanup_state();
        result
    }

    /// Produce a human-readable status string for display.
    pub fn status_text(&self) -> Result<String> {
        let workdir = self.repo.workdir().unwrap_or_else(|| self.repo.path());

        let mut out = String::new();
        out.push_str("Remotes:\n");
        for (name, url) in self.list_remotes_with_urls()? {
            let marker = if self.config.get_default_remote().is_some_and(|d| d == &name) {
                " [default]"
            } else {
                ""
            };
            out.push_str(&format!("  {}{}: {}\n", name, marker, url));
        }

        out.push_str("\nLocal branches:\n");
        let info = self.list_all_branches()?;
        for b in &info.local {
            out.push_str(&format!("  {}{}\n", b.name, if b.is_head { " (HEAD)" } else { "" }));
        }

        out.push_str("\nWorking tree:\n");
        let st = git_run(workdir, &["status", "--short"])
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default();
        if st.trim().is_empty() {
            out.push_str("  clean\n");
        } else {
            for line in st.lines() {
                out.push_str(&format!("  {}\n", line));
            }
        }
        Ok(out)
    }

    /// Copy files from one commit/branch into the working directory.
    ///
    /// `patterns` are glob patterns (e.g. `src/**/*.rs`) matched against the
    /// paths present in `from_ref`. When `prune` is set, files in the working
    /// tree that match a pattern but are not present in the source are removed.
    /// All writes are resolved relative to the repository root.
    pub fn copy_files(&self, from_ref: &str, patterns: &[String], prune: bool) -> Result<Vec<String>> {
        let from_commit = self.resolve_commit_spec(from_ref)?;
        let from_tree = self.repo.find_commit(from_commit)?.tree()?;

        let mut source_paths = Vec::new();
        self.tree_files(&from_tree, "", &mut source_paths)?;

        let matchers: Vec<GlobPattern> = patterns
            .iter()
            .filter_map(|p| GlobPattern::new(p).ok())
            .collect();
        if matchers.is_empty() {
            return Ok(Vec::new());
        }

        let matches = |path: &str| matchers.iter().any(|m| m.matches(path));

        let mut copied = Vec::new();
        let root = self.repo.workdir().unwrap_or_else(|| self.repo.path());

        for path in &source_paths {
            if !matches(path) {
                continue;
            }
            let entry = from_tree.get_path(Path::new(path))?;
            let obj = entry.to_object(&self.repo)?;
            let blob = obj.peel_to_blob()?;

            let out_path = root.join(path);
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&out_path, blob.content())?;
            copied.push(path.clone());
        }

        if prune {
            let source_set: std::collections::HashSet<&String> = source_paths.iter().collect();
            self.prune_workdir(root, &matches, &source_set)?;
        }

        Ok(copied)
    }

    /// Recursively list every file path in a tree.
    fn tree_files(&self, tree: &git2::Tree<'_>, prefix: &str, out: &mut Vec<String>) -> Result<()> {
        for entry in tree.iter() {
            let name = entry.name().unwrap_or("");
            if name.is_empty() {
                continue;
            }
            let path = if prefix.is_empty() {
                name.to_string()
            } else {
                format!("{}/{}", prefix, name)
            };
            match entry.kind() {
                Some(git2::ObjectType::Tree) => {
                    if let Ok(obj) = entry.to_object(&self.repo) {
                        if let Ok(sub) = obj.peel_to_tree() {
                            self.tree_files(&sub, &path, out)?;
                        }
                    }
                }
                _ => out.push(path),
            }
        }
        Ok(())
    }

    /// Remove working-tree files matching `matches` that do not exist in the
    /// source tree (`source_set`), skipping `.git`.
    fn prune_workdir(
        &self,
        dir: &Path,
        matches: &dyn Fn(&str) -> bool,
        source_set: &std::collections::HashSet<&String>,
    ) -> Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                if path.file_name().map(|n| n == ".git").unwrap_or(false) {
                    continue;
                }
                self.prune_workdir(&path, matches, source_set)?;
            } else {
                let rel = path
                    .strip_prefix(self.repo.workdir().unwrap_or_else(|| self.repo.path()))
                    .unwrap_or(&path);
                let rel_str = rel.to_string_lossy().to_string();
                if matches(&rel_str) && !source_set.contains(&rel_str) {
                    let _ = fs::remove_file(&path);
                }
            }
        }
        Ok(())
    }

    /// Create a Pull Request using gh CLI.
    ///
    /// The `gh --repo` argument must be an `owner/repo` slug, which is derived
    /// from the remote's URL (both https and scp-like ssh URLs are handled).
    pub fn create_pr(
        &self,
        remote_name: &str,
        base_branch: &str,
        head_branch: &str,
        title: &str,
        description: Option<&str>,
        open: bool,
    ) -> Result<()> {
        let workdir = self.workdir();

        // Confirm gh is actually installed before trying to use it.
        let check = run_captured("gh", &["--version"], workdir, &[], Duration::from_secs(15))?;
        if !check.status.success() {
            return Err(GitMultiError::SyncError(
                "The `gh` CLI is required for PR creation but was not found on PATH.".to_string(),
            ));
        }

        let remote = self.repo.find_remote(remote_name)?;
        let url = remote.url().unwrap_or("");
        let slug = repo_slug_from_url(url).ok_or_else(|| {
            GitMultiError::SyncError(format!(
                "Cannot determine owner/repo from remote URL '{}'",
                url
            ))
        })?;

        let mut args: Vec<String> = vec![
            "pr".to_string(),
            "create".to_string(),
            "--repo".to_string(),
            slug.clone(),
            "--base".to_string(),
            base_branch.to_string(),
            "--title".to_string(),
            title.to_string(),
        ];
        if head_branch != base_branch {
            args.push("--head".to_string());
            args.push(head_branch.to_string());
        }
        if let Some(d) = description {
            args.push("--body".to_string());
            args.push(d.to_string());
        }
        let cargs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

        let out = run_captured("gh", &cargs, workdir, &[], GIT_TIMEOUT)?;
        let stderr = String::from_utf8_lossy(&out.stderr);
        if !out.status.success() {
            return Err(GitMultiError::SyncError(format!(
                "gh pr create failed: {}",
                stderr.trim()
            )));
        }
        let pr_url = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if open {
            // Fire-and-forget: open the PR in the browser.
            let _ = run_captured("gh", &["pr", "view", "--web", "--repo", &slug], workdir, &[], Duration::from_secs(30));
            if !pr_url.is_empty() {
                println!("PR created: {}", pr_url);
            }
        } else if !pr_url.is_empty() {
            println!("PR created: {}", pr_url);
        }

        Ok(())
    }

    /// Create a commit with optional body. Stages all changes first.
    pub fn create_commit(&self, subject: &str, body: Option<&str>) -> Result<()> {
        // Stage all changes (delegates to git so it honours .gitignore etc.)
        self.stage_file(".")?;

        let full_msg = if let Some(b) = body {
            format!("{}\n\n{}", subject, b)
        } else {
            subject.to_string()
        };

        let workdir = self.workdir();
        git_run_str(workdir, &["commit", "-m", &full_msg])?;
        Ok(())
    }

    /// List recent commits for display
    pub fn list_recent_commits(&self, count: usize) -> Result<Vec<String>> {
        let workdir = self.workdir();
        let out = git_run_str(workdir, &["log", "-n", &count.to_string(), "--oneline", "--decorate"])?;
        Ok(out.lines().map(|l| l.to_string()).collect())
    }

    // ========================================================================
    // Stash
    // ========================================================================

    /// Stash the working tree (`git stash push` with an optional message).
    pub fn stash_save(&self, message: Option<&str>) -> Result<()> {
        let workdir = self.workdir();
        match message {
            Some(m) => git_run_str(workdir, &["stash", "push", "-m", m])?,
            None => git_run_str(workdir, &["stash", "push"])?,
        };
        Ok(())
    }

    /// Re-apply the most recent stash.
    pub fn stash_pop(&self) -> Result<()> {
        let workdir = self.workdir();
        git_run_str(workdir, &["stash", "pop"])?;
        Ok(())
    }

    /// List stashes.
    pub fn stash_list(&self) -> Result<Vec<String>> {
        let workdir = self.workdir();
        let out = git_run_str(workdir, &["stash", "list"])?;
        Ok(out.lines().map(|l| l.to_string()).collect())
    }

    // ========================================================================
    // Tags
    // ========================================================================

    /// List all tags.
    pub fn list_tags(&self) -> Result<Vec<String>> {
        let workdir = self.workdir();
        let out = git_run_str(workdir, &["tag", "--list", "--sort=-creatordate"])?;
        Ok(out.lines().map(|l| l.to_string()).filter(|l| !l.is_empty()).collect())
    }

    /// Create a tag (annotated when a message is given, otherwise lightweight).
    pub fn create_tag(&self, name: &str, target: &str, message: Option<&str>) -> Result<()> {
        let workdir = self.workdir();
        match message {
            Some(m) => git_run_str(workdir, &["tag", "-a", name, target, "-m", m])?,
            None => git_run_str(workdir, &["tag", name, target])?,
        };
        Ok(())
    }

    /// Delete a local tag.
    pub fn delete_tag(&self, name: &str) -> Result<()> {
        let workdir = self.workdir();
        git_run_str(workdir, &["tag", "-d", name])?;
        Ok(())
    }

    // ========================================================================
    // Reflog
    // ========================================================================

    /// Show the reflog for the current HEAD.
    pub fn reflog(&self, count: usize) -> Result<Vec<String>> {
        let workdir = self.workdir();
        let out = git_run_str(workdir, &["reflog", "-n", &count.to_string(), "--date=iso"])?;
        Ok(out.lines().map(|l| l.to_string()).collect())
    }

    // ========================================================================
    // Git graph (subprocess ASCII DAG for the TUI)
    // ========================================================================

    /// A `git log --graph`-style ASCII commit DAG, one line per commit plus
    /// any graph/edge lines. Includes `(HEAD -> branch, tag)` decoration.
    pub fn log_graph(&self, all: bool, limit: usize) -> Result<Vec<String>> {
        let workdir = self.workdir();
        let mut args: Vec<String> = vec!["log".to_string(), "--graph".to_string(), "--decorate".to_string()];
        args.push("--format=%H%x00%h%x00%an%x00%aI%x00%s".to_string());
        args.push("-n".to_string());
        args.push(limit.to_string());
        if all {
            args.push("--all".to_string());
        }
        let cargs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let out = git_run_str(workdir, &cargs)?;
        Ok(out.lines().map(|l| l.to_string()).collect())
    }

    // ========================================================================
    // Working-tree status & granular staging
    // ========================================================================

    /// Every file with a changed status, with its two-letter git status code.
    pub fn working_status(&self) -> Result<Vec<FileStatus>> {
        let statuses = self.repo.statuses(None)?;
        let mut entries: Vec<_> = statuses.iter().collect();
        entries.sort_by_key(|s| {
            s.path()
                .map(|p| p.to_string())
                .unwrap_or_default()
        });
        let mut out = Vec::new();
        for s in entries {
            let Some(path) = s.path().map(|p| p.to_string()) else {
                continue;
            };
            let (staged, unstaged) = status_codes(s.status());
            out.push(FileStatus {
                path,
                staged,
                unstaged,
                in_index: s.status().contains(git2::Status::INDEX_NEW)
                    || s.status().contains(git2::Status::INDEX_MODIFIED)
                    || s.status().contains(git2::Status::INDEX_DELETED)
                    || s.status().contains(git2::Status::INDEX_RENAMED)
                    || s.status().contains(git2::Status::INDEX_TYPECHANGE),
                in_workdir: s.status().contains(git2::Status::WT_NEW)
                    || s.status().contains(git2::Status::WT_MODIFIED)
                    || s.status().contains(git2::Status::WT_DELETED)
                    || s.status().contains(git2::Status::WT_TYPECHANGE)
                    || s.status().contains(git2::Status::WT_RENAMED),
            });
        }
        Ok(out)
    }

    /// Stage a single file (or all with ".").
    pub fn stage_file(&self, path: &str) -> Result<()> {
        let workdir = self.workdir();
        git_run_str(workdir, &["add", "--", path])?;
        Ok(())
    }

    /// Unstage a single file (reset its entry out of the index, keeping the
    /// working-tree contents).
    pub fn unstage_file(&self, path: &str) -> Result<()> {
        let workdir = self.workdir();
        git_run_str(workdir, &["restore", "--staged", "--", path])?;
        Ok(())
    }

    /// Discard working-tree changes for a single file (restore from index/HEAD).
    pub fn restore_file(&self, path: &str) -> Result<()> {
        let workdir = self.workdir();
        git_run_str(workdir, &["restore", "--", path])?;
        Ok(())
    }

    // ========================================================================
    // Diffs
    // ========================================================================

    /// A unified diff according to `mode`.
    pub fn diff(&self, mode: DiffMode, pathspec: Option<&str>) -> Result<String> {
        let workdir = self.workdir();
        let mut args: Vec<String> = vec!["diff".to_string()];
        match mode {
            DiffMode::Staged => args.push("--cached".to_string()),
            DiffMode::Unstaged => {}
            DiffMode::Head => args.push("HEAD".to_string()),
        }
        if let Some(p) = pathspec {
            args.push("--".to_string());
            args.push(p.to_string());
        }
        let cargs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let out = git_run(workdir, &cargs)?;
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    }

    /// Line-level diff hunks for a single file in the given mode (used by the
    /// GUI blame/diff panels). Returns (old_lines, new_lines) tuples keyed by
    /// line content so callers can highlight.
    pub fn diff_lines(&self, mode: DiffMode, path: &str) -> Result<Vec<DiffLineEntry>> {
        let workdir = self.workdir();
        let mut args: Vec<String> = vec!["diff".to_string()];
        match mode {
            DiffMode::Staged => args.push("--cached".to_string()),
            DiffMode::Unstaged => {}
            DiffMode::Head => args.push("HEAD".to_string()),
        }
        args.push("--".to_string());
        args.push(path.to_string());
        let cargs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let out = git_run(workdir, &cargs)?;
        let text = String::from_utf8_lossy(&out.stdout);
        Ok(parse_diff_lines(&text))
    }

    /// Show the full diff of a commit (equivalent to `git show COMMIT`).
    pub fn commit_diff(&self, commit_spec: &str) -> Result<String> {
        let workdir = self.workdir();
        let oid = self.resolve_commit_spec(commit_spec)?;
        git_run_str(workdir, &["show", "--no-color", &oid.to_string()])
    }

    // ========================================================================
    // Amend / revert / reset
    // ========================================================================

    /// Amend the last commit with the given message. Stages everything first.
    pub fn amend_commit(&self, subject: &str, body: Option<&str>) -> Result<()> {
        let workdir = self.workdir();
        self.stage_file(".")?;
        let full_msg = match body {
            Some(b) => format!("{}\n\n{}", subject, b),
            None => subject.to_string(),
        };
        git_run_str(workdir, &["commit", "--amend", "-m", &full_msg])?;
        Ok(())
    }

    /// Create a new commit that reverts the given commit (uses `git revert`).
    pub fn revert_commit(&self, commit_spec: &str) -> Result<()> {
        let workdir = self.workdir();
        git_run_str(workdir, &["revert", "--no-edit", commit_spec])?;
        Ok(())
    }

    /// Reset the current branch. `soft` keeps index+workdir, `mixed` keeps
    /// workdir, `hard` discards everything (use with care — handled by caller).
    pub fn reset(&self, mode: ResetMode, commit_spec: &str) -> Result<()> {
        let workdir = self.workdir();
        let flag = match mode {
            ResetMode::Soft => "--soft",
            ResetMode::Mixed => "--mixed",
            ResetMode::Hard => "--hard",
        };
        git_run_str(workdir, &["reset", flag, commit_spec])?;
        Ok(())
    }

    // ========================================================================
    // GitLens: blame
    // ========================================================================

    /// Blame a file, returning one entry per line.
    pub fn blame_file(&self, path: &str, commit_spec: Option<&str>) -> Result<Vec<BlameLine>> {
        let repo_path = self.repo.workdir().unwrap_or_else(|| self.repo.path());
        let abspath = repo_path.join(path);
        if !abspath.exists() {
            // Try to blame the blob at HEAD if the file is deleted.
            let tree = self.head_commit()?.tree()?;
            if let Some(entry) = tree.get_name(path) {
                let blob = entry.to_object(&self.repo)?.peel_to_blob()?;
                let content = String::from_utf8_lossy(blob.content());
                return Ok(content
                    .lines()
                    .enumerate()
                    .map(|(i, _)| BlameLine {
                        line: i + 1,
                        commit: String::new(),
                        author: String::new(),
                        date: String::new(),
                        summary: String::new(),
                        final_line: i + 1,
                    })
                    .collect());
            }
            return Ok(Vec::new());
        }

        let mut opts = git2::BlameOptions::new();
        if let Some(spec) = commit_spec {
            let oid = self.resolve_commit_spec(spec)?;
            opts.newest_commit(oid);
        }
        let blame = self.repo.blame_file(std::path::Path::new(path), Some(&mut opts))?;
        let mut out = Vec::new();
        for hunk in blame.iter() {
            let commit = hunk.final_commit_id();
            let sig = hunk.final_signature();
            let summary = self
                .repo
                .find_commit(commit)
                .ok()
                .and_then(|c| c.summary().map(|s| s.to_string()))
                .unwrap_or_default();
            let when = sig.when();
            let date = format_timestamp(when.seconds());
            let count = hunk.lines_in_hunk();
            let author = sig.name().unwrap_or("").to_string();
            for i in 0..count {
                out.push(BlameLine {
                    line: hunk.final_start_line() + i,
                    commit: commit.to_string(),
                    author: author.clone(),
                    date: date.clone(),
                    summary: summary.clone(),
                    final_line: hunk.final_start_line() + i,
                });
            }
        }
        Ok(out)
    }

    /// Commits that touched a file (GitLens "file history"), newest first.
    pub fn file_history(&self, path: &str) -> Result<Vec<CommitSummary>> {
        let workdir = self.workdir();
        let out = git_run_str(workdir, &[
            "log",
            "--follow",
            "--format=%H%x00%an%x00%aI%x00%s",
            "--",
            path,
        ])?;
        Ok(parse_log_null_sep(&out))
    }

    /// Line history for a single file (GitLens "line history"), newest first.
    pub fn line_history(&self, path: &str, line: usize) -> Result<Vec<CommitSummary>> {
        let workdir = self.workdir();
        match git_run_str(workdir, &[
            "-c",
            "core.pager=cat",
            "log",
            "-L",
            &format!("{},{}:{}", line, line, path),
            "--format=%H%x00%an%x00%aI%x00%s",
        ]) {
            Ok(out) => Ok(parse_log_null_sep(&out)),
            Err(_) => Ok(Vec::new()),
        }
    }

    // ========================================================================
    // GitGraph: commit DAG
    // ========================================================================

    /// Build a commit graph reachable from HEAD (or `--all` refs) with
    /// branch/ref labels per commit.
    pub fn commit_graph(&self, all: bool, limit: usize) -> Result<CommitGraph> {
        let mut walk = self.repo.revwalk()?;
        walk.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::TIME)?;
        if all {
            walk.push_glob("refs/heads/*")?;
            walk.push_glob("refs/remotes/*")?;
            if self.repo.head().is_ok() {
                let _ = walk.push_head();
            }
        } else if let Ok(head) = self.repo.head() {
            walk.push_head()?;
            let _ = head;
        } else {
            return Err(GitMultiError::SyncError("No HEAD; cannot graph".to_string()));
        }

        let ref_labels = self.collect_ref_labels()?;

        let mut nodes: Vec<CommitNode> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for oid in walk {
            let oid = match oid {
                Ok(o) => o,
                Err(_) => continue,
            };
            if !seen.insert(oid) {
                continue;
            }
            let Ok(commit) = self.repo.find_commit(oid) else {
                continue;
            };
            let parents: Vec<String> = commit
                .parent_ids()
                .map(|p| p.to_string())
                .collect();
            let author = commit.author();
            let node = CommitNode {
                id: oid.to_string(),
                short_id: oid.to_string()[..8.min(oid.to_string().len())].to_string(),
                message: commit.summary().unwrap_or("").to_string(),
                author: author.name().unwrap_or("").to_string(),
                date: author.when().seconds(),
                parents,
                refs: ref_labels.get(&oid.to_string()).cloned().unwrap_or_default(),
            };
            nodes.push(node);
            if nodes.len() >= limit {
                break;
            }
        }

        // Refs that are not on a visited commit (e.g. detached or beyond limit).
        let mut detached: Vec<RefLabel> = ref_labels
            .values()
            .flatten()
            .cloned()
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        detached.sort_by(|a, b| a.name.cmp(&b.name));

        Ok(CommitGraph { nodes, detached_refs: detached })
    }

    /// Full commit metadata for a single commit (used by graph/detail views).
    pub fn commit_detail(&self, commit_spec: &str) -> Result<CommitSummary> {
        let oid = self.resolve_commit_spec(commit_spec)?;
        let commit = self.repo.find_commit(oid)?;
        let author = commit.author();
        let committer = commit.committer();
        Ok(CommitSummary {
            id: oid.to_string(),
            short_id: oid.to_string()[..8.min(oid.to_string().len())].to_string(),
            author: author.name().unwrap_or("").to_string(),
            author_email: author.email().unwrap_or("").to_string(),
            author_date: author.when().seconds(),
            committer: committer.name().unwrap_or("").to_string(),
            committer_date: committer.when().seconds(),
            message: commit.message().unwrap_or("").to_string(),
            parents: commit.parent_ids().map(|p| p.to_string()).collect(),
        })
    }

    /// Cherry-pick a single commit onto the current HEAD (interactive pick).
    pub fn cherry_pick_commit(&self, commit_spec: &str) -> Result<()> {
        let oid = self.resolve_commit_spec(commit_spec)?;
        let commit = self.repo.find_commit(oid)?;
        let mut opts = git2::CherrypickOptions::new();
        if let Err(e) = self.repo.cherrypick(&commit, Some(&mut opts)) {
            self.repo.cleanup_state()?;
            return Err(GitMultiError::GitError(e));
        }
        if self.repo.index()?.has_conflicts() {
            self.repo.cleanup_state()?;
            return Err(GitMultiError::SyncConflict);
        }
        let result = (|| -> Result<()> {
            let tree_oid = self.repo.index()?.write_tree()?;
            let tree = self.repo.find_tree(tree_oid)?;
            let parent = self.head_commit()?;
            let parents = [&parent];
            let sig = self.repo.signature()?;
            self.repo.commit(
                Some("HEAD"),
                &sig,
                &sig,
                &format!("Cherry-pick: {}", commit.summary().unwrap_or("")),
                &tree,
                &parents,
            )?;
            Ok(())
        })();
        let _ = self.repo.cleanup_state();
        result
    }

    // ------------------------------------------------------------------------
    // helpers
    // ------------------------------------------------------------------------

    fn workdir(&self) -> &std::path::Path {
        self.repo.workdir().unwrap_or_else(|| self.repo.path())
    }

    fn collect_ref_labels(&self) -> Result<HashMap<String, Vec<RefLabel>>> {
        let mut map: HashMap<String, Vec<RefLabel>> = HashMap::new();
        let head = self.repo.head().ok();
        let head_oid = head.as_ref().and_then(|h| h.target());

        let refs = self.repo.references()?;
        for r in refs {
            let Ok(r) = r else { continue };
            let Some(name) = r.name() else { continue };
            let is_remote = r.is_remote();
            let Some(target) = r.target() else { continue };
            let short = name
                .rsplit("refs/")
                .next()
                .unwrap_or(name)
                .to_string();
            let kind = if name.starts_with("refs/heads/") {
                RefKind::Local
            } else if is_remote {
                RefKind::Remote
            } else if name.starts_with("refs/tags/") {
                RefKind::Tag
            } else {
                RefKind::Other
            };
            let label = RefLabel {
                name: short,
                kind,
                is_head: head_oid == Some(target),
            };
            map.entry(target.to_string()).or_default().push(label);
        }
        Ok(map)
    }
}

/// Two-letter git status code (staged, unstaged).
fn status_codes(status: git2::Status) -> (char, char) {
    let staged = if status.contains(git2::Status::INDEX_NEW) {
        'A'
    } else if status.contains(git2::Status::INDEX_MODIFIED) {
        'M'
    } else if status.contains(git2::Status::INDEX_DELETED) {
        'D'
    } else if status.contains(git2::Status::INDEX_RENAMED) {
        'R'
    } else if status.contains(git2::Status::INDEX_TYPECHANGE) {
        'T'
    } else {
        ' '
    };
    let unstaged = if status.contains(git2::Status::WT_NEW) {
        if staged == ' ' {
            '?'
        } else {
            ' '
        }
    } else if status.contains(git2::Status::WT_MODIFIED) {
        'M'
    } else if status.contains(git2::Status::WT_DELETED) {
        'D'
    } else if status.contains(git2::Status::WT_RENAMED) {
        'R'
    } else if status.contains(git2::Status::WT_TYPECHANGE) {
        'T'
    } else {
        ' '
    };
    (staged, unstaged)
}

fn parse_log_null_sep(text: &str) -> Vec<CommitSummary> {
    // `git log` emits records as `id\0author\0date\0summary` with NO trailing
    // separator, so splitting on NUL yields individual fields. Group them in
    // fours.
    let fields: Vec<&str> = text.split('\0').filter(|s| !s.is_empty()).collect();
    let mut out = Vec::new();
    for chunk in fields.chunks_exact(4) {
        let id = chunk[0].to_string();
        out.push(CommitSummary {
            id: id.clone(),
            short_id: id[..8.min(id.len())].to_string(),
            author: chunk[1].to_string(),
            author_email: String::new(),
            author_date: parse_iso_date(chunk[2]),
            committer: String::new(),
            committer_date: 0,
            message: chunk[3].to_string(),
            parents: Vec::new(),
        });
    }
    out
}

pub fn parse_iso_date(s: &str) -> i64 {
    // ISO-8601 like 2024-01-02T03:04:05+00:00 (or ...Z) -> epoch seconds.
    let s = s.trim();
    let bytes = s.as_bytes();
    if bytes.len() < 19 {
        return 0;
    }

    // Date part.
    let y: i64 = match s.get(0..4).and_then(|v| v.parse().ok()) {
        Some(v) => v,
        None => return 0,
    };
    let m: i64 = match s.get(5..7).and_then(|v| v.parse().ok()) {
        Some(v) => v,
        None => return 0,
    };
    let d: i64 = match s.get(8..10).and_then(|v| v.parse().ok()) {
        Some(v) => v,
        None => return 0,
    };
    let hh: i64 = match s.get(11..13).and_then(|v| v.parse().ok()) {
        Some(v) => v,
        None => return 0,
    };
    let mm: i64 = match s.get(14..16).and_then(|v| v.parse().ok()) {
        Some(v) => v,
        None => return 0,
    };
    let ss: i64 = match s.get(17..19).and_then(|v| v.parse().ok()) {
        Some(v) => v,
        None => return 0,
    };

    // Timezone offset: trailing +HH:MM / -HH:MM / Z.
    let mut tz_offset = 0i64;
    let rest = &s[19..];
    if let Some(plus) = rest.find('+') {
        let (oh, om) = parse_tz_offset(&rest[plus..]);
        tz_offset = oh * 3600 + om * 60;
    } else if let Some(minus) = rest.find('-') {
        let (oh, om) = parse_tz_offset(&rest[minus..]);
        tz_offset = -(oh * 3600 + om * 60);
    }

    let days = days_from_civil(y, m, d);
    days * 86400 + hh * 3600 + mm * 60 + ss - tz_offset
}

fn parse_tz_offset(s: &str) -> (i64, i64) {
    let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() >= 4 {
        let h: i64 = digits[0..2].parse().unwrap_or(0);
        let m: i64 = digits[2..4].parse().unwrap_or(0);
        (h, m)
    } else {
        (0, 0)
    }
}

/// Days from 1970-01-01 (Howard Hinnant's civil-from-days algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Format an epoch-seconds timestamp as `YYYY-MM-DD HH:MM:SS` (UTC).
pub fn format_timestamp(secs: i64) -> String {
    let days = secs.div_euclid(86400);
    let tod = secs.rem_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    let hh = tod / 3600;
    let mm = (tod % 3600) / 60;
    let ss = tod % 60;
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", y, m, d, hh, mm, ss)
}

/// Civil date (Howard Hinnant's civil-to-days inverse).
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// Extract an `owner/repo` slug from a remote URL, handling https, http,
/// ssh:// and scp-like (`git@host:owner/repo`) forms.
pub fn repo_slug_from_url(url: &str) -> Option<String> {
    let trimmed = url.trim().trim_end_matches(".git").trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let after_host = if let Some(rest) = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        .or_else(|| trimmed.strip_prefix("ssh://"))
    {
        // host/owner/repo
        rest.split_once('/').map(|(_, owner)| owner)
    } else if trimmed.contains('@') && trimmed.contains(':') {
        // scp-like: git@github.com:owner/repo
        trimmed
            .split('@')
            .nth(1)
            .and_then(|rest| rest.split_once(':').map(|(_, owner)| owner))
    } else {
        None
    }?;

    let slug = after_host.trim_matches('/').to_string();
    if slug.contains('/') && !slug.starts_with('/') && slug.split('/').count() >= 2 {
        Some(slug)
    } else {
        None
    }
}

/// Best-effort parse of a unified diff into per-line entries for highlighting.
fn parse_diff_lines(text: &str) -> Vec<DiffLineEntry> {
    let mut out = Vec::new();
    let mut old_line = 0i64;
    let mut new_line = 0i64;
    for line in text.lines() {
        if line.starts_with("@@") {
            if let Some(caps) = parse_hunk_header(line) {
                old_line = caps.0 as i64 - 1;
                new_line = caps.1 as i64 - 1;
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("+") {
            new_line += 1;
            out.push(DiffLineEntry {
                old_line: 0,
                new_line,
                origin: '+',
                content: rest.to_string(),
            });
        } else if let Some(rest) = line.strip_prefix("-") {
            old_line += 1;
            out.push(DiffLineEntry {
                old_line,
                new_line: 0,
                origin: '-',
                content: rest.to_string(),
            });
        } else if let Some(rest) = line.strip_prefix(" ") {
            old_line += 1;
            new_line += 1;
            out.push(DiffLineEntry {
                old_line,
                new_line,
                origin: ' ',
                content: rest.to_string(),
            });
        } else if let Some(rest) = line.strip_prefix("\\") {
            out.push(DiffLineEntry {
                old_line: 0,
                new_line: 0,
                origin: '\\',
                content: rest.to_string(),
            });
        }
    }
    out
}

fn parse_hunk_header(line: &str) -> Option<(usize, usize)> {
    // @@ -old,count +new,count @@
    let inner = line.trim_start_matches("@@").trim_end_matches("@@").trim();
    let mut parts = inner.split_whitespace();
    let old = parts.next()?.trim_start_matches('-');
    let new = parts.next()?.trim_start_matches('+');
    let old_start = old.split(',').next()?.parse().ok()?;
    let new_start = new.split(',').next()?.parse().ok()?;
    Some((old_start, new_start))
}

/// What to diff against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffMode {
    Staged,
    Unstaged,
    Head,
}

/// Reset style.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetMode {
    Soft,
    Mixed,
    Hard,
}

/// A changed file in the working tree.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct FileStatus {
    pub path: String,
    pub staged: char,
    pub unstaged: char,
    pub in_index: bool,
    pub in_workdir: bool,
}

/// A line of blame output.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct BlameLine {
    pub line: usize,
    pub commit: String,
    pub author: String,
    pub date: String,
    pub summary: String,
    pub final_line: usize,
}

/// A line in a unified diff.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct DiffLineEntry {
    pub old_line: i64,
    pub new_line: i64,
    pub origin: char,
    pub content: String,
}

/// Summary metadata for a commit.
#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct CommitSummary {
    pub id: String,
    pub short_id: String,
    pub author: String,
    pub author_email: String,
    pub author_date: i64,
    pub committer: String,
    pub committer_date: i64,
    pub message: String,
    pub parents: Vec<String>,
}

/// A node in the commit DAG.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct CommitNode {
    pub id: String,
    pub short_id: String,
    pub message: String,
    pub author: String,
    pub date: i64,
    pub parents: Vec<String>,
    pub refs: Vec<RefLabel>,
}

/// A ref (branch/tag/remote) pointing at a commit.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RefLabel {
    pub name: String,
    pub kind: RefKind,
    pub is_head: bool,
}

/// Kind of ref.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RefKind {
    Local,
    Remote,
    Tag,
    Other,
}

/// A commit graph and any detached refs.
#[derive(Debug, Clone)]
pub struct CommitGraph {
    pub nodes: Vec<CommitNode>,
    pub detached_refs: Vec<RefLabel>,
}

/// Information about branches
#[derive(Debug, Default)]
pub struct BranchesInfo {
    pub local: Vec<BranchInfo>,
    pub remote: HashMap<String, Vec<BranchInfo>>,
}

/// Information about a single branch
#[derive(Debug, Clone)]
pub struct BranchInfo {
    pub name: String,
    pub is_head: bool,
    pub upstream: Option<String>,
}

impl std::fmt::Display for BranchInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)?;
        if self.is_head {
            write!(f, " (HEAD)")?;
        }
        if let Some(upstream) = &self.upstream {
            write!(f, " -> {}", upstream)?;
        }
        Ok(())
    }
}

// Re-export git2 types for convenience

// Auto-save ------------------------------------------------------------------

const AUTOSAVE_REF: &str = "refs/gitmulti/autosave";

impl GitRepo {
    /// Ensure the autosave ref exists, pointing at HEAD if it does not yet exist.
    pub fn ensure_autosave_ref(&self) -> Result<()> {
        if self.repo.find_reference(AUTOSAVE_REF).is_err() {
            let head = self.repo.head()?;
            let oid = head.target().ok_or_else(|| {
                GitMultiError::SyncError("HEAD has no target".to_string())
            })?;
            self.repo.reference(AUTOSAVE_REF, oid, true, "init autosave ref")?;
        }
        Ok(())
    }

    /// Returns true if the autosave ref exists in this repository.
    pub fn autosave_ref_exists(&self) -> bool {
        self.repo.find_reference(AUTOSAVE_REF).is_ok()
    }

    /// If the working tree is dirty, create a new unreferenced commit capturing
    /// the current state and fast-forward `refs/gitmulti/autosave` to it.
    /// Returns `Ok(true)` if a snapshot was written, `Ok(false)` if the repo
    /// was clean (or the snapshot already matches).
    ///
    /// The staging of the working tree is done against a *throwaway* index
    /// (`GIT_INDEX_FILE`) so the user's real index — and therefore their
    /// granular stage/unstage decisions — is never touched.
    pub fn write_autosave_snapshot(&self) -> Result<bool> {
        let statuses = self.repo.statuses(None)?;
        let dirty = statuses.iter().any(|s| {
            s.status().intersects(
                git2::Status::WT_NEW
                    | git2::Status::WT_MODIFIED
                    | git2::Status::WT_DELETED
                    | git2::Status::WT_RENAMED
                    | git2::Status::WT_TYPECHANGE,
            )
        });
        if !dirty {
            return Ok(false);
        }

        let workdir = self.workdir();
        let tmp_index = std::env::temp_dir().join(format!(
            "gitmulti-autosave-{}.idx",
            std::process::id()
        ));
        let _ = fs::remove_file(&tmp_index);
        let idx = tmp_index.to_str().unwrap_or("");

        let envs = [("GIT_INDEX_FILE", idx), ("GIT_TERMINAL_PROMPT", "0")];

        let add = run_captured("git", &["add", "-A"], workdir, &envs, GIT_TIMEOUT)?;
        if !add.status.success() {
            let _ = fs::remove_file(&tmp_index);
            return Err(GitMultiError::SyncError("git add failed for auto-save".to_string()));
        }

        let wt = run_captured("git", &["write-tree"], workdir, &envs, GIT_TIMEOUT)?;
        if !wt.status.success() {
            let _ = fs::remove_file(&tmp_index);
            return Err(GitMultiError::SyncError("git write-tree failed for auto-save".to_string()));
        }
        let tree_oid = String::from_utf8_lossy(&wt.stdout).trim().to_string();

        // Skip when the snapshot tree is identical to the current autosave ref
        // (avoids creating a new orphan commit every 30s of idle time).
        if let Ok(reference) = self.repo.find_reference(AUTOSAVE_REF) {
            if let Some(target) = reference.target() {
                if let Ok(existing) = self.repo.find_commit(target) {
                    if existing.tree_id().to_string() == tree_oid {
                        let _ = fs::remove_file(&tmp_index);
                        return Ok(false);
                    }
                }
            }
        }

        let ct = run_captured(
            "git",
            &["commit-tree", &tree_oid, "-p", "HEAD", "-m", "[auto-save] workspace snapshot"],
            workdir,
            &[],
            GIT_TIMEOUT,
        )?;
        let _ = fs::remove_file(&tmp_index);
        if !ct.status.success() {
            let stderr = String::from_utf8_lossy(&ct.stderr);
            return Err(GitMultiError::SyncError(format!(
                "git commit-tree failed for auto-save: {}",
                stderr.trim()
            )));
        }
        let new_oid = String::from_utf8_lossy(&ct.stdout).trim().to_string();

        git_run_str(workdir, &["update-ref", AUTOSAVE_REF, &new_oid])?;
        Ok(true)
    }

    /// Merge the auto-saved state into the current working tree.
    ///
    /// Restores both the index and the working tree from the hidden snapshot,
    /// so files destroyed by something like `git reset --hard` are recovered.
    pub fn restore_from_autosave(&self) -> Result<()> {
        if !self.autosave_ref_exists() {
            return Err(GitMultiError::SyncError(
                "No auto-save snapshot found. Use the TUI (O) after an idle autosave has occurred.".to_string(),
            ));
        }
        let workdir = self.workdir();
        git_run_str(workdir, &["restore", "--source", AUTOSAVE_REF, "--staged", "--worktree", "--", "."])?;
        Ok(())
    }
}

// ========================================================================
// Subprocess helpers
//
// Every git subprocess goes through these helpers so that:
//   * stdout/stderr are captured (never inherited) — the TUI screen is never
//     corrupted by git progress output;
//   * prompts are disabled (GIT_TERMINAL_PROMPT=0) and SSH uses batch mode,
//     so a credential request cannot hang the process;
//   * every command has a hard timeout, after which the child is killed.
// ========================================================================

const GIT_TIMEOUT: Duration = Duration::from_secs(120);

/// Run a program with piped stdout/stderr, capturing output, with a hard
/// timeout. Never inherits the terminal, so an interactive prompt cannot hang
/// the UI; a timeout kills the child and returns an error.
pub fn run_captured(
    program: &str,
    args: &[&str],
    workdir: &Path,
    envs: &[(&str, &str)],
    timeout: Duration,
) -> Result<Output> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(workdir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(GitMultiError::SyncError(format!(
                "Program '{}' not found on PATH. Is it installed?",
                program
            )));
        }
        Err(e) => return Err(GitMultiError::IoError(e)),
    };
    let output = wait_with_timeout(&mut child, timeout)?;
    Ok(output)
}

/// Run `git` capturing output, with prompts disabled and a hard timeout.
pub fn git_run(workdir: &Path, args: &[&str]) -> Result<Output> {
    run_captured(
        "git",
        args,
        workdir,
        &[("GIT_TERMINAL_PROMPT", "0"), ("GIT_OPTIONAL_LOCKS", "0")],
        GIT_TIMEOUT,
    )
}

/// Like [`git_run`] but for network operations: also forces SSH into
/// non-interactive batch mode with a short connect timeout so a dead or
/// unauthenticated host cannot hang the caller.
pub fn git_run_net(workdir: &Path, args: &[&str]) -> Result<Output> {
    run_captured(
        "git",
        args,
        workdir,
        &[
            ("GIT_TERMINAL_PROMPT", "0"),
            ("GIT_OPTIONAL_LOCKS", "0"),
            ("GIT_SSH_COMMAND", "ssh -oBatchMode=yes -oConnectTimeout=10"),
        ],
        GIT_TIMEOUT,
    )
}

/// Run git and return stdout as a string, erroring on a non-zero exit.
pub fn git_run_str(workdir: &Path, args: &[&str]) -> Result<String> {
    let out = git_run(workdir, args)?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() {
        return Err(GitMultiError::SyncError(format!(
            "git {} failed: {}",
            args.join(" "),
            stderr.trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Like [`git_run_str`] but for network operations.
pub fn git_run_net_str(workdir: &Path, args: &[&str]) -> Result<String> {
    let out = git_run_net(workdir, args)?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() {
        return Err(GitMultiError::SyncError(format!(
            "git {} failed: {}",
            args.join(" "),
            stderr.trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Wait for a child with a hard timeout, draining stdout/stderr on reader
/// threads so a chatty child can never fill the pipe buffer and deadlock.
pub fn wait_with_timeout(child: &mut Child, timeout: Duration) -> Result<Output> {
    let out_reader = child.stdout.take().map(|mut pipe| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = pipe.read_to_end(&mut buf);
            buf
        })
    });
    let err_reader = child.stderr.take().map(|mut pipe| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = pipe.read_to_end(&mut buf);
            buf
        })
    });

    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(GitMultiError::IoError)? {
            break status;
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(GitMultiError::SyncError(format!(
                "command timed out after {}s",
                timeout.as_secs()
            )));
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    let stdout = out_reader.and_then(|h| h.join().ok()).unwrap_or_default();
    let stderr = err_reader.and_then(|h| h.join().ok()).unwrap_or_default();
    Ok(Output { status, stdout, stderr })
}
