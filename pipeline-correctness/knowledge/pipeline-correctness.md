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
ExternalNodeCommandSource → BlockExecutor → BlockCanonizer → BlockApplier
→ [optional: RevmConsistencyChecker] → TreeManager → [optional: BatchVerification]
```

---

## 2. Core Invariants

### 2.1 Sequential Block Processing

**What:** Blocks MUST be processed in strictly ascending block number order, one at a time,
through the entire pipeline. No block may be skipped, reordered, or processed concurrently
within the pipeline.

**Why:** Each block's execution depends on the state produced by the previous block.
`BlockContextProvider` tracks `next_block_number` and would produce wrong block context
if blocks arrived out of order. The replay storage (`WriteReplay`) panics if a block is
not the next after latest.

**Where enforced:**
- `BlockContextProvider` — maintains `next_block_number`, `next_l1_priority_id`, `block_hashes_for_next_block`
- `WriteReplay::write()` — "MUST panic if the record is not next after the latest record"
- `TreeManager` — tracks processed block count, skips already-processed blocks
- Pipeline channel ordering — single-producer single-consumer channels guarantee FIFO

**Failure mode:** If a component buffers and reorders, or if a tokio::select! arm processes
items out of order, the chain state becomes corrupted.

### 2.2 Canonization Fence

**What:** Only blocks that have been canonized by consensus may proceed past `BlockCanonizer`
to persistence. Produced blocks must wait for consensus confirmation before being sent
downstream.

**Why:** In a multi-node setup, the leader proposes blocks but another node might become
leader and propose a different block for the same number. The canonization fence prevents
persisting blocks that consensus hasn't agreed on.

**Where enforced:**
- `BlockCanonizer` — the core gatekeeper. Has a `produced_queue` (max size 2) of blocks
  waiting for canonization confirmation.
- When a Produce/Rebuild block arrives from BlockExecutor, it's proposed to consensus and
  queued. When consensus returns a canonized block, it's matched against the queue.
- When a Replay block arrives, it passes through immediately (already canonical).

**Critical behavior:**
- If the canonized block matches the front of `produced_queue` → send downstream (same node produced it)
- If `produced_queue` is empty when canonized arrives → send to `canonized_blocks_for_execution`
  channel, which routes back to CommandSource for re-execution as a Replay (another node's block)
- If canonized block does NOT match the queued block → `anyhow::bail!` with "canonized replay
  record mismatch" — this means consensus diverged, continuing would corrupt state

**Where to look:** `lib/sequencer/src/execution/block_canonizer.rs`

### 2.3 Backpressure Model

**What:** Each pipeline component declares `OUTPUT_BUFFER_SIZE` which creates a bounded
channel to the next component. When the downstream component is slow, the upstream component
blocks on send, propagating backpressure up the chain.

**Why:** Without backpressure, a fast producer (BlockExecutor) would outrun a slow consumer
(e.g., TreeManager doing disk I/O), consuming unbounded memory.

**Key buffer sizes and their rationale:**
- `BlockCanonizer::OUTPUT_BUFFER_SIZE = 2` — "allow for mild persistence latency spikes
  without allowing BlockCanonizer to be too far ahead"
- `BlockCanonizer::MAX_PRODUCED_QUEUE_SIZE = 2` — limits how far ahead execution can get
  before consensus confirms. Uses `tokio::select!` with a guard `if produced_queue.len() < MAX_PRODUCED_QUEUE_SIZE`
- `BlockApplier::OUTPUT_BUFFER_SIZE = 5` — persistence is generally fast

**Failure mode:** If someone changes a buffer size to 0, the component won't start processing
the next item until the current one is picked up. If changed to unbounded (or very large),
backpressure is lost and memory can grow without bound.

### 2.4 L1 Priority Transaction Ordering

**What:** L1 priority transactions must be processed in strictly monotonic order by
`starting_l1_priority_id`. No ID may be skipped or duplicated.

**Why:** The L1 smart contract enforces this ordering. If the sequencer skips a priority
transaction, the batch will fail to verify on L1, and the chain effectively halts.

**Where enforced:**
- `BlockContextProvider` tracks `next_l1_priority_id`, increments after each L1 tx
- `ReplayRecord::new()` asserts: "First L1 tx priority id must match next_l1_priority_id"
- `Pool::best_transactions_stream()` yields L1 txs from `L1Subpool` before L2 txs

### 2.5 Replay Idempotency

**What:** Replaying a block that was already executed must produce the same result, or be
safely skippable. The `override_allowed` flag on `WriteState` and `WriteReplay` controls
whether re-execution overwrites existing data.

**Why:** On restart, the node replays blocks from WAL to rebuild in-memory state.
Components must handle seeing the same block number again without corrupting data.

**Where enforced:**
- `BlockApplier` sets `override_allowed = true` for `Rebuild` commands and external nodes
- `TreeManager` skips blocks already in the tree (idempotent)
- `WriteReplay::write()` returns false (no write) for existing blocks when override not allowed

### 2.6 Gapless L1 Commitment

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
- Generated by `ConsensusNodeCommandSource` when node is leader
- Transactions selected from mempool by `BlockContextProvider`
- Uses `SealPolicy::Decide(timeout, max_txs)` — block seals on timeout or tx limit
- Uses `InvalidTxPolicy::RejectAndContinue` — skip bad txs, keep producing
- Goes through canonization (proposed to consensus, waits for confirmation)

### BlockCommand::Replay
- Two sources: WAL replay on startup, or canonized blocks from other nodes via consensus
- Transactions come from the `ReplayRecord` itself (predetermined)
- Uses `SealPolicy::UntilExhausted` — must execute all txs in the record
- Uses `InvalidTxPolicy::Abort` — any invalid tx is a fatal error (deterministic replay)
- Bypasses canonization in `BlockCanonizer` (already canonical)

### BlockCommand::Rebuild
- Used to rollback to an earlier state and re-execute
- Like Produce but with `override_allowed = true` in `BlockApplier`
- Goes through canonization like Produce

**Key distinction:** A Replay that fails is a critical error (the canonical chain is
inconsistent). A Produce that fails can be retried with different transactions.

**Where defined:** `lib/sequencer/src/model/blocks.rs`

---

## 4. Consensus Integration

### Single-node (NoopCanonization)
- Simple `mpsc::channel(1)` loopback — propose sends, next_canonized receives
- Used when `consensus.enabled = false`
- `LeadershipSignal::AlwaysLeader` — CommandSource always produces

### Multi-node (OpenRaft)
- Raft cluster with election timeout 150-300ms, heartbeat 50ms
- `propose()` calls `raft.client_write(record)` — goes through Raft log
- `next_canonized()` receives from a channel fed by the Raft state machine
- `LeadershipSignal::Watch` — CommandSource watches for role changes

### Leadership Transition

When leadership changes:
1. `ConsensusNodeCommandSource` receives role change via `LeadershipSignal`
2. If became leader: starts sending `Produce` commands
3. If became replica: stops producing, waits for canonized blocks from new leader
4. `BlockCanonizer` may have blocks in `produced_queue` that will never be canonized
   → the canonized block from new leader won't match → `bail!`

**This is a known sharp edge.** During leadership transitions, the pipeline may error and
restart. This is by design — it's simpler and safer than trying to gracefully drain the
produced queue.

**Where to look:**
- `lib/raft/src/model.rs` — consensus types
- `lib/sequencer/src/execution/block_canonizer.rs` — canonization logic
- `node/bin/src/command_source.rs` — leadership-aware command generation

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

### OverlayBuffer
`BlockExecutor` uses an `OverlayBuffer` to accumulate state diffs in memory during execution.
This buffer is separate from the persistent state and is used to provide state views for
subsequent block execution without waiting for persistence.

**Where to look:**
- `lib/storage_api/src/` — all trait definitions
- `lib/sequencer/src/execution/block_applier.rs` — write orchestration

---

## 7. Common Review Concerns

When reviewing PRs that touch pipeline code, check for:

### Channel and Concurrency Issues
- **Unbounded channels:** Any `mpsc::channel` without a size limit is a memory leak risk
- **Deadlock potential:** Two components waiting on each other's channels
- **`tokio::select!` bias:** The first arm in select! has slight priority — ensure
  the canonized block arm and the produced block arm in `BlockCanonizer` don't starve each other
- **Cancellation safety:** Operations in `tokio::select!` branches must be cancellation-safe.
  If a future is dropped mid-execution, state must remain consistent.

### State Consistency
- **override_allowed correctness:** Only Rebuild and external node should allow overrides.
  A bug here could silently overwrite canonical state.
- **Block number continuity:** Any new component that touches block numbers must maintain
  the sequential invariant
- **Partial persistence:** If adding a new store to BlockApplier, consider crash recovery

### Pipeline Topology
- **Buffer size changes:** Changing `OUTPUT_BUFFER_SIZE` affects backpressure for the entire
  pipeline above that component
- **New components:** Adding a pipeline stage changes the backpressure profile and increases
  latency. Consider whether the new stage needs to be in the critical path.
- **Conditional components:** `pipe_opt()` and `pipe_if()` must preserve type compatibility

### Consensus Interaction
- **Produced blocks during transition:** During leadership changes, the produced_queue in
  BlockCanonizer may contain stale blocks
- **NoopCanonization equivalence:** Changes to canonization logic must work identically
  for both Noop and OpenRaft implementations

---

## 8. File Reference Map

Quick reference for the most important files in the pipeline:

| Component | Path | What to look for |
|-----------|------|-----------------|
| Pipeline framework | `lib/pipeline/src/traits.rs` | PipelineComponent trait |
| Pipeline builder | `lib/pipeline/src/builder.rs` | pipe(), spawn() |
| Pipeline construction | `node/bin/src/lib.rs` | Main & EN pipeline wiring |
| Block commands | `lib/sequencer/src/model/blocks.rs` | Command types, seal policies |
| BlockExecutor | `lib/sequencer/src/execution/block_executor.rs` | VM execution orchestration |
| BlockApplier | `lib/sequencer/src/execution/block_applier.rs` | Persistence logic |
| BlockCanonizer | `lib/sequencer/src/execution/block_canonizer.rs` | Consensus fence |
| BlockContextProvider | `lib/sequencer/src/execution/block_context_provider.rs` | Tx selection, block context |
| VM execution | `lib/sequencer/src/execution/execute_block_in_vm.rs` | Actual block execution |
| Command source | `node/bin/src/command_source.rs` | Leadership-aware block production |
| Transaction pool | `lib/mempool/src/pool.rs` | Subpool priority ordering |
| Replay storage | `lib/storage_api/src/replay.rs` | ReadReplay/WriteReplay traits |
| State storage | `lib/storage_api/src/state.rs` | ReadStateHistory/WriteState traits |
| Consensus types | `lib/raft/src/model.rs` | BlockCanonization, LeadershipSignal |
| Tree manager | `node/bin/src/tree_manager.rs` | Merkle tree updates |
| Batcher | `node/bin/src/batcher/mod.rs` | Batch boundary logic |
