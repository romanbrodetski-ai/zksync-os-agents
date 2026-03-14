# Test Plan — Settling Batches on L1

All integration tests use **fake FRI + fake SNARK provers**. No real prover binary is required.

Available local-chain fixtures: `v30.2` (= protocol minor 30, `PROTOCOL_VERSION`) and `v31.0` (= protocol minor 31, `NEXT_PROTOCOL_VERSION`).

---

## Category 1 — Calldata encoding (unit, pure)

These tests live in `tests/unit_calldata.rs` and require no L1 or running node. They test
`encode_commit_batch_data` and `CommitCalldata::decode` directly.

### T1.1 — Commit calldata round-trip: v30
- **Invariant:** encode then decode for protocol v30 yields the same `StoredBatchInfo` + `CommitBatchInfo`.
- **Observable outcome:** decoded fields equal the inputs byte-for-byte.
- **Regression trigger:** changing encoding version byte, wrong ABI type in the tuple, or swapping v30/v31 struct.

### T1.2 — Commit calldata round-trip: v31
- **Invariant:** same as T1.1 for protocol v31 (adds `first_block_number`, `last_block_number`, `sl_chain_id`).
- **Observable outcome:** decoded fields equal the inputs; `sl_chain_id` is preserved.
- **Regression trigger:** missing/extra field in the v31 struct, wrong version prefix byte.

### T1.3 — Commit calldata version byte
- **Invariant:** each protocol version uses its designated version prefix: v29=`0x02`, v30=`0x03`, v31=`0x04`.
- **Observable outcome:** `encode_commit_batch_data(_, _, minor)[0]` equals the expected byte.
- **Regression trigger:** off-by-one in the `match` arm or swapped constants.

### T1.4 — Decoder rejects unknown version byte
- **Invariant:** `CommitCalldata::decode` returns `Err` for any version byte not in {`0x03`, `0x04`}.
- **Observable outcome:** `decode` errors on a buffer with `data[4] = 0x02` (V29) or `0x99`.
- **Regression trigger:** accidentally relaxing the version guard.

### T1.5 — Prove/execute calldata version byte is `0x01`
- **Invariant:** both `ProofCommand::to_calldata_suffix` and `ExecuteCommand::to_calldata_suffix` prefix with `SUPPORTED_ENCODING_VERSION = 1`.
- **Observable outcome:** `calldata_suffix[0] == 0x01` for both.
- **Regression trigger:** constant changed or prefix step removed.

---

## Category 2 — `StoredBatchInfo` hash (unit, pure)

### T2.1 — Hash is stable for known inputs
- **Invariant:** `StoredBatchInfo::hash()` produces a deterministic keccak over ABI-encoded fields with `indexRepeatedStorageChanges=0` and `timestamp=0`.
- **Observable outcome:** known input → known expected hash (golden value computed once and hardcoded).
- **Regression trigger:** any field reordering in the ABI encoding, change to hardcoded-zero fields.

### T2.2 — `last_block_timestamp` does not affect hash
- **Invariant:** `PartialEq` skips `last_block_timestamp`; hash also must not depend on it (field is not in the ABI-encoded struct sent to L1).
- **Observable outcome:** two `StoredBatchInfo` differing only in `last_block_timestamp` are equal and produce the same hash.
- **Regression trigger:** accidentally including `last_block_timestamp` in the ABI encoding.

### T2.3 — Changing `state_commitment` changes hash
- **Invariant:** `state_commitment` is stored as `batchHash` in the ABI struct; a one-byte flip must change the hash.
- **Observable outcome:** flipped `state_commitment` → different hash.
- **Regression trigger:** `state_commitment` mapped to wrong field.

---

## Category 3 — SNARK public input (unit, pure)

### T3.1 — Single-batch public input matches spec
- **Invariant:** `public_input = keccak(prev_state || state || commitment) >> 32`.
- **Observable outcome:** result matches manual computation from known batch data.
- **Regression trigger:** wrong field order, wrong shift amount, or wrong hash input length.

