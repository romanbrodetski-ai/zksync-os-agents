/// Path (relative to the agent directory) where the agent writes its structured output.
pub const OUTPUT_FILE: &str = "runner-output.json";

/// Injected into every agent session via --append-system-prompt.
/// Defines the output contract: file path and JSON schema.
pub fn system_ctx() -> String {
    format!(
        "When you have finished your work, write a JSON file to `{OUTPUT_FILE}` \
         in the current directory. Use exactly this schema — no extra fields:\n\
         \n\
         {{\n\
           \"comments\": [\n\
             {{\n\
               \"file\": \"<path relative to zksync-os-server>\",\n\
               \"lines\": \"<start>-<end>\",\n\
               \"issue\": \"<one-sentence summary>\",\n\
               \"detail\": \"<what breaks, when, correct behaviour>\",\n\
               \"suggestion\": \"<concrete fix or open question>\"\n\
             }}\n\
           ],\n\
           \"severity\": \"<none|low|medium|high|critical>\",\n\
           \"scope\": \"<none|minor|major>\",\n\
           \"tests_status\": \"<passing|failing|not_run>\",\n\
           \"summary\": \"<one short paragraph suitable for forwarding to a human>\"\n\
         }}\n\
         \n\
         `severity` is the worst issue found. \
         `scope` is how much of this agent's feature area the diff touches. \
         `comments` is empty if no issues were found. \
         Do not post anything to GitHub or take any other external action \
         other than the PR described in your instructions."
    )
}

/// Single prompt for all agent invocations: examine the diff from `from_sha` to `to_sha`,
/// update knowledge and tests, create a PR in this repo, write structured output.
pub fn agent_prompt(from_sha: &str, to_sha: &str) -> String {
    format!(
        "Examine the server changes from {from_sha} to {to_sha}, update this agent's \
         knowledge and tests accordingly, and open a PR in this repo with the results.\n\
         \n\
         Steps:\n\
         1. In `zksync-os-server/`: `git fetch origin` then `git checkout --detach {to_sha}`.\n\
         2. Read what changed: `git log --oneline {from_sha}..{to_sha}` and the relevant diffs.\n\
         3. Update `knowledge/` to reflect the new server version.\n\
         4. Update `tests/` so they compile and pass against the new code.\n\
         5. Run the test suite (command in AGENTS.md) and confirm all tests pass.\n\
         6. Create a branch in this repo, commit `knowledge/`, `tests/`, and the bumped \
            submodule pointer atomically, push, and open a PR. The PR description should \
            include the summary and any comments from step 7.\n\
         7. Write `{OUTPUT_FILE}` with your findings (issues found, severity, scope).",
    )
}
