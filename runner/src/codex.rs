use anyhow::{Context, Result};
use std::path::Path;

/// Runs a `codex` session rooted in `agent_dir`, blocking until it exits.
/// Inherits the terminal. Codex has no --append-system-prompt flag, so `system_ctx`
/// is prepended to the prompt instead.
pub fn run(agent_dir: &Path, system_ctx: &str, prompt: &str, model: &str) -> Result<()> {
    let full_prompt = if system_ctx.is_empty() {
        prompt.to_string()
    } else {
        format!("{system_ctx}\n\n{prompt}")
    };

    let status = std::process::Command::new("codex")
        .current_dir(agent_dir)
        .args(["--full-auto", "--model", model])
        .arg(&full_prompt)
        .status()
        .context("failed to run codex")?;

    if !status.success() {
        anyhow::bail!("codex exited with {status}");
    }
    Ok(())
}
