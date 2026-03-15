/// Path (relative to the agent directory) where the agent writes its structured output.
pub const OUTPUT_FILE: &str = "runner-output.json";

/// Injected into every agent session via --append-system-prompt.
/// Defines the output contract and git cleanliness requirement.
pub const SYSTEM_CTX: &str = "\
You are a maintenance agent. When you finish, two things are required:\n\
\n\
1. The repository (including submodules) must be clean and on the same commit \
as when you started, except for one new PR you have opened in this repo \
(zksync-os-agents) containing all your changes. Print the PR URL.\n\
\n\
2. Write `runner-output.json` in the current directory with exactly this schema:\n\
{\n\
  \"pr_url\": \"<URL of the PR you opened>\",\n\
  \"comments\": [\n\
    {\n\
      \"file\": \"<path relative to zksync-os-server>\",\n\
      \"lines\": \"<start>-<end>\",\n\
      \"issue\": \"<one-sentence summary>\",\n\
      \"detail\": \"<what breaks, when, correct behaviour>\",\n\
      \"suggestion\": \"<concrete fix or open question>\"\n\
    }\n\
  ],\n\
  \"severity\": \"<none|low|medium|high|critical>\",\n\
  \"scope\": \"<none|minor|major>\",\n\
  \"tests_status\": \"<passing|failing|not_run>\",\n\
  \"summary\": \"<one paragraph suitable for forwarding to a human>\"\n\
}\n\
\n\
`severity` is the worst issue found. `scope` is how much of this agent's feature \
area the diff touches. `comments` is empty if no issues were found. \
Do not post anything to GitHub other than the one PR.\
";

/// Prompt for all agent invocations. The agent decides how best to do the work.
pub fn agent_prompt(from_sha: &str, to_sha: &str) -> String {
    format!(
        "Review the server changes from {from_sha} to {to_sha}.\n\
         \n\
         Your deliverables:\n\
         - Updated knowledge and tests that reflect the new server version, \
           with all tests passing.\n\
         - A PR in this repo (zksync-os-agents) containing those changes.\n\
         - Comments on any high-severity issues found in the diff \
           (as defined in AGENTS.md), captured in `{OUTPUT_FILE}`.\n\
         \n\
         How you get there is up to you. When done, follow the output \
         requirements in your system instructions."
    )
}
