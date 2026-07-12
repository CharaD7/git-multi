# git-multi — Installation & Packages

Every [GitHub release](https://github.com/CharaD7/git-multi/releases) ships ready-to-use
binaries and installers for **Linux**, **Windows**, and **macOS**. Download the asset that
matches your platform from the release page (all links below use the tag `v<VERSION>`).

> The `git-multi` binary is a single self-contained executable — once it is on your `PATH`
> you can run `git-multi --gui` (terminal UI) or any CLI subcommand
> (`git-multi init`, `git-multi remote list`, …).

---

## Linux

| Package | Asset | Best for |
| --- | --- | --- |
| Debian / Ubuntu | `git-multi_<version>_amd64.deb` | `.deb`-based distros |
| Fedora / RHEL | `git-multi-<version>-1.x86_64.rpm` | `.rpm`-based distros |
| Portable | `git-multi-x86_64-unknown-linux-gnu.AppImage` | Any Linux (no root) |
| Tarball | `git-multi-x86_64-unknown-linux-gnu.tar.xz` | Manual install |

### `.deb`
```bash
sudo apt install ./git-multi_<version>_amd64.deb
git-multi --gui
```

### `.rpm`
```bash
sudo dnf install ./git-multi-<version>-1.x86_64.rpm
# or: sudo rpm -Uvh ./git-multi-<version>-1.x86_64.rpm
git-multi --gui
```

### `.AppImage` (no installation required)
```bash
chmod +x git-multi-x86_64-unknown-linux-gnu.AppImage
./git-multi-x86_64-unknown-linux-gnu.AppImage --gui
# install system-wide:
mv git-multi-x86_64-unknown-linux-gnu.AppImage ~/.local/bin/git-multi
```

### Tarball
```bash
tar xf git-multi-x86_64-unknown-linux-gnu.tar.xz
sudo mv git-multi-*/git-multi /usr/local/bin/
git-multi --gui
```

---

## Windows

| Package | Asset | Best for |
| --- | --- | --- |
| Installer | `git-multi-x86_64-pc-windows-msvc.msi` | Guided install |
| Portable | `git-multi-x86_64-pc-windows-msvc.zip` | No install, just unzip |

### `.msi`
Double-click the installer, or install silently from a terminal:
```powershell
msiexec /i git-multi-x86_64-pc-windows-msvc.msi /quiet
git-multi --gui
```

### `.zip` (portable `.exe`)
```powershell
Expand-Archive git-multi-x86_64-pc-windows-msvc.zip
.\git-multi-x86_64-pc-windows-msvc\git-multi.exe --gui
```

---

## macOS

| Package | Asset | Best for |
| --- | --- | --- |
| Installer | `git-multi-x86_64-apple-darwin.pkg` | Guided install |
| Tarball | `git-multi-x86_64-apple-darwin.tar.xz` | Manual install |

> Apple Silicon (M1/M2/…) users: also grab `git-multi-aarch64-apple-darwin.pkg` / `.tar.xz`.

### `.pkg`
```bash
sudo installer -pkg git-multi-x86_64-apple-darwin.pkg -target /
git-multi --gui
```
If Gatekeeper blocks it, allow the package once:
```bash
sudo xattr -dr com.apple.quarantine git-multi-x86_64-apple-darwin.pkg
```

### Tarball
```bash
tar xf git-multi-x86_64-apple-darwin.tar.xz
sudo mv git-multi-*/git-multi /usr/local/bin/
git-multi --gui
```

---

## From source (Cargo)

```bash
cargo install --path .
```
