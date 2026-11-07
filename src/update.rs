//! `lspyx update`: replace this binary with the latest GitHub release when it's newer.
//!
//! It downloads and checks the same files as `install.sh`, with the same tools (`curl`, `tar`, and
//! `sha256sum` or `shasum`), so lspyx doesn't need an HTTP client of its own.

use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

const REPO: &str = "iyazerski/lspyx";
const BINARY: &str = "lspyx";
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Install the latest release over this binary if it's newer, and return a summary.
pub(crate) fn run_update() -> Result<String> {
    let exe = env::current_exe().context("failed to resolve current lspyx binary")?;
    // Homebrew owns the files under its Cellar, so replacing them would break `brew upgrade`.
    if fs::canonicalize(&exe)
        .is_ok_and(|path| path.components().any(|part| part.as_os_str() == "Cellar"))
    {
        bail!("lspyx was installed with Homebrew; run `brew upgrade lspyx` instead");
    }

    // GitHub redirects `releases/latest` to the latest release's tag page.
    let latest_url = run(
        Command::new("curl")
            .args(["-fsSLI", "-o", "/dev/null", "-w", "%{url_effective}"])
            .arg(format!("https://github.com/{REPO}/releases/latest")),
        "asking GitHub for the latest release",
    )?;
    let tag = latest_url
        .rsplit_once("/tag/")
        .map(|(_, tag)| tag.trim().to_string())
        .with_context(|| format!("unexpected latest release address {latest_url}"))?;
    let latest = tag.trim_start_matches('v');
    if !is_newer(latest, VERSION) {
        return Ok(format!("summary: lspyx {VERSION} is up to date"));
    }

    let dir = exe
        .parent()
        .context("the lspyx binary has no parent directory")?;
    let staging = dir.join(".lspyx-update");
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging)
        .with_context(|| format!("failed to create {}", staging.display()))?;
    let installed = install(&tag, &staging, &exe);
    let _ = fs::remove_dir_all(&staging);
    installed?;
    Ok(format!(
        "summary: updated lspyx {VERSION} to {latest}\nnote: running daemons and MCP servers use the new version after they restart"
    ))
}

/// Download, verify, and unpack `tag`'s archive in `staging`, then move the binary over `exe`
/// in one step, so a failure never leaves a broken binary.
fn install(tag: &str, staging: &Path, exe: &Path) -> Result<()> {
    let asset = asset()?;
    let url = format!("https://github.com/{REPO}/releases/download/{tag}/{asset}");
    for (url, file) in [
        (url.clone(), asset.clone()),
        (format!("{url}.sha256"), format!("{asset}.sha256")),
    ] {
        run(
            Command::new("curl")
                .args(["-fsSL", "-o"])
                .arg(staging.join(file))
                .arg(url),
            "downloading the release",
        )?;
    }
    // The checksum file names the archive relative to its own folder.
    let checksum = format!("{asset}.sha256");
    let mut verify = if which::which("sha256sum").is_ok() {
        Command::new("sha256sum")
    } else {
        let mut shasum = Command::new("shasum");
        shasum.args(["-a", "256"]);
        shasum
    };
    run(
        verify.args(["-c", &checksum]).current_dir(staging),
        "checking the download",
    )?;
    run(
        Command::new("tar")
            .args(["-xzf", &asset, BINARY])
            .current_dir(staging),
        "unpacking the release",
    )?;
    fs::rename(staging.join(BINARY), exe)
        .with_context(|| format!("failed to replace {}", exe.display()))
}

/// Return the release archive name for this platform, matching `install.sh`.
fn asset() -> Result<String> {
    let platform = match env::consts::OS {
        "macos" => "apple-darwin",
        "linux" => "unknown-linux-musl",
        os => bail!("lspyx releases don't support {os}"),
    };
    Ok(format!("{BINARY}-{}-{platform}.tar.gz", env::consts::ARCH))
}

/// Run `command` and return its trimmed stdout, or fail with `what` and its stderr.
fn run(command: &mut Command, what: &str) -> Result<String> {
    let output = command
        .output()
        .with_context(|| format!("{what}: failed to run {:?}", command.get_program()))?;
    if !output.status.success() {
        bail!("{what}: {}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Return whether version `a` (like `0.5.10`) is newer than `b`, comparing numbers in order.
fn is_newer(a: &str, b: &str) -> bool {
    let parts = |version: &str| -> Vec<u64> {
        version
            .split('.')
            .map(|part| part.parse().unwrap_or(0))
            .collect()
    };
    parts(a) > parts(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_by_number() {
        assert!(is_newer("0.5.10", "0.5.9"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(!is_newer("0.5.1", "0.5.1"));
        assert!(!is_newer("0.5.0", "0.5.1"));
    }
}
