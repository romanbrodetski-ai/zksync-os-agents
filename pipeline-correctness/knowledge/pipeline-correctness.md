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

Note: The EN pipeline has no `BlockCanonizer` — EN blocks are already canonized by the main node.

### Three-Stage Block Processing

Block processing is split into three pipeline components:

1. **BlockExecutor** — executes blocks in the VM, maintains in-memory state overlay
2. **BlockCanonizer** — consensus fence that ensures only canonized blocks proceed
3. **BlockApplier** — persists blocks to storage (WAL, state, repository)

This replaces the previous unified `Sequencer` component. The split enables consensus
integration by inserting a canonization fence between execution and persistence.

#### BlockExecutor

- Input: `BlockCommand` (from CommandSource)
- Output: `(BlockOutput, ReplayRecord, BlockCommandType)` (to BlockCanonizer)
- `OUTPUT_BUFFER_SIZE = 1`

The BlockExecutor's run loop for each block:
1. Receive `BlockCommand` from upstream
2. Check production limit (for Produce commands)
3. Prepare the command via `BlockContextProvider`
4. Build state view from `OverlayBuffer` (syncs with persisted base + in-memory overlays)
5. Call `execute_block_in_vm()` — a stateless pure function
6. Update mempool state via `BlockContextProvider::on_canonical_state_change()`
7. Add block to `OverlayBuffer` (storage writes + preimages)
8. Send `(BlockOutput, ReplayRecord, BlockCommandType)` downstream

**Key difference from old Sequencer:** BlockExecutor does NOT persist to storage. Instead,
it maintains an `OverlayBuffer` that tracks in-memory state diffs for blocks that haven't
been persisted yet. This allows BlockExecutor to run ahead of BlockApplier.

#### BlockCanonizer

- Input/Output: `(BlockOutput, ReplayRecord, BlockCommandType)`
- `OUTPUT_BUFFER_SIZE = 2`
- Internal `MAX_PRODUCED_QUEUE_SIZE = 2`

The BlockCanonizer serves as a consensus fence:
- **Replay** commands pass through directly (already canonized)
- **Produce/Rebuild** commands are proposed to consensus, queued in `produced_queue`,
  and only sent downstream when consensus confirms them
- Uses `tokio::select!` with two arms: receiving from consensus, receiving from upstream
- When consensus returns a block that wasn't locally produced, sends it back to
  CommandSource via `canonized_blocks_for_execution` channel for re-execution

Currently uses `NoopCanonization` (unbounded channel to self), which makes every proposed
block immediately canonical. Real consensus will replace this.

#### BlockApplier

- Input: `(BlockOutput, ReplayRecord, BlockCommandType)`
- Output: `(BlockOutput, ReplayRecord)`
- `OUTPUT_BUFFER_SIZE = 5`

The BlockApplier persists each block:
1. `WriteReplay::write()` — WAL record
2. `WriteState::add_block_result()` — key-value state diffs
3. `WriteRepository::populate()` — API-facing block/tx/receipt data

Sets `override_allowed = true` for Rebuild commands and external nodes.

**Key files:**
- `lib/sequencer/src/execution/block_executor.rs` — BlockExecutor
- `lib/sequencer/src/execution/block_canonizer.rs` — BlockCanonizer + BlockCanonization trait
- `lib/sequencer/src/execution/block_applier.rs` — BlockApplier
- `lib/sequencer/src/execution/execute_block_in_vm.rs` — Pure VM execution function
- `lib/sequencer/src/execution/mod.rs` — Module declarations and re-exports

---

## 2. Core Invariants

### 2.1 Sequential Block Processing

**What:** Blocks MUST be processed in strictly ascending block number order, one at a time,
through the entire pipeline. No block may be skipped, reordered, or processed concurrently
within the pipeline.

**Why:** Each block's execution depends on the state produced by the previous block.
The replay storage (`WriteReplay`) panics if a block is not the next after latest.

