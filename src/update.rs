use std::io::Read;
use std::path::Path;
use std::process::Command;
use std::time::Duration;
use std::{env, fs, io, path::PathBuf, process};

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    #[error("Git error: {0}")]
    Git(#[from] crate::error::GitMultiError),

    #[error("Network error: {0}")]
    Network(String),

    #[error("No matching asset for this platform ({0})")]
    NoAsset(String),

    #[error("{0}")]
    Refused(String),
}

#[derive(Debug, serde::Deserialize)]
struct GitHubRelease {
    tag_name: String,
    assets: Vec<GitHubAsset>,
}

#[derive(Debug, serde::Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
}

pub fn self_update() -> Result<(), UpdateError> {
    let current_exe = env::current_exe()?;
    let target = detect_target()?;

    if is_cargo_install(&current_exe) {
        return Err(UpdateError::Refused(
            "git-multi appears to be managed by Cargo. Use `cargo install --force` to update.".into(),
        ));
    }

    if is_managed_by_package_manager(&current_exe) {
        return Err(UpdateError::Refused(
            "git-multi appears to be installed by your system package manager (.deb/.rpm/.pkg/.msi). Use your package manager to update.".into(),
        ));
    }

    let release = fetch_latest_release()?;
    let latest_version = release.tag_name.trim_start_matches('v');
    let current_version = env!("CARGO_PKG_VERSION");

    if compare_versions(latest_version, current_version) == std::cmp::Ordering::Less {
        println!("Already on the latest version: {}", current_version);
        return Ok(());
    }
    if latest_version == current_version {
        println!("Already on the latest version: {}", current_version);
        return Ok(());
    }

    let asset_suffix = if cfg!(target_os = "windows") {
        ".zip"
    } else {
        ".tar.xz"
    };
    let asset_name = format!("git-multi-{}{}", target, asset_suffix);

    let asset = release
        .assets
        .iter()
        .find(|a| a.name == asset_name)
        .ok_or_else(|| UpdateError::NoAsset(target.to_string()))?;

    println!("Updating from {} to {}...", current_version, latest_version);

    let tmp_dir = env::temp_dir();
    let archive_path = tmp_dir.join(&asset_name);
    println!("Downloading {}...", asset_name);
    download_file(&asset.browser_download_url, &archive_path)?;

    let bin_name = if cfg!(target_os = "windows") {
        "git-multi.exe"
    } else {
        "git-multi"
    };
    let tmp_bin = tmp_dir.join(format!("git-multi-new-{}", process::id()));
    let _ = fs::remove_file(&tmp_bin);

    // The release archives may store the binary under a different name
    // (e.g. `git-multi-bin`) or nested in a subdirectory, so we search the
    // extracted contents for the executable rather than assuming a layout.
    let extracted = if cfg!(target_os = "windows") {
        #[cfg(target_os = "windows")]
        {
            extract_zip(&archive_path)?
        }
        #[cfg(not(target_os = "windows"))]
        {
            unreachable!()
        }
    } else {
        #[cfg(not(target_os = "windows"))]
        {
            extract_tar_xz(&archive_path)?
        }
        #[cfg(target_os = "windows")]
        {
            unreachable!()
        }
    };

    let found = find_executable(&extracted, bin_name).ok_or_else(|| {
        UpdateError::Network(format!(
            "Extraction failed: executable not found in archive ({})",
            asset_name
        ))
    })?;

    if !found.exists() {
        return Err(UpdateError::Network(
            "Extraction failed: executable not found in archive".into(),
        ));
    }

    let backup_path = current_exe.with_extension(format!("previous.{}", process::id()));
    fs::copy(&current_exe, &backup_path)?;
    println!("Previous binary backed up to: {}", backup_path.display());

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&found)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&found, perms)?;
    }

    let _ = fs::remove_file(&tmp_bin);
    match fs::rename(&found, &current_exe) {
        Ok(_) => {}
        Err(_e) => {
            return Err(UpdateError::Network(format!(
                "Failed to replace binary (on Windows the running .exe may be locked). \
                 The new binary has been saved to: {}. Close git-multi and replace the old binary manually.",
                found.display()
            )));
        }
    }

    let _ = fs::remove_dir_all(&extracted);

    println!("Updated to {} successfully!", latest_version);
    println!("Previous binary backed up at: {}", backup_path.display());

    Ok(())
}

