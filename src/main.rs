mod cli;
mod config;
mod error;
mod git;
mod tui;
mod update;

use cli::*;
use error::*;
use git::*;

use clap::Parser;
use clap::CommandFactory;
use clap_complete::{generate, Shell};
use console::style;
use dialoguer::Confirm;
use std::io;
use std::process;
use tracing::info;fn main() {
    let cli = Cli::parse();
    
    if cli.gui {
        if let Err(e) = tui::run_tui() {
            eprintln!("TUI error: {}", e);
            std::process::exit(1);
        }
        return;
    }
    
    // Initialize tracing
    let filter = match cli.verbose {
        0 => "git_multi=info",
        1 => "git_multi=debug",
        _ => "git_multi=trace",
    };
    
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .init();

    if let Err(e) = run(&cli) {
        eprintln!("{}", style("Error: ").red().bold());
        eprintln!("{}", style(e.to_string()).red());
        process::exit(1);
    }
}

fn run(cli: &Cli) -> Result<()> {
    if let Some(command) = &cli.command {
        match command {
            Commands::Init => cmd_init(),
            Commands::Remote { command } => cmd_remote(command, cli.json),
            Commands::Branch { command } => cmd_branch(command, cli.json),
            Commands::Fetch { all, remote, branches, all_branches } => {
                cmd_fetch(*all, remote.clone(), branches.clone(), *all_branches)
            }
            Commands::Pull { all, remote, branches, all_branches } => {
                cmd_pull(*all, remote.clone(), branches.clone(), *all_branches)
            }
            Commands::Push { all, remote, branches, all_branches, force } => {
                cmd_push(*all, remote.clone(), branches.clone(), *all_branches, *force)
            }
            Commands::Checkout { branch, remote, new } => cmd_checkout(branch.clone(), remote.clone(), *new),
            Commands::Sync { from_remote, to_remote, from_branch, to_branch, commits, strategy, force } => {
                cmd_sync(from_remote.clone(), to_remote.clone(), from_branch.clone(), to_branch.clone(), 
                        commits.clone(), *strategy, *force)
            }
            Commands::Merge { from_remote, from_branch, to_branch, to_remote, push } => {
                cmd_merge(from_remote.clone(), from_branch.clone(), to_branch.clone(), to_remote.clone(), *push)
            }
            Commands::Copy { from, to, files, prune } => cmd_copy(from.clone(), to.clone(), files.clone(), *prune),
            Commands::Pr { remote, base, head, title, description, open } => {
                cmd_pr(remote.clone(), base.clone(), head.clone(), title.clone(), description.clone(), *open)
            }
            Commands::Use { remote } => cmd_use(remote.clone()),
            Commands::Status => cmd_status(cli.json),
            Commands::List => cmd_list(cli.json),
            Commands::Commit { subject, body, amend } => cmd_commit(subject.clone(), body.clone(), *amend),
            Commands::Diff { what, path } => cmd_diff(what.clone(), path.clone()),
            Commands::Blame { path, commit } => cmd_blame(path.clone(), commit.clone()),
            Commands::Log { path, count } => cmd_log(path.clone(), *count),
            Commands::Graph { all, limit } => cmd_graph(*all, *limit, cli.json),
            Commands::Revert { commit } => cmd_revert(commit.clone()),
            Commands::Reset { mode, target } => cmd_reset(mode.clone(), target.clone()),
            Commands::Pick { commit } => cmd_pick(commit.clone()),
            Commands::Stage { path } => {
                GitRepo::open()?.stage_file(path.as_str())?;
                println!("{}", style(format!("Staged: {}", path)).green());
                Ok(())
            }
            Commands::Unstage { path } => {
                GitRepo::open()?.unstage_file(path.as_str())?;
                println!("{}", style(format!("Unstaged: {}", path)).green());
                Ok(())
            }
            Commands::Restore { path } => {
                GitRepo::open()?.restore_file(path.as_str())?;
                println!("{}", style(format!("Restored: {}", path)).green());
                Ok(())
            }
            Commands::SelfUpdate => {
                update::self_update()
                    .map_err(|e| crate::error::GitMultiError::SyncError(e.to_string()))?;
                Ok(())
            }
            Commands::Stash { command } => cmd_stash(command),
            Commands::Tag { command } => cmd_tag(command),
            Commands::Reflog { count } => cmd_reflog(*count),
            Commands::Completions { shell } => cmd_completions(*shell),
        }
    } else {
        println!("No command specified. Use --help for usage.");
        Ok(())
    }
}

