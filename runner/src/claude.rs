use anyhow::{bail, Result};
use std::os::unix::process::CommandExt;
use std::path::Path;

/// Runs an interactive `claude` session rooted in `agent_dir` and waits for it
/// to finish. Use this when another step must follow after the session ends.
pub fn run_claude(agent_dir: &Path, session_name: &str, system_ctx: &str, prompt: &str) -> Result<()> {
    let status = std::process::Command::new("claude")
        .current_dir(agent_dir)
        .arg("--name")
        .arg(session_name)
        .arg("--append-system-prompt")
        .arg(system_ctx)
        .arg(prompt)
        .status()
        .map_err(|e| anyhow::anyhow!("failed to spawn claude: {e}"))?;

    if !status.success() {
        bail!("claude session exited with {status}");
    }
    Ok(())
}

/// Replaces the current process with an interactive `claude` session rooted in
/// `agent_dir`. Use this for the final step where nothing follows.
pub fn exec_claude(agent_dir: &Path, session_name: &str, system_ctx: &str, prompt: &str) -> ! {
    let err = std::process::Command::new("claude")
        .current_dir(agent_dir)
        .arg("--name")
        .arg(session_name)
        .arg("--append-system-prompt")
        .arg(system_ctx)
        .arg(prompt)
        .exec();

    panic!("failed to exec claude: {err}");
}
