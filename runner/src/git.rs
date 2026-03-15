use anyhow::{bail, Context, Result};
use std::path::Path;

/// Fails if the submodule working tree has any uncommitted or untracked changes.
pub fn check_submodule_clean(submodule_path: &Path) -> Result<()> {
    let out = git(submodule_path, &["status", "--porcelain"])?;
    if !out.is_empty() {
        bail!(
            "submodule {} has uncommitted changes — clean it up before running the agent:\n{}",
            submodule_path.display(),
            out
        );
    }
    Ok(())
}

/// Returns the current HEAD SHA of the submodule.
pub fn current_sha(submodule_path: &Path) -> Result<String> {
    git(submodule_path, &["rev-parse", "HEAD"])
}

/// Fetches origin and returns the SHA of origin/main.
pub fn server_main_sha(submodule_path: &Path) -> Result<String> {
    git_status(submodule_path, &["fetch", "origin", "main"])?;
    git(submodule_path, &["rev-parse", "origin/main"])
}

/// Returns (base_sha, head_sha) for a PR in matter-labs/zksync-os-server.
pub fn pr_shas(pr_number: u64) -> Result<(String, String)> {
    let out = std::process::Command::new("gh")
        .args([
            "pr", "view",
            &pr_number.to_string(),
            "--repo", "matter-labs/zksync-os-server",
            "--json", "baseRefOid,headRefOid",
            "--jq", "[.baseRefOid, .headRefOid] | @tsv",
        ])
        .output()
        .context("failed to run gh pr view")?;

    if !out.status.success() {
        bail!("gh pr view failed for PR #{pr_number}: {}", String::from_utf8_lossy(&out.stderr));
    }

    let line = String::from_utf8(out.stdout)?;
    let mut parts = line.trim().splitn(2, '\t');
    let base = parts.next().context("missing baseRefOid")?.to_string();
    let head = parts.next().context("missing headRefOid")?.to_string();
    Ok((base, head))
}

/// Runs a git command in `dir`, checks success, and returns trimmed stdout.
fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .with_context(|| format!("failed to run git {}", args[0]))?;

    if !out.status.success() {
        bail!("git {} failed in {}: {}", args[0], dir.display(), String::from_utf8_lossy(&out.stderr));
    }
    Ok(String::from_utf8(out.stdout)?.trim().to_string())
}

/// Runs a git command in `dir` and checks success (discards stdout).
fn git_status(dir: &Path, args: &[&str]) -> Result<()> {
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .status()
        .with_context(|| format!("failed to run git {}", args[0]))?;

    if !status.success() {
        bail!("git {} failed in {}", args[0], dir.display());
    }
    Ok(())
}