### T3.2 — Multi-batch chaining is associative
- **Invariant:** chaining batches [N, N+1] equals `keccak(pi_N || pi_{N+1}) >> 32` where each pi is the single-batch result.
- **Observable outcome:** `snark_public_input(prev, [A, B])` == expected chained value.
- **Regression trigger:** loop off-by-one, wrong chaining formula.

### T3.3 — Fake proof public input is included in calldata
- **Invariant:** fake proof payload contains `public_input` as the 4th U256 element (after type, previous hash, magic).
- **Observable outcome:** parse the fake proof U256 array and check index 3 matches expected public input.
- **Regression trigger:** index shift in the fake proof construction.

---

## Category 4 — 2FA / CommitCommand signature validation (unit, pure)

### T4.1 — Commit accepted with verification disabled
- **Invariant:** `BatchVerificationSL::Disabled` → `CommitCommand::try_new` succeeds regardless of signature data.
- **Observable outcome:** `Ok` returned for `Signed`, `AlreadyCommitted`, `NotNeeded` variants.
- **Regression trigger:** accidentally checking signatures when disabled.

### T4.2 — Commit accepted when threshold met
- **Invariant:** threshold=1, one valid signer → `Ok`.
- **Observable outcome:** `CommitCommand::try_new` returns `Ok` with one signature from the allowed set.
- **Regression trigger:** off-by-one in `<` vs `<=` comparison.

### T4.3 — Commit rejected when below threshold
- **Invariant:** threshold=2, one signature → `Err(NotEnoughSignatures(1, 2))`.
- **Observable outcome:** error variant and counts match exactly.
- **Regression trigger:** wrong comparison operator, wrong error payload.

### T4.4 — Commit rejected when batch not signed (threshold > 0)
- **Invariant:** `BatchVerificationSL::Enabled` + `BatchSignatureData::NotNeeded` + threshold=1 → `Err(BatchNotSigned)`.
- **Observable outcome:** `BatchVerificationError::BatchNotSigned` returned.
- **Regression trigger:** missing guard for non-`Signed` variant.

### T4.5 — Threshold=0 bypasses signatures
- **Invariant:** `BatchVerificationSL::Enabled` with threshold=0 and `NotNeeded` → `Ok`.
- **Observable outcome:** `CommitCommand::try_new` returns `Ok`.
- **Regression trigger:** checking count < 0 incorrectly, or applying threshold guard before checking 0.

### T4.6 — Signatures sorted by signer address
- **Invariant:** signers in the produced calldata are in ascending address order.
- **Observable outcome:** decode the `commitBatchesMultisigCall` calldata and assert signers are sorted.
- **Regression trigger:** sort step removed, or wrong sort key.

### T4.7 — Out-of-set signatures filtered out
- **Invariant:** signatures from addresses not in the validator set are stripped before threshold check.
- **Observable outcome:** 3 signatures, 2 valid — only 2 appear in calldata; threshold=2 → `Ok`.
- **Regression trigger:** `filter()` call removed or allowed-set check inverted.

---

## Category 5 — Execute command structure (unit, pure)

### T5.1 — `batches.len() != priority_ops.len()` panics
- **Invariant:** constructor asserts equal lengths.
- **Observable outcome:** `std::panic::catch_unwind` on mismatched lengths returns `Err`.
- **Regression trigger:** assert removed.

### T5.2 — Execute calldata protocol fork: v29/v30 vs v31+
- **Invariant:** v29/v30 encode `(stored_batch_infos, priority_ops, interop_roots)` only; v31+ add `(logs, messages, multichain_roots)` when `gateway=true`, and empty vecs when `gateway=false`.
- **Observable outcome:** v30 calldata ABI-decodes without logs/messages; v31+gateway calldata ABI-decodes with those fields.
- **Regression trigger:** wrong match arm, missing gateway branch.

### T5.3 — v31 non-gateway execute omits logs/messages
- **Invariant:** `gateway=false` for v31+ → logs, messages, multichain_roots are empty in calldata.
- **Observable outcome:** ABI-decode the suffix and verify all three are empty.
- **Regression trigger:** gateway flag ignored.

---

## Category 6 — Integration: batch settling happy path

These tests start a full node (fake provers) and verify on-chain L1 state via `L1State`.

