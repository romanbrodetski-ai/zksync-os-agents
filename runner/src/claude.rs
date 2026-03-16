use anyhow::{Context, Result};
use std::path::Path;

/// Runs an interactive `claude` session rooted in `agent_dir`, blocking until it exits.
/// Inherits the terminal (stdin/stdout/stderr). `system_ctx` is appended to the system
/// prompt on top of CLAUDE.md; `prompt` becomes the opening user message.
pub fn run(agent_dir: &Path, system_ctx: &str, prompt: &str, model: Option<&str>) -> Result<()> {
    let mut cmd = std::process::Command::new("claude");
    cmd.current_dir(agent_dir)
        .arg("--dangerously-skip-permissions")
        .args(["--append-system-prompt", system_ctx])
        .arg(prompt);
    if let Some(m) = model {
        cmd.args(["--model", m]);
    }
    let status = cmd.status().context("failed to run claude")?;

    if !status.success() {
        anyhow::bail!("claude exited with {status}");
    }
    Ok(())
}
