# Block Rebuild Feature

Scope:
- `node/bin/src/command_source.rs`
- `lib/sequencer/src/execution/block_context_provider.rs`
- `lib/sequencer/src/execution/block_canonizer.rs`
- startup selection in `node/bin/src/lib.rs`

Core flow:
- Startup chooses the earliest block that must be replayed to restore correctness.
- `ConsensusNodeCommandSource.run()` replays WAL records via `forward_range_with`, then sends `Rebuild` commands for `[rebuild_from_block..=latest_record]`, then enters `run_loop` which produces `Produce` commands only when the node holds the `Leader` role (controlled by `LeadershipSignal`).
- `ProduceCommand` is a unit struct; `block_number`, `block_time`, and `max_transactions_in_block` are held inside `BlockContextProvider` and tracked via `next_block_number` (incremented in `on_canonical_state_change`).
- `BlockContextProvider` converts `Rebuild` into execution-ready commands.
- After WAL replay and rebuilds, the main node pipeline is: `BlockExecutor → BlockCanonizer → BlockApplier`.
- `BlockCanonizer` holds `MAX_PRODUCED_QUEUE_SIZE = 2` in-flight produced/rebuild blocks before applying backpressure. `NoopCanonization`'s internal channel is **unbounded** (`mpsc::unbounded_channel`); backpressure comes from `MAX_PRODUCED_QUEUE_SIZE`, not the channel.
- `ConsensusNodeCommandSource` has `OUTPUT_BUFFER_SIZE = 1`, so only one command is buffered before backpressure from `BlockExecutor`.
- In the `run_loop`, `tokio::select!` handles three arms: leadership changes, inbound canonized replay records (from `BlockCanonizer` via `replays_to_execute`), and producing new blocks (only when Leader).

Important invariants:
- `rebuild_from_block` must be within `[starting_block, latest_record]` (asserted as `rebuild_from_block >= starting_block` and `rebuild_from_block <= last_block_in_wal`).
- `BlockContextProvider` asserts `self.next_block_number == record.block_context.block_number` for Replay commands (out-of-order detection).
- Rebuild keeps the original block number, timestamp, fee params, execution version, protocol version and force preimages from the replay record.
- Rebuild does not reuse the old block hash chain. It uses the provider's current `block_hashes_for_next_block`.
- Rebuild starts from the provider's current cursors:
  - `starting_l1_priority_id`
  - `starting_interop_event_index`
  - `starting_migration_number`
  - `starting_interop_fee_number`
- Empty rebuilds are forbidden when the replay record contains an upgrade tx.
- Empty rebuilds execute no txs.
- Non-empty rebuilds may drop all L1 txs if the first replayed L1 priority id does not match the provider's current `next_l1_priority_id`.
- If the first replayed L1 tx matches, rebuild keeps the full replay tx list.
- Non-empty rebuilds with no L1 txs at all (`first_l1_tx == None`) set `filter_l1_txs = false`, so all transactions (upgrade, interop, etc.) are kept intact.
- `force_preimages` is taken from the replay record unchanged in both empty and non-empty rebuilds.
- Rebuild commands go through consensus canonization (proposed to `BlockCanonizationEngine` in `BlockCanonizer`), same as `Produce` commands; `Replay` commands bypass canonization.

Operational note:
- Rebuild is intentionally more permissive than replay:
  - `SealPolicy::UntilExhausted { allowed_to_finish_early: true }`
  - `InvalidTxPolicy::RejectAndContinue`
  - `expected_block_output_hash = None`

No-rebuild path:
- When `rebuild_options` is `None`, all WAL records from `starting_block` through `latest_record` are emitted as `Replay` commands; the rebuild phase is empty; `Produce` follows immediately after (when Leader).

Testing strategy:
- Cover command sequencing and rebuild preparation separately.
- For each test, validate it with a temporary code mutation that removes the guarded behavior, then restore the production code.
- All 15 tests have been fail-first validated (mutation applied → test failed → mutation reverted → test passes).
- `noop_canonization_channel_supports_max_produced_queue_size_proposals`: guards that `NoopCanonization`'s channel is unbounded; proposes 3 blocks without draining and expects no deadlock. Fails with a 5 s timeout if channel is bounded.
- `replay_rejects_out_of_order_block_number`: guards the `next_block_number` consistency check on Replay commands added in the consensus refactor.
