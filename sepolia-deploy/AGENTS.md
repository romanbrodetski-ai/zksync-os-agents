# Sepolia Deploy Agent

This agent owns the "Launch a new ZKsync OS chain on Sepolia" workflow. It deploys ecosystem contracts, generates server config, runs the sequencer, verifies L1 batch settlement, and reports gas costs and rollout UX issues.

---

## Activation

Invoke this agent when:
- Deploying a new chain on Sepolia (or any L1 testnet)
- Updating contract deployment scripts in `era-contracts`
- Changing server config structure (`node/bin/src/config/mod.rs`)
- Modifying L1 sender or genesis initialization code
- Bumping protocol version or contract versions
- Assessing deployment gas costs or rollout UX

---

## Knowledge

- `knowledge/overview.md` — deployment pipeline, gas costs, rollout UX issues, key invariants
- `knowledge/test-plan.md` — test matrix for config generation, role verification, settlement

---

## Scripts

### Full deploy + verify (automated)
```bash
cd sepolia-deploy
./deploy.sh ../path/to/sepolia_ecosystem \
    --l1-rpc-url https://l1-api-sepolia-1.zksync-nodes.com \
    --zkstack /path/to/zkstack \
    --server-binary /path/to/zksync-os-server
```

### Generate server config only
```bash
python3 generate_server_config.py <ecosystem_dir> \
    --l1-rpc-url https://l1-api-sepolia-1.zksync-nodes.com \
    --output config.yaml
```

### Skip deploy, just verify settlement
```bash
./deploy.sh ../path/to/sepolia_ecosystem --skip-deploy --skip-fund --timeout 120
```

---

## Tests

```bash
cargo nextest run -p zksync_os_sepolia_deploy_tests
```

Tests cover:
- Config generation from zkstack YAML output
- Operator role-to-key mapping correctness
- Gas cost bounds
- End-to-end settlement on local Anvil (mirrors Sepolia flow)

---

## High-Severity Issues

- Incorrect operator key mapping → L1 reverts on commit/prove/execute
- Wrong bridgehub address → server can't discover chain
- Missing DA validator pair → first commit reverts
- Unfunded operators → settlement hangs indefinitely
- genesis_root mismatch → server startup failure
- Chain ID collision → RegisterZKChain revert
