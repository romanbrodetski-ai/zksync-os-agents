# Block Rebuild Feature

Scope:
- `node/bin/src/command_source.rs`
- `lib/sequencer/src/execution/block_context_provider.rs`
- startup selection in `node/bin/src/lib.rs`

Core flow:
- Startup chooses the earliest block that must be replayed to restore correctness.
- `MainNodeCommandSource` calls the `command_source()` free function which returns a `BoxStream`. It emits `Replay` commands up to `rebuild_from_block - 1`, then `Rebuild` commands for `[rebuild_from_block..=latest_record]`, then an infinite stream of `Produce` commands.
- `ProduceCommand` now carries `block_number`, `block_time`, and `max_transactions_in_block` (moved from `BlockContextProvider`).
- `BlockContextProvider` converts `Rebuild` into execution-ready commands.

Important invariants:
- `rebuild_from_block` must be within `[block_to_start, latest_record]` (asserted as `rebuild_from_block >= block_to_start` and `rebuild_from_block <= last_block_in_wal`).
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
- All 13 tests have been fail-first validated (mutation applied → test failed → mutation reverted → test passes).