// ========== JSON helpers ==========

fn print_json(value: &serde_json::Value) -> Result<()> {
    let s = serde_json::to_string_pretty(value)
        .map_err(|e| GitMultiError::SyncError(e.to_string()))?;
    println!("{}", s);
    Ok(())
}

// ========== INIT ==========

fn cmd_init() -> Result<()> {
    info!("Initializing git-multi configuration");
    let _repo = GitRepo::init()?;
    info!("Git-multi initialized successfully!");
    println!("{}", style("Git-multi initialized successfully!").green());
    Ok(())
}

// ========== REMOTE ==========

fn cmd_remote(command: &RemoteCommands, json: bool) -> Result<()> {
    match command {
        RemoteCommands::Add { name, url, default } => {
            let mut repo = GitRepo::open()?;
            repo.add_remote(name, url)?;
            
            if *default {
                repo.config.set_default_remote(name.clone())?;
                repo.config.save(&repo.repo)?;
            }
            
            println!("Added remote '{}' with URL: {}", style(name).green(), url);
            if *default {
                println!("Set as default remote");
            }
            Ok(())
        }
        RemoteCommands::Remove { name, force } => {
            let mut repo = GitRepo::open()?;
            
            if !*force
                && !confirm(&format!("Remove remote '{}'? This cannot be undone.", name))?
            {
                return Ok(());
            }
            
            repo.remove_remote(name)?;
            println!("Removed remote '{}'", style(name).green());
            Ok(())
        }
        RemoteCommands::List { urls } => {
            let repo = GitRepo::open()?;
            let remotes = if *urls {
                repo.list_remotes_with_urls()?
            } else {
                let names = repo.list_remotes()?;
                names.into_iter().map(|n| (n, "".to_string())).collect()
            };

            if json {
                let default = repo.config.get_default_remote().cloned();
                let items: Vec<serde_json::Value> = remotes
                    .iter()
                    .map(|(name, url)| {
                        serde_json::json!({
                            "name": name,
                            "url": url,
                            "is_default": default.as_deref() == Some(name.as_str()),
                        })
                    })
                    .collect();
                return print_json(&serde_json::json!({ "remotes": items }));
            }

            println!("Remotes:");
            for (name, url) in remotes {
                let default_marker = if repo.config.get_default_remote() == Some(&name) {
                    " *"
                } else {
                    ""
                };
                if *urls {
                    println!("  {}{}: {}", style(&name).cyan(), default_marker, url);
                } else {
                    println!("  {}{}", style(&name).cyan(), default_marker);
                }
            }
            Ok(())
        }
        RemoteCommands::Rename { old_name, new_name } => {
            let mut repo = GitRepo::open()?;
            
            // Rename in git config
            let remote = repo.repo.find_remote(old_name)?;
            let url = remote.url().unwrap().to_string();
            repo.repo.remote_delete(old_name)?;
            repo.repo.remote(new_name, &url)?;
            
            // Update in git-multi config
            if let Some(remote_config) = repo.config.remotes.remove(old_name) {
                repo.config.remotes.insert(new_name.clone(), remote_config);
            }
            
            // Update default remote if needed
            if repo.config.get_default_remote() == Some(old_name) {
                repo.config.set_default_remote(new_name.clone())?;
            }
            
            repo.config.save(&repo.repo)?;
            println!("Renamed remote '{}' to '{}'", old_name, style(new_name).green());
            Ok(())
        }
        RemoteCommands::SetDefault { name } => {
            let mut repo = GitRepo::open()?;
            repo.config.set_default_remote(name.clone())?;
            repo.config.save(&repo.repo)?;
            println!("Default remote set to '{}'", style(name).green());
            Ok(())
        }
        RemoteCommands::SetPrimary { name } => {
            let mut repo = GitRepo::open()?;
            repo.config.set_primary_remote(name)?;
            repo.config.save(&repo.repo)?;
            println!("Primary remote set to '{}'", style(name).green());
            Ok(())
        }
        RemoteCommands::Show { name } => {
            let repo = GitRepo::open()?;
            let remote = repo.repo.find_remote(name)?;

            if json {
                return print_json(&serde_json::json!({
                    "name": name,
                    "url": remote.url().unwrap_or("unknown"),
                    "push_url": remote.pushurl().unwrap_or(""),
                }));
            }

            println!("Remote: {}", style(name).cyan().bold());
            println!("URL: {}", remote.url().unwrap_or("unknown"));

            if let Some(push_url) = remote.pushurl() {
                println!("Push URL: {}", push_url);
            }

            // Show config details
            if let Ok(config) = repo.config.get_remote(name) {
                println!("Is Primary: {}", config.is_primary);
            }
            if let Some((name, _)) = repo.config.get_primary_remote() {
                println!("Primary Remote: {}", name);
            }

            let branches = repo.list_remote_branches(name)?;
            println!("\nBranches:");
            for branch in branches {
                println!("  {}", branch);
            }
            Ok(())
        }
        RemoteCommands::ListNames {} => {
            let repo = GitRepo::open()?;
            if json {
                let names: Vec<String> = repo
                    .config
                    .get_remote_names()
                    .into_iter()
                    .map(|s| s.to_string())
                    .collect();
                return print_json(&serde_json::json!({ "remotes": names }));
            }
            for name in repo.config.get_remote_names() {
                println!("{}", name);
            }
            Ok(())
        }
    }
}

