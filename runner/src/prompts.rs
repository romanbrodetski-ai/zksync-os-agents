pub const TONE: &str = "
## Tone and Style

- Be concise and technical. One sentence per issue where possible.
- No hedging. If the code is wrong, say so directly.
- If you are uncertain, say what context you need rather than speculating.
- Do not comment on what is correct — only what is wrong or suspicious.
";

pub const ISSUE_ISOLATION: &str  = "
## Isolating and Investigating Issues
 When issue is found or suspected, it is important to isolate it and investigate it.
 Use existent tests or create new ones as needed.
";

/// Prompt for all agent invocations. The agent decides how best to do the work.
pub fn agent_prompt(from_sha: &str, to_sha: &str) -> String {
    format!(
        "Review and update knowledge and tests for the zksync-os-server changes from {from_sha} to {to_sha}.\n\
         Your current knowledge and tests are for {from_sha}. 
        
        You should deliver A PR in this repo (zksync-os-agents) containing the tests and knowledge updates - 
        and the updated submodule `zksync-os-server` to {to_sha}. 
        The PR description should include:
        - A short summary of the server changes this update covers\n\
        - Severity of issues found (none | low | medium | high | critical)\n\
        - Scope of impact on this agent's feature area (none | minor | major)\n\
        - A clear list of any issues or comments found in the diff (the ones you'd put to github PR for such diff)\n\
        - Anything else you consider relevant for a human reviewer\n\
        \
        Do not post anything to GitHub other than this one PR.\
        The goal is to both provide feedback to diff and to update own understanding and tests.
        These knowledge and tests should be used to infer the correctness of the server changes and provide feedback.

        {TONE}\
        {ISSUE_ISOLATION}\
     "
    )
}
