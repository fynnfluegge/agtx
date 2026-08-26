//! Download the release for this host, verify it, and replace the running
//! binary in place.

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

use super::release;

/// A package manager owns this binary, so agtx must not replace it.
///
/// Silently overwriting a file Homebrew or Nix believes it owns breaks the
/// machine in a way that is hard to diagnose later — the manager's manifest and
/// the file on disk disagree, and the next upgrade reverts the update without
/// explanation. Refuse and name the right command instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagedBy {
    Homebrew,
    Nix,
}

impl ManagedBy {
    pub fn advice(&self) -> &'static str {
        match self {
            Self::Homebrew => "This agtx was installed by Homebrew — update it with `brew upgrade agtx`.",
            Self::Nix => "This agtx lives in the Nix store and cannot be replaced in place — update it through Nix.",
        }
    }
}

/// Pure so the path table is testable without installing anything.
pub fn managed_by(exe: &Path) -> Option<ManagedBy> {
    let p = exe.to_string_lossy();
    if p.starts_with("/nix/store/") {
        return Some(ManagedBy::Nix);
    }
    if p.starts_with("/opt/homebrew/")
        || p.contains("/Cellar/")
        || p.starts_with("/home/linuxbrew/")
    {
        return Some(ManagedBy::Homebrew);
    }
    None
}

/// Where the replacement lands.
///
/// `canonicalize` matters: the install path may be a symlink (`install.sh`
/// moves the binary directly, but a manual install or a dotfiles setup may
/// not), and replacing the symlink instead of its target would leave the real
/// binary stale and the link pointing at a different file than the user thinks.
pub fn target_path() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("could not determine the running binary's path")?;
    Ok(exe.canonicalize().unwrap_or(exe))
}

/// `<hash>  <filename>` — the format `sha256sum`/`shasum -a 256` write and
/// `install.sh` feeds back to `-c`.
pub fn parse_sha256_file(contents: &str) -> Option<String> {
    let hash = contents.split_whitespace().next()?;
    if hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        Some(hash.to_ascii_lowercase())
    } else {
        None
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// What `agtx update` reports back, so the CLI and the TUI can render the same
/// outcome differently.
pub struct Installed {
    pub tag: String,
    pub path: PathBuf,
}

fn curl_to_file(url: &str, dest: &Path) -> Result<()> {
    let status = Command::new("curl")
        .args(["-fsSL", "--max-time", "300", url, "-o"])
        .arg(dest)
        .status()
        .context("failed to run curl (is it installed?)")?;
    if !status.success() {
        bail!("download failed: {url}");
    }
    Ok(())
}

fn curl_to_string(url: &str) -> Result<String> {
    let out = Command::new("curl")
        .args(["-fsSL", "--max-time", "30", url])
        .output()
        .context("failed to run curl (is it installed?)")?;
    if !out.status.success() {
        bail!("download failed: {url}");
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Download `tag`'s archive for this host, verify it, and swap it into place.
///
/// `progress` is called with human-readable steps so the CLI can print them and
/// the TUI can put the last one in a popup.
pub fn install_release(tag: &str, progress: &mut dyn FnMut(&str)) -> Result<Installed> {
    let target = target_path()?;
    if let Some(managed) = managed_by(&target) {
        bail!("{}", managed.advice());
    }

    let dir = target
        .parent()
        .context("the running binary has no parent directory")?
        .to_path_buf();

    // Check writability before spending a download on it. `install.sh`'s
    // default is ~/.local/bin, which is writable; a binary moved under
    // /usr/local/bin with sudo is not, and agtx must not try to escalate.
    if !is_writable_dir(&dir) {
        bail!(
            "{} is not writable — re-run the installer with the right permissions:\n  \
             curl -fsSL https://raw.githubusercontent.com/{}/main/install.sh | AGTX_INSTALL_DIR={} bash",
            dir.display(),
            release::repo(),
            dir.display()
        );
    }

    let os = release::host_os().context("no agtx release is published for this OS")?;
    let arch =
        release::host_arch().context("no agtx release is published for this architecture")?;
    let archive = release::archive_name(tag, arch, os);
    let repo = release::repo();
    let url = release::download_url(&repo, tag, &archive);

    // Everything staged inside the *target's own directory*, so the final
    // rename is same-filesystem. /tmp frequently is not, and `fs::rename`
    // across filesystems fails with EXDEV.
    let staging = dir.join(format!(".agtx-update-{}", std::process::id()));
    std::fs::create_dir_all(&staging)
        .with_context(|| format!("could not create staging dir {}", staging.display()))?;
    let _guard = DirGuard(staging.clone());

    progress(&format!("downloading {archive}"));
    let archive_path = staging.join(&archive);
    curl_to_file(&url, &archive_path)?;

    progress("verifying checksum");
    let bytes = std::fs::read(&archive_path)?;
    match curl_to_string(&format!("{url}.sha256")) {
        Ok(sums) => {
            let expected = parse_sha256_file(&sums)
                .with_context(|| format!("malformed checksum file for {archive}"))?;
            let actual = sha256_hex(&bytes);
            if actual != expected {
                bail!("checksum mismatch for {archive}: expected {expected}, got {actual}");
            }
        }
        // Releases before the workflow published checksums have none. Warn
        // rather than refuse, matching install.sh — but say so, so it is
        // visible rather than silent.
        Err(_) => progress("no checksum published for this release — skipping verification"),
    }

    progress("extracting");
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(&archive_path)
        .arg("-C")
        .arg(&staging)
        .status()
        .context("failed to run tar")?;
    if !status.success() {
        bail!("could not extract {archive}");
    }
    let new_binary = staging.join("agtx");
    if !new_binary.exists() {
        bail!("{archive} did not contain an `agtx` binary");
    }
    set_executable(&new_binary)?;

    progress(&format!("installing to {}", target.display()));
    replace_binary(&new_binary, &target)?;

    Ok(Installed {
        tag: tag.to_string(),
        path: target,
    })
}

/// The swap, in the order that leaves a recoverable file at every step.
///
/// Renaming over a *running* binary is legal on Unix: `ETXTBSY` applies to
/// writing into the busy inode, not to replacing the directory entry that
/// points at it. The running process keeps its old inode until it exits, which
/// is why the caller has to say "restart agtx".
///
/// Moving the current binary aside first, rather than renaming straight over
/// it, means a failure between the two renames leaves `agtx.old` on disk
/// instead of no `agtx` at all.
pub fn replace_binary(new: &Path, target: &Path) -> Result<()> {
    let backup = target.with_extension("old");
    let had_backup = match std::fs::rename(target, &backup) {
        Ok(()) => true,
        // Nothing at the target yet is not a failure worth aborting for.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => return Err(e).context("could not move the current binary aside"),
    };

    if let Err(e) = std::fs::rename(new, target) {
        if had_backup {
            let _ = std::fs::rename(&backup, target);
        }
        return Err(e).context("could not move the new binary into place");
    }

    if had_backup {
        let _ = std::fs::remove_file(&backup);
    }
    Ok(())
}

fn set_executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn is_writable_dir(dir: &Path) -> bool {
    let probe = dir.join(format!(".agtx-write-probe-{}", std::process::id()));
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Removes the staging directory however `install_release` returns — including
/// the early `?` on a failed download, which would otherwise leave a stray
/// directory next to the user's binary.
struct DirGuard(PathBuf);

impl Drop for DirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
