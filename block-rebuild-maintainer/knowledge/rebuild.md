# Block Rebuild Feature

Scope:
- `node/bin/src/command_source.rs`
- `lib/sequencer/src/execution/block_context_provider.rs`
- startup selection in `node/bin/src/lib.rs`

Core flow:
- Startup chooses the earliest block that must be replayed to restore correctness.
- `MainNodeCommandSource.run()` directly sends commands over the output channel (no intermediate `BoxStream`). It emits `Replay` commands up to `rebuild_from_block - 1`, then `Rebuild` commands for `[rebuild_from_block..=latest_record]`, then enters `run_loop` which produces `Produce` commands indefinitely.
- `ProduceCommand` is a unit struct; `block_number`, `block_time`, and `max_transactions_in_block` are held inside `BlockContextProvider` and tracked via `next_block_number` (incremented in `on_block_executed`).
- `BlockContextProvider` converts `Rebuild` into execution-ready commands. On `Replay` commands it validates `next_block_number == record.block_context.block_number` (blocks must arrive in order).
- After WAL replay and rebuilds, the main node pipeline is: `BlockExecutor → BlockCanonizer → BlockApplier`.
- `MainNodeCommandSource.run_loop` uses `tokio::select!` to race between producing new blocks and receiving canonized replays via `replays_to_execute`. On the main node, receiving a replay in leader mode is an error ("Leader node received block produced by someone else").
- `BlockCanonizer` holds `MAX_PRODUCED_QUEUE_SIZE = 2` in-flight produced/rebuild blocks before applying backpressure. `NoopCanonization`'s internal channel **must have capacity ≥ MAX_PRODUCED_QUEUE_SIZE** (currently 2) to avoid a deadlock where `propose(blockN).await` blocks with the channel full while `next_canonized()` is unreachable.

Important invariants:
- `rebuild_from_block` must be within `[starting_block, latest_record]` (asserted as `rebuild_from_block >= starting_block` and `rebuild_from_block <= last_block_in_wal`).
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

Operational note:
- Rebuild is intentionally more permissive than replay:
  - `SealPolicy::UntilExhausted { allowed_to_finish_early: true }`
  - `InvalidTxPolicy::RejectAndContinue`
  - `expected_block_output_hash = None`

No-rebuild path:
- When `rebuild_options` is `None`, all WAL records from `starting_block` through `latest_record` are emitted as `Replay` commands; the rebuild stream is empty; `Produce` follows immediately after.

Testing strategy:
- Cover command sequencing and rebuild preparation separately.
- For each test, validate it with a temporary code mutation that removes the guarded behavior, then restore the production code.
- All 14 tests have been fail-first validated (mutation applied → test failed → mutation reverted → test passes).
- `noop_canonization_channel_supports_max_produced_queue_size_proposals`: guards that `NoopCanonization`'s channel capacity is ≥ `MAX_PRODUCED_QUEUE_SIZE` (2). Fails with a 5 s timeout if capacity is 1.
