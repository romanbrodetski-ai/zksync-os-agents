# Pipeline Correctness Knowledge Base

This document captures the invariants, design rationale, and failure modes of the
ZKsync OS Server block processing pipeline. It is the primary context source for the
Pipeline Correctness review agent.

**How to use this document:** Read it fully before reviewing any PR that touches pipeline
components. When a section references source files, read the current version of those files
— do not trust code snippets here, as the code may have evolved.

---

## 1. Pipeline Architecture Overview

The server processes blocks through a linear chain of `PipelineComponent` stages connected
by bounded `mpsc` channels. Each component:

- Receives from an input channel (`PeekableReceiver<Input>`)
- Sends to an output channel (`mpsc::Sender<Output>`)
- Declares `OUTPUT_BUFFER_SIZE` which controls backpressure to downstream
- Runs as an independent tokio task

**Key files:**
- `lib/pipeline/src/traits.rs` — `PipelineComponent` trait definition
- `lib/pipeline/src/builder.rs` — `Pipeline` builder with `.pipe()` chaining
- `node/bin/src/lib.rs` — Main and external node pipeline construction

### Main Node Pipeline

```
CommandSource → BlockExecutor → BlockCanonizer → BlockApplier
→ TreeManager → ProverInputGenerator → Batcher → BatchVerification → FriProving
→ GaplessCommitter → L1Sender(Commit) → SnarkProving → GaplessL1ProofSender
→ L1Sender(Proof) → PriorityTree → L1Sender(Execute) → BatchSink
```

### External Node Pipeline (replay only)

```
ExternalNodeCommandSource → BlockExecutor → BlockApplier
→ [optional: RevmConsistencyChecker] → TreeManager → [optional: BatchVerification]
```

Note: External nodes do **not** have a `BlockCanonizer` stage — they replay blocks that
are already canonized by the main node.

### BlockExecutor → BlockCanonizer → BlockApplier Split

As of v0.17.0, the former unified `Sequencer` component is split into three pipeline stages:

1. **`BlockExecutor`** — Executes blocks, updates in-memory state (`OverlayBuffer`), does NOT persist.
   - Input: `BlockCommand`
   - Output: `(BlockOutput, ReplayRecord, BlockCommandType)`
   - `OUTPUT_BUFFER_SIZE = 1`

2. **`BlockCanonizer`** — Consensus fence. Passes `Replay` blocks through immediately;
   `Produce`/`Rebuild` blocks are proposed to consensus, wait for canonization, then proceed.
   - Input: `(BlockOutput, ReplayRecord, BlockCommandType)` from `BlockExecutor`
   - Output: `(BlockOutput, ReplayRecord, BlockCommandType)` to `BlockApplier`
   - `OUTPUT_BUFFER_SIZE = 2`, `produced_queue` max = 2
   - Sends newly-canonized blocks (from other nodes) back to `MainNodeCommandSource` via `canonized_blocks_for_execution`

3. **`BlockApplier`** — Persists blocks to WAL, state, and repository.
   - Input: `(BlockOutput, ReplayRecord, BlockCommandType)` from `BlockCanonizer`
   - Output: `(BlockOutput, ReplayRecord)` to `TreeManager`
   - `OUTPUT_BUFFER_SIZE = 5`

**Key files:**
- `lib/sequencer/src/execution/mod.rs` — Module exports
- `lib/sequencer/src/execution/block_executor.rs` — `BlockExecutor` struct and impl
- `lib/sequencer/src/execution/block_canonizer.rs` — `BlockCanonizer` struct and consensus fence
- `lib/sequencer/src/execution/block_applier.rs` — `BlockApplier` struct and persistence
- `lib/sequencer/src/execution/execute_block_in_vm.rs` — `execute_block_in_vm()` pure function

---

## 2. Core Invariants

### 2.1 Sequential Block Processing

**What:** Blocks MUST be processed in strictly ascending block number order, one at a time,
through the entire pipeline. No block may be skipped, reordered, or processed concurrently
within the pipeline.

