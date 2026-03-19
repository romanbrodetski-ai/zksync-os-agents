use anyhow::{Context, Result};
use std::path::Path;

/// Runs a `codex` session rooted in `agent_dir`, blocking until it exits.
/// Inherits the terminal. `prompt` is passed directly to Codex.
pub fn run(agent_dir: &Path, prompt: &str, model: Option<&str>) -> Result<()> {
    let mut cmd = std::process::Command::new("codex");
    cmd.current_dir(agent_dir)
        .arg("--dangerously-bypass-approvals-and-sandbox")
        .arg(prompt);
    if let Some(m) = model {
        cmd.args(["--model", m]);
    }
    let status = cmd.status().context("failed to run codex")?;

    if !status.success() {
        anyhow::bail!("codex exited with {status}");
    }
    Ok(())
}
