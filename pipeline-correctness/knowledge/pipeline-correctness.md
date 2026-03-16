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

### Main Node Pipeline (block production + L1 commitment)

```
CommandSource → BlockExecutor → BlockCanonizer → BlockApplier → TreeManager
→ ProverInputGenerator → Batcher → BatchVerification → FriProving
→ GaplessCommitter → L1Sender(Commit) → SnarkProving → GaplessL1ProofSender
→ L1Sender(Proof) → PriorityTree → L1Sender(Execute) → BatchSink
```

### External Node Pipeline (replay only)

```
ExternalNodeCommandSource → BlockExecutor → BlockApplier
→ [optional: RevmConsistencyChecker] → TreeManager → [optional: BatchVerification]
```

### Execution / Canonization / Persistence Split

The server now uses three correctness-critical stages instead of a unified `Sequencer`:

- `BlockExecutor`
  - Input: `BlockCommand`
  - Output: `(BlockOutput, ReplayRecord, BlockCommandType)`
  - `OUTPUT_BUFFER_SIZE = 1`
  - Executes the block, updates in-memory mempool / block-context state, and appends the
    state diff to `OverlayBuffer`. It does not persist anything.
- `BlockCanonizer`
  - Input: `(BlockOutput, ReplayRecord, BlockCommandType)`
  - Output: `(BlockOutput, ReplayRecord, BlockCommandType)`
  - `OUTPUT_BUFFER_SIZE = 2`
  - `Replay` blocks bypass consensus. `Produce` / `Rebuild` blocks are proposed and held
    behind a canonization fence until the matching canonized `ReplayRecord` arrives.
- `BlockApplier`
  - Input: `(BlockOutput, ReplayRecord, BlockCommandType)`
  - Output: `(BlockOutput, ReplayRecord)`
  - `OUTPUT_BUFFER_SIZE = 5`
  - Persists replay WAL, state diffs, and repository rows after canonization.

This means block `N+1` may execute before block `N` is persisted, but only against
base-state plus `OverlayBuffer`. If canonization diverges, `BlockCanonizer` aborts
before any non-canonical block reaches `BlockApplier`.

**Key files:**
- `lib/sequencer/src/execution/mod.rs` — exported execution stages
- `lib/sequencer/src/execution/block_executor.rs` — execution loop and overlay updates
- `lib/sequencer/src/execution/block_canonizer.rs` — canonization fence
- `lib/sequencer/src/execution/block_applier.rs` — persistence ordering

---

## 2. Core Invariants

### 2.1 Sequential Block Processing

**What:** Blocks MUST be processed in strictly ascending block number order, one at a time,
through the entire pipeline. No block may be skipped, reordered, or processed concurrently
within the pipeline.

**Why:** Each block's execution depends on the state produced by the previous block.
The replay storage (`WriteReplay`) panics if a block is not the next after latest.

**Where enforced:**
- `MainNodeCommandSource` — replays WAL in order, optionally emits rebuilds, then emits
  parameterless `Produce` commands one at a time
- `BlockContextProvider::next_block_number` — the authoritative execution cursor; `Replay`
  commands are rejected if their record block number does not match it
- `WriteReplay::write()` — "MUST panic if the record is not next after the latest record"
- `OverlayBuffer::add_block()` — requires contiguous speculative overlays
- `TreeManager` — tracks processed block count, skips already-processed blocks
- Pipeline channel ordering — single-producer single-consumer channels guarantee FIFO

**Failure mode:** If a component buffers and reorders, or if a tokio::select! arm processes
items out of order, the chain state becomes corrupted.

### 2.2 Backpressure Model

**What:** Each pipeline component declares `OUTPUT_BUFFER_SIZE` which creates a bounded
channel to the next component. When the downstream component is slow, the upstream component
blocks on send, propagating backpressure up the chain.

**Why:** Without backpressure, a fast producer would outrun a slow consumer
(e.g., TreeManager doing disk I/O), consuming unbounded memory.

**Key buffer sizes and their rationale:**
- `MainNodeCommandSource::OUTPUT_BUFFER_SIZE = 1` — produce commands are parameterless, so the
  source should not get far ahead of executor-owned block context state
- `BlockExecutor::OUTPUT_BUFFER_SIZE = 1` — speculation ahead of persistence is bounded tightly
- `BlockCanonizer::OUTPUT_BUFFER_SIZE = 2` and `MAX_PRODUCED_QUEUE_SIZE = 2` — allows a small
  amount of execution to wait for canonization without losing control of memory or ordering
- `BlockApplier::OUTPUT_BUFFER_SIZE = 5` — persistence to downstream tree / prover stages remains
  moderately buffered