**Why:** Each block's execution depends on the state produced by the previous block.
The replay storage (`WriteReplay`) panics if a block is not the next after latest.

**Where enforced:**
- `BlockContextProvider` tracks `next_block_number`, validates Replay/Rebuild commands match it
- `CommandSource` — generates commands in sequential order (WAL replay, then produce starting
  from `last_block_in_wal + 1`)
- `WriteReplay::write()` — "MUST panic if the record is not next after the latest record"
- `TreeManager` — tracks processed block count, skips already-processed blocks
- Pipeline channel ordering — single-producer single-consumer channels guarantee FIFO
- `OverlayBuffer::add_block()` — panics if new block is not contiguous with existing head

**Failure mode:** If a component buffers and reorders, or if a tokio::select! arm processes
items out of order, the chain state becomes corrupted.

### 2.2 Backpressure Model

**What:** Each pipeline component declares `OUTPUT_BUFFER_SIZE` which creates a bounded
channel to the next component. When the downstream component is slow, the upstream component
blocks on send, propagating backpressure up the chain.

**Why:** Without backpressure, a fast producer would outrun a slow consumer
(e.g., TreeManager doing disk I/O), consuming unbounded memory.

**Key buffer sizes and their rationale:**
- `MainNodeCommandSource::OUTPUT_BUFFER_SIZE = 1` — tight coupling; command source feeds one
  block ahead of `BlockExecutor`
- `BlockExecutor::OUTPUT_BUFFER_SIZE = 1` — minimal buffer before `BlockCanonizer`;
  `BlockCanonizer` has its own internal `produced_queue` buffer
- `BlockCanonizer::OUTPUT_BUFFER_SIZE = 2` — allows mild persistence latency spikes
- `BlockApplier::OUTPUT_BUFFER_SIZE = 5` — downstream pipeline components get breathing room

**Maximum in-flight (un-persisted) blocks:**
`BlockExecutor` buffer (1) + `BlockCanonizer` produced_queue (2) + `BlockCanonizer` output buffer (2)
+ `BlockApplier` output buffer (5) = up to ~10 blocks can be executed before persistence catches up.
`OverlayBuffer` must hold all of these in memory.

**Failure mode:** If someone changes a buffer size to 0, mpsc::channel(0) panics at runtime.
If changed to unbounded (or very large), backpressure is lost and memory can grow without bound.

### 2.3 L1 Priority Transaction Ordering

**What:** L1 priority transactions must be processed in strictly monotonic order by
`starting_l1_priority_id`. No ID may be skipped or duplicated.

**Why:** The L1 smart contract enforces this ordering. If the sequencer skips a priority
transaction, the batch will fail to verify on L1, and the chain effectively halts.

**Where enforced:**
- `BlockContextProvider` tracks `next_l1_priority_id`, increments after each L1 tx
- `ReplayRecord::new()` asserts: "First L1 tx priority id must match next_l1_priority_id"
- `Pool::best_transactions_stream()` yields L1 txs from `L1Subpool` before L2 txs

### 2.4 Replay Idempotency

**What:** Replaying a block that was already executed must produce the same result, or be
safely skippable. The `override_allowed` flag on `WriteState` and `WriteReplay` controls
whether re-execution overwrites existing data.

**Why:** On restart, the node replays blocks from WAL to rebuild in-memory state.
Components must handle seeing the same block number again without corrupting data.

**Where enforced:**
- `BlockApplier` sets `override_allowed = true` for `Rebuild` commands and external nodes
  (determined by `BlockCommandType` passed through `BlockCanonizer`)
- `TreeManager` skips blocks already in the tree (idempotent)
- `WriteReplay::write()` returns false (no write) for existing blocks when override not allowed

### 2.5 Gapless L1 Commitment

**What:** Batches must be committed, proved, and executed on L1 in strict sequential order.
No batch number may be skipped.

**Why:** The L1 contract validates batch continuity. A gap would cause the commit transaction
to revert.