// ========== BRANCH ==========

fn cmd_branch(command: &BranchCommands, json: bool) -> Result<()> {
    match command {
        BranchCommands::List { all, remote } => {
            let repo = GitRepo::open()?;
            
            if let Some(remote_name) = remote {
                let branches = repo.list_remote_branches(remote_name)?;
                if json {
                    return print_json(&serde_json::json!({ "remote": remote_name, "branches": branches }));
                }
                println!("Branches on remote '{}':", style(remote_name).cyan());
                for branch in branches {
                    println!("  {}", style(&branch).green());
                }
            } else if *all {
                let info = repo.list_all_branches()?;
                if json {
                    let mut remote_map = serde_json::Map::new();
                    for (rname, brs) in &info.remote {
                        remote_map.insert(
                            rname.clone(),
                            serde_json::json!(brs.iter().map(|b| &b.name).collect::<Vec<_>>()),
                        );
                    }
                    return print_json(&serde_json::json!({
                        "local": info.local.iter().map(|b| &b.name).collect::<Vec<_>>(),
                        "remote": remote_map,
                    }));
                }
                println!("Local branches:");
                for branch in &info.local {
                    println!("  {}", style(branch.to_string()).green());
                }
                
                println!("\nRemote branches:");
                for (remote_name, branches) in &info.remote {
                    println!("  {}:", style(remote_name).cyan());
                    for branch in branches {
                        println!("    {}", style(&branch.name).green());
                    }
                }
            } else {
                let info = repo.list_all_branches()?;
                if json {
                    return print_json(&serde_json::json!({
                        "local": info.local.iter().map(|b| &b.name).collect::<Vec<_>>(),
                    }));
                }
                println!("Local branches:");
                for branch in &info.local {
                    println!("  {}", style(branch.to_string()).green());
                }
            }
            Ok(())
        }
        BranchCommands::Delete { branch, force, remote } => {
            let repo = GitRepo::open()?;
            
            if !*force
                && !confirm(&format!("Delete branch '{}'? This cannot be undone.", branch))?
            {
                return Ok(());
            }
            
            if *remote {
                // Delete remote branch
                let remote_names = repo.repo.remotes()?;
                for remote_name in remote_names.iter().flatten() {
                    let refspec = format!(":refs/heads/{}", branch);
                    let mut remote = repo.repo.find_remote(remote_name)?;
                    remote.push(&[&refspec], None)?;
                    println!("Deleted branch '{}' from remote '{}'", branch, remote_name);
                }
            } else {
                // Delete local branch
                let mut local_branch = repo.repo.find_branch(branch, git2::BranchType::Local)?;
                local_branch.delete()?;
                println!("Deleted local branch '{}'", style(branch).green());
            }
            Ok(())
        }
        BranchCommands::Create { branch, base, remotes, checkout } => {
            let repo = GitRepo::open()?;
            
            let base_oid = repo.resolve_commit_spec(base)?;
            let base_commit = repo.repo.find_commit(base_oid)?;
            
            // Create local branch
            repo.repo.branch(branch, &base_commit, false)?;
            println!("Created local branch '{}' from '{}'", style(branch).green(), base);
            
            // Create on remotes
            if let Some(remote_names) = remotes {
                for remote_name in remote_names {
                    let mut remote = repo.repo.find_remote(remote_name)?;
                    let refspec = format!("refs/heads/{}:refs/heads/{}", branch, branch);
                    remote.push(&[&refspec], None)?;
                    println!("Created branch '{}' on remote '{}'", style(branch).green(), style(&remote_name).cyan());
                }
            }
            
            if *checkout {
                repo.checkout_branch(branch)?;
                println!("Checked out '{}'", branch);
            }
            
            Ok(())
        }
        BranchCommands::Rename { old_name, new_name } => {
            let repo = GitRepo::open()?;
            repo.rename_branch(old_name, new_name)?;
            println!("Renamed branch '{}' to '{}'", style(old_name).green(), style(new_name).green());
            Ok(())
        }
    }
}

