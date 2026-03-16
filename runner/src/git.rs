use anyhow::{bail, Context, Result};
use std::path::Path;

/// Initializes the submodule if it hasn't been initialized yet.
pub fn ensure_submodule_initialized(repo_root: &Path, submodule_path: &Path) -> Result<()> {
    if !submodule_path.join(".git").exists() {
        println!("Initializing submodule {}…", submodule_path.display());
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(repo_root)
            .args(["submodule", "update", "--init", "--"])
            .arg(submodule_path)
            .status()
            .context("failed to run git submodule update --init")?;
        if !status.success() {
            bail!("failed to initialize submodule {}", submodule_path.display());
        }
    }
    Ok(())
}

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

/// Fetches origin and resolves a git ref (branch, tag, or SHA) to a concrete SHA.
pub fn resolve_ref(submodule_path: &Path, git_ref: &str) -> Result<String> {
    git_status(submodule_path, &["fetch", "origin"])?;
    // Try as a remote branch first (picks up the freshly-fetched ref), then as-is (SHA, tag).
    git(submodule_path, &["rev-parse", &format!("origin/{git_ref}")])
        .or_else(|_| git(submodule_path, &["rev-parse", git_ref]))
        .with_context(|| format!("could not resolve ref '{git_ref}'"))
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

/// Prints a human-readable summary of the diff between `from` and `to`:
/// commits on each side (with dates) and per-file change stats.
pub fn print_diff_summary(submodule_path: &Path, from: &str, to: &str) -> Result<()> {
    // Symmetric log: commits in from but not to (<) and in to but not from (>).
    println!("\nUpdating {} → {}", &from[..from.len().min(12)], &to[..to.len().min(12)]);
    println!("\n=== Commits ===");
    let log = git(
        submodule_path,
        &[
            "log",
            "--left-right",
            "--date=short",
            "--format=%m %ad %h %an: %s",
            &format!("{from}...{to}"),
        ],
    )?;
    if log.is_empty() {
        println!("(no commits differ)");
    } else {
        // Label the arrows so the reader knows which direction is which.
        println!("< = only in {}", &from[..from.len().min(12)]);
        println!("> = only in {}", &to[..to.len().min(12)]);
        println!("{log}");
    }

    // Diff stats: files changed, insertions, deletions.
    println!("\n=== Changed files ===");
    let stat = git(submodule_path, &["diff", "--stat", &format!("{from}..{to}")])?;
    if stat.is_empty() {
        println!("(no file changes)");
    } else {
        println!("{stat}");
    }

    println!();
    Ok(())
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