**Where enforced:**
- `GaplessCommitter` pipeline component — ensures commit order
- `GaplessL1ProofSender` — ensures proof order
- `Batcher` — starts from `last_executed_batch + 1`, reconstructs already-committed batches

---

## 3. OverlayBuffer — In-Memory State Between Execution and Persistence

Since `BlockExecutor` does not persist blocks, but execution of block N+1 needs state from
block N, `BlockExecutor` maintains an `OverlayBuffer` that accumulates in-memory state diffs.

**Key files:** `lib/storage_api/src/overlay_buffer.rs`

**How it works:**
1. Before executing block N, call `sync_with_base_and_build_view_for_block(state, N)`:
   - Drops overlays for blocks already persisted to base storage
   - If `base_latest >= N-1`, returns a view directly from base (no overlays needed)
   - Otherwise, stacks overlays on top of base: `base_view + overlays[base_latest+1 .. N-1]`
2. Execute block N using the stacked view
3. Call `add_block(N, storage_writes, preimages)` to store the diff in memory
4. Overlays are searched in reverse order (most recent first) with O(1) HashMap lookups

**Invariants:**
- Overlays are always contiguous: block `k` is in the buffer iff blocks `base_latest+1` through `k` are all present
- `add_block` panics if the new block is not `last_overlay + 1`
- `Arc` refcount on the overlays BTreeMap must be 1 when mutating (asserted defensively)
- The view returned by `sync_with_base_and_build_view_for_block` must be dropped before `add_block` is called

**Failure modes:**
- If `BlockApplier` falls too far behind, overlays accumulate in memory (bounded by pipeline buffers)
- If `add_block` is called while a view is still held (Arc refcount > 1), it will panic

---

## 4. Command Types and Their Semantics

Understanding the three command types is critical for reviewing pipeline changes:

### BlockCommand::Produce
- Generated by `MainNodeCommandSource` in `run_loop()` (after WAL replay phase)
- `ProduceCommand` is now an **empty marker struct** — no fields
- Block number, block_time, max_transactions_in_block are now stored in `BlockContextProvider`
- Uses `SealPolicy::Decide(timeout, max_txs)` — block seals on timeout or tx limit
- Uses `InvalidTxPolicy::RejectAndContinue` — skip bad txs, keep producing
- Goes through consensus via `BlockCanonizer` before persistence

### BlockCommand::Replay
- Two sources: WAL replay on startup, or canonized blocks from other nodes
- Transactions come from the `ReplayRecord` itself (predetermined)
- Uses `SealPolicy::UntilExhausted` — must execute all txs in the record
- Uses `InvalidTxPolicy::Abort` — any invalid tx is a fatal error (deterministic replay)
- Passes through `BlockCanonizer` immediately (already canonized)

### BlockCommand::Rebuild
- Used to rollback to an earlier state and re-execute
- Like Produce but with `override_allowed = true` in `BlockApplier`
- Goes through `BlockCanonizer` (waits for consensus like Produce)

**Key distinction:** A Replay that fails is a critical error (the canonical chain is
inconsistent). A Produce that fails can be retried with different transactions.

**Where defined:** `lib/sequencer/src/model/blocks.rs`

---

## 5. BlockContextProvider — Block Parameter Ownership

`BlockContextProvider` now owns the block production parameters that were previously on
`ProduceCommand`:

- `next_block_number: u64` — incremented on each `on_canonical_state_change` call
- `block_time: Duration` — target time per block (used for `SealPolicy::Decide`)
- `max_transactions_in_block: usize` — transaction count limit

**Block number validation (added in v0.17.0):**
For `Replay` commands, `BlockContextProvider::prepare_command()` now asserts:
```
self.next_block_number == record.block_context.block_number
```
This catches out-of-order replay delivery. If this fires, the pipeline delivered blocks
in wrong order — a correctness bug.

**Where to look:**
- `lib/sequencer/src/execution/block_context_provider.rs`

