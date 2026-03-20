use anyhow::{Context, Result};
use std::path::Path;
use std::time::Duration;

/// Returns the URL of the first PR in `server_repo` that contains `head_sha`, if any.
/// Uses GitHub's commits-to-pulls API endpoint.
pub fn find_server_pr_url(server_repo: &str, head_sha: &str) -> Option<String> {
    let out = std::process::Command::new("gh")
        .args([
            "api",
            &format!("repos/{server_repo}/commits/{head_sha}/pulls"),
            "--jq",
            ".[0].html_url // empty",
        ])
        .output()
        .ok()?;

    if !out.status.success() {
        return None;
    }

    let s = String::from_utf8(out.stdout).ok()?;
    let s = s.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

/// Returns the URL of the most recently created open PR in the repo containing `dir`.
pub fn latest_open_pr_url(dir: &Path) -> Result<Option<String>> {
    let out = std::process::Command::new("gh")
        .current_dir(dir)
        .args([
            "pr",
            "list",
            "--state",
            "open",
            "--json",
            "url,createdAt",
            "--jq",
            "max_by(.createdAt) | .url // empty",
        ])
        .output()
        .context("failed to run gh pr list")?;

    let s = String::from_utf8(out.stdout)?.trim().to_string();
    Ok(if s.is_empty() { None } else { Some(s) })
}

/// Prepends a run-metadata block to the PR's body and prefixes the title with `[{bot_name}]`.
pub fn prepend_pr_metadata(
    pr_url: &str,
    bot_name: &str,
    model: Option<&str>,
    duration: Duration,
    server_pr_url: Option<String>,
) -> Result<()> {
    let title = gh_pr_field(pr_url, "title")?;
    let body = gh_pr_field(pr_url, "body")?;

    let mins = duration.as_secs() / 60;
    let secs = duration.as_secs() % 60;

    let server_line = match server_pr_url {
        Some(ref url) => format!("[link]({url})"),
        None => "N/A".to_string(),
    };

    let model_str = model.unwrap_or("default");

    let new_body = format!(
        "**Agent**: {bot_name}  \n\
         **Model**: {model_str}  \n\
         **Duration**: {mins}m {secs}s  \n\
         **Server PR**: {server_line}  \n\
         \n\
         ---\n\
         \n\
         {body}"
    );

    let new_title = format!("[{bot_name}] {title}");

    let status = std::process::Command::new("gh")
        .args([
            "pr", "edit", pr_url, "--title", &new_title, "--body", &new_body,
        ])
        .status()
        .context("failed to run gh pr edit")?;

    if !status.success() {
        anyhow::bail!("gh pr edit failed for {pr_url}");
    }

    println!("Updated PR: {pr_url}");
    Ok(())
}

fn gh_pr_field(pr_url: &str, field: &str) -> Result<String> {
    let jq = format!(".{field}");
    let out = std::process::Command::new("gh")
        .args(["pr", "view", pr_url, "--json", field, "--jq", &jq])
        .output()
        .context("failed to run gh pr view")?;

    if !out.status.success() {
        anyhow::bail!(
            "gh pr view failed for {pr_url}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    Ok(String::from_utf8(out.stdout)?
        .trim_end_matches('\n')
        .to_string())
}