// ========== FETCH ==========

fn cmd_fetch(all: bool, remote: Option<String>, branches: Vec<String>, all_branches: bool) -> Result<()> {
    let repo = GitRepo::open()?;
    
    if all {
        let fetched = repo.fetch_all()?;
        println!("Fetched from {} remote(s):", style(fetched.len()).green());
        for name in fetched {
            println!("  {}", style(&name).cyan());
        }
    } else if let Some(remote_name) = remote {
        if !branches.is_empty() {
            repo.fetch_branches(&remote_name, &branches)?;
            println!("Fetched branches {:?} from '{}'", branches, style(&remote_name).green());
        } else if all_branches {
            repo.fetch_remote(&remote_name)?;
            println!("Fetched all branches from '{}'", style(&remote_name).green());
        } else {
            repo.fetch_remote(&remote_name)?;
            println!("Fetched from '{}'", style(&remote_name).green());
        }
    } else {
        // Default: fetch from all remotes
        let fetched = repo.fetch_all()?;
        println!("Fetched from {} remote(s)", style(fetched.len()).green());
    }
    Ok(())
}

// ========== PULL ==========

fn cmd_pull(all: bool, remote: Option<String>, branches: Vec<String>, all_branches: bool) -> Result<()> {
    let repo = GitRepo::open()?;
    
    if all {
        let pulled = repo.pull_from_all(None)?;
        println!("Pulled from {} remote(s):", style(pulled.len()).green());
        for name in pulled {
            println!("  {}", style(&name).cyan());
        }
    } else if let Some(remote_name) = remote {
        if !branches.is_empty() {
            repo.pull_branches(&remote_name, &branches)?;
            println!("Pulled/refreshed branches {:?} from '{}'", branches, style(&remote_name).green());
        } else if all_branches {
            repo.fetch_remote(&remote_name)?;
            let brs = repo.list_remote_branches(&remote_name)?;
            repo.pull_branches(&remote_name, &brs)?;
            println!("Pulled/refreshed all branches from '{}'", style(&remote_name).green());
        } else {
            // Default: pull current branch
            repo.pull_from_remote(&remote_name, None)?;
            println!("Pulled from '{}'", style(&remote_name).green());
        }
    } else {
        // Default: pull from default remote
        if let Some(default_remote) = repo.config.get_default_remote() {
            repo.pull_from_remote(default_remote, None)?;
            println!("Pulled from default remote '{}'", style(default_remote).green());
        } else {
            return Err(GitMultiError::NoRemotesConfigured);
        }
    }
    Ok(())
}

// ========== PUSH ==========

fn cmd_push(all: bool, remote: Option<String>, branches: Vec<String>, all_branches: bool, force: bool) -> Result<()> {
    let repo = GitRepo::open()?;
    
    if all {
        let pushed = repo.push_to_all(None)?;
        println!("Pushed to {} remote(s):", style(pushed.len()).green());
        for name in pushed {
            println!("  {}", style(&name).cyan());
        }
    } else if let Some(remote_name) = remote {
        if !branches.is_empty() {
            repo.push_branches(&remote_name, &branches, force)?;
            println!("Pushed branches {:?} to '{}'", branches, style(&remote_name).green());
            if force {
                println!("  Force: yes");
            }
        } else if all_branches {
            let brs = repo.local_branch_names()?;
            repo.push_branches(&remote_name, &brs, force)?;
            println!("Pushed all local branches to '{}'", style(&remote_name).green());
            if force {
                println!("  Force: yes");
            }
        } else {
            // Default: push current branch
            repo.push_to_remote(&remote_name, None)?;
            println!("Pushed to '{}'", style(&remote_name).green());
        }
    } else {
        // Default: push to default remote
        if let Some(default_remote) = repo.config.get_default_remote() {
            repo.push_to_remote(default_remote, None)?;
            println!("Pushed to default remote '{}'", style(default_remote).green());
        } else {
            return Err(GitMultiError::NoRemotesConfigured);
        }
    }
    Ok(())
}

