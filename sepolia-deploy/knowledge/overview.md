# Feature: Launching a New ZKsync OS Chain on Sepolia

## What It Does

Deploys a complete ZKsync OS ecosystem and chain on Ethereum Sepolia testnet, runs the L2 sequencer, and verifies end-to-end L1 batch settlement (commit/prove/execute). This agent owns the deployment pipeline from contract deployment through first settled batch, including gas cost tracking and rollout UX reporting.

---

## Definitely In Scope

| Area | Key artifacts |
|------|---------------|
| Ecosystem contract deployment | `zkstack ecosystem init`, forge scripts (DeployL1CoreContracts, DeployCTM, RegisterCTM) |
| Chain registration | RegisterZKChain.s.sol, DeployL2Contracts.sol, DA validator pair setup |
| Governance acceptance | AdminFunctions.s.sol (accept governance, accept admin) |
| Server config generation | Mapping zkstack output YAMLs to zksync-os-server config.yaml |
| Operator wallet funding | Transferring ETH to blob_operator, prove_operator, execute_operator |
| Server startup & genesis | zksync-os-server --config, genesis.json loading, L1 state discovery |
| L1 batch settlement | Commit, prove, execute lifecycle for first batches |
| Gas cost reporting | Deployment gas + per-batch settlement gas |
| Rollout UX issues | Friction points, manual steps, error messages, missing automation |

## Out of Scope

- FRI/SNARK proof generation internals (fake provers used)
- L2 application-level testing (token transfers, contract deployment on L2)
- External node sync
- Gateway migration
- Production security (key management, HSM, etc.)

---

## Deployment Pipeline (6 Steps)

### Step 1: Prerequisites
- `era-contracts` at correct tag (e.g. `zkos-v0.30.2`) with `forge build` complete
- `zkstack` CLI built (`cargo build --release` in `zksync-era/zkstack_cli`)
- `zksync-os-server` built (`cargo build --release`)
- Funded deployer wallets on Sepolia (deployer ~1 ETH, governor ~1 ETH)

### Step 2: Ecosystem Init (single command)
```bash
cd <ecosystem_dir>
zkstack ecosystem init \
  --deploy-paymaster=false --deploy-erc20=false --observability=false \
  --no-port-reallocation --deploy-ecosystem \
  --l1-rpc-url="<SEPOLIA_RPC>" \
  --zksync-os --ignore-prerequisites --skip-contract-compilation-override \
  --no-genesis --validium-type=no-da
```
This deploys: L1 core contracts, CTM, registers chain, accepts governance, deploys L2 contracts, sets DA validator pair.

### Step 3: Generate Server Config
Map deployed addresses from zkstack output to server config.yaml:
- `bridgehub_address` from `configs/contracts.yaml` → `genesis.bridgehub_address`
- `l1_bytecodes_supplier_addr` from `configs/contracts.yaml` → `genesis.bytecode_supplier_address`
- `chain_id` from chain registration → `genesis.chain_id`
- `genesis.json` from `chains/<name>/configs/genesis.json` → `genesis.genesis_input_path`
- Operator private keys from `chains/<name>/configs/wallets.yaml`:
  - `blob_operator.private_key` → `l1_sender.operator_commit_sk`
  - `prove_operator.private_key` → `l1_sender.operator_prove_sk`
  - `execute_operator.private_key` → `l1_sender.operator_execute_sk`
- `fee_account.address` → `sequencer.fee_collector_address`

### Step 4: Fund Operator Wallets
Transfer Sepolia ETH to:
- `blob_operator.address` (commit role)
- `prove_operator.address` (prove role)
- `execute_operator.address` (execute role)

### Step 5: Run Server
```bash
target/release/zksync-os-server --config <config.yaml>
```

### Step 6: Verify Settlement
Monitor logs for:
- `Block sealed in block executor` — L2 blocks produced
- `Batch created` — batches formed
- `sending L1 transactions command_name="commit"` — commit sent
- `succeeded on L1 command=commit batch N` — commit confirmed
- `succeeded on L1 command=prove batches N-N` — prove confirmed
- `succeeded on L1 command=execute batches N-N` — execute confirmed

