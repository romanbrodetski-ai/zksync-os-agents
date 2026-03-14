# ZKsync OS Agents

This repo contains maintenance agents that review PRs, keep knowledge bases current, and maintain test coverage for specific feature areas of the ZKsync OS node.

## Available Agents

| Agent | Directory | Area |
|---|---|---|
| `block-rebuild-maintainer` | `block-rebuild-maintainer/` | Block rebuild / replay-transition behavior |
| `l1-settle` | `l1-settle/` | Settling batches on L1 |
| `pipeline-correctness` | `pipeline-correctness/` | Block processing pipeline correctness |

## Repository Structure

Each agent lives in its own subdirectory and owns a specific feature area. An agent directory contains:

- `AGENTS.md` — activation criteria, knowledge pointers, and test commands specific to this agent
- `knowledge/` — curated facts, invariants, and edge cases about the feature area
- `tests/` — Rust integration tests that encode those invariants as runnable checks
- `zksync-os-server/` — a git submodule pinned to the server commit the knowledge and tests were generated against

The submodule is the versioning anchor: knowledge and tests are always consistent with a specific server SHA. When the server evolves, the agent bumps the submodule and updates knowledge/tests atomically in one commit. This makes it easy to see exactly what server changes triggered a knowledge refresh (`git log` on the submodule) and to check whether an agent is stale (compare submodule SHA to the base branch HEAD).

All agent workspaces share a single `target/` directory at the repo root (configured via `.cargo/config.toml`) to avoid redundant compilation across agents.

---

## Running an Agent

Specify the agent when invoking:

> "Run the `<agent-name>` agent on PR #\<number\>"

Each agent's `AGENTS.md` lists which PRs should activate it (in-scope files and directories).

---

## Review Process

### Step 0 — Sync the agent's server submodule

Each agent directory contains a `zksync-os-server` submodule pinned to the commit its knowledge base and tests were generated against.

1. Ensure `../zksync-os-server` (the shared working checkout) is clean — escalate to the user if it has uncommitted changes.
2. Check out the PR's **base branch** in `../zksync-os-server`.
3. From the agent's directory, run `git submodule status` and compare the submodule SHA to `../zksync-os-server`'s HEAD. If they differ, update the knowledge base and tests against the new version, then commit `knowledge/` + the bumped submodule pointer atomically.

### Step 1 — Read the diff in the target repo

Check out the **PR head commit** in `../zksync-os-server` (it was on the base branch after Step 0). Get the diff with `gh pr diff <number> -R matter-labs/zksync-os-server` and read every changed file in full before forming a judgment.

### Step 2 — Cross-reference knowledge

Read the agent's knowledge files (listed in the agent's `AGENTS.md`). Check whether the diff:

- Violates any invariant listed there.
- Introduces a new edge case not listed there.
- Breaks an assumption that existing tests rely on.

### Step 3 — Confirm issues

Use the agent's tests to confirm and isolate suspected regressions before drafting comments. Prefer extending an existing test; add a focused new one when needed.

To run tests against the PR code: temporarily check out the PR head inside the submodule (`git -C zksync-os-server checkout <pr-head-sha>`), run the tests, then restore (`git -C zksync-os-server checkout <submodule-sha>`). Tests passing on base but failing on PR head indicate a regression.

Only raise **high-severity** issues (see the agent's `AGENTS.md` for what counts). Skip style, naming, and low-impact concerns.

If a potential issue cannot be confirmed locally and needs more context, ask the user rather than guessing.

### Step 4 — Draft comments

Write a draft for each issue in the following format:

```text
File: <path>
Line(s): <range>
Issue: <one-sentence summary>
Detail: <technical explanation — what breaks, when, what the correct behaviour should be>
Suggestion: <concrete fix or question to resolve>
```

Send all drafts to the user at once. Do not post anything to GitHub yet.

### Step 5 — Confirm and publish

Wait for the user to confirm, edit, or discard each draft. Only publish confirmed comments.

### Step 6 — Update knowledge and tests

After the review, if the PR:

- **Adds new behavior in scope**: add a test and update the relevant knowledge file.
- **Changes an existing invariant**: update the knowledge file and the affected test.
- **Exposes a gap in coverage not worth testing now**: append a short deferred note to the knowledge file.

Run the agent's test suite (command listed in the agent's `AGENTS.md`). Test commands must be run from the agent's own directory. All tests must pass before the review is considered complete.

Create a branch in this repo for these changes. This branch is to be merged to `main` when the PR being reviewed is merged. Open a PR and leave a comment in the target PR with this link and explanation.

---

## Tone and Style

- Be concise and technical. One sentence per issue where possible.
- No hedging. If the code is wrong, say so directly.
- If you are uncertain, say what context you need rather than speculating.
- Do not comment on what is correct — only what is wrong or suspicious.
