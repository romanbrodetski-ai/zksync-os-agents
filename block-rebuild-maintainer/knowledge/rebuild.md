# Block Rebuild Feature

Scope:
- `node/bin/src/command_source.rs`
- `lib/sequencer/src/execution/block_context_provider.rs`
- startup selection in `node/bin/src/lib.rs`

Core flow:
- Startup chooses the earliest block that must be replayed to restore correctness.
- `ConsensusNodeCommandSource.run()` sends commands over the output channel. It uses `forward_range_with` to emit `Replay` commands up to `rebuild_from_block - 1`, then sends `Rebuild` commands for `[rebuild_from_block..=latest_record]` via `send_block_rebuilds`, then enters `run_loop`.
- `run_loop` uses `tokio::select!` with three arms: leadership change watch, inbound `replays_to_execute` channel (canonized blocks from consensus), and `Produce` command emission (only when `role == ConsensusRole::Leader`).
- `ProduceCommand` is a unit struct; `block_number`, `block_time`, and `max_transactions_in_block` are held inside `BlockContextProvider`. `next_block_number` is incremented in `on_canonical_state_change`.
- `BlockContextProvider` converts `Rebuild` and `Produce` into execution-ready `PreparedBlockCommand`s.
- After WAL replay and rebuilds, the main node pipeline is: `BlockExecutor → BlockCanonizer → BlockApplier`.
- `BlockCanonizer` holds `MAX_PRODUCED_QUEUE_SIZE = 2` in-flight produced/rebuild blocks before applying backpressure. Replay commands bypass canonization and go directly downstream.
- `NoopCanonization` uses an unbounded channel (no capacity concern).

Important invariants:
- `rebuild_from_block` must be within `[starting_block, latest_record]` (asserted as `rebuild_from_block >= starting_block` and `rebuild_from_block <= last_block_in_wal`).
- Replay commands are checked for block number ordering: `self.next_block_number == record.block_context.block_number` (enforced via `anyhow::ensure!`).
- Replay commands also check `previous_block_timestamp` and `block_hashes` consistency against the provider state.
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
- When `rebuild_options` is `None`, all WAL records from `starting_block` through `latest_record` are emitted as `Replay` commands; the rebuild phase is skipped; `run_loop` follows immediately.

Consensus integration:
- `ConsensusNodeCommandSource` requires a `LeadershipSignal` (either `AlwaysLeader` or watch-based).
- `run_loop` only emits `Produce` when `role == ConsensusRole::Leader`.
- Canonized blocks from consensus arrive via `replays_to_execute` channel and are forwarded as `Replay` commands.
- The loopback consensus (`loopback_consensus()`) provides `LeadershipSignal::AlwaysLeader` + `NoopCanonization`.

Testing strategy:
- Cover command sequencing and rebuild preparation separately.
- For each test, validate it with a temporary code mutation that removes the guarded behavior, then restore the production code.
- 16 tests cover: command sequencing (4), rebuild preparation (7), replay ordering (2), canonization channel (1), no-rebuild path (1), metadata preservation (1).
- The Produce path of `BlockContextProvider.prepare_command()` cannot be unit-tested in isolation because `pool.best_transactions_stream()` blocks without real mempool activity.
