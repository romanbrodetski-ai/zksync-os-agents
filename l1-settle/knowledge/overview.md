# Feature: Settling Batches on L1

## What It Does

The L1 settling pipeline commits, proves, and executes ZKsync OS batches on Ethereum (or a Settlement Layer). A "settled" batch has gone through all three on-chain operations in strict order: **Commit → Prove → Execute**. Only after execution are L2→L1 withdrawals finalizable and the batch's state transitions considered permanent.

---

## Definitely In Scope

| Area | Key files |
|------|-----------|
| `CommitCommand` encoding + sending | `lib/l1_sender/src/commands/commit.rs` |
| `ProofCommand` encoding + sending | `lib/l1_sender/src/commands/prove.rs` |
| `ExecuteCommand` encoding + sending | `lib/l1_sender/src/commands/execute.rs` |
| Generic L1 sender loop | `lib/l1_sender/src/lib.rs` |
| Pipeline passthrough logic | `lib/l1_sender/src/lib.rs` (`process_prepending_passthrough_commands`) |
| Calldata encoding (versioned) | `lib/contract_interface/src/calldata.rs` |
| `StoredBatchInfo` hash + ABI encoding | `lib/contract_interface/src/models.rs` |
| SNARK public input computation | `lib/l1_sender/src/commands/prove.rs` (`snark_public_input`) |
| 2FA (batch commit signing) | `lib/l1_sender/src/commands/commit.rs` (`CommitCommand::try_new`) |
| L1 state discovery | `lib/contract_interface/src/l1_discovery.rs` |
| GaplessCommitter (ordering before commit) | `node/bin/src/prover_api/gapless_committer.rs` |
| PriorityTreePipelineStep (ordering before execute) | `node/bin/src/priority_tree_steps/priority_tree_pipeline_step.rs` |
| L1 commit/execute watchers | `lib/l1_watcher/src/commit_watcher.rs`, `execute_watcher.rs` |
| Batch persistence on execute | `lib/l1_watcher/src/persist_batch_watcher.rs` |
| Orchestration / wiring | `node/bin/src/lib.rs` (`run()`) — wires all pipeline stages, fetches initial `L1State`, dispatches L1 senders, enforces pubdata-mode vs DA-mode consistency, checks 2FA config mismatch |

## Possibly Out of Scope (adjacent but distinct ownership)

- FRI proof generation internals (`lib/multivm/`, prover binary)
- SNARK prover binary (external binary)
- Batcher block-sealing logic (`node/bin/src/batcher/seal_criteria.rs`)
- Priority ops tree construction (`lib/priority_tree/`)
- L1→L2 deposit / bridgehub interaction (covered by existing `l1.rs` integration tests)
- Upgrade transaction handling
- Gateway migration watcher

---

## Key Invariants / Business Rules

### Ordering
1. Batches must be committed before they can be proved.
2. Batches must be proved before they can be executed.
3. Commit, prove, and execute transactions are sent **in nonce order** (sequential within the same sender address); the L1 contract enforces monotonic batch numbers.
4. After startup, passthrough commands are processed first (already-committed/proved batches), then only `SendToL1` commands are accepted. Receiving a `Passthrough` after the first `SendToL1` is a fatal error.

### Calldata encoding
5. Commit calldata is prefixed with a 1-byte encoding version: V29=`0x02`, V30=`0x03`, V31=`0x04`. Wrong version causes an L1 revert.
6. Execute calldata and prove calldata both carry a 1-byte `SUPPORTED_ENCODING_VERSION = 1`.
7. For protocol versions 31+, the execute calldata includes additional fields (`logs`, `messages`, `multichain_roots`) when running behind a gateway; for v29/v30 these are absent.

### 2FA (batch commit signing)
8. When `BatchVerificationSL::Enabled`, the commit must carry ≥ `threshold` signatures from the allowed validator set; threshold=0 is allowed without any signatures.
9. Signatures are sorted by signer address before encoding.
10. `AlreadyCommitted` or `NotNeeded` signature data is accepted when 2FA is disabled or threshold=0.

### Batch verification transport (2FA wire protocol)
26. The batch verification wire format is **v2** (`BATCH_VERIFICATION_WIRE_FORMAT_VERSION = 2`). The server sends v2-encoded requests to ENs. The server's `BatchVerificationRequest::decode` accepts both v1 and v2 (selects on the version field); it panics on any other version.
27. `CommitBatchInfo` is the transport object for batch verification. The v2 wire format ABI-encodes commit data using `IExecutor::CommitBatchInfoZKsyncOS` (v31 layout); v1 used `IExecutorV30::CommitBatchInfoZKsyncOS` (v30 layout). The field difference between v1 and v2 wire format is the inclusion of `sl_chain_id` and `first_block_number`/`last_block_number` in v2.

