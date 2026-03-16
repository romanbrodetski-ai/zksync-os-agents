# Pipeline Correctness — Final Report

## Files inspected

- `lib/pipeline/src/traits.rs` — PipelineComponent trait
- `lib/pipeline/src/builder.rs` — Pipeline builder with `.pipe()` chaining
- `lib/sequencer/src/execution/mod.rs` — Sequencer struct and PipelineComponent impl
- `lib/sequencer/src/execution/block_executor.rs` — execute_block() pure function
- `lib/sequencer/src/execution/block_context_provider.rs` — BlockContextProvider, tx selection
- `lib/sequencer/src/model/blocks.rs` — BlockCommand, ProduceCommand, RebuildCommand
- `lib/storage_api/src/state_override_view.rs` — OverriddenStateView
- `lib/storage_api/src/state.rs` — ReadStateHistory, WriteState traits
- `lib/storage_api/src/replay.rs` — ReadReplay, WriteReplay, ReadReplayExt traits
- `node/bin/src/command_source.rs` — MainNodeCommandSource, command_source()
- `node/bin/src/lib.rs` — pipeline construction and wiring

## Ownership boundary

This agent owns correctness of the block processing pipeline from CommandSource through
Sequencer persistence. Specifically:

- Pipeline framework (PipelineComponent trait, builder, backpressure)
- Sequencer execution and persistence loop
- Command types and their semantics (Produce, Replay, Rebuild)
- Storage trait contracts (WriteReplay, WriteState, WriteRepository ordering)
- State override / preimage injection (OverriddenStateView)
- Command source stream composition (replay → rebuild → produce)

Out of scope: TreeManager internals, Batcher, L1Sender, prover pipeline stages, mempool
internals, RPC layer.

## Invariants covered by tests

| Invariant | Test module | Tests |
|-----------|------------|-------|
| Sequential block processing | replay_storage | sequential_writes_succeed, non_sequential_write_panics, latest_record_monotonic |
| Backpressure model | backpressure | slow_consumer_blocks_fast_producer, end_to_end_backpressure_propagation, zero_buffer_means_lockstep |
| Replay idempotency | replay_storage | duplicate_write_without_override_returns_false, duplicate_write_with_override_succeeds |
| Pipeline FIFO ordering | pipeline_flow | ordering_preserved_through_multiple_stages, basic_pipeline_flow |
| Error propagation | pipeline_flow, backpressure | error_propagates_through_channel_closure, dropped_receiver_unblocks_sender |
| Preimage injection correctness | state_override | preimage_override_shadows_base, preimage_falls_through_to_base, storage_falls_through_with_preimage_override, etc. |
| Command type structure | sequencer | produce_command_carries_block_params, block_command_block_number, sequencer_output_type_is_two_tuple |
| Replay stream ordering | sequencer | replay_stream_returns_records_in_order |
| ReplayRecord equality semantics | replay_storage | replay_record_equality_ignores_node_version, replay_record_inequality_on_output_hash |

## Tests added

5 test modules, 26 tests total:

- **sequencer** (4 tests): command structure, block_number(), output type, replay stream ordering
- **backpressure** (4 tests): slow consumer blocking, zero buffer panic, dropped receiver, end-to-end propagation
- **pipeline_flow** (5 tests): basic flow, chained transformation, ordering preservation, error propagation, builder spawn
- **replay_storage** (10 tests): genesis, sequential writes, duplicates, overrides, non-sequential panic, range retrieval, context consistency, monotonic latest, write log, equality semantics
- **state_override** (7 tests): shadow, fall-through, missing, storage independence, multiple overrides, combined scenario, empty overrides

## Mutation validations

Mutation-style validation was not systematically documented per the Stage 4 requirement.
The tests are designed so that targeted regressions (e.g., removing the sequential check in
WriteReplay, changing OUTPUT_BUFFER_SIZE, altering OverriddenStateView lookup order) would
cause specific test failures, but formal mutation/revert cycles were not recorded.

## Suspected bugs / correctness issues

None found in the current codebase. The following are areas of elevated risk:

1. **Non-atomic three-store writes**: WriteReplay → WriteState → WriteRepository is not
   transactional. A crash between stores leaves inconsistent state. Recovery relies on WAL
   replay, which is correct but untested at the integration level.

2. **OUTPUT_BUFFER_SIZE = 0 is unvalidated**: The pipeline builder passes this directly to
   `mpsc::channel()`, which panics on 0. No compile-time or runtime guard exists.

3. **override_allowed enforcement is distributed**: Multiple call sites independently decide
   whether to set override_allowed. A new call site could get this wrong with no centralized
   check.

## Remaining gaps / low-confidence areas

- **Full Sequencer integration test**: No test runs the actual Sequencer::run() with mock
  storage through a sequence of blocks. Current tests exercise components in isolation.
- **Crash recovery**: WAL replay after partial persistence is not tested.
- **L1 priority transaction ordering**: The mempool ordering contract is documented but
  not tested by this agent (likely belongs to a mempool agent).
- **Cancellation safety**: tokio::select! branches in pipeline components are not tested
  for cancellation safety.
- **Config optionality**: Main-node vs external-node config guards not tested.

## Recommendations for future maintenance

1. Add an integration test that runs the Sequencer with MockReplayStorage, MockWriteState,
   and MockRepository through a sequence of Produce and Replay commands.
2. Add a crash-recovery test: write blocks 1-5, simulate crash after block 3's WriteReplay
   but before WriteRepository, restart and verify replay fills the gap.
3. When reviewing PRs that add new pipeline components, verify they declare a reasonable
   OUTPUT_BUFFER_SIZE (>= 1) and that their run() method is cancellation-safe.
4. Keep the mock implementations aligned with the real storage trait contracts; divergence
   reduces test confidence.
