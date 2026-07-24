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
        let status = Command::new("git")
            .args(["commit", "-m", &full_msg])
            .current_dir(workdir)
            .status()
            .map_err(GitMultiError::IoError)?;

        if !status.success() {
            return Err(GitMultiError::SyncError(
                format!("git commit failed with exit code: {}", status.code().unwrap_or(-1))
            ));
        }
        Ok(())
    }

    /// List recent commits for display
    pub fn list_recent_commits(&self, count: usize) -> Result<Vec<String>> {
        let workdir = self.repo.workdir()
            .unwrap_or_else(|| self.repo.path());

        let status = Command::new("git")
            .args(["log", "-n", &count.to_string(), "--oneline", "--decorate"])
            .current_dir(workdir)
            .output()
            .map_err(GitMultiError::IoError)?;

        let out = String::from_utf8_lossy(&status.stdout);
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
        let status = Command::new("git")
            .args(["add", "--", path])
            .current_dir(workdir)
            .status()
            .map_err(GitMultiError::IoError)?;
        if !status.success() {
            return Err(GitMultiError::SyncError(format!("git add {} failed", path)));
        }
        Ok(())
    }

    /// Unstage a single file (reset its entry out of the index, keeping the
    /// working-tree contents).
    pub fn unstage_file(&self, path: &str) -> Result<()> {
        let workdir = self.workdir();
        let status = Command::new("git")
            .args(["restore", "--staged", "--", path])
            .current_dir(workdir)
            .status()
            .map_err(GitMultiError::IoError)?;
        if !status.success() {
            return Err(GitMultiError::SyncError(format!(
                "git restore --staged {} failed",
                path
            )));
        }
        Ok(())
    }

    /// Discard working-tree changes for a single file (restore from index/HEAD).
    pub fn restore_file(&self, path: &str) -> Result<()> {
        let workdir = self.workdir();
        let status = Command::new("git")
            .args(["restore", "--", path])
            .current_dir(workdir)
            .status()
            .map_err(GitMultiError::IoError)?;
        if !status.success() {
            return Err(GitMultiError::SyncError(format!("git restore {} failed", path)));
        }
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
        let out = Command::new("git")
            .args(&cargs)
            .current_dir(workdir)
            .output()
            .map_err(GitMultiError::IoError)?;
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
        let out = Command::new("git")
            .args(&cargs)
            .current_dir(workdir)
            .output()
            .map_err(GitMultiError::IoError)?;
        let text = String::from_utf8_lossy(&out.stdout);
        Ok(parse_diff_lines(&text))
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
        let status = Command::new("git")
            .args(["commit", "--amend", "-m", &full_msg])
            .current_dir(workdir)
            .status()
            .map_err(GitMultiError::IoError)?;
        if !status.success() {
            return Err(GitMultiError::SyncError("git commit --amend failed".to_string()));
        }
        Ok(())
    }

    /// Create a new commit that reverts the given commit (uses `git revert`).
    pub fn revert_commit(&self, commit_spec: &str) -> Result<()> {
        let workdir = self.workdir();
        let status = Command::new("git")
            .args(["revert", "--no-edit", commit_spec])
            .current_dir(workdir)
            .status()
            .map_err(GitMultiError::IoError)?;
        if !status.success() {
            return Err(GitMultiError::SyncError(format!(
                "git revert {} failed",
                commit_spec
            )));
        }
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
        let status = Command::new("git")
            .args(["reset", flag, commit_spec])
            .current_dir(workdir)
            .status()
            .map_err(GitMultiError::IoError)?;
        if !status.success() {
            return Err(GitMultiError::SyncError(format!(
                "git reset {} {} failed",
                flag, commit_spec
            )));
        }
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
            let date = format!("{}", when.seconds());
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
        let out = Command::new("git")
            .args([
                "log",
                "--follow",
                "--format=%H%x00%an%x00%aI%x00%s",
                "--",
                path,
            ])
            .current_dir(workdir)
            .output()
            .map_err(GitMultiError::IoError)?;
        Ok(parse_log_null_sep(&String::from_utf8_lossy(&out.stdout)))
    }

    /// Line history for a single file (GitLens "line history"), newest first.
    pub fn line_history(&self, path: &str, line: usize) -> Result<Vec<CommitSummary>> {
        let workdir = self.workdir();
        let out = Command::new("git")
            .args([
                "-c",
                "core.pager=cat",
                "log",
                "-L",
                &format!("{},{}:{}", line, line, path),
                "--format=%H%x00%an%x00%aI%x00%s",
            ])
            .current_dir(workdir)
            .output()
            .map_err(GitMultiError::IoError)?;
        Ok(parse_log_null_sep(&String::from_utf8_lossy(&out.stdout)))
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
        self.repo.cherrypick(&commit, Some(&mut opts))?;
        if self.repo.index()?.has_conflicts() {
            return Err(GitMultiError::SyncConflict);
        }
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
        let mut riter = refs.into_iter();
        while let Some(r) = riter.next() {
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

fn parse_iso_date(s: &str) -> i64 {
    // ISO-8601 like 2024-01-02T03:04:05+00:00 -> best-effort epoch seconds.
    chrono_parse(s).unwrap_or(0)
}

fn chrono_parse(_s: &str) -> Option<i64> {
    // Avoid adding a chrono dependency; return None and let callers fall back
    // to displaying the raw string where needed. Kept as a seam for later.
    None
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
pub struct FileStatus {
    pub path: String,
    pub staged: char,
    pub unstaged: char,
    pub in_index: bool,
    pub in_workdir: bool,
}

/// A line of blame output.
#[derive(Debug, Clone)]
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
pub struct DiffLineEntry {
    pub old_line: i64,
    pub new_line: i64,
    pub origin: char,
    pub content: String,
}

/// Summary metadata for a commit.
#[derive(Debug, Clone, Default)]
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

    /// If the working tree is dirty, create a new unreferenced commit capturing the
    /// current state and fast-forward `refs/gitmulti/autosave` to it. Returns
    /// `Ok(true)` if a snapshot was written, `Ok(false)` if the repo was clean.
    pub fn write_autosave_snapshot(&self) -> Result<bool> {
        let statuses = self.repo.statuses(None)?;
        let dirty = statuses.iter().any(|s| {
            s.status()
                .intersects(git2::Status::WT_NEW | git2::Status::WT_MODIFIED | git2::Status::WT_DELETED | git2::Status::WT_RENAMED | git2::Status::WT_TYPECHANGE)
        });
        if !dirty {
            return Ok(false);
        }

        let workdir = self.workdir();
        let stage_status = Command::new("git")
            .args(["add", "-A"])
            .current_dir(workdir)
            .status()
            .map_err(GitMultiError::IoError)?;
        if !stage_status.success() {
            return Err(GitMultiError::SyncError("git add failed for auto-save".to_string()));
        }

        let mut index = self.repo.index()?;
        let tree_oid = index.write_tree()?;
        let tree = self.repo.find_tree(tree_oid)?;
        let parent = self.head_commit()?;
        let sig = self.repo.signature()?;
        let parents = [&parent];
        let commit_oid = self.repo.commit(
            None,
            &sig,
            &sig,
            "[auto-save] workspace snapshot",
            &tree,
            &parents,
        )?;

        let mut ref_obj = self.repo.find_reference(AUTOSAVE_REF)?;
        ref_obj.set_target(commit_oid, "[auto-save] update snapshot")?;
        Ok(true)
    }

    /// Merge the auto-saved state into the current working tree.
    ///
    /// This performs `git checkout refs/gitmulti/autosave -- .` so that files
    /// from the hidden snapshot overwrite whatever is currently on disk.
    pub fn restore_from_autosave(&self) -> Result<()> {
        if !self.autosave_ref_exists() {
            return Err(GitMultiError::SyncError(
                "No auto-save snapshot found. Use the TUI (O) after an idle autosave has occurred.".to_string(),
            ));
        }
        let workdir = self.workdir();
        let status = Command::new("git")
            .args(["checkout", AUTOSAVE_REF, "--", "."])
            .current_dir(workdir)
            .status()
            .map_err(GitMultiError::IoError)?;
        if !status.success() {
            return Err(GitMultiError::SyncError(
                "git checkout refs/gitmulti/autosave failed".to_string(),
            ));
        }
        Ok(())
    }
}
