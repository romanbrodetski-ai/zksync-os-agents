# Pipeline Correctness — Test Plan

Test matrix for the pipeline execution correctness feature area.
Each test states: what it protects, what it checks, and what regression would break it.

---

## 1. Happy Path

### `sequencer::produce_command_carries_block_params`
- **Protects:** ProduceCommand struct carries block_number, block_time, max_transactions_in_block
- **Checks:** Field values round-trip correctly
- **Breaks if:** Fields removed or renamed on ProduceCommand

### `sequencer::block_command_block_number`
- **Protects:** BlockCommand::block_number() returns correct value for all 3 variants (Replay, Produce, Rebuild)
- **Checks:** block_number() matches the value embedded in each variant
- **Breaks if:** A variant returns the wrong block number or block_number() is not updated for new variants

### `sequencer::sequencer_output_type_is_two_tuple`
- **Protects:** Sequencer output type is `(BlockOutput, ReplayRecord)` — not the old 3-tuple with BlockCommandType
- **Checks:** Compile-time type assertion
- **Breaks if:** Output type changes (compile error)

### `pipeline_flow::basic_pipeline_flow`
- **Protects:** Data flows end-to-end through a single pipeline component
- **Checks:** Input values arrive at output in order, with correct recording
- **Breaks if:** PipelineComponent::run contract changes or PeekableReceiver breaks ordering

### `pipeline_flow::chained_transformation`
- **Protects:** Multi-stage pipeline composition
- **Checks:** Input → Doubler → EvenFilter produces correct output sequence
- **Breaks if:** Component chaining breaks or output buffer sizing prevents flow

### `replay_storage::write_genesis_block`
- **Protects:** Genesis block (0) is writable as the first record
- **Checks:** write() returns true, latest_record() is 0, record is retrievable
- **Breaks if:** WriteReplay rejects block 0 or changes genesis handling

### `replay_storage::sequential_writes_succeed`
- **Protects:** Sequential block writes (1..=10) all succeed after genesis
- **Checks:** Each write returns true, latest_record advances
- **Breaks if:** Sequential ordering check becomes too strict or off-by-one

---

## 2. Ordering / Sequencing

### `pipeline_flow::ordering_preserved_through_multiple_stages`
- **Protects:** FIFO ordering through a 3-stage pipeline (100 items)
- **Checks:** Items arrive at output in exactly the same order as sent
- **Breaks if:** Any stage reorders items, or concurrent processing violates ordering

### `sequencer::replay_stream_returns_records_in_order`
- **Protects:** ReadReplayExt::stream() returns records in block number order
- **Checks:** Streaming blocks 2-4 returns them with ascending block_context.block_number
- **Breaks if:** Stream implementation reorders or skips records

### `replay_storage::latest_record_monotonic`
- **Protects:** latest_record() never decreases
- **Checks:** After each sequential write, latest_record >= previous latest_record
- **Breaks if:** latest_record tracking is corrupted by concurrent writes or override logic

---

## 3. Boundary Conditions

### `backpressure::zero_buffer_means_lockstep`
- **Protects:** OUTPUT_BUFFER_SIZE = 0 is invalid (tokio requires >= 1)
- **Checks:** mpsc::channel(0) panics
- **Breaks if:** tokio changes channel(0) behavior, or pipeline builder adds validation

### `replay_storage::non_sequential_write_panics`
- **Protects:** WriteReplay contract: block N+1 only after block N
- **Checks:** Writing block 5 when latest is 0 panics with "not next after latest"
- **Breaks if:** Sequential check is removed or relaxed

---

## 4. Idempotency / Duplication

### `replay_storage::duplicate_write_without_override_returns_false`
- **Protects:** Writing the same block twice without override is a no-op
- **Checks:** Second write returns false
- **Breaks if:** Duplicate detection removed, allowing silent data corruption

### `replay_storage::duplicate_write_with_override_succeeds`
- **Protects:** override_allowed=true permits re-writing an existing block
- **Checks:** Second write with override returns true, new data replaces old
- **Breaks if:** Override path broken — Rebuild commands would fail

---

## 5. Backpressure

### `backpressure::slow_consumer_blocks_fast_producer`
- **Protects:** Fundamental backpressure: bounded buffer limits producer progress
- **Checks:** After 150ms with a 50ms/item consumer, producer has sent < 20 items
- **Breaks if:** Buffer size changed to unbounded, or send() becomes non-blocking

### `backpressure::end_to_end_backpressure_propagation`
- **Protects:** Backpressure propagates through a 3-stage pipeline
- **Checks:** Producer limited to ~consumed + sum(buffer_sizes) items
- **Breaks if:** An intermediate stage decouples upstream from downstream pressure

### `backpressure::dropped_receiver_unblocks_sender`
- **Protects:** Channel closure propagates: dropped receiver causes send error, not deadlock
- **Checks:** send() to dropped receiver returns Err
- **Breaks if:** Channel implementation changes error behavior

---

## 6. Error Propagation

