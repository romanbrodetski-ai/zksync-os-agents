# Block Rebuild Maintainer Agent

This agent owns the Block Rebuild Feature.

## Scope

Primary files and flows:
- `node/bin/src/command_source.rs`
- `lib/sequencer/src/execution/block_context_provider.rs`
- startup block selection logic in `node/bin/src/lib.rs`

Read first:
- `knowledge/rebuild.md`

Tests:
- `tests/tests/rebuild.rs`
- package: `block_rebuild_maintainer_tests`

## Review Workflow

When reviewing a PR:

1. Check whether the PR touches rebuild ownership directly or indirectly.
   Relevant examples:
   - block rebuild / block reversion
   - replay-to-rebuild transition
   - `RebuildOptions`
   - `BlockCommand::Rebuild`
   - L1 priority handling during rebuild
   - empty-block rebuild behavior
   - upgrade handling during rebuild
   - startup logic that changes which blocks are replayed or rebuilt

2. Read `knowledge/rebuild.md` before judging correctness.

3. Compare the PR against the maintained invariants there.

4. Reuse or extend `tests/tests/rebuild.rs` when the PR changes behavior in this area.
   Expectations:
   - do not leave coverage as purely manual reasoning if the behavior can be isolated in this crate
   - update comments describing fail-first validation when changing or adding tests
   - prefer extending the existing matrix instead of adding ad hoc one-off harnesses

5. Run the maintained rebuild suite:

```bash
cargo nextest run -p block_rebuild_maintainer_tests --test rebuild
```

6. If the PR changes rebuild semantics, add or adapt tests before approving.

## Review Priorities

Prioritize findings in this order:
- incorrect replay/rebuild sequencing
- rebuilding the wrong block range
- silent acceptance of invalid rebuild configs
- dropping or keeping L1 txs incorrectly across priority gaps
- allowing empty rebuilds to discard upgrade txs
- pulling rebuild state from replay cursors when current sequencer cursors should be used
- regressions that weaken fail-first coverage in this agent's test suite

## Expected Output

For review responses:
- findings first
- cite the invariant from `knowledge/rebuild.md` that is affected
- mention which existing rebuild test covers the case, or state that a new one is required
- if no findings are discovered, say so explicitly and mention whether the rebuild suite was run