// ========== CHECKOUT ==========

fn cmd_checkout(branch: String, remote: Option<String>, new: bool) -> Result<()> {
    let repo = GitRepo::open()?;
    
    if let Some(remote_name) = remote {
        repo.checkout_remote_branch(&remote_name, &branch)?;
        println!("Checked out '{}' from remote '{}'", style(&branch).green(), remote_name);
    } else if new {
        // Create new branch
        let head_commit = repo.head_commit()?;
        repo.repo.branch(&branch, &head_commit, false)?;
        repo.checkout_branch(&branch)?;
        println!("Created and checked out new branch '{}'", style(&branch).green());
    } else {
        repo.checkout_branch(&branch)?;
        println!("Checked out '{}'", style(&branch).green());
    }
    Ok(())
}

// ========== SYNC ==========

fn cmd_sync(
    from_remote: String,
    to_remote: String,
    from_branch: String,
    to_branch: String,
    commits: Option<String>,
    strategy: Option<SyncStrategy>,
    force: bool,
) -> Result<()> {
    let repo = GitRepo::open()?;

    let strategy = resolve_strategy(&repo, strategy);

    info!("Syncing from {}/{}", from_remote, from_branch);
    info!("Syncing to   {}/{}", to_remote, to_branch);
    info!("Strategy: {}", strategy);

    let from_ref = format!("refs/remotes/{}/{}", from_remote, from_branch);
    let to_ref = format!("refs/remotes/{}/{}", to_remote, to_branch);

    // Fetch both remotes first so the remote-tracking refs are current.
    repo.fetch_remote(&from_remote)?;
    repo.fetch_remote(&to_remote)?;

    // The destination local branch is based on the destination remote's tip
    // when it exists, otherwise on the source tip (first-time sync).
    let base_ref = if repo.repo.find_reference(&to_ref).is_ok() {
        &to_ref
    } else {
        &from_ref
    };
    repo.ensure_local_branch(&to_branch, base_ref)?;
    if repo.current_branch()?.as_deref() != Some(&to_branch) {
        repo.checkout_branch(&to_branch)?;
    }

    // Resolve the commit range against the source tip.
    let range = match &commits {
        Some(r) if !r.trim().is_empty() => r.clone(),
        _ => {
            let tip = repo.repo.find_reference(&from_ref)?.peel_to_commit()?.id();
            tip.to_string()
        }
    };

    match strategy {
        SyncStrategy::CherryPick => {
            let picked = repo.cherry_pick_range(&range, &to_branch)?;
            println!("Cherry-picked {} commit(s):", style(picked.len()).green());
            for sha in picked {
                println!("  {}", style(&sha[..8.min(sha.len())]).cyan());
            }
        }
        SyncStrategy::Merge => {
            repo.merge_and_commit(&from_ref)?;
            println!("Merged '{}/{}' into '{}'", style(&from_remote).cyan(), style(&from_branch).green(), to_branch);
        }
        SyncStrategy::Rebase => {
            repo.rebase_onto(&to_branch, &from_ref)?;
            println!("Rebased '{}' onto '{}/{}'", style(&to_branch).green(), style(&from_remote).cyan(), from_branch);
        }
    }

    // Push the result to the destination remote.
    if force {
        repo.push_branches(&to_remote, std::slice::from_ref(&to_branch), true)?;
        println!("Force-pushed '{}' to '{}'", style(&to_branch).green(), style(&to_remote).cyan());
    } else {
        repo.push_branches(&to_remote, std::slice::from_ref(&to_branch), false)?;
        println!("Pushed '{}' to '{}'", style(&to_branch).green(), style(&to_remote).cyan());
    }

    Ok(())
}

/// Pick the sync strategy from the CLI flag or the configured default.
fn resolve_strategy(repo: &GitRepo, strategy: Option<SyncStrategy>) -> SyncStrategy {
    if let Some(s) = strategy {
        return s;
    }
    match repo.config.sync_preferences.default_strategy.as_str() {
        "merge" => SyncStrategy::Merge,
        "rebase" => SyncStrategy::Rebase,
        _ => SyncStrategy::CherryPick,
    }
}

// ========== MERGE ==========

