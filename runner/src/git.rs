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

fn path(p: &Path) -> Result<&str> {
    p.to_str()
        .with_context(|| format!("path is not valid UTF-8: {}", p.display()))
}
