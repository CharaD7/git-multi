mod cli;
mod config;
mod error;
mod git;

use cli::*;
use error::*;
use git::*;

use clap::Parser;
use console::style;
use dialoguer::Confirm;
use std::process;
use tracing::{info, warn};

fn main() {
    let cli = Cli::parse();
    
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
    match &cli.command {
        Commands::Init => cmd_init(),
        Commands::Remote { command } => cmd_remote(command),
        Commands::Branch { command } => cmd_branch(command),
        Commands::Fetch { all, remote } => cmd_fetch(*all, remote.clone()),
        Commands::Pull { all, remote, branch } => cmd_pull(*all, remote.clone(), branch.clone()),
        Commands::Push { all, remote, branch, force } => cmd_push(*all, remote.clone(), branch.clone(), *force),
        Commands::Checkout { branch, remote, new } => cmd_checkout(branch.clone(), remote.clone(), *new),
        Commands::Sync { from_remote, to_remote, from_branch, to_branch, commits, strategy, force } => {
            cmd_sync(from_remote.clone(), to_remote.clone(), from_branch.clone(), to_branch.clone(), 
                   commits.clone(), *strategy, *force)
        }
        Commands::Copy { from, to, files, prune } => cmd_copy(from.clone(), to.clone(), files.clone(), *prune),
        Commands::Pr { remote, base, head, title, description, open } => {
            cmd_pr(remote.clone(), base.clone(), head.clone(), title.clone(), description.clone(), *open)
        }
        Commands::Use { remote } => cmd_use(remote.clone()),
        Commands::Status => cmd_status(),
        Commands::List => cmd_list(),
    }
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

fn cmd_remote(command: &RemoteCommands) -> Result<()> {
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
            
            if !*force {
                let confirm = Confirm::new()
                    .with_prompt(format!("Remove remote '{}'? This cannot be undone.", name))
                    .interact()?;
                if !confirm {
                    return Ok(());
                }
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
            repo.config.set_primary_remote(&name)?;
            repo.config.save(&repo.repo)?;
            println!("Primary remote set to '{}'", style(name).green());
            Ok(())
        }
        RemoteCommands::Show { name } => {
            let repo = GitRepo::open()?;
            let remote = repo.repo.find_remote(&name)?;

            println!("Remote: {}", style(name).cyan().bold());
            println!("URL: {}", remote.url().unwrap_or("unknown"));

            if let Some(push_url) = remote.pushurl() {
                println!("Push URL: {}", push_url);
            }

            // Show config details
            if let Ok(config) = repo.config.get_remote(&name) {
                println!("Is Primary: {}", config.is_primary);
            }
            if let Some((name, _)) = repo.config.get_primary_remote() {
                println!("Primary Remote: {}", name);
            }

            let branches = repo.list_remote_branches(&name)?;
            println!("\nBranches:");
            for branch in branches {
                println!("  {}", branch);
            }
            Ok(())
        }
        RemoteCommands::ListNames {} => {
            let repo = GitRepo::open()?;
            for name in repo.config.get_remote_names() {
                println!("{}", name);
            }
            Ok(())
        }
    }
}

// ========== BRANCH ==========

