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
NodeCommandSource → BlockExecutor → BlockCanonizer → BlockApplier → TreeManager
→ ProverInputGenerator → Batcher → BatchVerification → FriProving
→ GaplessCommitter → L1Sender(Commit) → SnarkProving → GaplessL1ProofSender
→ L1Sender(Proof) → PriorityTree → L1Sender(Execute) → BatchSink
```

### External Node Pipeline (replay only)

```
ExternalNodeCommandSource → BlockExecutor → BlockApplier
→ [optional: RevmConsistencyChecker] → TreeManager → [optional: BatchVerification]
```

### Sequencer Split: BlockExecutor + BlockCanonizer + BlockApplier

The previous monolithic `Sequencer` component has been split into three pipeline stages:

**BlockExecutor** (`lib/sequencer/src/execution/block_executor.rs`)
- Input: `BlockCommand`
- Output: `(BlockOutput, ReplayRecord, BlockCommandType)`
- `OUTPUT_BUFFER_SIZE = 1`
- Executes blocks in the VM but does NOT persist anything to disk
- Keeps an `OverlayBuffer` in memory — each executed block's state diffs are held there
  until `BlockApplier` confirms persistence (via `sync_with_base_and_build_view_for_block`)
- Updates mempool state via `BlockContextProvider::on_canonical_state_change()` immediately
  after execution (before persistence)
- Passes `BlockCommandType` downstream so `BlockApplier` can determine `override_allowed`

**BlockCanonizer** (`lib/sequencer/src/execution/block_canonizer.rs`)
- Input: `(BlockOutput, ReplayRecord, BlockCommandType)`
- Output: `(BlockOutput, ReplayRecord, BlockCommandType)`
- `OUTPUT_BUFFER_SIZE = 2`
- Acts as a canonization fence: only canonized blocks flow downstream to `BlockApplier`
- `Replay` blocks pass through immediately (already canonical)
- `Produce`/`Rebuild` blocks are sent to consensus via `BlockCanonization::propose()`,
  queued in `produced_queue` (max `MAX_PRODUCED_QUEUE_SIZE = 2`), and only released
  when consensus returns the matching canonized record
- If canonized record doesn't match the front of `produced_queue`, fatal error
- Used only in the Main Node pipeline; External Node pipeline skips it

**BlockApplier** (`lib/sequencer/src/execution/block_applier.rs`)
- Input: `(BlockOutput, ReplayRecord, BlockCommandType)`
- Output: `(BlockOutput, ReplayRecord)`
- `OUTPUT_BUFFER_SIZE = 5`
- Persists each block to the three stores (WAL, state, repository)
- Determines `override_allowed` from `BlockCommandType`:
  - `Rebuild` → true
  - Any type on external node → true
  - Otherwise → false

### OverlayBuffer

Because `BlockExecutor` executes blocks before they are persisted, it maintains an
`OverlayBuffer` of all executed-but-not-yet-persisted block state diffs. When preparing
state for the next block execution, `sync_with_base_and_build_view_for_block` purges
already-persisted blocks and builds a composite view layering the overlays on top of the
base state. The overlay buffer is implicitly bounded by the total buffering in the pipeline
between `BlockExecutor` and `BlockApplier`.

**Key invariant:** The `OverlayBuffer.overlays` `Arc` must have a strong count of exactly 1
when `add_block` or `purge_already_persisted_blocks` is called. Any live `OverriddenStateView`
holding a clone of the Arc would violate this and trigger an assertion panic.

**Key files:**
- `lib/storage_api/src/overlay_buffer.rs` — `OverlayBuffer`, `BlockOverlay`
- `lib/sequencer/src/execution/block_executor.rs` — `BlockExecutor` with overlay integration
- `lib/sequencer/src/execution/block_canonizer.rs` — `BlockCanonizer`, `NoopCanonization`
- `lib/sequencer/src/execution/block_applier.rs` — `BlockApplier`

---

## 2. Core Invariants

### 2.1 Sequential Block Processing

**What:** Blocks MUST be processed in strictly ascending block number order, one at a time,
through the entire pipeline. No block may be skipped, reordered, or processed concurrently
within the pipeline.

**Why:** Each block's execution depends on the state produced by the previous block.
The replay storage (`WriteReplay`) panics if a block is not the next after latest.

**Where enforced:**
- `BlockContextProvider` tracks `next_block_number`, incremented in
  `on_canonical_state_change()` after each block. For Replay commands, validates that
  `next_block_number == record.block_context.block_number` (hard error if mismatch).
- `CommandSource` — generates commands in sequential order (WAL replay, then produce)
- `WriteReplay::write()` — "MUST panic if the record is not next after the latest record"
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

**Key buffer sizes and their rationale (as of 0.17.0):**
- `NodeCommandSource::OUTPUT_BUFFER_SIZE = 1` — reduced from 5; command source stays close
  to execution to avoid queuing stale produce commands
- `BlockExecutor::OUTPUT_BUFFER_SIZE = 1` — small buffer before `BlockCanonizer`; the
  canonizer has its own `produced_queue` buffer
- `BlockCanonizer::OUTPUT_BUFFER_SIZE = 2` — mild persistence latency buffer before
  `BlockApplier`
- `BlockCanonizer::MAX_PRODUCED_QUEUE_SIZE = 2` — max blocks awaiting consensus; when
  reached, backpressure is applied to `BlockExecutor`
- `BlockApplier::OUTPUT_BUFFER_SIZE = 5` — breathing room for downstream (TreeManager etc.)

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
- `BlockApplier` derives `override_allowed` from `BlockCommandType`:
  `Rebuild` → true, external node → true, otherwise → false
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
- Generated by `MainNodeCommandSource` in its `run_loop()`
- `ProduceCommand` is now a unit struct — carries no parameters
- Block number, block time, and max transactions are fields on `BlockContextProvider`
  (`next_block_number`, `block_time`, `max_transactions_in_block`)
- Transactions selected from mempool by `BlockContextProvider`
- Uses `SealPolicy::Decide(timeout, max_txs)` — block seals on timeout or tx limit
- Uses `InvalidTxPolicy::RejectAndContinue` — skip bad txs, keep producing

### BlockCommand::Replay
- Two sources: WAL replay on startup (via `forward_range_with()`), or canonized blocks from
  `BlockCanonizer` when consensus elects a different leader
- Transactions come from the `ReplayRecord` itself (predetermined)
- `BlockContextProvider` validates `next_block_number == record.block_context.block_number`
  (hard error on mismatch)
- Uses `SealPolicy::UntilExhausted` — must execute all txs in the record
- Uses `InvalidTxPolicy::Abort` — any invalid tx is a fatal error (deterministic replay)

### BlockCommand::Rebuild
- Used to rollback to an earlier state and re-execute
- Like Replay but `BlockApplier` sets `override_allowed = true`
- `BlockCanonizer` sends Rebuild blocks to consensus (same path as Produce)

**Key distinction:** A Replay that fails is a critical error (the canonical chain is
inconsistent). A Produce that fails can be retried with different transactions.

**Where defined:** `lib/sequencer/src/model/blocks.rs`

---

## 4. Command Source Design

### Main Node (`MainNodeCommandSource`)

The main node command source operates in three phases:

1. **Replay phase**: Sends WAL replay records via `ReadReplayExt::forward_range_with()` to
   the output channel as `BlockCommand::Replay`. Covers `starting_block..=replay_until`.
2. **Rebuild phase** (optional): If `rebuild_options` is set, sends `BlockCommand::Rebuild`
   for each block from `rebuild_from_block` to `last_block_in_wal`.
3. **Produce loop** (`run_loop()`): Sends `BlockCommand::Produce(ProduceCommand)` indefinitely.
   Also listens on `replays_to_execute` channel (fed by `BlockCanonizer` when another node
   is canonizing blocks). If any replay arrives, the main node crashes — it asserts it is
   the sole leader.

**Key change from previous design:** The command source no longer generates a stream; it
directly sends to the mpsc channel. `ProduceCommand` no longer carries `block_number`,
`block_time`, or `max_transactions_in_block` — those are owned by `BlockContextProvider`.
`OUTPUT_BUFFER_SIZE` reduced from 5 to 1.

**Where to look:**
- `node/bin/src/command_source.rs` — `MainNodeCommandSource`

### External Node (`ExternalNodeCommandSource`)

- Receives `ReplayRecord`s from the main node via a channel
- Wraps each in `BlockCommand::Replay` and sends downstream
- Optional `up_to_block` limit for syncing to a specific block
- No `BlockCanonizer` in the EN pipeline

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
Each block write involves three stores, in this order:
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

`BlockExecutor` builds a state view for block N by calling
`OverlayBuffer::sync_with_base_and_build_view_for_block(base_state, N)`. This:
1. Purges already-persisted overlays from the buffer
2. If `base_latest >= N - 1`, returns a direct view of the base state (overlays empty)
3. Otherwise, layers the in-memory overlays on top of the base state via
   `OverriddenStateView<impl ViewState, Arc<BTreeMap<BlockNumber, BlockOverlay>>>`

Forced preimages from `Replay`/`Rebuild` commands are injected via
`OverriddenStateView::with_preimages()` at execution time (not stored in the overlay buffer).

### OverrideProvider Trait

`OverriddenStateView` is now generic over `O: OverrideProvider`. Two implementations:
- `OwnedOverrides` — owned HashMap pair, used for RPC `eth_call` with `StateOverride`
- `Arc<BTreeMap<BlockNumber, BlockOverlay>>` — the overlay buffer, used by `BlockExecutor`

The constructor change: `OverriddenStateView::new(inner, state_overrides: StateOverride)` is
replaced by:
- `OverriddenStateView::new(inner, overrides: O)` — generic constructor
- `OverriddenStateView::with_state_overrides(inner, state_overrides)` — for RPC use
- `OverriddenStateView::with_preimages(inner, preimage_overrides)` — unchanged

**Where to look:**
- `lib/storage_api/src/` — all trait definitions
- `lib/storage_api/src/overlay_buffer.rs` — OverlayBuffer
- `lib/sequencer/src/execution/block_applier.rs` — BlockApplier persistence orchestration
- `lib/sequencer/src/execution/block_executor.rs` — BlockExecutor with overlay

---

## 7. Common Review Concerns

When reviewing PRs that touch pipeline code, check for:

### Channel and Concurrency Issues
- **Unbounded channels:** Any `mpsc::channel` without a size limit is a memory leak risk
- **Deadlock potential:** Two components waiting on each other's channels; especially watch
  the `BlockCanonizer ↔ MainNodeCommandSource` channel (`canonized_blocks_for_execution`)
- **Cancellation safety:** Operations in `tokio::select!` branches must be cancellation-safe.
  If a future is dropped mid-execution, state must remain consistent.
- **Arc refcount contract in OverlayBuffer:** `OverriddenStateView` holds an `Arc` clone of
  the overlay map; if any view is still alive when `add_block` is called, the Arc assertion
  panics. Ensure VM execution fully drops the view before `add_block`.

### State Consistency
- **override_allowed correctness:** Only Rebuild and external node should allow overrides.
  A bug here could silently overwrite canonical state. Now determined in `BlockApplier` from
  `BlockCommandType`, not in `BlockExecutor`.
- **Block number continuity:** `BlockContextProvider.next_block_number` must match the actual
  block being produced. It is incremented in `on_canonical_state_change()` — if this is not
  called after each block, the counter drifts.
- **Partial persistence:** If adding a new store to BlockApplier, consider crash recovery
- **State view freshness:** `BlockExecutor` reads state at `block_number - 1` via
  `OverlayBuffer`. If the overlay is stale (e.g., block N-1 executed but not in overlay or
  base), the bail in `sync_with_base_and_build_view_for_block` will fire.
- **OverlayBuffer contiguity:** `add_block` panics if block number is not contiguous with
  the last overlay. If `BlockExecutor` restarts or skips a block, the overlay becomes invalid.

### Pipeline Topology
- **Buffer size changes:** Changing `OUTPUT_BUFFER_SIZE` affects backpressure for the entire
  pipeline above that component
- **New components:** Adding a pipeline stage changes the backpressure profile and increases
  latency. Consider whether the new stage needs to be in the critical path.
- **Conditional components:** `pipe_opt()` and `pipe_if()` must preserve type compatibility
- **BlockCanonizer absence in EN pipeline:** The EN pipeline correctly skips `BlockCanonizer`
  since it only replays. Any future EN change that adds non-replay block production must
  consider adding a canonizer or equivalent fence.

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
| BlockExecutor | `lib/sequencer/src/execution/block_executor.rs` | Execution + overlay buffer |
| BlockCanonizer | `lib/sequencer/src/execution/block_canonizer.rs` | Canonization fence |
| BlockApplier | `lib/sequencer/src/execution/block_applier.rs` | Persistence orchestration |
| VM execution | `lib/sequencer/src/execution/execute_block_in_vm.rs` | Pure function VM execution |
| BlockContextProvider | `lib/sequencer/src/execution/block_context_provider.rs` | Tx selection, block context |
| Command source | `node/bin/src/command_source.rs` | Channel-based block production |
| Transaction pool | `lib/mempool/src/pool.rs` | Subpool priority ordering |
| Replay storage | `lib/storage_api/src/replay.rs` | ReadReplay/WriteReplay traits |
| State storage | `lib/storage_api/src/state.rs` | ReadStateHistory/WriteState traits |
| State override view | `lib/storage_api/src/state_override_view.rs` | OverriddenStateView, OverrideProvider |
| Overlay buffer | `lib/storage_api/src/overlay_buffer.rs` | In-memory state diffs |
| Tree manager | `node/bin/src/tree_manager.rs` | Merkle tree updates |
| Batcher | `node/bin/src/batcher/mod.rs` | Batch boundary logic |
| Fee provider | `lib/sequencer/src/execution/fee_provider.rs` | Pubdata pricing, Option<PubdataMode> |
| Node config | `node/bin/src/config/mod.rs` | Config structs, Option fields for EN |