**Failure mode:** If someone changes a buffer size to 0, the component won't start processing
the next item until the current one is picked up. If changed to unbounded (or very large),
backpressure is lost and memory can grow without bound.

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

## 3. Command Types and Their Semantics

Understanding the three command types is critical for reviewing pipeline changes:

### BlockCommand::Produce
- Generated by `MainNodeCommandSource` after replay / rebuild completes
- `ProduceCommand` is parameterless; `BlockContextProvider` owns `next_block_number`,
  `block_time`, and `max_transactions_in_block`
- Transactions selected from mempool by `BlockContextProvider`
- Uses `SealPolicy::Decide(timeout, max_txs)` — block seals on timeout or tx limit
- Uses `InvalidTxPolicy::RejectAndContinue` — skip bad txs, keep producing

### BlockCommand::Replay
- Two sources: WAL replay on startup, or canonized blocks from other nodes
- Transactions come from the `ReplayRecord` itself (predetermined)
- Uses `SealPolicy::UntilExhausted` — must execute all txs in the record
- Uses `InvalidTxPolicy::Abort` — any invalid tx is a fatal error (deterministic replay)

### BlockCommand::Rebuild
- Used to rollback to an earlier state and re-execute
- Like Produce but persisted with `override_allowed = true` in `BlockApplier`

**Key distinction:** A Replay that fails is a critical error (the canonical chain is
inconsistent). A Produce that fails can be retried with different transactions.

**Where defined:** `lib/sequencer/src/model/blocks.rs`

---

## 4. Command Source Design

### Main Node (`MainNodeCommandSource`)

The main node command source is now an active loop instead of a composed stream:

1. **Replay phase**: Uses `ReadReplayExt::forward_range_with()` to forward WAL replay
   records from `starting_block` to `replay_until`
2. **Rebuild phase** (optional): Emits `Rebuild` commands for
   `rebuild_from_block..=last_block_in_wal`
3. **Produce phase**: Enters `run_loop()` and continuously emits
   `BlockCommand::Produce(ProduceCommand)`

The source also owns `replays_to_execute`, which is fed by `BlockCanonizer` for canonized
blocks that must re-enter execution as replays. In the current main-node wiring, receiving
such a block during produce mode is treated as a fatal error.

**Where to look:**
- `node/bin/src/command_source.rs` — `MainNodeCommandSource::run()` and `run_loop()`

### External Node (`ExternalNodeCommandSource`)

- Receives `ReplayRecord`s from the main node via a channel
- Wraps each in `BlockCommand::Replay` and sends downstream
- Optional `up_to_block` limit for syncing to a specific block

---

## 5. Transaction Selection Priority

Within a single block, transactions are selected in this strict order:

1. **Upgrade subpool** — protocol upgrade transactions (highest priority)
2. **L1 Subpool** — L1→L2 priority transactions (must maintain ordering)
3. **SL Chain ID subpool** — settlement layer chain migration
4. **Interop roots subpool** — cross-chain interop roots (with configurable delay)
5. **Interop fee subpool** — interop fee collection
6. **L2 Subpool** — regular user transactions (ordered by fees)

**Why this matters:** Changing the priority order or adding a new subpool before L1
could cause L1 priority ID desynchronization.

**Where to look:** `lib/mempool/src/pool.rs` — `best_transactions_stream()`

---

## 6. State and Persistence Model

### Write path (BlockApplier)
Each canonized block write involves three stores, in this order:
1. `WriteReplay::write()` — WAL record (source of truth for block history)
2. `WriteState::add_block_result()` — key-value state diffs
3. `WriteRepository::populate()` — API-facing block/tx/receipt data

**Important:** These are NOT atomic across all three stores. A crash between step 1 and 3
means the WAL has the block but the repository doesn't. On restart, replay from WAL will
fill in the gaps.

### Read path
- `ReadStateHistory::state_view_at(block_number)` — get read-only state at a point in time
- `ReadReplay::get_replay_record(block_number)` — get canonical block data
- `ReadRepository` — API queries (blocks, transactions, receipts)

### Block Execution State View
`BlockExecutor` creates the execution state view from:

1. Persistent state at the latest block already applied to base storage
2. `OverlayBuffer` entries for executed-but-not-yet-persisted blocks
3. Forced preimages injected through `OverriddenStateView`

`OverlayBuffer::sync_with_base_and_build_view_for_block()` must purge overlays that are
already persisted in base storage and must reject non-contiguous overlay ranges. If this
logic regresses, execution can read stale speculative state.

**Where to look:**
- `lib/storage_api/src/` — all trait definitions
- `lib/sequencer/src/execution/block_executor.rs` — execution loop and overlay updates
- `lib/storage_api/src/overlay_buffer.rs` — speculative state buffering