---

## Key Invariants

1. **Ecosystem init is idempotent on fresh salt** — a new `create2_factory_salt` guarantees fresh contract addresses.
2. **Chain ID must be unique** — cannot register the same chain ID twice on the same bridgehub.
3. **Operator roles must match on-chain registration** — blob_operator gets committer role, prove_operator gets prover role, execute_operator gets executor role. Using the wrong key causes L1 reverts.
4. **Genesis root must match** — the `genesis_root` in genesis.json must equal the root computed from genesis state. Mismatch causes server startup failure.
5. **Settlement order** — commit before prove, prove before execute. Pipeline enforces this.
6. **DA validator pair** — must be set before first commit. BlobsZKSyncOS mode for rollup chains.

---

## Gas Costs (Sepolia Benchmark, 2026-03-14)

### Deployment (one-time)
| Script | Txs | Total Gas |
|--------|-----|-----------|
| DeployL1CoreContracts | 32 | 36,309,096 |
| DeployCTM | 36 | 43,505,814 |
| RegisterCTM | 2 | 277,908 |
| RegisterZKChain | 10 | 11,628,968 |
| DeployL2Contracts | 5 | 2,597,089 |
| AdminFunctions (governance) | ~7 | ~500,000 |
| **Total deployment** | **~92** | **~94,800,000** |

### Per-Batch Settlement (ongoing)
| Operation | Gas (batch 1, genesis) | Gas (batch 2, normal) |
|-----------|----------------------|----------------------|
| Commit | 192,614 | 138,894 |
| Prove | 104,152 | 87,844 |
| Execute | 131,489 | 154,273 |
| **Total** | **428,255** | **381,011** |

### Per-L2-Transaction (batch 2, 5 txs)
| Operation | Gas per L2 tx |
|-----------|--------------|
| Commit | 27,778 |
| Prove | 17,568 |
| Execute | 30,854 |
| **Total** | **76,200** |

---

## Rollout UX Issues & Improvement Opportunities

### Current Friction Points
1. **No config generation script** — mapping zkstack YAML output to server config.yaml is manual, error-prone, and requires understanding of role-to-key mappings.
2. **Operator funding is manual** — no tooling to detect unfunded operators or transfer ETH.
3. **Salt management** — reusing a salt causes CREATE2 collisions; must manually increment or randomize.
4. **`--no-genesis` is implicit for ZKsync OS** — not documented that the flag is forced when `--zksync-os` is used.
5. **Balance warnings are noisy** — "recommended to have 5 ETH" fires for every step even when 1 ETH suffices.
6. **No gas cost summary** — deployment completes with no aggregate gas report.
7. **genesis.json is copied from local-chains/** — the zkstack ecosystem init uses the base genesis.json and only adds `l1_chain_id`/`l2_chain_id` fields. Not obvious where genesis.json comes from or how to regenerate it.
8. **No health check** — no way to verify the server is running and settling without reading logs.

### Recommended Improvements
1. Add a `zkstack generate-server-config` command that outputs a zksync-os-server config.yaml from ecosystem state.
2. Add `--fund-operators` flag to `ecosystem init` that distributes ETH from deployer.
3. Add a post-deploy gas report summarizing total deployment cost.
4. Add a `--verify-settlement` flag that waits for first batch commit/prove/execute.
5. Document the genesis.json generation flow (era-contracts/tools/zksync-os-genesis-gen/).

---

## Important Edge Cases

- **RPC rate limiting** — Sepolia public RPCs may throttle; deployment takes ~3 minutes with 92 txs.
- **Nonce gaps** — if a tx fails mid-deployment, subsequent txs with higher nonces will hang. No retry logic in zkstack.
- **Contract verification** — deployed contracts are not automatically verified on Etherscan.
- **Chain ID collision** — if chain ID is already registered, RegisterZKChain will revert silently.
- **Epoch boundaries** — blob transactions may fail near epoch boundaries on Sepolia due to blob gas price spikes.