### SNARK public input
11. For each batch `i`: `public_input_i = keccak(prev_state_commitment || state_commitment || commitment)`.
12. For a range of batches: result is chained as `keccak(prev_result || next_input) >> 32` (shift-right 4 bytes).
13. The fake proof type (`3`) still verifies the correct public input against on-chain batch data.

### StoredBatchInfo hash
14. `StoredBatchInfo::hash()` = `keccak256(ABI-encode(IExecutor::StoredBatchInfo))`.
15. Fields `indexRepeatedStorageChanges` and `timestamp` are hardcoded to zero for ZKsync OS batches.
16. `batchHash` stores `state_commitment`; `commitment` stores `batch_output_hash` (not a generic hash).

### Node role config requirements
23. `L1SenderConfig::pubdata_mode` is a required `PubdataMode` field — all nodes (main and external) must have it configured; there is no Optional wrapper.
24. `Config::external_price_api_client_config` is a required `ExternalPriceApiClientConfig` field; all nodes must configure it.
25. Certain operational checks (pubdata-mode / DA-mode consistency panic, L1 state fetch, 2FA wiring) are guarded by `if node_role.is_main()` and are skipped on external nodes, but the config fields themselves are always required.

### L1 transaction lifecycle
17. Transactions are sent with 1 required confirmation and a 300-second timeout; a timeout causes a crash-and-restart recovery.
18. Failed L1 transactions (receipt `status=false`) are fatal and trigger a debug trace for diagnosis.
19. Operator balance must be non-zero at startup; zero balance causes an immediate failure.

### Execute: priority ops
20. `priority_ops.len() == batches.len()` (asserted in `ExecuteCommand::new`).
21. The `interop_roots` array is also per-batch.

---

## Important Edge Cases and Failure Modes

- **Batch already committed on L1** (e.g., after a restart): `GaplessCommitter` emits `Passthrough` instead of `SendToL1`; the L1 sender must drain all passthrough commands before switching to normal mode.
- **Protocol version mismatch**: `encode_commit_batch_data` panics on unsupported `protocol_version_minor` (currently only 29, 30, 31 are supported; 32 is handled in execute but not in commit calldata).
- **Unsupported execution version in prove**: panics if execution version is not in {4, 5, 6}.
- **CommitCalldata decoder**: only accepts V30 (`0x03`) and V31 (`0x04`) encoding; V29 commits cannot be re-decoded (returns error).
- **Gas cap warn-not-fail**: if L1's estimated gas fees exceed configured caps, a warning is logged but the configured (lower) cap is used, potentially causing inclusion delays.
- **Threshold=0 with 2FA enabled**: allowed per code; edge case that bypasses signature requirement.
- **`last_block_timestamp` in `StoredBatchInfo`**: field is ignored in `PartialEq` (explicitly skipped); present in struct for wire-format compatibility but semantically unused.
- **Multiple batches in one prove/execute tx**: `ProofCommand` and `ExecuteCommand` both support batch ranges; `CommitCommand` is always single-batch.
- **`pubdata_mode` missing**: `pubdata_mode` is required for all nodes; omitting it from config will cause a startup failure.

---

## Ambiguities / Suspicious Areas

1. **Protocol v32 in execute but not in commit**: execute calldata handles `31 | 32` together, but `encode_commit_batch_data` only handles 29, 30, 31 — v32 commit would panic. Likely intentional (v32 may use same commit encoding as v31), but not documented.
2. **`CommitCalldata::decode` rejects V29**: the decoder bails on V29 encoding version, meaning any watcher that calls `fetch_commit_calldata` on a V29 batch will fail. This could be a latent bug if V29 batches are re-processed after a restart.
3. **`into_stored` for protocol version**: `BatchInfo::into_stored` takes a protocol version but the implementation details need checking for correctness across versions.
4. **`shift_b256_right` zero-fills the top 4 bytes** (not the bottom 4): the 4-byte shift is toward the most-significant end of the B256 value, which is semantically ">> 32 bits" in big-endian. Worth verifying alignment with the L1 verifier contract expectation.