---

## 7. Common Review Concerns

When reviewing PRs that touch pipeline code, check for:

### Channel and Concurrency Issues
- **Unbounded channels:** Any `mpsc::channel` without a size limit is a memory leak risk
- **Deadlock potential:** Two components waiting on each other's channels
- **Cancellation safety:** Operations in `tokio::select!` branches must be cancellation-safe.
  If a future is dropped mid-execution, state must remain consistent.
- **Canonization fence leaks:** `BlockCanonizer` must never let `Produce` / `Rebuild` blocks
  reach `BlockApplier` before their matching canonized replay returns

### State Consistency
- **override_allowed correctness:** Only Rebuild and external node should allow overrides.
  A bug here could silently overwrite canonical state.
- **Block number continuity:** Any new component that touches block numbers must maintain
  the sequential invariant
- **Partial persistence:** If adding a new store to `BlockApplier`, consider crash recovery
- **Overlay purge / freshness:** `BlockExecutor` may run ahead of persistence only if
  `OverlayBuffer` exactly mirrors the unpersisted prefix and drops overlays once base catches up
- **Pre-canonization mempool updates:** `BlockContextProvider::on_canonical_state_change()` now
  runs in `BlockExecutor`; any change that weakens abort-on-mismatch can leak speculative state

### Pipeline Topology
- **Buffer size changes:** Changing `OUTPUT_BUFFER_SIZE` affects backpressure for the entire
  pipeline above that component
- **New components:** Adding a pipeline stage changes the backpressure profile and increases
  latency. Consider whether the new stage needs to be in the critical path.
- **Conditional components:** `pipe_opt()` and `pipe_if()` must preserve type compatibility
- **Main vs EN topology drift:** Main nodes require `BlockCanonizer`; external nodes do not

### Config Optionality (Main Node vs External Node)

Several pipeline-related configs are `Option` because External Nodes don't need them:

- **`L1SenderConfig::pubdata_mode`** — `Option<PubdataMode>`. Only the Main Node uses this
  (block production, batcher, prover input generator). Guarded by `.expect()` in main-node paths.
  `FeeProvider` also holds `Option<PubdataMode>` and panics if `None` when producing blocks.
- **`Config::external_price_api_client_config`** — `Option<ExternalPriceApiClientConfig>`.
  Only the Main Node runs the base token price updater.

**Failure mode:** If a new main-node-only code path accesses an optional config without
guarding with `.expect()` or `if node_role.is_main()`, the External Node will panic at
runtime. Conversely, if a previously optional config becomes required for EN, the `Option`
must be removed or a new guard added.

---

## 8. File Reference Map

Quick reference for the most important files in the pipeline:

| Component | Path | What to look for |
|-----------|------|-----------------|
| Pipeline framework | `lib/pipeline/src/traits.rs` | PipelineComponent trait |
| Pipeline builder | `lib/pipeline/src/builder.rs` | pipe(), spawn() |
| Pipeline construction | `node/bin/src/lib.rs` | Main & EN pipeline wiring |
| Block commands | `lib/sequencer/src/model/blocks.rs` | Command types, seal policies |
| Execution exports | `lib/sequencer/src/execution/mod.rs` | Which execution stages are wired |
| Block executor | `lib/sequencer/src/execution/block_executor.rs` | Execution loop, overlay usage, tx acceptance limit |
| Block canonizer | `lib/sequencer/src/execution/block_canonizer.rs` | Canonization fence, proposal queue |
| Block applier | `lib/sequencer/src/execution/block_applier.rs` | Persistence ordering, override policy |
| BlockContextProvider | `lib/sequencer/src/execution/block_context_provider.rs` | Tx selection, block context |
| Command source | `node/bin/src/command_source.rs` | WAL replay forwarding, rebuild emission, produce loop |
| Transaction pool | `lib/mempool/src/pool.rs` | Subpool priority ordering |
| Replay storage | `lib/storage_api/src/replay.rs` | ReadReplay/WriteReplay traits |
| State storage | `lib/storage_api/src/state.rs` | ReadStateHistory/WriteState traits |
| State override view | `lib/storage_api/src/state_override_view.rs` | OverriddenStateView for preimages |
| Overlay buffer | `lib/storage_api/src/overlay_buffer.rs` | Unpersisted state diffs between executor and applier |
| Tree manager | `node/bin/src/tree_manager.rs` | Merkle tree updates |
| Batcher | `node/bin/src/batcher/mod.rs` | Batch boundary logic |
| Fee provider | `lib/sequencer/src/execution/fee_provider.rs` | Pubdata pricing, Option<PubdataMode> |
| Node config | `node/bin/src/config/mod.rs` | Config structs, Option fields for EN |
