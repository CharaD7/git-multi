use crate::config::Config;
use crate::error::{GitMultiError, Result};
use git2::{BranchType, Repository};
use std::collections::HashMap;
use std::fs;
use std::process::Command;

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
        let workdir = self.repo.workdir()
            .unwrap_or_else(|| self.repo.path());

        let status = Command::new("git")
            .args(["fetch", name])
            .current_dir(workdir)
            .status()
            .map_err(GitMultiError::IoError)?;

        if !status.success() {
            return Err(GitMultiError::SyncError(
                format!("git fetch {} failed with exit code: {}", name, status.code().unwrap_or(-1))
            ));
        }
        Ok(())
    }

    /// Fetch from all remotes
    pub fn fetch_all(&self) -> Result<Vec<String>> {
        let remote_names = self.repo.remotes()?;
        let mut fetched = Vec::new();
        
        for name in remote_names.iter().flatten() {
            self.fetch_remote(name)?;
            fetched.push(name.to_string());
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

    /// Cherry-pick commits from one branch to another
    pub fn cherry_pick_range(
        &self,
        from_branch: &str,
        to_branch: &str,
        commit_range: &str,
    ) -> Result<Vec<String>> {
        let mut picked_commits = Vec::new();
        
        // Get the commit range
        let from_ref = self.repo.find_reference(from_branch)?;
        let to_ref = self.repo.find_reference(to_branch)?;
        
        let from_commit = from_ref.peel_to_commit()?;
        let _to_commit = to_ref.peel_to_commit()?;
        
        // Parse commit range (e.g., "HEAD~3..HEAD" or "abc123..def456")
        let (from_sha, to_sha) = self.parse_commit_range(commit_range, &from_commit)?;
        
        // Get the commits to cherry-pick
        let _from_obj = self.repo.find_object(from_sha, None)?;
        let _to_obj = self.repo.find_object(to_sha, None)?;
        
        let mut revwalk = self.repo.revwalk()?;
        revwalk.set_sorting(git2::Sort::TOPOLOGICAL)?;
        revwalk.push(from_sha)?;
        revwalk.hide(to_sha)?;
        
        let commits: Vec<git2::Commit> = revwalk
            .filter_map(|oid_result| {
                let oid = oid_result.ok()?;
                self.repo.find_commit(oid).ok()
            })
            .collect();
        
        // Checkout the target branch
        self.checkout_branch(to_branch)?;
        
        // Cherry-pick each commit
        for commit in commits {
            let commit_sha = commit.id().to_string();
            let mut options = git2::CherrypickOptions::new();
            
            self.repo.cherrypick(&commit, Some(&mut options))?;
            
            // Check for conflicts
            if self.repo.index()?.has_conflicts() {
                return Err(GitMultiError::SyncConflict);
            }
            
            // Commit the cherry-pick
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
            
            picked_commits.push(commit_sha);
        }
        
        Ok(picked_commits)
    }

    /// Parse commit range string
    fn parse_commit_range(
        &self,
        range: &str,
        _from_commit: &git2::Commit,
    ) -> Result<(git2::Oid, git2::Oid)> {
        let parts: Vec<&str> = range.split("..").map(|s| s.trim()).collect();
        
        if parts.len() != 2 {
            return Err(GitMultiError::SyncError(
                format!("Invalid commit range: {}", range)
            ));
        }
        
        let from_sha = self.resolve_commit_spec(parts[0])?;
        let to_sha = self.resolve_commit_spec(parts[1])?;
        
        Ok((from_sha, to_sha))
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
        
        self.repo.merge(&[&annotated_commit], Some(&mut options), None)?;
        
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
        let workdir = self.repo.workdir()
            .unwrap_or_else(|| self.repo.path());

        // Default to the current branch when no branch is specified.
        let branch = match branch_name {
            Some(b) => b.to_string(),
            None => self.current_branch()?
                .ok_or_else(|| GitMultiError::SyncError(
                    "Cannot determine current branch to push".to_string()
                ))?,
        };

        let status = Command::new("git")
            .args(["push", remote_name, &branch])
            .current_dir(workdir)
            .status()
            .map_err(GitMultiError::IoError)?;

        if !status.success() {
            return Err(GitMultiError::SyncError(
                format!("git push {} failed with exit code: {}", remote_name, status.code().unwrap_or(-1))
            ));
        }
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
        let workdir = self.repo.workdir()
            .unwrap_or_else(|| self.repo.path());

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

        let status = Command::new("git")
            .args(&cargs)
            .current_dir(workdir)
            .status()
            .map_err(GitMultiError::IoError)?;

        if !status.success() {
            return Err(GitMultiError::SyncError(
                format!("git pull {} failed with exit code: {}", remote_name, status.code().unwrap_or(-1))
            ));
        }
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

        let workdir = self.repo.workdir().unwrap_or_else(|| self.repo.path());
        let mut args: Vec<String> = vec!["fetch".to_string(), remote_name.to_string()];
        for b in branches {
            args.push(b.clone());
        }
        let cargs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

        let status = Command::new("git")
            .args(&cargs)
            .current_dir(workdir)
            .status()
            .map_err(GitMultiError::IoError)?;

        if !status.success() {
            return Err(GitMultiError::SyncError(format!(
                "git fetch {} failed with exit code: {}",
                remote_name,
                status.code().unwrap_or(-1)
            )));
        }
        Ok(())
    }

    /// Push specific branches to a remote using the system git binary.
    pub fn push_branches(&self, remote_name: &str, branches: &[String], force: bool) -> Result<()> {
        let workdir = self.repo.workdir().unwrap_or_else(|| self.repo.path());

        for branch in branches {
            let mut args: Vec<String> = vec!["push".to_string(), remote_name.to_string()];
            if force {
                args.push("--force".into());
            }
            args.push(branch.clone());
            let cargs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

            let status = Command::new("git")
                .args(&cargs)
                .current_dir(workdir)
                .status()
                .map_err(GitMultiError::IoError)?;

            if !status.success() {
                return Err(GitMultiError::SyncError(format!(
                    "git push {} {} failed with exit code: {}",
                    remote_name,
                    branch,
                    status.code().unwrap_or(-1)
                )));
            }
        }
        Ok(())
    }

    /// Pull specific branches from a remote using the system git binary.
    pub fn pull_branches(&self, remote_name: &str, branches: &[String]) -> Result<()> {
        let workdir = self.repo.workdir().unwrap_or_else(|| self.repo.path());

        for branch in branches {
            let args: Vec<String> = vec!["pull".to_string(), remote_name.to_string(), branch.clone()];
            let cargs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

            let status = Command::new("git")
                .args(&cargs)
                .current_dir(workdir)
                .status()
                .map_err(GitMultiError::IoError)?;

            if !status.success() {
                return Err(GitMultiError::SyncError(format!(
                    "git pull {} {} failed with exit code: {}",
                    remote_name,
                    branch,
                    status.code().unwrap_or(-1)
                )));
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
        if self.config.get_default_remote().map_or(false, |d| d == old) {
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

    /// Fetch a remote ref and merge it into the current branch, creating a
    /// merge commit when the merge is clean. Used for cross-remote merges.
    pub fn merge_and_commit(&self, src_ref: &str) -> Result<()> {
        let reference = self.repo.find_reference(src_ref)?;
        let oid = reference.target().ok_or_else(|| {
            GitMultiError::SyncError(format!("Reference {} has no target", src_ref))
        })?;
        let src_commit = self.repo.find_commit(oid)?;
        let annotated = self.repo.find_annotated_commit(oid)?;

        let mut opts = git2::MergeOptions::default();
        opts.fail_on_conflict(true);
        self.repo.merge(&[&annotated], Some(&mut opts), None)?;

        if self.repo.index()?.has_conflicts() {
            return Err(GitMultiError::SyncConflict);
        }

        let signature = self.repo.signature()?;
        let tree_oid = self.repo.index()?.write_tree()?;
        let tree = self.repo.find_tree(tree_oid)?;
        let head = self.head_commit()?;
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
    }

    /// Produce a human-readable status string for display.
    pub fn status_text(&self) -> Result<String> {
        let workdir = self.repo.workdir().unwrap_or_else(|| self.repo.path());

        let mut out = String::new();
        out.push_str("Remotes:\n");
        for (name, url) in self.list_remotes_with_urls()? {
            let marker = if self.config.get_default_remote().map_or(false, |d| d == &name) {
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
        let status_out = Command::new("git")
            .args(["status", "--short"])
            .current_dir(workdir)
            .output()
            .map_err(GitMultiError::IoError)?;
        let st = String::from_utf8_lossy(&status_out.stdout);
        if st.trim().is_empty() {
            out.push_str("  clean\n");
        } else {
            for line in st.lines() {
                out.push_str(&format!("  {}\n", line));
            }
        }
        Ok(out)
    }

    /// Copy files from one commit/branch to current working directory
    pub fn copy_files(
        &self,
        from_ref: &str,
        files: &[String],
    ) -> Result<Vec<String>> {
        let from_commit = self.resolve_commit_spec(from_ref)?;
        let from_tree = self.repo.find_commit(from_commit)?.tree()?;
        
        let mut copied = Vec::new();
        
        for file in files {
            if let Some(entry) = from_tree.get_name(file) {
                let obj = entry.to_object(&self.repo)?;
                let blob = obj.peel_to_blob()?;
                
                // Write to working directory
                fs::write(file, blob.content())?;
                copied.push(file.clone());
            }
        }
        
        Ok(copied)
    }

    /// Create a Pull Request using gh CLI (if available)
    pub fn create_pr(
        &self,
        remote_name: &str,
        base_branch: &str,
        head_branch: &str,
        title: &str,
        description: Option<&str>,
    ) -> Result<()> {
        let status = Command::new("gh")
            .args(["pr", "create"])
            .arg("--repo").arg(remote_name)
            .arg("--base").arg(base_branch)
            .arg("--head").arg(head_branch)
            .arg("--title").arg(title)
            .arg("--body").arg(description.unwrap_or(""))
            .status()?;
        
        if !status.success() {
            return Err(GitMultiError::SyncError(
                format!("gh CLI failed with exit code: {}", status.code().unwrap_or(-1))
            ));
        }
        
        Ok(())
    }
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
