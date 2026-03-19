## Files Inspected

- `zksync-os-server/node/bin/src/lib.rs`
- `zksync-os-server/node/bin/src/command_source.rs`
- `zksync-os-server/lib/sequencer/src/execution/block_executor.rs`
- `zksync-os-server/lib/sequencer/src/execution/block_applier.rs`
- `zksync-os-server/lib/storage_api/src/overlay_buffer.rs`
- `zksync-os-server/lib/storage/src/db/replay.rs`
- `zksync-os-server/lib/l1_watcher/src/persist_batch_watcher.rs`

## Ownership Boundary

Main-node restart recovery for already included transactions, plus resumed inclusion after replay.

## Invariants Covered

- Receipt stability across restart
- Persisted balance effect survives restart
- Safe/finalized progression resumes after restart
- New transactions can still be included after replay

## Tests Added / Updated

- Kept the restart suite in `external-tests/tests/restart_regressions.rs`
- Added `main_node_can_include_new_tx_after_restart_replay`

## Server Changes Covered

- Sequencer split into executor/applier with in-memory overlay before persistence
- Startup replay / production handoff after command-source refactor
- Loopback canonization wiring in the single-node main-node path
- Batch persistence timing fix as a secondary effect on settlement timing

## Mutation-Style Validation

- Not performed in this update. The repo task was a diff review and agent refresh, not a fail-first mutation cycle, and no safe temporary production mutations were staged in this turn.

## Issues Found

- None in this feature area for the reviewed range.

## Remaining Gaps

- No crash window test between execution and persistence
- No coverage for replay overrides / rebuild mode
- No external-node restart assertions
