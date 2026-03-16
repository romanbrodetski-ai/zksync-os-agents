use anyhow::{Context, Result};
use std::path::Path;

/// Runs a `codex` session rooted in `agent_dir`, blocking until it exits.
/// Inherits the terminal. Codex has no --append-system-prompt flag, so `system_ctx`
/// is prepended to the prompt instead.
pub fn run(agent_dir: &Path, system_ctx: &str, prompt: &str, model: Option<&str>) -> Result<()> {
    let full_prompt = if system_ctx.is_empty() {
        prompt.to_string()
    } else {
        format!("{system_ctx}\n\n{prompt}")
    };

    let mut cmd = std::process::Command::new("codex");
    cmd.current_dir(agent_dir)
        .arg("--dangerously-bypass-approvals-and-sandbox")
        .arg(&full_prompt);
    if let Some(m) = model {
        cmd.args(["--model", m]);
    }
    let status = cmd.status().context("failed to run codex")?;

    if !status.success() {
        anyhow::bail!("codex exited with {status}");
    }
    Ok(())
}