### T6.1 — Single batch commit-prove-execute: v30
- **Invariant:** after startup (v30.2 genesis), at least one batch is committed, proved, and executed on L1.
- **Observable outcome:** `L1State::fetch` returns `last_executed_batch >= 1`.
- **Regression trigger:** any broken step in the Commit→Prove→Execute pipeline.

### T6.2 — Single batch commit-prove-execute: v31
- **Invariant:** same as T6.1 with `NEXT_PROTOCOL_VERSION` (v31.0) genesis.
- **Observable outcome:** `last_executed_batch >= 1` on v31 chain.
- **Regression trigger:** v31-specific calldata path broken.

### T6.3 — Batch counter monotonicity
- **Invariant:** `last_committed >= last_proved >= last_executed` at all observable moments.
- **Observable outcome:** poll `L1State` several times while batches settle; assert ordering holds each time.
- **Regression trigger:** out-of-order submission, wrong passthrough routing.

### T6.4 — Multiple sequential batches all execute
- **Invariant:** the pipeline continues to settle after the first batch; batch N+1 commits only after batch N is committed.
- **Observable outcome:** after producing several L2 transactions, `last_executed_batch >= 3`.
- **Regression trigger:** pipeline stalls after first batch, nonce collision.

---

## Category 7 — Integration: passthrough / restart

### T8.1 — Already-committed batches become passthroughs on restart
- **Invariant:** on restart, `GaplessCommitter` emits `Passthrough` for batches whose number ≤ `last_committed_batch_number`.
- **Setup:** let batch N commit on L1, restart the node, observe that batch N does not trigger a second commit transaction.
- **Observable outcome:** `last_committed_batch` on L1 stays at N (not N+1 due to a duplicate commit attempt); `last_executed_batch` advances to N.
- **Regression trigger:** passthrough logic removed or `last_committed_batch_number` initialized incorrectly.

### T8.2 — Node recovers and continues settling after restart
- **Invariant:** after restart the pipeline resumes from where it left off; `last_executed_batch` eventually catches up.
- **Observable outcome:** `last_executed_batch` before restart ≤ `last_executed_batch` after restart resumes.
- **Regression trigger:** state recovery broken, L1 state fetch misreports counters.

---

## Category 8 — Integration: 2FA enabled

### T8.1 (formerly T9.1) — Commit succeeds with threshold=1 and one valid signature
- **Invariant:** 2FA enabled, threshold=1, one matching signer → batch commits on L1.
- **Setup:** `TesterBuilder::batch_verification(1)`, use default signing key from `BATCH_VERIFICATION_KEYS[0]`.
- **Observable outcome:** `last_committed_batch >= 1` on L1.
- **Regression trigger:** signature filtering broken, calldata uses wrong selector.

### T8.2 — Node warns (not crashes) on 2FA threshold mismatch
- **Invariant:** if configured threshold differs from what's on-chain, the node logs a warning and continues.
- **Observable outcome:** node stays alive and eventually commits a batch (matching the on-chain threshold).
- **Regression trigger:** warning promoted to panic, or mismatch silently causes wrong threshold.

---

## Deferred: requires real L1 / Sepolia deployment

The following cases are important but cannot be tested against Anvil, and instructions for
deploying the system against a real L1 (Sepolia) are not yet available. These are deferred
until that setup is documented and provisioned.

- **Blob sidecar path (EIP-4844 / EIP-7594)**: Anvil doesn't support EIP-7594; the blob DA commit path is untested.
- **Real L1 timeout behavior** (300s crash-restart): needs a real or mock stuck L1 RPC.
- **Operator zero-balance guard**: needs a real funded/unfunded operator key against a live network.
- **Gas cap warn-but-not-fail under congestion**: needs observable real mempool pressure.

## Low-confidence / hard-to-test within Anvil scope

- **Multi-batch prove in one tx**: the pipeline issues one prove per batch range by default; exercising >1 batch in one prove tx requires direct `ProofCommand` construction at unit level.
- **T7.1 restart passthrough**: killing and restarting `Tester` mid-flight needs careful TempDir reuse — may be covered at unit level instead if integration setup proves fragile.
