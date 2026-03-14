# Pipeline Correctness Agent

This agent owns the "Pipeline Execution Correctness" area. It reviews PRs that touch the block processing pipeline, overlay storage, canonization, or backpressure behavior. It keeps the knowledge base current and adds or updates tests when the code changes.

See the root `AGENTS.md` for the general review process, tone, and style.

---

## Activation

Invoke this agent when a PR touches any file from the in-scope list in `knowledge/pipeline-correctness.md`:

- `lib/pipeline/src/traits.rs`
- `lib/pipeline/src/builder.rs`
- `lib/sequencer/src/execution/block_executor.rs`
- `lib/sequencer/src/execution/block_applier.rs`
- `lib/sequencer/src/execution/block_canonizer.rs`
- `lib/sequencer/src/consensus/`
- `lib/storage_api/src/overlay_buffer.rs`
- `lib/storage_api/src/state_override_view.rs`
- `lib/storage_api/src/state.rs`
- `lib/storage_api/src/replay.rs`
- `node/bin/src/command_source.rs`
- `node/bin/src/lib.rs` (pipeline wiring sections)

Also invoke when a PR explicitly touches `pipeline-correctness/tests/`.

---

## Knowledge

Read `knowledge/pipeline-correctness.md`. High-severity issues include: incorrect block execution, stale state reads, overlay corruption, broken backpressure, consensus fence bypass, or other correctness regressions (sequential processing, canonization fence, backpressure, L1 priority ordering, replay idempotency, gapless commitment).

---

## Additional Step: Run Tests Before Identifying Issues

After cross-referencing knowledge, run the relevant tests against the PR branch before drafting comments:

```sh
cargo nextest run -p pipeline_correctness_tests
```

Or run specific modules:

```sh
cargo nextest run -p pipeline_correctness_tests --test-threads 1 -- overlay
cargo nextest run -p pipeline_correctness_tests --test-threads 1 -- canonization
cargo nextest run -p pipeline_correctness_tests --test-threads 1 -- backpressure
cargo nextest run -p pipeline_correctness_tests --test-threads 1 -- pipeline_flow
cargo nextest run -p pipeline_correctness_tests --test-threads 1 -- replay_storage
```

If tests fail on the PR branch but pass on base, the PR likely breaks an invariant. If tests fail on both, the test suite has drifted — investigate and fix.
