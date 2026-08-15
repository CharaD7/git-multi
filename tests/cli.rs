//! Integration tests for the `git-multi` CLI.
//!
//! These run the compiled binary against throwaway git repositories in a
//! temp directory, so they never touch a real checkout.

use predicates::prelude::*;
use std::fs;
use std::path::Path;
use std::process::{Command as ProcCommand, Stdio};
use tempfile::TempDir;

/// Run `git` in `dir` with an isolated HOME, asserting success.
fn git(dir: &Path, args: &[&str]) {
    let out = ProcCommand::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn git {:?}: {}", args, e));
    assert!(
        out.status.success(),
        "git {:?} failed:\nstdout: {}\nstderr: {}",
        args,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Run `git` in `dir`, returning trimmed stdout, asserting success.
fn git_out(dir: &Path, args: &[&str]) -> String {
    let out = ProcCommand::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A throwaway repo with a `main` branch and one initial commit.
struct Repo {
    dir: TempDir,
}

impl Repo {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-b", "main"]);
        git(dir.path(), &["config", "user.email", "test@example.com"]);
        git(dir.path(), &["config", "user.name", "Test User"]);
        fs::write(dir.path().join("file.txt"), "hello\n").unwrap();
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-m", "initial commit"]);
        Repo { dir }
    }

    fn gm(&self) -> assert_cmd::Command {
        let mut cmd = assert_cmd::Command::cargo_bin("git-multi").unwrap();
        cmd.current_dir(self.dir.path());
        cmd.env("GIT_CONFIG_NOSYSTEM", "1");
        cmd.env("GIT_TERMINAL_PROMPT", "0");
        cmd
    }

    fn commit(&self, msg: &str) {
        git(self.dir.path(), &["add", "-A"]);
        git(self.dir.path(), &["commit", "-m", msg]);
    }

    /// Write a fresh file and commit, guaranteeing a non-empty commit.
    fn change_and_commit(&self, msg: &str) {
        let n = git_out(self.dir.path(), &["rev-list", "--count", "HEAD"]);
        fs::write(
            self.dir.path().join(format!("change-{}.txt", n)),
            format!("{}\n", n),
        )
        .unwrap();
        self.commit(msg);
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }
}

/// A bare repo usable as a remote.
struct BareRemote {
    dir: TempDir,
}

impl BareRemote {
    fn new(name: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--bare", "-b", "main"]);
        let _ = name;
        BareRemote { dir }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }
}

fn bare_path(dir: &TempDir) -> String {
    dir.path().display().to_string()
}

// ---------------------------------------------------------------------------
// init & version
// ---------------------------------------------------------------------------

#[test]
fn init_creates_config() {
    let repo = Repo::new();
    repo.gm().arg("init").assert().success();
    assert!(repo.path().join(".gitmulti/config.toml").exists());
}

#[test]
fn version_matches_package() {
    let repo = Repo::new();
    repo.gm()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains(env!("CARGO_PKG_VERSION")));
}

// ---------------------------------------------------------------------------
// remotes
// ---------------------------------------------------------------------------

#[test]
fn remote_lifecycle() {
    let repo = Repo::new();
    repo.gm()
        .args(["remote", "add", "upstream", "https://github.com/user/repo.git"])
        .assert()
        .success();
    repo.gm()
        .args(["remote", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("upstream"));
    repo.gm()
        .args(["remote", "list-names"])
        .assert()
        .success()
        .stdout(predicate::str::contains("upstream"));
    repo.gm()
        .args(["remote", "rename", "upstream", "origin"])
        .assert()
        .success();
    repo.gm()
        .args(["remote", "list-names"])
        .assert()
        .success()
        .stdout(predicate::str::contains("origin"))
        .stdout(predicate::str::contains("upstream").not());
}

#[test]
fn remote_remove_requires_confirmation_offline() {
    let repo = Repo::new();
    repo.gm()
        .args(["remote", "add", "upstream", "https://github.com/user/repo.git"])
        .assert()
        .success();
    // Non-interactive stdin -> must fail with a message, not hang.
    repo.gm()
        .arg("remote")
        .arg("remove")
        .arg("upstream")
        .write_stdin("n\n")
        .assert()
        .failure()
        .stderr(predicate::str::contains("--force"));
    // With --force it should succeed.
    repo.gm()
        .args(["remote", "remove", "upstream", "--force"])
        .assert()
        .success();
    repo.gm()
        .args(["remote", "list-names"])
        .assert()
        .success()
        .stdout(predicate::str::contains("upstream").not());
}

#[test]
fn remote_list_json() {
    let repo = Repo::new();
    repo.gm()
        .args(["remote", "add", "origin", "https://github.com/user/repo.git"])
        .assert()
        .success();
    repo.gm()
        .args(["remote", "list", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"name\""))
        .stdout(predicate::str::contains("origin"));
}

// ---------------------------------------------------------------------------
// branches
// ---------------------------------------------------------------------------

#[test]
fn branch_lifecycle() {
    let repo = Repo::new();
    repo.gm()
        .args(["branch", "create", "feature", "--checkout"])
        .assert()
        .success();
    repo.gm()
        .args(["branch", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("feature"));
    repo.gm()
        .args(["branch", "rename", "feature", "feature2"])
        .assert()
        .success();
    repo.gm()
        .args(["branch", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("feature2"))
        .stdout(predicate::str::contains("feature\n").not());
}

#[test]
fn branch_list_json() {
    let repo = Repo::new();
    repo.gm()
        .args(["branch", "list", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"local\""));
}

// ---------------------------------------------------------------------------
// commit / stage / diff / reset / log
// ---------------------------------------------------------------------------

#[test]
fn commit_and_log() {
    let repo = Repo::new();
    fs::write(repo.path().join("file.txt"), "hello world\n").unwrap();
    repo.gm()
        .args(["commit", "feat: update content"])
        .assert()
        .success();
    repo.gm()
        .args(["log", "--count", "5"])
        .assert()
        .success()
        .stdout(predicate::str::contains("feat: update content"));
}

#[test]
fn diff_unstaged() {
    let repo = Repo::new();
    fs::write(repo.path().join("file.txt"), "changed\n").unwrap();
    repo.gm()
        .args(["diff", "unstaged"])
        .assert()
        .success()
        .stdout(predicate::str::contains("changed"));
}

#[test]
fn stage_unstage_restore() {
    let repo = Repo::new();
    fs::write(repo.path().join("file.txt"), "staged\n").unwrap();
    repo.gm().args(["stage", "file.txt"]).assert().success();
    repo.gm()
        .args(["diff", "staged"])
        .assert()
        .success()
        .stdout(predicate::str::contains("staged"));
    repo.gm().args(["unstage", "file.txt"]).assert().success();
    repo.gm()
        .args(["diff", "staged"])
        .assert()
        .success()
        .stdout(predicate::str::is_empty().or(predicate::str::contains("(no diff)")));
    repo.gm().args(["restore", "file.txt"]).assert().success();
}

#[test]
fn reset_and_pick() {
    let repo = Repo::new();
    repo.change_and_commit("second commit");
    let head = git_out(repo.path(), &["rev-parse", "HEAD"]);
    let prev = git_out(repo.path(), &["rev-parse", "HEAD~1"]);

    // Reset (hard) back one commit so the working tree is clean for the pick.
    repo.gm().args(["reset", "hard", "HEAD~1"]).assert().success();
    let after = git_out(repo.path(), &["rev-parse", "HEAD"]);
    assert_eq!(prev, after);

    // Cherry-pick the commit back onto HEAD.
    repo.gm().args(["pick", &head]).assert().success();
    let after_pick = git_out(repo.path(), &["rev-parse", "HEAD"]);
    assert_ne!(after_pick, after);
    assert_eq!(git_out(repo.path(), &["rev-parse", "HEAD^"]), prev);
}

#[test]
fn revert_creates_revert_commit() {
    let repo = Repo::new();
    repo.change_and_commit("second commit");
    let head = git_out(repo.path(), &["rev-parse", "HEAD"]);
    repo.gm().args(["revert", &head]).assert().success();
    let msg = git_out(repo.path(), &["log", "-1", "--format=%s"]);
    assert!(msg.contains("Revert"), "unexpected message: {}", msg);
}

// ---------------------------------------------------------------------------
// status / json / errors
// ---------------------------------------------------------------------------

#[test]
fn status_json() {
    let repo = Repo::new();
    repo.gm()
        .args(["status", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"current_branch\""));
}

#[test]
fn no_remotes_push_fails() {
    let repo = Repo::new();
    repo.gm()
        .arg("push")
        .assert()
        .failure()
        .stderr(predicate::str::contains("No remotes configured"));
}

#[test]
fn bad_commit_range_is_rejected() {
    let repo = Repo::new();
    let bare = BareRemote::new("upstream");
    let upstream = bare_path(&bare.dir);
    git(repo.path(), &["remote", "add", "upstream", &upstream]);
    git(repo.path(), &["push", "-u", "upstream", "main"]);
    repo.gm()
        .args([
            "sync",
            "--from-remote",
            "upstream",
            "--to-remote",
            "upstream",
            "--commits",
            "not-a-real-range..",
        ])
        .assert()
        .failure();
}

// ---------------------------------------------------------------------------
// copy
// ---------------------------------------------------------------------------

#[test]
fn copy_files_glob_from_branch() {
    let repo = Repo::new();
    fs::create_dir_all(repo.path().join("src")).unwrap();
    fs::write(repo.path().join("src/lib.rs"), "pub fn x() {}\n").unwrap();
    repo.commit("add src");

    // Branch with different content.
    git(repo.path(), &["checkout", "-b", "other"]);
    fs::write(repo.path().join("src/lib.rs"), "pub fn y() {}\n").unwrap();
    repo.commit("change src");
    git(repo.path(), &["checkout", "main"]);

    repo.gm()
        .args(["copy", "--from", "other", "--files", "src/*.rs"])
        .assert()
        .success();
    let content = fs::read_to_string(repo.path().join("src/lib.rs")).unwrap();
    assert!(content.contains("pub fn y"), "unexpected content: {}", content);
}

// ---------------------------------------------------------------------------
// sync across remotes (cherry-pick)
// ---------------------------------------------------------------------------

#[test]
fn sync_cherry_picks_commits_to_destination_remote() {
    let repo = Repo::new();
    let fork = BareRemote::new("fork");
    let upstream = BareRemote::new("upstream");

    git(
        repo.path(),
        &["remote", "add", "fork", &bare_path(&fork.dir)],
    );
    git(
        repo.path(),
        &["remote", "add", "upstream", &bare_path(&upstream.dir)],
    );
    // Both remotes start at the same base.
    git(repo.path(), &["push", "fork", "main"]);
    git(repo.path(), &["push", "upstream", "main"]);

    // Create a feature branch with a new commit, push it to the fork only.
    git(repo.path(), &["checkout", "-b", "feature"]);
    fs::write(repo.path().join("feature.txt"), "feature work\n").unwrap();
    repo.commit("feature: new capability");
    git(repo.path(), &["push", "fork", "feature"]);
    git(repo.path(), &["checkout", "main"]);

    // Sync the feature commits from fork into upstream/main.
    repo.gm()
        .args([
            "sync",
            "--from-remote",
            "fork",
            "--from-branch",
            "feature",
            "--to-remote",
            "upstream",
            "--to-branch",
            "main",
            "--strategy",
            "cherry-pick",
        ])
        .assert()
        .success();

    // upstream/main should now contain the feature commit.
    let log = git_out(repo.path(), &["log", "--oneline", "refs/remotes/upstream/main"]);
    assert!(
        log.contains("feature: new capability"),
        "upstream/main missing synced commit:\n{}",
        log
    );

    // And it must actually be pushed (the bare upstream has it).
    let bare_log = git_out(upstream.path(), &["log", "--oneline", "main"]);
    assert!(
        bare_log.contains("feature: new capability"),
        "bare upstream missing commit:\n{}",
        bare_log
    );
}

// ---------------------------------------------------------------------------
// stash / tag / reflog / completions
// ---------------------------------------------------------------------------

#[test]
fn stash_list() {
    let repo = Repo::new();
    fs::write(repo.path().join("file.txt"), "dirty\n").unwrap();
    repo.gm()
        .args(["stash", "save", "--message", "wip"])
        .assert()
        .success();
    repo.gm()
        .args(["stash", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("wip"));
    repo.gm().args(["stash", "pop"]).assert().success();
}

#[test]
fn tag_lifecycle() {
    let repo = Repo::new();
    repo.gm()
        .args(["tag", "create", "v1.0.0", "HEAD", "--message", "release"])
        .assert()
        .success();
    repo.gm()
        .args(["tag", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("v1.0.0"));
    repo.gm().args(["tag", "delete", "v1.0.0"]).assert().success();
    repo.gm()
        .args(["tag", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("v1.0.0").not());
}

#[test]
fn reflog_works() {
    let repo = Repo::new();
    repo.change_and_commit("second");
    repo.gm()
        .args(["reflog", "--count", "10"])
        .assert()
        .success()
        .stdout(predicate::str::contains("second"));
}

#[test]
fn completions_generated() {
    let repo = Repo::new();
    repo.gm()
        .args(["completions", "bash"])
        .assert()
        .success()
        .stdout(predicate::str::contains("git-multi"));
}

// Keep Stdio referenced so the import is not flagged if future tests change.
#[allow(dead_code)]
fn _unused() -> Stdio {
    Stdio::null()
}