fn cmd_merge(
    from_remote: String,
    from_branch: String,
    to_branch: Option<String>,
    to_remote: Option<String>,
    push: bool,
) -> Result<()> {
    let repo = GitRepo::open()?;

    let target_branch = match to_branch {
        Some(b) => b,
        None => repo.current_branch()?
            .ok_or_else(|| GitMultiError::SyncError("Cannot determine current branch to merge into".to_string()))?,
    };

    let src_ref = format!("refs/remotes/{}/{}", from_remote, from_branch);

    info!("Merging {}/{} into {}", from_remote, from_branch, target_branch);

    repo.fetch_remote(&from_remote)?;
    if repo.current_branch()?.as_deref() != Some(&target_branch) {
        repo.checkout_branch(&target_branch)?;
    }
    repo.merge_and_commit(&src_ref)?;
    println!(
        "Merged '{}/{}' into '{}'",
        style(&from_remote).cyan(),
        style(&from_branch).green(),
        style(&target_branch).green()
    );

    if push {
        let target = to_remote.ok_or_else(|| {
            GitMultiError::SyncError("Specify --to-remote to push the merged result".to_string())
        })?;
        repo.push_to_remote(&target, Some(&target_branch))?;
        println!("Pushed '{}' to '{}'", style(&target_branch).green(), style(&target).cyan());
    }

    Ok(())
}

// ========== COPY ==========

fn cmd_copy(from: String, to: Option<String>, files: Vec<String>, prune: bool) -> Result<()> {
    let repo = GitRepo::open()?;
    
    // Parse from and to specifications (format: remote:branch or just branch)
    let (from_remote, from_branch) = parse_ref_spec(&from);
    let to_info = to.as_deref().map(parse_ref_spec);
    
    info!("Copying files from {}/{}", from_remote.as_deref().unwrap_or("local"), from_branch);
    if let Some((ref to_remote, ref to_branch)) = to_info {
        info!("Copying files to   {}/{}", to_remote.as_deref().unwrap_or("local"), to_branch);
    } else {
        info!("Copying files to working directory");
    }
    
    let from_ref = if let Some(ref remote) = from_remote {
        format!("refs/remotes/{}/{}", remote, from_branch)
    } else {
        from_branch
    };
    
    let copied = repo.copy_files(&from_ref, &files, prune)?;
    
    println!("Copied {} file(s):", style(copied.len()).green());
    for file in copied {
        println!("  {}", style(&file).cyan());
    }
    
    if prune {
        println!("{}", style("Pruned stale files not present in the source").yellow());
    }
    
    Ok(())
}

fn parse_ref_spec(spec: &str) -> (Option<String>, String) {
    if spec.contains(':') {
        let parts: Vec<&str> = spec.splitn(2, ':').collect();
        (Some(parts[0].to_string()), parts[1].to_string())
    } else {
        (None, spec.to_string())
    }
}

// ========== PR ==========

fn cmd_pr(
    remote: String,
    base: String,
    head: Option<String>,
    title: String,
    description: Option<String>,
    open: bool,
) -> Result<()> {
    let repo = GitRepo::open()?;
    
    let head_branch = head.unwrap_or_else(|| {
        repo.current_branch().ok().flatten().unwrap_or_else(|| "HEAD".to_string())
    });
    
    info!("Creating PR on {}", remote);
    info!("Base: {}", base);
    info!("Head: {}", head_branch);
    info!("Title: {}", title);
    
    repo.create_pr(&remote, &base, &head_branch, &title, description.as_deref(), open)?;
    
    println!("Pull request created successfully!");
    println!("  Repository: {}", style(&remote).cyan());
    println!("  Base: {} <- Head: {}", style(&base).green(), style(&head_branch).green());
    println!("  Title: {}", style(&title).yellow());
    
    Ok(())
}

// ========== USE ==========

fn cmd_use(remote: String) -> Result<()> {
    let mut repo = GitRepo::open()?;
    
    repo.config.set_default_remote(remote.clone())?;
    repo.config.save(&repo.repo)?;
    
    println!("Default remote set to '{}'", style(&remote).green());
    Ok(())
}

// ========== STATUS ==========

