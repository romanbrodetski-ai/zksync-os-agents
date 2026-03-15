# Sepolia Deployment — Final Report

**Date**: 2026-03-14
**Chain ID**: 837101
**L1 Network**: Ethereum Sepolia (chain ID 11155111)

## Deployment Summary

A fresh ZKsync OS ecosystem and chain were deployed on Ethereum Sepolia using `zkstack ecosystem init`.  The server was started, produced blocks, and successfully settled two batches on L1 (commit → prove → execute).

### Deployed Contracts (Ecosystem Level)

| Contract | Address |
|---|---|
| Bridgehub Proxy | `0x1d9490c2f6513843bc6d694c0b499be5e2779b87` |
| Bytecodes Supplier | `0x024c7e6c9c2eff05be9e3586118de257e29b920c` |
| CTM (STM) Proxy | `0x5672953c9736dad421753cf25aec20df0ff77c3d` |
| Validator Timelock | `0x9ef840870068923270959e6a35705a5e08a64500` |
| DA Validator (Blobs) | `0x70cb09e928995c9a63d38e5fe2209e4f0803900f` |
| Shared Bridge | `0xc4aa880607166faf6ee3d1fba6a1e62cf3fe88bf` |

### Deployed Contracts (Chain Level)

| Contract | Address |
|---|---|
| Diamond Proxy | `0xd4de2ea0e2f085dcae9b0e34aff9febc4c98d09f` |
| Chain Admin | `0x942c0ccd7ee96bb6ae7de0c951e637869a84a5c5` |

## Gas Costs

### Deployment (~92 transactions total)

| Step | Gas Used |
|---|---|
| Deploy L1 Core Contracts | 36,309,096 |
| Deploy CTM | 43,505,814 |
| Register CTM | 277,908 |
| Register ZK Chain | 11,628,968 |
| Deploy L2 Contracts | 2,597,089 |
| **Total** | **94,318,875** |

### Per-Batch Settlement

| Step | Batch 1 (genesis) | Batch 2 (5 L2 txs) | Per-tx (Batch 2) |
|---|---|---|---|
| Commit | 192,614 | 138,894 | 27,779 |
| Prove | 104,152 | 87,844 | 17,569 |
| Execute | 131,489 | 154,273 | 30,855 |
| **Total** | **428,255** | **381,011** | **76,202** |

## L1 Settlement Verification

Both batches were fully settled:
- **Batch 1** (genesis): committed, proved, executed on Sepolia L1
- **Batch 2** (5 L2 transfers): committed, proved, executed on Sepolia L1

The prove step used fake SNARK proofs (`NoProofs` mode), which is appropriate for testnet.

## Issues Found

### Critical: Operator Key Role Mapping
The `blob_operator` wallet holds the "committer" role on `ValidatorTimelock`, not the generic `operator` wallet.  Using the wrong key causes `Unauthorized` reverts on L1 commit transactions.

**Mapping**: `blob_operator` → `operator_commit_sk`, `prove_operator` → `operator_prove_sk`, `execute_operator` → `operator_execute_sk`.

### High: Operator Wallets Start With 0 ETH
After `zkstack ecosystem init`, the three operator wallets (blob, prove, execute) have zero ETH balance.  The server will start but all L1 settlement transactions will fail silently until the wallets are funded.

**Recommendation**: `deploy.sh` includes an explicit funding step (0.2 ETH each from deployer).

### Medium: PyYAML Hex Parsing
Unquoted `0x...` values in zkstack YAML output are parsed as integers by PyYAML.  The `generate_server_config.py` script includes `to_hex()` conversion with proper zero-padding to handle this.

### Medium: Chain ID Discovery
The ecosystem-level `ZkStack.yaml` stores `chains` as a filesystem path, not a dictionary.  Chain ID must be read from the chain-level `ZkStack.yaml` or `genesis.json` instead.

## Rollout UX Recommendations

1. **Auto-fund operators**: `zkstack` should fund operator wallets during `ecosystem init` or warn prominently about zero balances
2. **Config generator**: Ship `generate_server_config.py` (or a Rust equivalent) as part of zksync-os-server tooling
3. **Health endpoint**: Server should expose a `/health` endpoint showing L1 settlement status and operator balances
4. **Key mapping docs**: Document the blob_operator→commit mapping prominently — it's the #1 foot-gun
5. **Single-command deploy**: Unify the 6-step deployment into a single `zkstack deploy --target sepolia` that handles everything including server config generation

## Test Coverage

15 tests across 2 test files:
- **config_generation.rs** (10 tests): bridgehub extraction, bytecodes supplier, operator key mapping, chain ID, genesis path, fee collector, L1 RPC URL, YAML round-trip, missing wallets, missing CTM
- **gas_costs.rs** (5 tests): deployment gas bounds, batch 1 settlement bounds, batch 2 settlement bounds, total settlement bounds, gas cost report
