use anyhow::{bail, Context, Result};
use std::path::Path;

/// Fails if the submodule working tree has any uncommitted or untracked changes.
pub fn check_submodule_clean(submodule_path: &Path) -> Result<()> {
    let out = std::process::Command::new("git")
        .args(["-C", path(submodule_path)?, "status", "--porcelain"])
        .output()
        .context("failed to run git status")?;

    if !out.status.success() {
        bail!("git status failed in {}", submodule_path.display());
    }

    let stdout = String::from_utf8(out.stdout)?;
    if !stdout.trim().is_empty() {
        bail!(
            "submodule {} has uncommitted changes — clean it up before running the agent:\n{}",
            submodule_path.display(),
            stdout
        );
    }

    Ok(())
}

/// Returns the current HEAD SHA of the submodule.
pub fn submodule_sha(submodule_path: &Path) -> Result<String> {
    let out = std::process::Command::new("git")
        .args(["-C", path(submodule_path)?, "rev-parse", "HEAD"])
        .output()
        .context("failed to run git rev-parse")?;

    if !out.status.success() {
        bail!("git rev-parse failed in {}", submodule_path.display());
    }

    Ok(String::from_utf8(out.stdout)?.trim().to_string())
}

/// Advances the agent's server submodule to the upstream main branch.
/// Returns (old_sha, new_sha).
pub fn update_submodule_to_main(repo_root: &Path, agent_dir: &str) -> Result<(String, String)> {
    let submodule_path = repo_root.join(agent_dir).join("zksync-os-server");
    let old_sha = submodule_sha(&submodule_path)?;

    let status = std::process::Command::new("git")
        .args([
            "-C",
            path(repo_root)?,
            "submodule",
            "update",
            "--remote",
            "--checkout",
            &format!("{agent_dir}/zksync-os-server"),
        ])
        .status()
        .context("failed to run git submodule update")?;

    if !status.success() {
        bail!("git submodule update --remote failed for {agent_dir}/zksync-os-server");
    }

    let new_sha = submodule_sha(&submodule_path)?;
    Ok((old_sha, new_sha))
}

/// Returns (base_sha, head_sha) for a PR in matter-labs/zksync-os-server.
pub fn pr_shas(pr_number: u64) -> Result<(String, String)> {
    let out = std::process::Command::new("gh")
        .args([
            "pr",
            "view",
            &pr_number.to_string(),
            "--repo",
            "matter-labs/zksync-os-server",
            "--json",
            "baseRefOid,headRefOid",
            "--jq",
            "[.baseRefOid, .headRefOid] | @tsv",
        ])
        .output()
        .context("failed to run gh pr view")?;

    if !out.status.success() {
        bail!(
            "gh pr view failed for PR #{pr_number}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    let line = String::from_utf8(out.stdout)?;
    let mut parts = line.trim().splitn(2, '\t');
    let base = parts.next().context("missing baseRefOid")?.to_string();
    let head = parts.next().context("missing headRefOid")?.to_string();
    Ok((base, head))
}

/// Fetches from origin and checks out a specific SHA in the submodule (detached HEAD).
pub fn checkout_submodule_sha(submodule_path: &Path, sha: &str) -> Result<()> {
    let fetch = std::process::Command::new("git")
        .args(["-C", path(submodule_path)?, "fetch", "origin"])
        .status()
        .context("failed to run git fetch")?;

    if !fetch.success() {
        bail!("git fetch failed in {}", submodule_path.display());
    }

    let checkout = std::process::Command::new("git")
        .args(["-C", path(submodule_path)?, "checkout", "--detach", sha])
        .status()
        .context("failed to run git checkout")?;

    if !checkout.success() {
        bail!(
            "git checkout {sha} failed in {}",
            submodule_path.display()
        );
    }

    Ok(())
}

fn path(p: &Path) -> Result<&str> {
    p.to_str()
        .with_context(|| format!("path is not valid UTF-8: {}", p.display()))
}
