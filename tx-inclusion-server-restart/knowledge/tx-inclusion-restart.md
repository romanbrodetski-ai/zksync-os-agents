# Tx Inclusion / Restart Knowledge

Scope:
- `external-tests/src/lib.rs`
- `external-tests/tests/restart_regressions.rs`
- `external-tests/tests/receipt_consistency_main_vs_en.rs`
- `external-tests/tests/external_node_restart_regressions.rs`
- `zksync-os-server/node/bin/src/lib.rs`
- `zksync-os-server/node/bin/src/command_source.rs`
- `zksync-os-server/lib/sequencer/src/execution/block_executor.rs`
- `zksync-os-server/lib/sequencer/src/execution/block_canonizer.rs`
- `zksync-os-server/lib/sequencer/src/execution/block_applier.rs`

Server changes covered by this update:
- Main-node execution was split into `BlockExecutor -> BlockCanonizer -> BlockApplier`, with loopback consensus inserted between execution and persistence.
- Main-node command sourcing was replaced by `ConsensusNodeCommandSource`, which replays WAL, optionally rebuilds blocks, then produces new blocks while leader.
- External-node integration coverage in the server repo was reorganized and expanded, including gateway-backed setups and replay-oriented EN tests.
- Local-chain fixtures now ship compressed L1 state (`l1-state.json.gz`), and the server integration tests unpack them before use.

Implications for this agent:
- Restart correctness still depends on WAL replay preserving receipt visibility after a main-node restart.
- Receipt consistency between main node and external node still depends on replay staying gapless across restarts.
- The command-source / executor / applier split makes post-restart coverage of both pre-restart receipts and post-restart inclusion more important, because execution can now run ahead of persistence behind the canonization fence.

Harness updates required for this server revision:
- The local test harness must unpack `local-chains/v30.2/l1-state.json.gz` before launching `anvil`; the old hardcoded `l1-state.json` path no longer exists in a fresh checkout.
- The harness must rebuild `zksync-os-server` before launching it; otherwise it can pick up a stale binary from a previous submodule revision and test the wrong code.

Current regression coverage:
- `restart_regressions.rs`
  - main-node receipt remains stable across a main-node restart
  - a tx included before restart still reaches safe/finalized after restart
- `receipt_consistency_main_vs_en.rs`
  - main node and external node expose identical receipts for the same tx
- `external_node_restart_regressions.rs`
  - a receipt observed before restart remains stable after restarting both the main node and the external node
  - the restarted external node can submit a new tx and observe the same receipt as the restarted main node

Review outcome for server diff `4f51503f..ba5821a6`:
- No correctness issue was confirmed in the server diff for this feature area.
- The only concrete breakages found were in this agent repo’s harness assumptions about fixture decompression and binary freshness.

Residual risk:
- This agent still tests the `v30.2` local chain only. The server diff added gateway / v31-oriented external-node coverage, so deeper assurance for gateway-backed restart behavior would require a dedicated fixture path in this repo.
