## Feature

This agent owns main-node transaction inclusion behavior across a local server restart. The feature boundary is narrow: a transaction that was already included before restart must remain visible after replay, and the restarted node must resume progressing the chain.

## In Scope

- Main-node startup replay from `BlockReplayStorage`
- Persistence of included blocks into replay/state/repositories before restart
- RPC-visible receipt continuity after restart
- Post-restart progression from included to safe/finalized
- Post-restart ability to include new transactions again

## Out Of Scope

- External-node restart behavior
- Block rebuild / revert flows
- Batch verification correctness beyond its effect on safe/finalized progression
- Consensus with another leader; this range still uses single-node / loopback canonization

## Entrypoints And State

- `node/bin/src/lib.rs`: startup wiring and main-node pipeline assembly
- `node/bin/src/command_source.rs`: WAL replay before resuming production
- `lib/sequencer/src/execution/block_executor.rs`: executes blocks against persisted state plus in-memory overlay
- `lib/sequencer/src/execution/block_applier.rs`: persists replay/state/repository data
- `lib/storage/src/db/replay.rs`: durable replay WAL used for restart recovery

Key state:

- `BlockReplayStorage` is the restart source of truth for executed historical blocks.
- `OverlayBuffer` allows execution to run ahead of persisted state inside one process, but its contents are lost on restart.
- Receipts stay observable only if repository persistence completed before the restart.

## Invariants

- A receipt observed before restart must remain byte-for-byte stable after restart.
- Restart replay must not erase an already persisted balance effect.
- A block included before restart must still reach `safe` and `finalized` after restart.
- After replay finishes, the main node must resume producing fresh blocks.

## Change Range Notes

- `2f588c2c`: sequencer split into `BlockExecutor` and `BlockApplier`, with `OverlayBuffer`
- `6e88dead`: single-node consensus-aware canonization wiring added around the same path
- `ce075bcf`: batch persistence delay fix; relevant only to post-inclusion settlement timing

## Suspicious Areas

- The restart boundary now sits between in-memory execution overlay and durable persistence; a regression here would show up as receipt loss or stalled production after restart.
- `MainNodeCommandSource` now has an explicit replay phase and then a steady-state production loop; if replay completion or handoff regresses, old receipts may survive while new transactions stop being included.
