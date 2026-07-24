use std::io::Write;
use std::process::Command;
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

    if latest_version == current_version {
        println!("Already on the latest version: {}", current_version);
        return Ok(());
    }

    let asset_name = format!(
        "git-multi-{}{}",
        target,
        if cfg!(target_os = "windows") {
            ".zip"
        } else {
            ".tar.xz"
        }
    );

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
    let tmp_bin = tmp_dir.join(bin_name);
    let _ = fs::remove_file(&tmp_bin);

    if cfg!(target_os = "windows") {
        #[cfg(target_os = "windows")]
        extract_zip(&archive_path, &tmp_bin)?;
    } else {
        #[cfg(not(target_os = "windows"))]
        extract_tar_xz(&archive_path, &tmp_bin)?;
    }

    if !tmp_bin.exists() {
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
        let mut perms = fs::metadata(&tmp_bin)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&tmp_bin, perms)?;
    }

    match fs::rename(&tmp_bin, &current_exe) {
        Ok(_) => {}
        Err(e) => {
            let _ = fs::remove_file(&tmp_bin);
            return Err(UpdateError::Network(format!(
                "Failed to replace binary (on Windows the running .exe may be locked). \
                 The new binary has been saved to: {}. Close git-multi and replace the old binary manually.",
                tmp_bin.display()
            )));
        }
    }

    println!("Updated to {} successfully!", latest_version);
    println!("Previous binary backed up at: {}", backup_path.display());

    Ok(())
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

fn is_cargo_install(exe: &PathBuf) -> bool {
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

fn is_managed_by_package_manager(exe: &PathBuf) -> bool {
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
    let output = Command::new("curl")
        .args([
            "-sL",
            "https://api.github.com/repos/CharaD7/git-multi/releases/latest",
        ])
        .output()
        .map_err(|e| UpdateError::Network(format!("Failed to fetch release info: {}", e)))?;

    if !output.status.success() {
        return Err(UpdateError::Network(format!(
            "GitHub API returned status: {}",
            output.status
        )));
    }

    let release: GitHubRelease = serde_json::from_slice(&output.stdout)
        .map_err(|e| UpdateError::Network(format!("Failed to parse release JSON: {}", e)))?;

    Ok(release)
}

#[cfg(target_os = "windows")]
fn extract_zip(archive: &PathBuf, output: &PathBuf) -> Result<(), UpdateError> {
    let extract_dir = env::temp_dir().join("git-multi-update-extract");
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

    let mut found = None;
    for entry in fs::read_dir(&extract_dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            let bin_path = entry.path().join("git-multi.exe");
            if bin_path.exists() {
                found = Some(bin_path);
                break;
            }
        }
    }

    let src = found.ok_or_else(|| {
        UpdateError::Network("Could not find git-multi.exe in archive".into())
    })?;
    fs::copy(&src, output)?;
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn extract_tar_xz(archive: &PathBuf, output: &PathBuf) -> Result<(), UpdateError> {
    let output_dir = output
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| env::temp_dir());

    let status = Command::new("tar")
        .args(["-xJf"])
        .arg(archive)
        .arg("-C")
        .arg(&output_dir)
        .status()
        .map_err(|e| UpdateError::Network(format!("Extraction failed: {}", e)))?;

    if !status.success() {
        return Err(UpdateError::Network(
            "Failed to extract tar.xz archive".into(),
        ));
    }

    let extracted = output_dir.join(if cfg!(target_os = "windows") {
        "git-multi.exe"
    } else {
        "git-multi"
    });
    if extracted.exists() {
        let _ = fs::remove_file(output);
        fs::rename(&extracted, output)?;
    }

    Ok(())
}

fn download_file(url: &str, dest: &PathBuf) -> Result<(), UpdateError> {
    let mut child = Command::new("curl")
        .args(["-fL", "--proto", "=https", "--tlsv1.2"])
        .arg("-o")
        .arg(dest)
        .arg(url)
        .spawn()
        .map_err(|e| UpdateError::Network(format!("Download failed: {}", e)))?;

    let status = child
        .wait()
        .map_err(|e| UpdateError::Network(format!("Download failed: {}", e)))?;

    if !status.success() {
        let _ = fs::remove_file(dest);
        return Err(UpdateError::Network(format!(
            "Download failed with status: {}",
            status
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
