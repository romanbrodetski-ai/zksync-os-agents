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
MainNodeCommandSource → BlockExecutor → BlockCanonizer → BlockApplier
→ [optional: RevmConsistencyChecker] → TreeManager
→ ProverInputGenerator → Batcher → BatchVerification → FriProving
→ GaplessCommitter → L1Sender(Commit) → SnarkProving → GaplessL1ProofSender
→ L1Sender(Proof) → PriorityTree → L1Sender(Execute) → BatchSink
```

Note: `BlockCanonizer` has a feedback channel back to `MainNodeCommandSource`.
When the canonizer receives a block from consensus that was NOT produced locally,
it sends the `ReplayRecord` back to the command source via `canonized_blocks_for_execution`.

### External Node Pipeline (replay only)

```
ExternalNodeCommandSource → BlockExecutor → BlockApplier
→ [optional: RevmConsistencyChecker] → TreeManager → [optional: BatchVerificationClient]
```

External nodes skip `BlockCanonizer` because all their blocks arrive as `Replay` commands
(already canonized by the network).

### Three-Component Block Processing

The block processing pipeline is split into three stages:

1. **BlockExecutor** — Executes blocks in the VM. Uses an in-memory `OverlayBuffer` to
   track uncommitted state diffs so the next block can read from the latest state without
   waiting for persistence. Does NOT write to disk.
2. **BlockCanonizer** (main node only) — Serves as a consensus fence. Replay commands
   pass through immediately. Produce/Rebuild commands are sent to consensus for
   canonization; the component waits for consensus to return them before forwarding
   downstream. If consensus returns a block not produced locally, it is sent back to
   `MainNodeCommandSource` for execution via the `canonized_blocks_for_execution` channel.
3. **BlockApplier** — Persists blocks to WAL (`WriteReplay`), state storage (`WriteState`),
   and repository (`WriteRepository`). Determines `override_allowed` from the command type
   and node role.

**Key files:**
- `lib/sequencer/src/execution/block_executor.rs` — `BlockExecutor` pipeline component
- `lib/sequencer/src/execution/block_canonizer.rs` — `BlockCanonizer` + `BlockCanonization` trait
- `lib/sequencer/src/execution/block_applier.rs` — `BlockApplier` pipeline component
- `lib/sequencer/src/execution/execute_block_in_vm.rs` — Pure block execution function
- `lib/sequencer/src/execution/mod.rs` — Module re-exports

---

## 2. Core Invariants

### 2.1 Sequential Block Processing

**What:** Blocks MUST be processed in strictly ascending block number order, one at a time,
through the entire pipeline. No block may be skipped, reordered, or processed concurrently
within the pipeline.

**Why:** Each block's execution depends on the state produced by the previous block.
The replay storage (`WriteReplay`) panics if a block is not the next after latest.
The `OverlayBuffer` in `BlockExecutor` enforces contiguous block numbering.

**Where enforced:**
- `BlockCommand::command_type()` — each command carries its type for downstream routing
- `CommandSource` — generates commands in sequential order (WAL replay, then produce)
- `WriteReplay::write()` — "MUST panic if the record is not next after the latest record"
- `OverlayBuffer::add_block()` — panics if block_number is not contiguous with last overlay
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
- `MainNodeCommandSource::OUTPUT_BUFFER_SIZE = 1` — tight coupling with BlockExecutor;
  the command source should not race far ahead.
- `BlockExecutor::OUTPUT_BUFFER_SIZE = 1` — minimal buffer before BlockCanonizer, which
  has its own internal `produced_queue` (max size 2). This allows executing block X+2
  while X+1 is buffered and X is being canonized.
- `BlockCanonizer::OUTPUT_BUFFER_SIZE = 2` — allows mild persistence latency spikes in
  BlockApplier without stalling canonization.
- `BlockApplier::OUTPUT_BUFFER_SIZE = 5` — moderate buffer for downstream (TreeManager etc.).
- `ExternalNodeCommandSource::OUTPUT_BUFFER_SIZE = 5` — allows feeding blocks ahead.

**Failure mode:** If someone changes a buffer size to 0, the `mpsc::channel(0)` call panics
(tokio requires capacity >= 1). The pipeline traits document that 0 means lockstep, but
this is aspirational — a future backpressure mechanism may support it. If changed to very
large, backpressure is lost and memory can grow without bound.

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
- `BlockExecutor`'s `OverlayBuffer` provides state for re-executed blocks via in-memory overlays

### 2.5 Gapless L1 Commitment

**What:** Batches must be committed, proved, and executed on L1 in strict sequential order.
No batch number may be skipped.

**Why:** The L1 contract validates batch continuity. A gap would cause the commit transaction
to revert.

**Where enforced:**
- `GaplessCommitter` pipeline component — ensures commit order
- `GaplessL1ProofSender` — ensures proof order
- `Batcher` — starts from `last_executed_batch + 1`, reconstructs already-committed batches

### 2.6 OverlayBuffer Consistency

**What:** The `OverlayBuffer` in `BlockExecutor` tracks in-memory state diffs for blocks
that have been executed but not yet persisted by `BlockApplier`. It must remain consistent
with both the persisted base state and the execution sequence.

**Why:** Because `BlockExecutor` does not persist to disk, it cannot read its own previous
outputs from the state store. The overlay buffer bridges this gap, layering uncommitted
diffs on top of the last persisted state.

**Where enforced:**
- `OverlayBuffer::sync_with_base_and_build_view_for_block()` — purges overlays already
  persisted, validates contiguity, builds a composed state view
- `OverlayBuffer::add_block()` — enforces contiguous block numbering, asserts Arc refcount
  is 1 (no outstanding borrows)
- `OverriddenStateView` with `Arc<BTreeMap<BlockNumber, BlockOverlay>>` as `OverrideProvider`
  — searches overlays in reverse order (most recent first) with O(1) per-block lookups

**Failure mode:** If the base state advances past the overlay range without purging, or if
overlays have gaps, `sync_with_base_and_build_view_for_block` will bail. If the Arc refcount
is > 1 during mutation, the assertion fires (would cause an expensive Arc clone otherwise).

---

## 3. Command Types and Their Semantics

Understanding the three command types is critical for reviewing pipeline changes:

### BlockCommand::Produce
- Generated by `MainNodeCommandSource` in its main loop
- `ProduceCommand` is a unit struct — block_number, block_time, and max_transactions_in_block
  are determined by `BlockContextProvider::prepare_command()` from the provider's internal state
- Transactions selected from mempool by `BlockContextProvider`
- Uses `SealPolicy::Decide(timeout, max_txs)` — block seals on timeout or tx limit
- Uses `InvalidTxPolicy::RejectAndContinue` — skip bad txs, keep producing

### BlockCommand::Replay
- Two sources: WAL replay on startup, or canonized blocks from other nodes
- Transactions come from the `ReplayRecord` itself (predetermined)
- Uses `SealPolicy::UntilExhausted { allowed_to_finish_early: false }` — must execute all txs
- Uses `InvalidTxPolicy::Abort` — any invalid tx is a fatal error (deterministic replay)

### BlockCommand::Rebuild
- Used to rollback to an earlier state and re-execute
- Carries a `ReplayRecord` and a `make_empty` flag
- Uses `SealPolicy::UntilExhausted { allowed_to_finish_early: true }`
- Uses `InvalidTxPolicy::RejectAndContinue`

**Key distinction:** A Replay that fails is a critical error (the canonical chain is
inconsistent). A Produce that fails can be retried with different transactions.

**Where defined:** `lib/sequencer/src/model/blocks.rs`

---

## 4. Command Source Design

### Main Node (`MainNodeCommandSource`)

The main node command source is a pipeline component that:

1. **Replay phase**: Forwards WAL replay records from `starting_block` to `replay_until`
   using `ReadReplayExt::forward_range_with()`
2. **Rebuild phase** (optional): If `rebuild_options` is set, sends `Rebuild` commands
   for blocks from `rebuild_from_block` to `last_block_in_wal`
3. **Main loop**: Sends `Produce` commands continuously via `tokio::select!`, while also
   listening on `replays_to_execute` for blocks sent back by `BlockCanonizer`. Currently,
   receiving a replay in the main loop is treated as an error (leader-only mode), but the
   channel infrastructure is in place for future consensus integration.

**Key change:** `OUTPUT_BUFFER_SIZE = 1` (previously 5). The command source is now tightly
coupled with `BlockExecutor` since the executor uses `OverlayBuffer` and doesn't need a
large lookahead.

**Where to look:**
- `node/bin/src/command_source.rs` — `MainNodeCommandSource` and `ExternalNodeCommandSource`

### External Node (`ExternalNodeCommandSource`)

- Receives `ReplayRecord`s from the main node via a channel
- Wraps each in `BlockCommand::Replay` and sends downstream
- Optional `up_to_block` limit for syncing to a specific block
- `OUTPUT_BUFFER_SIZE = 5`

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
The `BlockApplier` persists each block to three stores, in this order:
1. `WriteReplay::write()` — WAL record (source of truth for block history)
2. `WriteState::add_block_result()` — key-value state diffs
3. `WriteRepository::populate()` — API-facing block/tx/receipt data

**Important:** These are NOT atomic across all three stores. A crash between step 1 and 3
means the WAL has the block but the repository doesn't. On restart, replay from WAL will
fill in the gaps.

### Execution state (BlockExecutor + OverlayBuffer)
The `BlockExecutor` does not persist state. Instead, it uses an `OverlayBuffer` that:
1. Syncs with the base persisted state (`ReadStateHistory`) — purges overlays for blocks
   already persisted
2. Builds a composite state view: base state at the latest persisted block, overlaid with
   in-memory diffs from subsequent blocks
3. After execution, adds the new block's diffs to the overlay

This decoupling allows the executor to run ahead of persistence. The overlay acts as a
write-back cache. The `OverriddenStateView` composes the base state with the overlay
using the `OverrideProvider` trait.

### Read path
- `ReadStateHistory::state_view_at(block_number)` — get read-only state at a point in time
- `ReadReplay::get_replay_record(block_number)` — get canonical block data
- `ReadRepository` — API queries (blocks, transactions, receipts)

### Block Execution State View
The `execute_block_in_vm()` function receives a pre-built state view from `BlockExecutor`.
This view is constructed by `OverlayBuffer::sync_with_base_and_build_view_for_block()`.
Forced preimages are injected via `OverriddenStateView::with_preimages()`.

**Where to look:**
- `lib/storage_api/src/` — all trait definitions
- `lib/storage_api/src/overlay_buffer.rs` — OverlayBuffer and OverrideProvider impl
- `lib/sequencer/src/execution/block_executor.rs` — BlockExecutor run loop
- `lib/sequencer/src/execution/block_applier.rs` — BlockApplier persistence
- `lib/sequencer/src/execution/execute_block_in_vm.rs` — Pure block execution function

---

## 7. BlockCanonizer and Consensus Integration

### Design

The `BlockCanonizer` is a pipeline component that sits between `BlockExecutor` and
`BlockApplier`. It implements a consensus fence through the `BlockCanonization` trait:

```
trait BlockCanonization {
    async fn propose(&self, record: ReplayRecord) -> Result<()>;
    async fn next_canonized(&mut self) -> Result<ReplayRecord>;
}
```

Currently, `NoopCanonization` is used — it routes proposed blocks directly back as
canonized (leader-only mode). The infrastructure supports future real consensus
implementations.

### Behavior by command type

- **Replay**: Passes through immediately to `BlockApplier`. Replay blocks are already
  canonical (from WAL or from another node).
- **Produce / Rebuild**: Sent to consensus via `propose()`. The block output is queued
  in `produced_queue` (max size `MAX_PRODUCED_QUEUE_SIZE = 2`). When `next_canonized()`
  returns, the canonizer matches it against the front of the queue. If matched, sends
  downstream. If no local block is pending (queue empty), the canonized block came from
  another node — it's sent to `canonized_blocks_for_execution` for the command source
  to feed back through the pipeline.

### Backpressure

The `produced_queue` limit (2) plus the `tokio::select!` guard
(`if produced_queue.len() < MAX_PRODUCED_QUEUE_SIZE`) creates backpressure on the
`BlockExecutor`. The executor can be at most 2 blocks ahead of consensus confirmation.

### Failure modes
- **Mismatch**: If the canonized record doesn't match the locally produced one, the node
  bails with "canonized replay record mismatch" — this means another node became leader.
- **Channel closure**: If `canonized_blocks_for_execution` channel closes, the canonizer
  errors out.

---

## 8. Common Review Concerns

When reviewing PRs that touch pipeline code, check for:

### Channel and Concurrency Issues
- **Unbounded channels:** Any `mpsc::channel` without a size limit is a memory leak risk
- **Deadlock potential:** The `BlockCanonizer` uses `tokio::select!` with two arms —
  receiving from consensus and receiving from executor. Both arms can block. The
  `produced_queue` size guard prevents the executor arm from being selected when the
  queue is full, ensuring the consensus arm can drain.
- **Cancellation safety:** Operations in `tokio::select!` branches must be cancellation-safe.
  If a future is dropped mid-execution, state must remain consistent.

### State Consistency
- **override_allowed correctness:** Only Rebuild and external node should allow overrides
  in `BlockApplier`. A bug here could silently overwrite canonical state.
- **Block number continuity:** Any new component that touches block numbers must maintain
  the sequential invariant
- **Partial persistence:** `BlockApplier` writes to three stores non-atomically. Consider
  crash recovery.
- **OverlayBuffer sync:** The overlay must stay in sync with the base state. If a block
  is persisted but the overlay isn't purged, the overlay would contain stale data. The
  `sync_with_base_and_build_view_for_block` method handles this, but changes to
  persistence timing could break the invariant.
- **Arc refcount:** `OverlayBuffer` uses `Arc<BTreeMap>` and asserts refcount == 1 before
  mutation. If a state view is held across an `add_block` call, this will panic.

### Pipeline Topology
- **Buffer size changes:** Changing `OUTPUT_BUFFER_SIZE` affects backpressure for the entire
  pipeline above that component
- **New components:** Adding a pipeline stage changes the backpressure profile and increases
  latency. Consider whether the new stage needs to be in the critical path.
- **Conditional components:** `pipe_opt()` and `pipe_if()` must preserve type compatibility
- **Feedback channels:** The `BlockCanonizer` → `MainNodeCommandSource` feedback channel
  creates a loop in the pipeline graph. Changes to either component must consider this
  coupling.

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

## 9. File Reference Map

Quick reference for the most important files in the pipeline:

| Component | Path | What to look for |
|-----------|------|-----------------|
| Pipeline framework | `lib/pipeline/src/traits.rs` | PipelineComponent trait |
| Pipeline builder | `lib/pipeline/src/builder.rs` | pipe(), spawn() |
| Pipeline construction | `node/bin/src/lib.rs` | Main & EN pipeline wiring |
| Block commands | `lib/sequencer/src/model/blocks.rs` | Command types, seal policies |
| BlockExecutor | `lib/sequencer/src/execution/block_executor.rs` | Execution + OverlayBuffer |
| BlockCanonizer | `lib/sequencer/src/execution/block_canonizer.rs` | Consensus fence |
| BlockApplier | `lib/sequencer/src/execution/block_applier.rs` | Persistence to WAL/state/repo |
| Block execution | `lib/sequencer/src/execution/execute_block_in_vm.rs` | Pure function VM execution |
| BlockContextProvider | `lib/sequencer/src/execution/block_context_provider.rs` | Tx selection, block context |
| Command source | `node/bin/src/command_source.rs` | Stream-based block production |
| Transaction pool | `lib/mempool/src/pool.rs` | Subpool priority ordering |
| Replay storage | `lib/storage_api/src/replay.rs` | ReadReplay/WriteReplay traits |
| State storage | `lib/storage_api/src/state.rs` | ReadStateHistory/WriteState traits |
| OverlayBuffer | `lib/storage_api/src/overlay_buffer.rs` | In-memory state overlay for executor |
| State override view | `lib/storage_api/src/state_override_view.rs` | OverriddenStateView + OverrideProvider |
| Tree manager | `node/bin/src/tree_manager.rs` | Merkle tree updates |
| Batcher | `node/bin/src/batcher/mod.rs` | Batch boundary logic |
| Fee provider | `lib/sequencer/src/execution/fee_provider.rs` | Pubdata pricing, Option<PubdataMode> |
| Node config | `node/bin/src/config/mod.rs` | Config structs, Option fields for EN |
