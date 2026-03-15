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
         Do not post anything to GitHub or take any other external action."
    )
}

/// Prompt for syncing the agent from `current_sha` to `target_sha`:
/// update knowledge and tests so they compile, pass, and reflect the new server version.
pub fn sync_prompt(current_sha: &str, target_sha: &str) -> String {
    format!(
        "Update this agent's knowledge and tests from server SHA {current_sha} to {target_sha}.\n\
         \n\
         Steps:\n\
         1. In `zksync-os-server/`: `git fetch origin` then `git checkout --detach {target_sha}`.\n\
         2. Review what changed: `git log --oneline {current_sha}..{target_sha}` and read \
            the relevant diffs.\n\
         3. Update `knowledge/` so it reflects the new server version.\n\
         4. Update `tests/` so they compile and pass against the new code.\n\
         5. Run the test suite (command in AGENTS.md) and confirm all tests pass.\n\
         6. Commit `knowledge/`, `tests/`, and the bumped submodule pointer atomically.\n\
         7. Write `{OUTPUT_FILE}` with your findings.",
    )
}

/// Prompt for reviewing the diff between `base_sha` and `head_sha` in the server repo.
/// The submodule is already at `base_sha` — do not move it.
pub fn review_prompt(base_sha: &str, head_sha: &str) -> String {
    format!(
        "Review the server changes between {base_sha} (base) and {head_sha} (head).\n\
         \n\
         The submodule is already at {base_sha} — do not change it.\n\
         \n\
         Steps:\n\
         1. In `zksync-os-server/`: `git fetch origin` then \
            `git diff {base_sha}..{head_sha}` to read the diff. \
            Read every changed file in full before forming a judgment.\n\
         2. Cross-reference against `knowledge/`. Check for invariant violations, \
            new edge cases, and broken test assumptions.\n\
         3. Use `tests/` to confirm suspected issues. \
            Temporarily `git checkout --detach {head_sha}`, run the suite, \
            then restore with `git checkout --detach {base_sha}`. \
            Failures on head but not base confirm a regression.\n\
         4. Only raise high-severity issues (defined in AGENTS.md). \
            Skip style, naming, and low-impact concerns.\n\
         5. Write `{OUTPUT_FILE}` with all findings.",
    )
}