**Where enforced:**
- `BlockCommand::block_number()` — each command carries its block number (Replay/Rebuild only; Produce commands have no block number — it's assigned by BlockContextProvider)
- `CommandSource` — generates commands in sequential order (WAL replay, then produce)
- `WriteReplay::write()` — "MUST panic if the record is not next after the latest record"
- `TreeManager` — tracks processed block count, skips already-processed blocks
- `OverlayBuffer::add_block()` — asserts contiguous block numbers
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
- `MainNodeCommandSource::OUTPUT_BUFFER_SIZE = 1` — tight coupling with BlockExecutor
- `BlockExecutor::OUTPUT_BUFFER_SIZE = 1` — minimal buffer before BlockCanonizer,
  relying on BlockCanonizer's internal `produced_queue` for additional buffering
- `BlockCanonizer::OUTPUT_BUFFER_SIZE = 2` — allows mild persistence latency spikes
- `BlockCanonizer::MAX_PRODUCED_QUEUE_SIZE = 2` — internal backpressure: when 2 blocks
  are waiting for canonization, stops accepting from upstream
- `BlockApplier::OUTPUT_BUFFER_SIZE = 5` — downstream components (TreeManager etc.)
  get some breathing room
- `ExternalNodeCommandSource::OUTPUT_BUFFER_SIZE = 5` — allows EN source to buffer ahead

**Note:** `NoopCanonization` uses an unbounded channel internally, but backpressure is
still maintained through `MAX_PRODUCED_QUEUE_SIZE` and the bounded pipeline channels.

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
- Generated by `MainNodeCommandSource` in `run_loop()`
- `ProduceCommand` is a unit struct — carries no fields
- Block number, timestamp, and transaction selection determined by `BlockContextProvider::prepare_command()`
- Transactions selected from mempool by `BlockContextProvider`
- Uses `SealPolicy::Decide(timeout, max_txs)` — block seals on timeout or tx limit
- Uses `InvalidTxPolicy::RejectAndContinue` — skip bad txs, keep producing

### BlockCommand::Replay
- Two sources: WAL replay on startup, or canonized blocks from other nodes (via BlockCanonizer → CommandSource feedback loop)
- Transactions come from the `ReplayRecord` itself (predetermined)
- Uses `SealPolicy::UntilExhausted { allowed_to_finish_early: false }` — must execute all txs
- Uses `InvalidTxPolicy::Abort` — any invalid tx is a fatal error (deterministic replay)

### BlockCommand::Rebuild
- Used to rollback to an earlier state and re-execute
- Carries a `RebuildCommand` with `replay_record` and `make_empty` flag
- Uses `SealPolicy::UntilExhausted { allowed_to_finish_early: true }`
- Goes through consensus canonization in BlockCanonizer (same as Produce)
- `override_allowed = true` in BlockApplier

**Key distinction:** A Replay that fails is a critical error (the canonical chain is
inconsistent). A Produce that fails can be retried with different transactions.

**Where defined:** `lib/sequencer/src/model/blocks.rs`

---

## 4. Command Source Design

### Main Node (`MainNodeCommandSource`)

The main node command source generates a stream of commands:

1. **Replay phase**: Replays WAL records from `starting_block` to `replay_until`
   (via `forward_range_with`)
2. **Rebuild phase** (optional): If `rebuild_options` is set, rebuilds blocks from
   `rebuild_from_block` to `last_block_in_wal`
3. **Produce phase** (`run_loop`): Uses `tokio::select!` to either:
   - Bail if a replay record arrives on `replays_to_execute` (leader-only mode currently)
   - Send `Produce` commands downstream

The `replays_to_execute` channel connects BlockCanonizer back to CommandSource. Currently,
receiving any replay in `run_loop` is treated as an error (leader-only mode). When consensus
is fully integrated, this will switch to handling replays from other leaders.

**Where to look:**
- `node/bin/src/command_source.rs` — `MainNodeCommandSource` and `run_loop()`

### External Node (`ExternalNodeCommandSource`)

- Receives `ReplayRecord`s from the main node via a channel
- Wraps each in `BlockCommand::Replay` and sends downstream
- Optional `up_to_block` limit for syncing to a specific block
- No BlockCanonizer in EN pipeline (blocks already canonized)

---

## 5. OverlayBuffer (State Overlay Mechanism)

Because BlockExecutor runs ahead of BlockApplier (persistence), blocks N+1, N+2, etc.
may execute before block N is persisted. The `OverlayBuffer` bridges this gap by
maintaining in-memory state diffs.

### How it works:
- `OverlayBuffer` wraps `Arc<BTreeMap<BlockNumber, BlockOverlay>>`
- Each `BlockOverlay` contains `HashMap<B256, B256>` for storage writes and
  `HashMap<B256, Vec<u8>>` for preimages
- `sync_with_base_and_build_view_for_block()`: purges already-persisted blocks,
  builds `OverriddenStateView` with overlay as provider
- `add_block()`: appends a new block overlay (asserts contiguity)
- Lookups search overlays in reverse order (most recent first)

### Key invariants:
- **Arc refcount = 1 during mutation**: Both `add_block()` and `purge_already_persisted_blocks()`
  assert `Arc::strong_count == 1`. Views must be dropped before the next mutation.
- **Contiguous block numbers**: `add_block()` asserts the new block is `last + 1`
- **Overlay range validity**: When overlays exist, they must span from `base_latest + 1`
  to `block_number_to_execute - 1`

**Where to look:**
- `lib/storage_api/src/overlay_buffer.rs`

---

## 6. Transaction Selection Priority

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

## 7. State and Persistence Model

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
The `execute_block_in_vm()` function receives a state view built by `OverlayBuffer`.
The view combines the persisted base state with in-memory overlays for blocks that
haven't been persisted yet. Forced preimages are injected via `OverriddenStateView::with_preimages()`.

**Where to look:**
- `lib/storage_api/src/` — all trait definitions
- `lib/storage_api/src/overlay_buffer.rs` — OverlayBuffer and overlay view
- `lib/sequencer/src/execution/block_executor.rs` — BlockExecutor state management
- `lib/sequencer/src/execution/execute_block_in_vm.rs` — VM execution

---

## 8. OverriddenStateView (Generic Override Mechanism)

`OverriddenStateView<V: ViewState, O: OverrideProvider>` wraps a base state view with
an override provider. The `OverrideProvider` trait abstracts over different override sources:

- `OwnedOverrides` — HashMap-based, used for RPC `eth_call` with `StateOverride`
- `Arc<BTreeMap<BlockNumber, BlockOverlay>>` — used by `OverlayBuffer` in the execution pipeline

Convenience constructors:
- `with_state_overrides()` — from RPC StateOverride (owned)
- `with_preimages()` — preimage-only overrides (owned)
- `new()` — generic constructor for any override provider

**Where to look:**
- `lib/storage_api/src/state_override_view.rs`

---

## 9. Common Review Concerns

When reviewing PRs that touch pipeline code, check for:

### Channel and Concurrency Issues
- **Unbounded channels:** Any `mpsc::channel` without a size limit is a memory leak risk.
  Note: `NoopCanonization` intentionally uses an unbounded channel.
- **Deadlock potential:** Two components waiting on each other's channels. The
  BlockCanonizer ↔ CommandSource feedback loop (`canonized_blocks_for_execution`) is
  a potential deadlock site if the channel fills up.
- **Cancellation safety:** Operations in `tokio::select!` branches must be cancellation-safe.
  `PeekableReceiver::recv()` and `mpsc::UnboundedReceiver::recv()` are cancellation-safe.
- **`tokio::select!` fairness:** BlockCanonizer's select has no bias — both arms can win.
  With NoopCanonization, the consensus arm is always ready immediately after a propose,
  so ordering depends on tokio's random selection.

### State Consistency
- **override_allowed correctness:** Only Rebuild and external node should allow overrides.
  A bug here could silently overwrite canonical state. This logic is in BlockApplier.
- **Block number continuity:** Any new component that touches block numbers must maintain
  the sequential invariant
- **Partial persistence:** If adding a new store to BlockApplier, consider crash recovery
- **OverlayBuffer consistency:** The overlay must stay contiguous and have refcount=1
  during mutations. Any code that holds an Arc clone too long will panic.

### Pipeline Topology
- **Buffer size changes:** Changing `OUTPUT_BUFFER_SIZE` affects backpressure for the entire
  pipeline above that component
- **New components:** Adding a pipeline stage changes the backpressure profile and increases
  latency. Consider whether the new stage needs to be in the critical path.
- **Conditional components:** `pipe_opt()` and `pipe_if()` must preserve type compatibility

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

## 10. File Reference Map

Quick reference for the most important files in the pipeline:

| Component | Path | What to look for |
|-----------|------|-----------------|
| Pipeline framework | `lib/pipeline/src/traits.rs` | PipelineComponent trait |
| Pipeline builder | `lib/pipeline/src/builder.rs` | pipe(), spawn() |
| Pipeline construction | `node/bin/src/lib.rs` | Main & EN pipeline wiring |
| Block commands | `lib/sequencer/src/model/blocks.rs` | Command types, seal policies |
| BlockExecutor | `lib/sequencer/src/execution/block_executor.rs` | Execution + overlay management |
| BlockCanonizer | `lib/sequencer/src/execution/block_canonizer.rs` | Consensus fence |
| BlockApplier | `lib/sequencer/src/execution/block_applier.rs` | Persistence |
| VM execution | `lib/sequencer/src/execution/execute_block_in_vm.rs` | Pure function VM execution |
| BlockContextProvider | `lib/sequencer/src/execution/block_context_provider.rs` | Tx selection, block context |
| Command source | `node/bin/src/command_source.rs` | WAL replay + block production |
| OverlayBuffer | `lib/storage_api/src/overlay_buffer.rs` | In-memory state overlay |
| State override view | `lib/storage_api/src/state_override_view.rs` | OverrideProvider trait + OverriddenStateView |
| Transaction pool | `lib/mempool/src/pool.rs` | Subpool priority ordering |
| Replay storage | `lib/storage_api/src/replay.rs` | ReadReplay/WriteReplay traits |
| State storage | `lib/storage_api/src/state.rs` | ReadStateHistory/WriteState traits |
| Tree manager | `node/bin/src/tree_manager.rs` | Merkle tree updates |
| Batcher | `node/bin/src/batcher/mod.rs` | Batch boundary logic |
| Fee provider | `lib/sequencer/src/execution/fee_provider.rs` | Pubdata pricing, Option<PubdataMode> |
| Node config | `node/bin/src/config/mod.rs` | Config structs, Option fields for EN |