fn cmd_status(json: bool) -> Result<()> {
    let repo = GitRepo::open()?;

    if json {
        let remotes: Vec<serde_json::Value> = repo
            .list_remotes_with_urls()?
            .into_iter()
            .map(|(name, url)| serde_json::json!({ "name": name, "url": url }))
            .collect();
        let local_branches: Vec<String> = repo
            .list_all_branches()?
            .local
            .into_iter()
            .map(|b| b.name)
            .collect();
        let files: Vec<serde_json::Value> = repo
            .working_status()?
            .into_iter()
            .map(|f| {
                serde_json::json!({
                    "path": f.path,
                    "staged": f.staged.to_string(),
                    "unstaged": f.unstaged.to_string(),
                })
            })
            .collect();
        let current_branch = repo.current_branch()?.unwrap_or_default();
        return print_json(&serde_json::json!({
            "current_branch": current_branch,
            "remotes": remotes,
            "local_branches": local_branches,
            "working_tree": files,
        }));
    }

    println!("Git Multi-Remote Status");
    println!("{}", "=".repeat(40));
    
    // Current branch
    if let Some(branch) = repo.current_branch()? {
        println!("Current branch: {}", style(&branch).green().bold());
    }
    
    // Remotes
    println!("\nRemotes:");
    let remotes = repo.list_remotes_with_urls()?;
    for (name, url) in remotes {
        let default_marker = if repo.config.get_default_remote() == Some(&name) {
            " [default]"
        } else {
            ""
        };
        println!("  {}{}: {}", style(&name).cyan(), default_marker, url);
    }
    
    // Branches
    println!("\nLocal branches:");
    let info = repo.list_all_branches()?;
    for branch in &info.local {
        println!("  {}", style(branch.name.clone()).green());
    }
    
    Ok(())
}

// ========== LIST ==========

fn cmd_list(json: bool) -> Result<()> {
    let repo = GitRepo::open()?;
    
    let remotes = repo.list_remotes()?;

    if json {
        let mut items = Vec::new();
        for remote_name in remotes {
            let branches = repo.list_remote_branches(&remote_name)?;
            items.push(serde_json::json!({ "remote": remote_name, "branches": branches }));
        }
        return print_json(&serde_json::json!({ "remotes": items }));
    }
    
    for remote_name in remotes {
        println!("\nRemote: {}", style(&remote_name).cyan().bold());
        let branches = repo.list_remote_branches(&remote_name)?;
        for branch in branches {
            println!("  {}", style(&branch).green());
        }
    }
    
    Ok(())
}

// ========== COMMIT / AMEND ==========

fn cmd_commit(subject: String, body: Option<String>, amend: bool) -> Result<()> {
    let repo = GitRepo::open()?;
    if amend {
        repo.amend_commit(&subject, body.as_deref())?;
        println!("{}", style("Amended last commit").green());
    } else {
        repo.create_commit(&subject, body.as_deref())?;
        println!("{}", style(format!("Created commit: {}", subject)).green());
    }
    Ok(())
}

// ========== DIFF ==========

fn cmd_diff(what: String, path: Option<String>) -> Result<()> {
    let repo = GitRepo::open()?;
    let mode = match what.to_lowercase().as_str() {
        "staged" | "cached" => crate::git::DiffMode::Staged,
        "head" | "committed" => crate::git::DiffMode::Head,
        _ => crate::git::DiffMode::Unstaged,
    };
    let diff = repo.diff(mode, path.as_deref())?;
    if diff.trim().is_empty() {
        println!("(no diff)");
    } else {
        print!("{}", diff);
    }
    Ok(())
}

// ========== BLAME ==========

fn cmd_blame(path: String, commit: Option<String>) -> Result<()> {
    let repo = GitRepo::open()?;
    let blame = repo.blame_file(&path, commit.as_deref())?;
    for b in blame {
        println!(
            "{:>6}  {:<18}  {:.8}  {}",
            b.line, b.author, b.commit, b.summary
        );
    }
    Ok(())
}

// ========== LOG ==========

fn cmd_log(path: Option<String>, count: usize) -> Result<()> {
    let repo = GitRepo::open()?;
    match path {
        Some(p) => {
            for c in repo.file_history(&p)? {
                println!("{}  {}  {}  {}", c.short_id, c.author, crate::git::format_timestamp(c.author_date), c.message);
            }
        }
        None => {
            for line in repo.list_recent_commits(count)? {
                println!("{}", line);
            }
        }
    }
    Ok(())
}

// ========== GRAPH ==========