---

## 6. Command Source Design

### Main Node (`MainNodeCommandSource`)

The main node command source has been refactored from stream-based to channel-based:

1. **Replay phase**: Calls `block_replay_storage.forward_range_with(starting_block, replay_until, output, ...)` to send WAL blocks directly to channel
2. **Rebuild phase** (optional): Sends `Rebuild` commands for blocks from `rebuild_from_block` to `last_block_in_wal`
3. **Produce loop** (`run_loop()`): Sends `Produce(ProduceCommand)` indefinitely; errors if a `ReplayRecord` arrives via `replays_to_execute` channel (which would indicate this node received a block produced by another leader — currently expected to be impossible with `NoopCanonization`)

The `replays_to_execute` channel is fed by `BlockCanonizer` when it receives a canonized
block that wasn't produced locally (replica path). The current `NoopCanonization` never
triggers this path.

**`OUTPUT_BUFFER_SIZE` changed from 5 to 1** (tighter coupling with `BlockExecutor`).

### External Node (`ExternalNodeCommandSource`)

- Receives `ReplayRecord`s from the main node via a channel
- Wraps each in `BlockCommand::Replay` and sends downstream
- Optional `up_to_block` limit for syncing to a specific block

---

## 7. ReadReplayExt API Change

`ReadReplayExt::stream()` (returned a `BoxStream`) was replaced by `forward_range_with()`:

```rust
fn forward_range_with<'a, T, F>(
    &'a self, start: u64, end: u64,
    output: mpsc::Sender<T>,
    f: F,
) -> impl Future<Output = anyhow::Result<()>>
```

This sends records directly to a channel with a mapping function, avoiding heap allocation
for the stream. The method stops gracefully if the output channel closes.

---

## 8. BlockCanonizer — Consensus Fence

**Key design:** The `BlockCanonizer` uses `tokio::select!` with two arms:
1. `canonized = self.consensus.next_canonized()` — receives canonized blocks
2. `maybe_executed = input.recv(), if produced_queue.len() < MAX_PRODUCED_QUEUE_SIZE` —
   receives executed blocks from `BlockExecutor` (gated on queue capacity)

**Replay blocks** pass through immediately without queuing.
**Produce/Rebuild blocks** are sent to consensus and added to `produced_queue`
(max 2 entries). The `BlockExecutor` is backpressured when the queue is full.

**`NoopCanonization`** uses an unbounded channel that echoes proposals back as canonized.
This is the current production implementation; real consensus will replace it in future steps.

