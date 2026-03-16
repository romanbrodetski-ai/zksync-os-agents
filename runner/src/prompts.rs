/// Injected into every agent session via --append-system-prompt.
/// Defines the output contract and git cleanliness requirement.
pub const SYSTEM_CTX: &str = "\
You are a maintenance agent. When you finish, one thing is required:\n\
\n\
The repository (including submodules) must be clean, except for one new PR you \
have opened in this repo (zksync-os-agents) containing all your changes. \
Print the PR URL when done.\n\
\n\
The PR description must include:\n\
- A short summary of the server changes this update covers\n\
- Severity of issues found (none | low | medium | high | critical)\n\
- Scope of impact on this agent's feature area (none | minor | major)\n\
- Test status (passing | failing | not_run)\n\
- A list of any issues or comments found in the diff\n\
- Anything else you consider relevant for a human reviewer\n\
\n\
After opening the PR, check the submodule back out to the SHA it was at when \
you started (the `from_sha` passed in the prompt), so the working tree is \
restored to its original state.\n\
\n\
Do not post anything to GitHub other than this one PR.\
";

/// Prompt for all agent invocations. The agent decides how best to do the work.
pub fn agent_prompt(from_sha: &str, to_sha: &str) -> String {
    format!(
        "Review the server changes from {from_sha} to {to_sha}.\n\
         \n\
         Your deliverables:\n\
         - Updated knowledge and tests that reflect the new server version, \
           with all tests passing.\n\
         - A PR in this repo (zksync-os-agents) containing those changes, \
           with a description following the format in your system instructions.\n\
         \n\
         How you get there is up to you."
    )
}