fn cmd_graph(all: bool, limit: usize, json: bool) -> Result<()> {
    let repo = GitRepo::open()?;

    if json {
        let graph = repo.commit_graph(all, limit)?;
        let nodes: Vec<serde_json::Value> = graph
            .nodes
            .into_iter()
            .map(|n| {
                serde_json::json!({
                    "id": n.id,
                    "short_id": n.short_id,
                    "message": n.message,
                    "author": n.author,
                    "date": n.date,
                    "refs": n.refs.iter().map(|r| r.name.clone()).collect::<Vec<_>>(),
                })
            })
            .collect();
        return print_json(&serde_json::json!({ "nodes": nodes }));
    }

    let graph = repo.commit_graph(all, limit)?;
    for n in &graph.nodes {
        let refs: Vec<String> = n.refs.iter().map(|r| r.name.clone()).collect();
        let refstr = if refs.is_empty() { String::new() } else { format!("  ({})", refs.join(", ")) };
        println!("* {:.8} {} {}  {}{}", n.id, n.author, crate::git::format_timestamp(n.date), n.message, refstr);
    }
    for r in &graph.detached_refs {
        println!("  ref {} ({:?})", r.name, r.kind);
    }
    Ok(())
}

// ========== REVERT ==========

fn cmd_revert(commit: String) -> Result<()> {
    let repo = GitRepo::open()?;
    repo.revert_commit(&commit)?;
    println!("{}", style(format!("Reverted {}", commit)).green());
    Ok(())
}

// ========== RESET ==========

fn cmd_reset(mode: String, target: String) -> Result<()> {
    let repo = GitRepo::open()?;
    let m = match mode.to_lowercase().as_str() {
        "soft" => crate::git::ResetMode::Soft,
        "hard" => crate::git::ResetMode::Hard,
        _ => crate::git::ResetMode::Mixed,
    };
    repo.reset(m, &target)?;
    println!("{}", style(format!("Reset ({}) to {}", mode, target)).green());
    Ok(())
}

// ========== PICK (cherry-pick) ==========

fn cmd_pick(commit: String) -> Result<()> {
    let repo = GitRepo::open()?;
    repo.cherry_pick_commit(&commit)?;
    println!("{}", style(format!("Cherry-picked {}", commit)).green());
    Ok(())
}

// ========== STASH / TAG / REFLOG / COMPLETIONS ==========

fn cmd_stash(command: &StashCommands) -> Result<()> {
    let repo = GitRepo::open()?;
    match command {
        StashCommands::Save { message } => {
            repo.stash_save(message.as_deref())?;
            println!("{}", style("Working tree stashed").green());
        }
        StashCommands::Pop => {
            repo.stash_pop()?;
            println!("{}", style("Stash popped").green());
        }
        StashCommands::List => {
            let stashes = repo.stash_list()?;
            if stashes.is_empty() {
                println!("(no stashes)");
            } else {
                for s in stashes {
                    println!("  {}", style(&s).yellow());
                }
            }
        }
    }
    Ok(())
}

fn cmd_tag(command: &TagCommands) -> Result<()> {
    let repo = GitRepo::open()?;
    match command {
        TagCommands::List => {
            let tags = repo.list_tags()?;
            if tags.is_empty() {
                println!("(no tags)");
            } else {
                for t in tags {
                    println!("  {}", style(&t).green());
                }
            }
        }
        TagCommands::Create { name, target, message } => {
            let target = target.clone().unwrap_or_else(|| "HEAD".to_string());
            repo.create_tag(name, &target, message.as_deref())?;
            println!("{}", style(format!("Created tag '{}' at {}", name, target)).green());
        }
        TagCommands::Delete { name } => {
            repo.delete_tag(name)?;
            println!("{}", style(format!("Deleted tag '{}'", name)).green());
        }
    }
    Ok(())
}

fn cmd_reflog(count: usize) -> Result<()> {
    let repo = GitRepo::open()?;
    for line in repo.reflog(count)? {
        println!("{}", line);
    }
    Ok(())
}

fn cmd_completions(shell: Shell) -> Result<()> {
    let mut cmd = Cli::command();
    generate(shell, &mut cmd, "git-multi", &mut io::stdout());
    Ok(())
}

// ========== CONFIRMATION ==========

/// Prompt for confirmation, but fail cleanly in non-interactive contexts
/// instead of hanging on a TTY read. Callers should offer `--force` instead.
fn confirm(prompt: &str) -> Result<bool> {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        return Err(GitMultiError::SyncError(format!(
            "{} (confirmation required in interactive mode; pass --force to skip)",
            prompt
        )));
    }
    Ok(Confirm::new().with_prompt(prompt).interact()?)
}