/// Compare two dotted versions (`x.y.z`), ignoring any pre-release suffix.
fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let nums = |v: &str| -> Vec<u64> {
        v.split('.')
            .map(|p| p.chars().take_while(|c| c.is_ascii_digit()).collect::<String>())
            .filter_map(|p| p.parse::<u64>().ok())
            .collect()
    };
    let av = nums(a);
    let bv = nums(b);
    let len = av.len().max(bv.len());
    for i in 0..len {
        let x = av.get(i).copied().unwrap_or(0);
        let y = bv.get(i).copied().unwrap_or(0);
        match x.cmp(&y) {
            std::cmp::Ordering::Equal => continue,
            o => return o,
        }
    }
    std::cmp::Ordering::Equal
}

fn detect_target() -> Result<&'static str, UpdateError> {
    let os = env::consts::OS;
    let arch = env::consts::ARCH;
    match (os, arch) {
        ("linux", "x86_64") => Ok("x86_64-unknown-linux-gnu"),
        ("macos", "x86_64") => Ok("x86_64-apple-darwin"),
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        ("windows", "x86_64") => Ok("x86_64-pc-windows-msvc"),
        _ => Err(UpdateError::NoAsset(format!("{}-{}", os, arch))),
    }
}

fn is_cargo_install(exe: &Path) -> bool {
    exe.to_str()
        .map(|p| {
            let normal = p.contains(".cargo") || p.contains("/target/") || p.contains("\\target\\");
            let dev = p.contains("/target/debug/") || p.contains("/target/release/")
                || p.contains("\\target\\debug\\")
                || p.contains("\\target\\release\\");
            normal && !dev
        })
        .unwrap_or(false)
}

fn is_managed_by_package_manager(exe: &Path) -> bool {
    if let Some(path) = exe.to_str() {
        if path.starts_with("/usr/bin/")
            || path.starts_with("/usr/sbin/")
            || path.starts_with("/bin/")
            || path.starts_with("/sbin/")
        {
            return true;
        }

        #[cfg(target_os = "linux")]
        {
            if let Ok(out) = Command::new("dpkg").args(["-S", path]).output() {
                if out.status.success()
                    && !String::from_utf8_lossy(&out.stdout).trim().is_empty()
                {
                    return true;
                }
            }
            if let Ok(out) = Command::new("rpm").args(["-qf", path]).output() {
                if out.status.success()
                    && !String::from_utf8_lossy(&out.stdout).trim().is_empty()
                {
                    return true;
                }
            }
        }

        #[cfg(target_os = "macos")]
        {
            if let Ok(out) = Command::new("brew").args(["list", "--versions"]).output() {
                if String::from_utf8_lossy(&out.stdout).contains("git-multi") {
                    return true;
                }
            }
        }
    }
    false
}

fn fetch_latest_release() -> Result<GitHubRelease, UpdateError> {
    // GitHub's API requires a User-Agent; use curl with a short timeout.
    let mut child = Command::new("curl")
        .args(["-sL", "--max-time", "60", "-H", "User-Agent: git-multi"])
        .arg("https://api.github.com/repos/CharaD7/git-multi/releases/latest")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| UpdateError::Network(format!("curl is required for self-update: {}", e)))?;

    let out = wait_with_timeout_update(&mut child, Duration::from_secs(60))?;
    if !out.status.success() {
        return Err(UpdateError::Network(format!(
            "GitHub API returned status: {}",
            out.status
        )));
    }

    let release: GitHubRelease = serde_json::from_slice(&out.stdout)
        .map_err(|e| UpdateError::Network(format!("Failed to parse release JSON: {}", e)))?;

    Ok(release)
}