**Replica path:** If a canonized block arrives but `produced_queue` is empty, the block
was produced by another node. `BlockCanonizer` sends it to `canonized_blocks_for_execution`
(→ `MainNodeCommandSource::replays_to_execute`), which triggers a `run_loop` error ("Leader
received block from someone else"). This path is unused with `NoopCanonization`.

---

## 9. State and Persistence Model

### Write path (BlockApplier)
Each block write involves three stores, in this order:
1. `WriteReplay::write()` — WAL record (source of truth for block history)
2. `WriteState::add_block_result()` — key-value state diffs
3. `WriteRepository::populate()` — API-facing block/tx/receipt data

**Important:** These are NOT atomic across all three stores. A crash between step 1 and 3
means the WAL has the block but the repository doesn't. On restart, replay from WAL will
fill in the gaps.

**`override_allowed` logic in `BlockApplier`:**
- `Replay` from external node → `true`
- `Rebuild` → `true`
- `Produce` or `Replay` (main node) → `false`

### Read path
- `ReadStateHistory::state_view_at(block_number)` — get read-only state at a point in time
- `ReadReplay::get_replay_record(block_number)` — get canonical block data
- `ReadRepository` — API queries (blocks, transactions, receipts)

---

## 10. Common Review Concerns

When reviewing PRs that touch pipeline code, check for:

### Channel and Concurrency Issues
- **Unbounded channels:** Any `mpsc::channel` without a size limit is a memory leak risk
- **Deadlock potential:** Two components waiting on each other's channels
- **Cancellation safety:** Operations in `tokio::select!` branches must be cancellation-safe
- **BlockCanonizer `produced_queue`:** Max size is 2; changing it affects how far ahead
  `BlockExecutor` can run vs consensus

### State Consistency
- **override_allowed correctness:** Only Rebuild and external node Replay should allow overrides.
  A bug here could silently overwrite canonical state.
- **Block number continuity:** `BlockContextProvider::next_block_number` must match the block
  numbers of incoming commands. New code must call `on_canonical_state_change` for every block.
- **OverlayBuffer contiguity:** `add_block` must be called exactly once per block, in order.
  The Arc refcount must be 1 at the time of call.
- **Partial persistence:** If adding a new store to `BlockApplier`, consider crash recovery.
- **State view freshness:** `execute_block_in_vm` reads state through `OverlayBuffer`. If
  `add_block` was not called for a previous block, execution will use stale state.

### Pipeline Topology
- **Buffer size changes:** Changing `OUTPUT_BUFFER_SIZE` affects backpressure for the entire
  pipeline above that component
- **New components:** Adding a pipeline stage changes the backpressure profile and increases
  latency. Consider whether the new stage needs to be in the critical path.
- **EN pipeline vs Main Node pipeline:** External nodes do NOT have `BlockCanonizer`. Any
  changes that affect EN behavior must not require `BlockCanonizer`.

### Config Optionality (Main Node vs External Node)

Several pipeline-related configs are `Option` because External Nodes don't need them:

- **`L1SenderConfig::pubdata_mode`** — `Option<PubdataMode>`. Only the Main Node uses this.
- **`Config::external_price_api_client_config`** — `Option<ExternalPriceApiClientConfig>`.

**Failure mode:** If a new main-node-only code path accesses an optional config without
guarding with `.expect()` or `if node_role.is_main()`, the External Node will panic at
runtime.

---

## 11. File Reference Map

| Component | Path | What to look for |
|-----------|------|-----------------|
| Pipeline framework | `lib/pipeline/src/traits.rs` | PipelineComponent trait |
| Pipeline builder | `lib/pipeline/src/builder.rs` | pipe(), spawn() |
| Pipeline construction | `node/bin/src/lib.rs` | Main & EN pipeline wiring |
| Block commands | `lib/sequencer/src/model/blocks.rs` | Command types, seal policies |
| Block executor | `lib/sequencer/src/execution/block_executor.rs` | Execution, OverlayBuffer usage |
| Block canonizer | `lib/sequencer/src/execution/block_canonizer.rs` | Consensus fence |
| Block applier | `lib/sequencer/src/execution/block_applier.rs` | Persistence, override_allowed |
| Block execution fn | `lib/sequencer/src/execution/execute_block_in_vm.rs` | Pure VM execution |
| BlockContextProvider | `lib/sequencer/src/execution/block_context_provider.rs` | Block params, tx selection |
| Command source | `node/bin/src/command_source.rs` | Channel-based block production |
| Overlay buffer | `lib/storage_api/src/overlay_buffer.rs` | In-memory state between exec/persist |
| Transaction pool | `lib/mempool/src/pool.rs` | Subpool priority ordering |
| Replay storage | `lib/storage_api/src/replay.rs` | ReadReplay/WriteReplay traits |
| State storage | `lib/storage_api/src/state.rs` | ReadStateHistory/WriteState traits |
| State override view | `lib/storage_api/src/state_override_view.rs` | OverriddenStateView for preimages |
| Tree manager | `node/bin/src/tree_manager.rs` | Merkle tree updates |
| Batcher | `node/bin/src/batcher/mod.rs` | Batch boundary logic |
| Fee provider | `lib/sequencer/src/execution/fee_provider.rs` | Pubdata pricing, Option<PubdataMode> |
| Node config | `node/bin/src/config/mod.rs` | Config structs, Option fields for EN |
