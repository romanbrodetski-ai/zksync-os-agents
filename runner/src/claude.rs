use std::os::unix::process::CommandExt;
use std::path::Path;

/// Replaces the current process with an interactive `claude` session rooted in
/// `agent_dir`. The `system_ctx` string is appended to Claude's system prompt
/// (on top of whatever CLAUDE.md provides), and `prompt` becomes the opening
/// user message — giving the agent its full operational context before the
/// interactive chat starts.
pub fn exec_claude(agent_dir: &Path, session_name: &str, system_ctx: &str, prompt: &str) -> ! {
    let err = std::process::Command::new("claude")
        .current_dir(agent_dir)
        .arg("--name")
        .arg(session_name)
        .arg("--append-system-prompt")
        .arg(system_ctx)
        .arg(prompt)
        .exec(); // replaces the current process; only returns on error

    panic!("failed to exec claude: {err}");
}
