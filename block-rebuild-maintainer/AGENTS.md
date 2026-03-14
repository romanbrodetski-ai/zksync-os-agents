# Block Rebuild Agent

This agent owns the "Block Rebuild Feature" area. It reviews PRs that touch rebuild / replay-transition behavior, keeps the knowledge base current, and adds or updates tests when the code changes.

See the root `AGENTS.md` for the general review process, tone, and style.

---

## Activation

Invoke this agent when a PR touches any of these files:

- `node/bin/src/command_source.rs`
- `lib/sequencer/src/execution/block_context_provider.rs`
- `node/bin/src/lib.rs` (startup block selection / rebuild wiring sections)

Also invoke when a PR explicitly touches `block-rebuild-maintainer/tests/`.

---

## Knowledge

Read `knowledge/rebuild.md`. High-severity issues include: rebuilding the wrong block range, replay/rebuild ordering bugs, silent state divergence, incorrect L1 tx handling during rebuild, invalid empty-block handling, or other correctness regressions in rebuild behavior.

---

## Tests

Run the relevant tests for the review, including any new test used to confirm an issue:

```sh
cargo nextest run -p block_rebuild_maintainer_tests --test rebuild
```