### `pipeline_flow::error_propagates_through_channel_closure`
- **Protects:** Mid-pipeline error closes downstream channels gracefully
- **Checks:** After first stage errors on value 3, output channel eventually closes
- **Breaks if:** Error handling swallows the error or keeps the channel open

### `pipeline_flow::pipeline_builder_spawns_without_panic`
- **Protects:** Pipeline::new().spawn() doesn't panic with zero components
- **Checks:** Empty pipeline spawns and settles without panic
- **Breaks if:** Builder assumes at least one component

---

## 7. State Override (Preimage Injection)

### `state_override::preimage_override_shadows_base`
- **Protects:** OverriddenStateView overrides shadow base preimages
- **Checks:** Override value returned instead of base value for same hash
- **Breaks if:** Override lookup order reversed or skipped

### `state_override::preimage_falls_through_to_base`
- **Protects:** Non-overridden preimages fall through to base state
- **Checks:** Base preimage returned when override set doesn't contain the hash
- **Breaks if:** Fall-through broken, returning None for valid base preimages

### `state_override::missing_preimage_returns_none`
- **Protects:** Missing preimage (neither override nor base) returns None
- **Checks:** get_preimage for unknown hash returns None
- **Breaks if:** Default value returned instead of None

### `state_override::storage_falls_through_with_preimage_override`
- **Protects:** with_preimages() only overrides preimages, not storage reads
- **Checks:** Storage read returns base value even when preimage overrides are active
- **Breaks if:** Preimage override accidentally intercepts storage reads

### `state_override::multiple_preimage_overrides`
- **Protects:** Multiple overrides are all accessible
- **Checks:** Three distinct overrides all return correct values
- **Breaks if:** Override collection only stores last entry

### `state_override::preimage_override_does_not_affect_storage`
- **Protects:** Combined scenario: preimage override + storage read independence
- **Checks:** Preimage overridden, storage unchanged
- **Breaks if:** Override mechanism leaks into storage layer

### `state_override::empty_overrides_is_noop`
- **Protects:** Empty override set is a no-op
- **Checks:** All reads fall through to base
- **Breaks if:** Empty override set causes errors or changes behavior

---

## 8. Data Integrity

### `replay_storage::all_blocks_in_range_retrievable`
- **Protects:** All blocks [0, latest] are readable after writing
- **Checks:** 21 blocks written and all retrievable with correct block_number
- **Breaks if:** Storage drops records or has off-by-one in range

### `replay_storage::get_context_consistent_with_get_replay_record`
- **Protects:** get_context() and get_replay_record() return consistent data
- **Checks:** block_number and timestamp match between the two methods
- **Breaks if:** One method updated without the other

### `replay_storage::replay_record_equality_ignores_node_version`
- **Protects:** ReplayRecord PartialEq excludes node_version (by design)
- **Checks:** Records differing only in node_version are equal
- **Breaks if:** PartialEq derive added without custom impl

### `replay_storage::replay_record_inequality_on_output_hash`
- **Protects:** ReplayRecord PartialEq includes block_output_hash
- **Checks:** Records with different output hashes are not equal
- **Breaks if:** block_output_hash excluded from equality

### `replay_storage::write_log_tracks_operations`
- **Protects:** Mock write log accurately captures all writes with override status
- **Checks:** 4 writes (3 normal + 1 override) recorded correctly
- **Breaks if:** Mock diverges from real WriteReplay behavior

---

## 9. Crash Recovery

### `crash_recovery::crash_after_wal_write_recovers_on_replay`
- **Protects:** Partial persistence (WAL succeeds, WriteState fails) is recoverable via replay
- **Checks:** After simulated crash on block 3, WAL has the block but state/repo don't. After recovery replay, all three stores converge.
- **Breaks if:** Sequencer starts checking WriteReplay return value and skipping downstream stores

### `crash_recovery::recovery_replays_multiple_blocks_idempotently`
- **Protects:** Recovery can replay blocks that are already in all stores without corruption
- **Checks:** Block 1 (already complete) is re-processed during recovery alongside block 2 (incomplete). Both succeed.
- **Breaks if:** WriteState or WriteRepository reject duplicate writes during recovery

### `crash_recovery::rebuild_recovery_uses_override`
- **Protects:** Rebuild commands pass override_allowed=true through to all three stores
- **Checks:** Both original and rebuild writes recorded with correct override flags
- **Breaks if:** override_allowed not propagated correctly for Rebuild commands

### `crash_recovery::wal_duplicate_write_does_not_block_downstream_stores`
- **Protects:** The critical property: WriteReplay returning false does not prevent WriteState/WriteRepository from receiving the block
- **Checks:** Block written only to WAL, then full Sequencer write sequence re-run — state and repo get the data despite WAL returning false
- **Breaks if:** Sequencer starts branching on WriteReplay return value

---

## Known gaps (not yet tested)

- **L1 priority ordering**: Transaction selection ordering from mempool not tested at integration level
- **Cancellation safety**: No test verifies tokio::select! branches are cancellation-safe
- **Config optionality**: No test verifies main-node vs external-node config guards
- **Full Sequencer run loop**: Tests cover components in isolation; no end-to-end test runs the actual Sequencer with mock storage through multiple blocks
