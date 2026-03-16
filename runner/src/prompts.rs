/// Injected into every agent session via --append-system-prompt.
/// Defines the output contract and git cleanliness requirement.

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

pub const NEW_AGENT_PROMPT: &str = "
#Prepare environment for an agent that continuously maintains code related to feature X (“Block rebuild/revert functionality”).

Create a folder `agents/<agent_name>/` with:
- `knowledge/overview.md`
- `knowledge/test-plan.md`
- `knowledge/final-report.md`
- `tests/` containing a crate that may depend on any crates in this repo

Your goal is to build a high-confidence integration test suite for feature X, together with a concise and useful knowledge base for future maintenance.

Operate in stages. Do not skip stages. Stop at the required checkpoints for human feedback.

## Stage 1 — Recon and scope definition

Read all code that is plausibly relevant to feature X.

Identify:
- feature entrypoints
- direct implementation files
- callers / upstream users
- key structs / enums / state involved in X
- state transitions and business invariants
- config / feature flags / environment dependencies
- persistence, external side effects, and interactions with other subsystems
- existing tests and current coverage gaps
- adjacent areas that may look related but may actually be outside the ownership boundary of this agent

Write `knowledge/overview.md` as a concise feature model. Keep it compact, but include:
- what feature X is supposed to do
- what is definitely in scope
- what may be out of scope
- key invariants / business rules
- important edge cases and failure modes
- ambiguities or suspicious areas

At the end of Stage 1, STOP and present a short review packet for the human containing:
1. Proposed ownership boundary for this agent
2. Relevant files / modules
3. Top invariants and behaviors
4. Open questions / ambiguities
5. Initial testing strategy outline

Do not write tests yet. Wait for human feedback before proceeding.

## Stage 2 — Test plan

Incorporate the human feedback from Stage 1.

Create `knowledge/test-plan.md` with a concrete test matrix for feature X.
Group tests by categories such as:
- happy path
- boundary conditions
- invalid input / rejected transitions
- retry / recovery behavior
- idempotency / duplication
- ordering / sequencing
- persistence / restart effects
- config-dependent behavior
- known bug-prone or ambiguous cases

For each planned test, briefly state:
- what invariant / behavior it protects
- what observable outcome it checks
- what minimal plausible regression should make it fail

At the end of Stage 2, STOP and present a short review packet for the human containing:
1. Test categories
2. Concrete planned tests
3. Which invariants each test covers
4. Areas of low confidence or incomplete understanding
5. Any cases that seem important but hard to test well

Do not implement tests yet. Wait for human approval before proceeding.
The agent may add unit tests where they materially improve precision, speed, or diagnosability,
but the main confidence for feature X should come from higher-level behavioral tests.
## Stage 3 — Test implementation

Implement the tests in `agents/<agent_name>/tests/`.

You may reuse the harness from `integration-tests/` if useful, or build a separate harness if needed.
These tests are not optimized for human maintenance; prioritize confidence, coverage of distinct behaviors, and diagnostic value over elegance.

Prefer assertions on externally observable behavior and durable invariants, rather than incidental implementation details.

While implementing tests, you may refine `knowledge/overview.md` and `knowledge/test-plan.md`, but keep both coherent and not overly verbose.

## Stage 4 — Fail-first validation

For every new test, perform mutation-style validation:

- temporarily introduce the smallest plausible regression in feature X that should violate the tested behavior
- run the test and confirm it fails
- revert the mutation
- run the test again and confirm it passes

In each test, or in a nearby companion note, document briefly:
- the temporary mutation used
- why that mutation should break the invariant
- that the mutation was reverted

Do not leave production code modified.

## Stage 5 — Final audit and report

Write `knowledge/final-report.md` including:
- files inspected
- final ownership boundary used
- invariants covered
- tests added
- mutation validations performed
- suspected bugs / correctness issues found
- any tests still failing and why
- remaining gaps / low-confidence areas
- recommendations for future maintenance of this agent

If you discover a correctness issue:
- isolate it with the smallest possible reproducing test
- do not weaken or delete the test just to make the suite pass
- clearly report it in `knowledge/final-report.md`

## General constraints

- Optimize for confidence and coverage of distinct behaviors, not raw number of tests.
- Avoid redundant tests unless they protect meaningfully different invariants.
- Prefer strong behavioral assertions over weak implementation-detail assertions.
- Do not mock away the core behavior under test unless unavoidable.
- If behavior is ambiguous, document the ambiguity and test the interpretation best supported by the code and human feedback.
- The knowledge files should stay concise and readable as a whole; periodically refactor them to remove redundancy.
- These tests are intended to be maintained primarily by the agent, not by humans, so prioritize precision and regression-catching power over style.

Think like a maintenance engineer and a mutation tester:
first understand the feature as a system,
then define its ownership boundary,
then write tests that would catch realistic regressions.
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

        As a reference, see the prompt that is used for creating agents.
        While working on the PR, keep these principles in mind. If you see that some of them have degraded,
        You can make corresponding changes within the PR:
        ```
        {NEW_AGENT_PROMPT}
        ```
     "
    )
}
