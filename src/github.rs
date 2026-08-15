//! GitHub integration via the `gh` CLI (contributors, profiles, PRs).
//!
//! Every call runs through [`crate::git::run_captured`] so nothing can hang the
//! UI, and all JSON is parsed with `serde_json` (no jq dependency).

use crate::git::{repo_slug_from_url, GitRepo};
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

pub type GhResult<T> = Result<T, String>;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Contributor {
    pub login: String,
    pub name: String,
    pub contributions: u32,
    pub avatar_url: String,
}

#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct UserProfile {
    pub login: String,
    pub name: String,
    pub bio: String,
    pub location: String,
    pub followers: u32,
    pub following: u32,
    pub public_repos: u32,
    pub html_url: String,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct PrSummary {
    pub number: u32,
    pub title: String,
    pub author: String,
    pub state: String,
    pub is_draft: bool,
    pub created_at: String,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct PrCommit {
    pub oid: String,
    pub message: String,
    pub author: String,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct PrDetails {
    pub number: u32,
    pub title: String,
    pub body: String,
    pub state: String,
    pub is_draft: bool,
    pub author: String,
    pub created_at: String,
    pub updated_at: String,
    pub base: String,
    pub head: String,
    pub milestone: Option<String>,
    pub labels: Vec<String>,
    pub assignees: Vec<String>,
    pub reviewers: Vec<(String, String)>, // (login, review state)
    pub commits: Vec<PrCommit>,
    pub mergeable: bool,
    pub merge_state: String,
    /// Conventional-commit scope parsed from the title, e.g. `feat(scope): ...`.
    pub scope: Option<String>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct PrFile {
    pub path: String,
    pub status: String,
    pub additions: u32,
    pub deletions: u32,
}

/// Resolve `owner/repo` from the default (or any) remote of the repo.
pub fn resolve_slug(repo: &GitRepo) -> GhResult<String> {
    let default = repo.config.get_default_remote().cloned();
    let names = repo.list_remotes().map_err(|e| e.to_string())?;
    let mut tried = Vec::new();
    for name in default.iter().chain(names.iter()) {
        if tried.contains(name) {
            continue;
        }
        tried.push(name.clone());
        if let Ok(remote) = repo.repo.find_remote(name) {
            if let Some(url) = remote.url() {
                if let Some(slug) = repo_slug_from_url(url) {
                    return Ok(slug);
                }
            }
        }
    }
    Err("Cannot determine owner/repo from any configured remote URL".to_string())
}

fn gh(workdir: &Path, args: &[&str]) -> GhResult<Value> {
    let out = crate::git::run_captured("gh", args, workdir, &[], Duration::from_secs(60))
        .map_err(|e| e.to_string())?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() {
        let msg = stderr.trim().to_string();
        return Err(if msg.is_empty() {
            "gh command failed".to_string()
        } else {
            msg
        });
    }
    serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("Failed to parse gh output: {}", e))
}

pub fn gh_available() -> bool {
    crate::git::run_captured("gh", &["--version"], Path::new("."), &[], Duration::from_secs(10))
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn get_str(v: &Value, key: &str) -> String {
    v.get(key).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

fn get_u32(v: &Value, key: &str) -> u32 {
    v.get(key).and_then(|x| x.as_u64()).unwrap_or(0) as u32
}

fn get_bool(v: &Value, key: &str) -> bool {
    v.get(key).and_then(|x| x.as_bool()).unwrap_or(false)
}

fn login_of(v: &Value) -> String {
    v.get("login").and_then(|x| x.as_str()).unwrap_or("").to_string()
}

// ---------------------------------------------------------------------------
// Contributors
// ---------------------------------------------------------------------------

pub fn list_contributors(repo: &GitRepo) -> GhResult<Vec<Contributor>> {
    let slug = resolve_slug(repo)?;
    let workdir = repo.workdir_public();
    let out = crate::git::run_captured(
        "gh",
        &["api", &format!("repos/{}/contributors", slug), "--paginate"],
        workdir,
        &[],
        Duration::from_secs(120),
    )
    .map_err(|e| e.to_string())?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(if stderr.trim().is_empty() {
            "gh api failed".to_string()
        } else {
            stderr.trim().to_string()
        });
    }
    let arr: Vec<Value> = serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("Failed to parse contributors: {}", e))?;
    let mut list = Vec::new();
    for c in arr {
        let login = get_str(&c, "login");
        if login.is_empty() {
            continue;
        }
        list.push(Contributor {
            login,
            name: get_str(&c, "name"),
            contributions: get_u32(&c, "contributions"),
            avatar_url: get_str(&c, "avatar_url"),
        });
    }
    Ok(list)
}

/// Fallback contributor list from `git shortlog` when gh is unavailable.
pub fn contributors_from_shortlog(repo: &GitRepo) -> Vec<Contributor> {
    repo.shortlog(true)
        .unwrap_or_default()
        .into_iter()
        .map(|(count, name, _email)| Contributor {
            login: name.clone(),
            name,
            contributions: count as u32,
            avatar_url: String::new(),
        })
        .collect()
}

pub fn user_profile(repo: &GitRepo, login: &str) -> GhResult<UserProfile> {
    let workdir = repo.workdir_public();
    let v = gh(workdir, &["api", &format!("users/{}", login)])?;
    Ok(UserProfile {
        login: get_str(&v, "login"),
        name: get_str(&v, "name"),
        bio: get_str(&v, "bio"),
        location: get_str(&v, "location"),
        followers: get_u32(&v, "followers"),
        following: get_u32(&v, "following"),
        public_repos: get_u32(&v, "public_repos"),
        html_url: get_str(&v, "html_url"),
    })
}

// ---------------------------------------------------------------------------
// Pull requests
// ---------------------------------------------------------------------------

pub fn list_prs(repo: &GitRepo, state: &str) -> GhResult<Vec<PrSummary>> {
    let slug = resolve_slug(repo)?;
    let workdir = repo.workdir_public();
    let v = gh(
        workdir,
        &[
            "pr",
            "list",
            "--repo",
            &slug,
            "--state",
            state,
            "--json",
            "number,title,author,state,isDraft,createdAt",
        ],
    )?;
    let arr = v.as_array().cloned().unwrap_or_default();
    Ok(arr
        .into_iter()
        .map(|p| PrSummary {
            number: get_u32(&p, "number"),
            title: get_str(&p, "title"),
            author: p.get("author").and_then(|a| a.get("login")).and_then(|l| l.as_str()).unwrap_or("").to_string(),
            state: get_str(&p, "state"),
            is_draft: get_bool(&p, "isDraft"),
            created_at: get_str(&p, "createdAt"),
        })
        .collect())
}

pub fn pr_detail(repo: &GitRepo, number: u32) -> GhResult<PrDetails> {
    let slug = resolve_slug(repo)?;
    let workdir = repo.workdir_public();
    let num = number.to_string();
    let v = gh(
        workdir,
        &[
            "pr",
            "view",
            &num,
            "--repo",
            &slug,
            "--json",
            "number,title,body,state,isDraft,author,createdAt,updatedAt,baseRefName,headRefName,milestone,labels,assignees,reviews,reviewRequests,commits,mergeable,mergeStateStatus",
        ],
    )?;

    let title = get_str(&v, "title");
    let scope = parse_title_scope(&title);

    let milestone = v.get("milestone").and_then(|m| m.get("title")).and_then(|t| t.as_str()).map(|s| s.to_string());
    let labels = v.get("labels").and_then(|l| l.as_array()).cloned().unwrap_or_default()
        .into_iter().map(|l| get_str(&l, "name")).filter(|s| !s.is_empty()).collect();
    let assignees = v.get("assignees").and_then(|a| a.as_array()).cloned().unwrap_or_default()
        .into_iter().map(|a| login_of(&a)).filter(|s| !s.is_empty()).collect();

    // Reviewers: requested reviewers + submitted reviews (state: APPROVED / CHANGES_REQUESTED / COMMENTED / DISMISSED).
    let mut reviewers: Vec<(String, String)> = v.get("reviewRequests").and_then(|r| r.as_array()).cloned().unwrap_or_default()
        .into_iter().map(|r| (login_of(&r), "requested".to_string())).collect();
    if let Some(reviews) = v.get("reviews").and_then(|r| r.as_array()) {
        for rv in reviews {
            let login = rv.get("author").and_then(|a| a.get("login")).and_then(|l| l.as_str()).unwrap_or("").to_string();
            if login.is_empty() {
                continue;
            }
            let state = get_str(rv, "state");
            let summary = match state.as_str() {
                "APPROVED" => "approved",
                "CHANGES_REQUESTED" => "changes",
                "COMMENTED" => "commented",
                _ => "reviewed",
            };
            reviewers.retain(|(l, _)| l != &login);
            reviewers.push((login, summary.to_string()));
        }
    }

    let commits = v.get("commits").and_then(|c| c.as_array()).cloned().unwrap_or_default()
        .into_iter().map(|c| PrCommit {
            oid: get_str(&c, "oid"),
            message: get_str(&c, "messageHeadline"),
            author: c.get("authors").and_then(|a| a.as_array()).and_then(|a| a.first())
                .and_then(|f| f.get("login")).and_then(|l| l.as_str()).unwrap_or("").to_string(),
        }).collect();

    Ok(PrDetails {
        number,
        title,
        body: get_str(&v, "body"),
        state: get_str(&v, "state"),
        is_draft: get_bool(&v, "isDraft"),
        author: v.get("author").and_then(|a| a.get("login")).and_then(|l| l.as_str()).unwrap_or("").to_string(),
        created_at: get_str(&v, "createdAt"),
        updated_at: get_str(&v, "updatedAt"),
        base: get_str(&v, "baseRefName"),
        head: get_str(&v, "headRefName"),
        milestone,
        labels,
        assignees,
        reviewers,
        commits,
        mergeable: get_str(&v, "mergeable") == "MERGEABLE",
        merge_state: get_str(&v, "mergeStateStatus"),
        scope,
    })
}

pub fn pr_files(repo: &GitRepo, number: u32) -> GhResult<Vec<PrFile>> {
    let slug = resolve_slug(repo)?;
    let workdir = repo.workdir_public();
    let num = number.to_string();
    let v = gh(workdir, &["pr", "view", &num, "--repo", &slug, "--json", "files"])?;
    let arr = v.get("files").and_then(|f| f.as_array()).cloned().unwrap_or_default();
    Ok(arr
        .into_iter()
        .map(|f| PrFile {
            path: get_str(&f, "path"),
            status: get_str(&f, "status"),
            additions: get_u32(&f, "additions"),
            deletions: get_u32(&f, "deletions"),
        })
        .collect())
}

pub fn pr_diff(repo: &GitRepo, number: u32) -> GhResult<String> {
    let slug = resolve_slug(repo)?;
    let workdir = repo.workdir_public();
    let num = number.to_string();
    let out = crate::git::run_captured("gh", &["pr", "diff", &num, "--repo", &slug], workdir, &[], Duration::from_secs(120))
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(if stderr.trim().is_empty() { "gh pr diff failed".to_string() } else { stderr.trim().to_string() });
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn run_pr_action(repo: &GitRepo, args: &[&str]) -> GhResult<String> {
    let workdir = repo.workdir_public();
    let out = crate::git::run_captured("gh", args, workdir, &[], Duration::from_secs(120))
        .map_err(|e| e.to_string())?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() {
        return Err(if stderr.trim().is_empty() {
            "gh command failed".to_string()
        } else {
            stderr.trim().to_string()
        });
    }
    Ok(stdout.trim().to_string())
}

pub fn merge_pr(repo: &GitRepo, number: u32, strategy: &str, delete_branch: bool) -> GhResult<String> {
    let method = match strategy {
        "rebase" => "--rebase",
        "merge" => "--merge",
        _ => "--squash",
    };
    let mut args = vec!["pr".to_string(), "merge".to_string(), number.to_string(), method.to_string()];
    if delete_branch {
        args.push("--delete-branch".to_string());
    }
    let cargs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    run_pr_action(repo, &cargs)
}

pub fn close_pr(repo: &GitRepo, number: u32, comment: Option<&str>) -> GhResult<String> {
    let mut args = vec!["pr".to_string(), "close".to_string(), number.to_string()];
    if let Some(c) = comment {
        args.push("--comment".to_string());
        args.push(c.to_string());
    }
    let cargs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    run_pr_action(repo, &cargs)
}

pub fn reopen_pr(repo: &GitRepo, number: u32) -> GhResult<String> {
    run_pr_action(repo, &["pr", "reopen", &number.to_string()])
}

pub fn comment_pr(repo: &GitRepo, number: u32, body: &str) -> GhResult<String> {
    run_pr_action(repo, &["pr", "comment", &number.to_string(), "--body", body])
}

pub fn review_pr(repo: &GitRepo, number: u32, verdict: &str, body: Option<&str>) -> GhResult<String> {
    let num = number.to_string();
    let flag = match verdict {
        "approve" => "--approve",
        "changes" => "--request-changes",
        _ => "--comment",
    };
    let mut args = vec!["pr".to_string(), "review".to_string(), num.clone(), flag.to_string()];
    if let Some(b) = body {
        args.push("--body".to_string());
        args.push(b.to_string());
    }
    let _ = num;
    let cargs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    run_pr_action(repo, &cargs)
}

pub fn checkout_pr(repo: &GitRepo, number: u32) -> GhResult<String> {
    run_pr_action(repo, &["pr", "checkout", &number.to_string()])
}

pub fn edit_pr(repo: &GitRepo, number: u32, title: Option<&str>, body: Option<&str>) -> GhResult<String> {
    let num = number.to_string();
    let mut args = vec!["pr".to_string(), "edit".to_string(), num.clone()];
    if let Some(t) = title {
        args.push("--title".to_string());
        args.push(t.to_string());
    }
    if let Some(b) = body {
        args.push("--body".to_string());
        args.push(b.to_string());
    }
    let _ = num;
    let cargs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    run_pr_action(repo, &cargs)
}

pub fn edit_pr_list(repo: &GitRepo, number: u32, field: &str, flag: &str, values: &[String]) -> GhResult<String> {
    let num = number.to_string();
    let joined = values.join(",");
    let cargs = [
        "pr".to_string(),
        "edit".to_string(),
        num,
        flag.to_string(),
        joined,
    ];
    let _ = field;
    let cargs: Vec<&str> = cargs.iter().map(|s| s.as_str()).collect();
    run_pr_action(repo, &cargs)
}

pub fn open_pr_web(repo: &GitRepo, number: u32) -> GhResult<String> {
    run_pr_action(repo, &["pr", "view", &number.to_string(), "--web"])
}

/// Parse a conventional-commit scope from a title like `feat(scope): subject`.
fn parse_title_scope(title: &str) -> Option<String> {
    let rest = title.split_once('(')?.1;
    let scope = rest.split(')').next()?.trim();
    if scope.is_empty() {
        None
    } else {
        Some(scope.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_scope_parsing() {
        assert_eq!(parse_title_scope("feat(auth): add login"), Some("auth".to_string()));
        assert_eq!(parse_title_scope("fix: resolve bug"), None);
        assert_eq!(parse_title_scope("no parens here"), None);
        assert_eq!(parse_title_scope("chore(): empty"), None);
    }
}