#[cfg(target_os = "windows")]
fn extract_zip(archive: &Path) -> Result<PathBuf, UpdateError> {
    let extract_dir = env::temp_dir().join(format!("git-multi-update-extract-{}", process::id()));
    let _ = fs::remove_dir_all(&extract_dir);
    fs::create_dir_all(&extract_dir)?;

    let ps_cmd = format!(
        "Expand-Archive -Path '{}' -DestinationPath '{}' -Force",
        archive.display(),
        extract_dir.display()
    );

    let status = Command::new("powershell")
        .args(["-Command", &ps_cmd])
        .status()
        .map_err(|e| UpdateError::Network(format!("Extraction failed: {}", e)))?;

    if !status.success() {
        return Err(UpdateError::Network(
            "Failed to extract zip archive".into(),
        ));
    }
    Ok(extract_dir)
}

#[cfg(not(target_os = "windows"))]
fn extract_tar_xz(archive: &Path) -> Result<PathBuf, UpdateError> {
    let extract_dir = env::temp_dir().join(format!("git-multi-update-extract-{}", process::id()));
    let _ = fs::remove_dir_all(&extract_dir);
    fs::create_dir_all(&extract_dir)?;

    let status = Command::new("tar")
        .args(["-xJf"])
        .arg(archive)
        .arg("-C")
        .arg(&extract_dir)
        .status()
        .map_err(|e| UpdateError::Network(format!("Extraction failed: {}", e)))?;

    if !status.success() {
        return Err(UpdateError::Network(
            "Failed to extract tar.xz archive".into(),
        ));
    }
    Ok(extract_dir)
}

/// Recursively find the executable (`git-multi` / `git-multi.exe`) inside an
/// extracted directory, tolerating any archive layout or binary name.
fn find_executable(dir: &Path, bin_name: &str) -> Option<PathBuf> {
    let mut best: Option<(usize, PathBuf)> = None;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in fs::read_dir(&d).ok()? {
            let entry = entry.ok()?;
            let path = entry.path();
            let ft = entry.file_type().ok()?;
            if ft.is_dir() {
                stack.push(path);
            } else if ft.is_file() || ft.is_symlink() {
                let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                if name == bin_name || name.starts_with("git-multi") {
                    let depth = path.components().count();
                    if best.as_ref().is_none_or(|(d, _)| depth < *d) {
                        best = Some((depth, path));
                    }
                }
            }
        }
    }
    best.map(|(_, p)| p)
}

fn download_file(url: &str, dest: &Path) -> Result<(), UpdateError> {
    let mut child = Command::new("curl")
        .args(["-fL", "--proto", "=https", "--tlsv1.2", "--max-time", "300", "-o"])
        .arg(dest)
        .arg(url)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| UpdateError::Network(format!("curl is required for self-update: {}", e)))?;

    let out = wait_with_timeout_update(&mut child, Duration::from_secs(300))?;
    if !out.status.success() {
        let _ = fs::remove_file(dest);
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(UpdateError::Network(format!(
            "Download failed: {}",
            stderr.trim()
        )));
    }

    let meta = fs::metadata(dest)?;
    if meta.len() == 0 {
        let _ = fs::remove_file(dest);
        return Err(UpdateError::Network(
            "Downloaded file is empty".into(),
        ));
    }

    Ok(())
}

/// Wait for a child with a hard timeout, draining stdout/stderr so a chatty
/// process cannot fill the pipe buffer and deadlock.
fn wait_with_timeout_update(child: &mut process::Child, timeout: Duration) -> Result<process::Output, UpdateError> {
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

    let start = std::time::Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(UpdateError::Io)? {
            break status;
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(UpdateError::Network(format!(
                "command timed out after {}s",
                timeout.as_secs()
            )));
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    let stdout = out_reader.and_then(|h| h.join().ok()).unwrap_or_default();
    let stderr = err_reader.and_then(|h| h.join().ok()).unwrap_or_default();
    Ok(process::Output { status, stdout, stderr })
}