fn cmd_branch(command: &BranchCommands) -> Result<()> {
    match command {
        BranchCommands::List { all, remote } => {
            let repo = GitRepo::open()?;
            
            if let Some(remote_name) = remote {
                let branches = repo.list_remote_branches(remote_name)?;
                println!("Branches on remote '{}':", style(remote_name).cyan());
                for branch in branches {
                    println!("  {}", style(&branch).green());
                }
            } else if *all {
                let info = repo.list_all_branches()?;
                
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
                println!("Local branches:");
                for branch in &info.local {
                    println!("  {}", style(branch.to_string()).green());
                }
            }
            Ok(())
        }
        BranchCommands::Delete { branch, force, remote } => {
            let repo = GitRepo::open()?;
            
            if !*force {
                let confirm = Confirm::new()
                    .with_prompt(format!("Delete branch '{}'? This cannot be undone.", branch))
                    .interact()?;
                if !confirm {
                    return Ok(());
                }
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
    }
}

// ========== FETCH ==========

fn cmd_fetch(all: bool, remote: Option<String>) -> Result<()> {
    let repo = GitRepo::open()?;
    
    if all {
        let fetched = repo.fetch_all()?;
        println!("Fetched from {} remote(s):", style(fetched.len()).green());
        for name in fetched {
            println!("  {}", style(&name).cyan());
        }
    } else if let Some(remote_name) = remote {
        repo.fetch_remote(&remote_name)?;
        println!("Fetched from '{}'", style(&remote_name).green());
    } else {
        // Default: fetch from all remotes
        let fetched = repo.fetch_all()?;
        println!("Fetched from {} remote(s)", style(fetched.len()).green());
    }
    Ok(())
}

// ========== PULL ==========

fn cmd_pull(all: bool, remote: Option<String>, branch: Option<String>) -> Result<()> {
    let repo = GitRepo::open()?;
    
    if all {
        let pulled = repo.pull_from_all(branch.as_deref())?;
        println!("Pulled from {} remote(s):", style(pulled.len()).green());
        for name in pulled {
            println!("  {}", style(&name).cyan());
        }
    } else if let Some(remote_name) = remote {
        repo.pull_from_remote(&remote_name, branch.as_deref())?;
        println!("Pulled from '{}'", style(&remote_name).green());
        if let Some(b) = &branch {
            println!("  Branch: {}", b);
        }
    } else {
        // Default: pull from default remote
        if let Some(default_remote) = repo.config.get_default_remote() {
            repo.pull_from_remote(default_remote, branch.as_deref())?;
            println!("Pulled from default remote '{}'", style(default_remote).green());
        } else {
            return Err(GitMultiError::NoRemotesConfigured);
        }
    }
    Ok(())
}

// ========== PUSH ==========

fn cmd_push(all: bool, remote: Option<String>, branch: Option<String>, force: bool) -> Result<()> {
    let repo = GitRepo::open()?;
    
    if all {
        let pushed = repo.push_to_all(branch.as_deref())?;
        println!("Pushed to {} remote(s):", style(pushed.len()).green());
        for name in pushed {
            println!("  {}", style(&name).cyan());
        }
    } else if let Some(remote_name) = remote {
        repo.push_to_remote(&remote_name, branch.as_deref())?;
        println!("Pushed to '{}'", style(&remote_name).green());
        if let Some(b) = &branch {
            println!("  Branch: {}", b);
        }
        if force {
            println!("  Force: yes");
        }
    } else {
        // Default: push to default remote
        if let Some(default_remote) = repo.config.get_default_remote() {
            repo.push_to_remote(default_remote, branch.as_deref())?;
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
    commits: String,
    strategy: SyncStrategy,
    _force: bool,
) -> Result<()> {
    let repo = GitRepo::open()?;
    
    info!("Syncing from {}/{}", from_remote, from_branch);
    info!("Syncing to   {}/{}", to_remote, to_branch);
    info!("Strategy: {}", strategy);
    info!("Commit range: {}", commits);
    
    // Parse source and destination
    let from_ref = format!("refs/remotes/{}/{}", from_remote, from_branch);
    let to_ref = format!("refs/remotes/{}/{}", to_remote, to_branch);
    
    match strategy {
        SyncStrategy::CherryPick => {
            let picked = repo.cherry_pick_range(&from_ref, &to_ref, &commits)?;
            println!("Cherry-picked {} commit(s):", style(picked.len()).green());
            for sha in picked {
                println!("  {}", style(&sha[..8]).cyan());
            }
        }
        SyncStrategy::Merge => {
            // For merge, we need to fetch both branches first
            repo.fetch_remote(&from_remote)?;
            repo.fetch_remote(&to_remote)?;
            
            // Checkout target branch
            repo.checkout_branch(&to_branch)?;
            
            // Merge source branch
            repo.merge_branch(&from_branch)?;
            println!("Merged '{}' into '{}'", style(&from_branch).green(), to_branch);
        }
        SyncStrategy::Rebase => {
            repo.fetch_remote(&from_remote)?;
            repo.fetch_remote(&to_remote)?;
            
            // Checkout source branch
            repo.checkout_branch(&from_branch)?;
            
            // Rebase onto target branch
            repo.rebase_branch(&to_branch)?;
            println!("Rebased '{}' onto '{}'", style(&from_branch).green(), to_branch);
        }
    }
    
    Ok(())
}

// ========== COPY ==========

fn cmd_copy(from: String, to: String, files: Vec<String>, prune: bool) -> Result<()> {
    let repo = GitRepo::open()?;
    
    // Parse from and to specifications (format: remote:branch or just branch)
    let (from_remote, from_branch) = parse_ref_spec(&from);
    let (to_remote, to_branch) = parse_ref_spec(&to);
    
    info!("Copying files from {}/{}", from_remote.as_deref().unwrap_or("local"), from_branch);
    info!("Copying files to   {}/{}", to_remote.as_deref().unwrap_or("local"), to_branch);
    
    // For now, implement simple file copy from one ref to working directory
    // Full cross-remote copy would require more complex logic
    
    let from_ref = if let Some(remote) = &from_remote {
        format!("refs/remotes/{}/{}", remote, from_branch)
    } else {
        from_branch
    };
    
    let copied = repo.copy_files(&from_ref, &files)?;
    
    println!("Copied {} file(s):", style(copied.len()).green());
    for file in copied {
        println!("  {}", style(&file).cyan());
    }
    
    if prune {
        warn!("Prune option not yet implemented");
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
    
    repo.create_pr(&remote, &base, &head_branch, &title, description.as_deref())?;
    
    println!("Pull request created successfully!");
    println!("  Repository: {}", style(&remote).cyan());
    println!("  Base: {} <- Head: {}", style(&base).green(), style(&head_branch).green());
    println!("  Title: {}", style(&title).yellow());
    
    if open {
        // For now, just print a message
        // Could use `gh pr view --web` to open in browser
        println!("  Run `gh pr view --web` to open in browser");
    }
    
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

fn cmd_status() -> Result<()> {
    let repo = GitRepo::open()?;
    
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

fn cmd_list() -> Result<()> {
    let repo = GitRepo::open()?;
    
    let remotes = repo.list_remotes()?;
    
    for remote_name in remotes {
        println!("\nRemote: {}", style(&remote_name).cyan().bold());
        let branches = repo.list_remote_branches(&remote_name)?;
        for branch in branches {
            println!("  {}", style(&branch).green());
        }
    }
    
    Ok(())
}
