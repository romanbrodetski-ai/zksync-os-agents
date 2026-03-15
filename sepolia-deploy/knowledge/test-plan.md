# Test Plan: Sepolia Chain Deployment

## Strategy

Since this agent covers a deployment pipeline (not a library), tests are primarily **end-to-end validation scripts** rather than unit tests. The test harness reuses the existing integration test infrastructure (Anvil-based local L1) to validate the config generation and server startup pipeline without requiring live Sepolia access.

For live Sepolia validation, we provide a runnable script that can be invoked manually or by CI.

---

## Test Categories

### Category 1: Config Generation (Unit-level)

Tests that the zkstack YAML output is correctly transformed into zksync-os-server config.

| # | Test | Invariant | Observable Outcome | Plausible Regression |
|---|------|-----------|-------------------|---------------------|
| 1.1 | Bridgehub address extracted correctly | Server must discover diamond proxy from bridgehub | Config contains correct bridgehub address from ecosystem contracts.yaml | Swapping `core_ecosystem_contracts.bridgehub_proxy_addr` with `bridges.shared.l1_address` |
| 1.2 | Bytecodes supplier address extracted | Server needs this for genesis upgrade tx fetch | Config contains `l1_bytecodes_supplier_addr` from ecosystem contracts | Using chain-level contracts.yaml instead of ecosystem-level |
| 1.3 | Operator key role mapping | Wrong key → L1 revert on commit/prove/execute | blob_operator.private_key → commit_sk, prove_operator → prove_sk, execute_operator → execute_sk | Swapping commit and prove keys |
| 1.4 | Chain ID propagated | Server won't find chain on bridgehub with wrong ID | genesis.chain_id matches registered chain ID | Hardcoding default chain ID (6565) instead of deployed one |
| 1.5 | Genesis path resolved | Server can't start without genesis.json | genesis_input_path points to existing file with correct genesis_root | Pointing to local-chains default instead of chain-specific genesis |

### Category 2: Wallet Role Verification

| # | Test | Invariant | Observable Outcome | Plausible Regression |
|---|------|-----------|-------------------|---------------------|
| 2.1 | blob_operator has committer role | Only committer can submit commit txs through timelock | Commit tx succeeds from blob_operator address | Registering operator address for committer role instead |
| 2.2 | prove_operator has prover role | Prover role needed for prove txs | Prove tx succeeds from prove_operator address | Omitting prove_operator during registration |
| 2.3 | execute_operator has executor role | Executor role needed for execute txs | Execute tx succeeds from execute_operator address | Omitting execute_operator during registration |
| 2.4 | Wrong key causes revert | Unauthorized address must be rejected | Commit from wrong key reverts on L1 | Removing role check from ValidatorTimelock |

### Category 3: End-to-End Settlement (Integration)

| # | Test | Invariant | Observable Outcome | Plausible Regression |
|---|------|-----------|-------------------|---------------------|
| 3.1 | Genesis block produced | Server initializes from genesis.json and produces block #1 | Block #1 contains upgrade tx, block sealed | Corrupt genesis.json |
| 3.2 | Batch 1 committed on L1 | First batch (genesis upgrade) commits successfully | `succeeded on L1 command=commit batch 1` in logs | Wrong calldata encoding version |
| 3.3 | Batch 1 proved on L1 | Proof accepted after commit | `succeeded on L1 command=prove batches 1-1` | Invalid SNARK public input computation |
| 3.4 | Batch 1 executed on L1 | Execution succeeds after proof | `succeeded on L1 command=execute batches 1-1` | Missing priority tree root |
| 3.5 | Subsequent batches settle | Steady-state settlement works | Batch 2+ commit/prove/execute all succeed | Incorrect StoredBatchInfo for batch transition |

### Category 4: Gas Cost Tracking

| # | Test | Invariant | Observable Outcome | Plausible Regression |
|---|------|-----------|-------------------|---------------------|
| 4.1 | Deployment gas within bounds | Total deployment < 100M gas | Sum of all forge broadcast receipts < threshold | Adding expensive new contract to deployment |
| 4.2 | Per-batch commit gas reasonable | Commit gas < 300K for normal batch | Gas from L1 receipt < threshold | Adding redundant calldata fields |
| 4.3 | Per-batch prove gas reasonable | Prove gas < 200K | Gas from L1 receipt < threshold | Encoding larger proof data |
| 4.4 | Per-batch execute gas reasonable | Execute gas < 200K | Gas from L1 receipt < threshold | Adding unnecessary storage writes |

### Category 5: Error Handling & Recovery

| # | Test | Invariant | Observable Outcome | Plausible Regression |
|---|------|-----------|-------------------|---------------------|
| 5.1 | Server starts with unfunded operators | L1 sender initializes but logs warning | Server runs, blocks produced, settlement fails gracefully | Panic on zero balance |
| 5.2 | Duplicate chain ID rejected | Cannot register same chain twice | Forge script reverts with clear error | Silent overwrite of existing chain |
| 5.3 | Wrong bridgehub address | Server can't discover chain | Server exits with descriptive error | Hanging forever on L1 query |

---

## Implementation Notes

- Tests 1.x: Pure Rust tests that parse YAML files and validate config generation logic.
- Tests 2.x, 3.x: Use the existing integration test harness (local Anvil + zksync-os-server).
- Tests 4.x: Parse forge broadcast JSON files or L1 sender log output.
- Tests 5.x: Integration tests with intentional misconfigurations.

The `generate_server_config()` function (to be implemented) is the primary testable unit. It takes zkstack YAML outputs and produces a valid server config.
