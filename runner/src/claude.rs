use std::os::unix::process::CommandExt;
use std::path::Path;

/// Replaces the current process with an interactive `claude` session rooted in
/// `agent_dir`. `system_ctx` is appended to the system prompt (on top of CLAUDE.md);
/// `prompt` becomes the opening user message.
pub fn exec(agent_dir: &Path, session_name: &str, system_ctx: &str, prompt: &str) -> ! {
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
