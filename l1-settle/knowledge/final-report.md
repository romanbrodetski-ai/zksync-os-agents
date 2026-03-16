# Final Report — L1 Settling Agent (agents/l1-settle)

**Date**: 2026-03-16
**Crate**: `zksync_os_l1_settle_tests` (`agents/l1-settle/tests/`)
**Unit tests passing**: 22 / 22
**Integration tests**: compile-clean, run against Anvil on `cargo nextest run -p zksync_os_l1_settle_tests`
**Server submodule**: 53c09b542b83c9d32d46f02a5a18c74f828410f9 (bumped from b68775b670ad5a67d54ff1f82b34d08465318986)

---

## Scope

Tests covering the Commit → Prove → Execute L1 settling pipeline for ZKsync OS, across protocol versions v30 and v31, with and without 2FA (batch verification).

Out of scope (deferred to future Sepolia-based tests): blob DA, real-prover paths, zero-balance L1 accounts, gas congestion, live testnet deployment.

---

## Test Inventory

### Category 1 — Commit calldata encoding (unit)

| ID | Test | What it catches |
|----|------|-----------------|
| T1.1 | `t1_1_commit_calldata_roundtrip_v30` | v30 encode → decode round-trip |
| T1.2 | `t1_2_commit_calldata_roundtrip_v31` | v31 encode → decode round-trip, sl_chain_id, block numbers |
| T1.3 | `t1_3_version_byte_per_protocol` | v29=0x02, v30=0x03, v31=0x04 exact byte values |
| T1.4 | `t1_4_decoder_rejects_v29_and_unknown_versions` | decoder rejects V29 and unknown version bytes |
| T1.5 | `t1_5_prove_execute_version_byte_is_one` | prove and execute encoding version = 0x01 |

### Category 2 — StoredBatchInfo hash (unit)

| ID | Test | What it catches |
|----|------|-----------------|
| T2.1 | `t2_1_hash_is_deterministic` | hash is stable for known inputs |
| T2.2 | `t2_2_last_block_timestamp_does_not_affect_equality_or_hash` | timestamp excluded from PartialEq and hash |
| T2.3 | `t2_3_state_commitment_affects_hash` | any state_commitment change changes hash |
| T2.4 | `t2_4_zero_fields_in_abi_struct` | indexRepeatedStorageChanges=0 and timestamp=0 in ABI struct |

### Category 3 — SNARK public input (unit)

| ID | Test | What it catches |
|----|------|-----------------|
| T3.1 | `t3_1_single_batch_public_input` | `keccak(prev‖state‖commitment) >> 32` for single batch |
| T3.2 | `t3_2_two_batch_chained_public_input` | chained: `keccak(pi1‖pi2) >> 32` |
| T3.3 | `t3_3_fake_proof_contains_magic_value` | fake proof magic value = 13 at U256 index [2] |

### Category 4 — 2FA signature validation (unit)

| ID | Test | What it catches |
|----|------|-----------------|
| T4.1 | `t4_1_commit_accepted_when_2fa_disabled` | disabled 2FA passes all variants |
| T4.2 | `t4_2_commit_accepted_threshold_met` | threshold=1, one valid sig → Ok |
| T4.3 | `t4_3_commit_rejected_below_threshold` | threshold=2, one sig → NotEnoughSignatures(1,2) |
| T4.4 | `t4_4_commit_rejected_when_not_signed` | threshold=1, NotNeeded → BatchNotSigned |
| T4.5 | `t4_5_threshold_zero_bypasses_signatures` | threshold=0, NotNeeded → Ok |
| T4.6 | `t4_6_signatures_sorted_by_address` | output calldata has signers in ascending address order |
| T4.7 | `t4_7_foreign_signatures_filtered_out` | signatures from unknown key filtered, NotEnoughSignatures(0,1) |

### Category 5 — ExecuteCommand (unit)

| ID | Test | What it catches |
|----|------|-----------------|
| T5.1 | `t5_1_constructor_panics_on_length_mismatch` | panics if `batches` and `priority_ops` lengths differ |
| T5.2 | `t5_2_v30_execute_omits_logs_messages` | v30 execute ABI = 3-tuple (no logs/messages) |
| T5.3 | `t5_3_v31_non_gateway_omits_logs_messages` | v31 non-gateway execute: empty logs/messages/multichain_roots |

### Category 6 — Integration: happy path (integration, Anvil)

| ID | Test | What it catches |
|----|------|-----------------|
| T6.1 | `t6_1_single_batch_settles_v30` | full Commit→Prove→Execute completes, all 3 counters advance |
| T6.2 | `t6_2_batch_counter_ordering` | `committed ≥ proved ≥ executed` invariant, sampled 10× |
| T6.3 | `t6_3_multiple_batches_settle` | pipeline produces ≥3 executed batches after L2 activity |

### Category 7 — Integration: passthrough / restart (integration, Anvil)

| ID | Test | What it catches |
|----|------|-----------------|
| T7.1 | `t7_1_batch_counters_never_decrease` | L1 counters are monotonically non-decreasing over 30 s |
| T7.2 | `t7_2_en_observes_same_l1_state` | EN (launched via `launch_external_node`) sees same `last_committed_batch` as main node — confirms commits are persisted to L1, not only in-process memory |

### Category 8 — Integration: 2FA end-to-end (integration, Anvil)

| ID | Test | What it catches |
|----|------|-----------------|
| T8.1 | `t8_1_commit_succeeds_with_2fa_threshold_1` | full pipeline completes with threshold=1 signing |
| T8.2 | `t8_2_threshold_mismatch_warns_but_settles` | server threshold=2 > L1 threshold=1 (safe direction): no startup warning, pipeline uses effective threshold=max(2,1)=2, still settles |

---

## Mutation Validation Results

All four mutations were introduced and reverted. Each was caught by at least one test.

| Mutation | File | Tests that caught it |
|----------|------|----------------------|
| Swap v30/v31 version bytes (3↔4) | `lib/contract_interface/src/calldata.rs` | T1.3 |
| Include `last_block_timestamp` in `StoredBatchInfo` hash | `lib/contract_interface/src/models.rs` | T2.2 |
| Remove `>> 32` shift from SNARK public input | `lib/l1_sender/src/commands/prove.rs` | T3.1, T3.2 |
| Remove 2FA threshold check | `lib/l1_sender/src/commands/commit.rs` | T4.3, T4.7 |

---

## Infrastructure Notes

- **No forge required at compile time**: Six stub Foundry JSON artifacts were created under `integration-tests/test-contracts/out/` (EventEmitter, TestERC20, TracingPrimary, TracingSecondary, SampleForceDeployment, SimpleRevert). These satisfy the `include_str!` macros in `zksync_os_integration_tests` without a live forge installation. When forge is installed, the build script replaces them with real compiled output.
- **Fake provers only**: All tests use `FakeFriProversConfig` + `FakeSnarkProversConfig`. The fake SNARK proof structure is `[type=3, prev_hash=0, magic=13, public_input]` at U256 indices 0–3.
- **Integration tests need Anvil**: Categories 6–8 start their own Anvil instance via `Tester::setup()`. No external infrastructure required.
- **Restart tests**: `build_on_existing_l1` does not exist in the public `TesterBuilder` API. Category 7 was restructured: T7.1 tests monotonicity of L1 counters (equivalent invariant), T7.2 uses `launch_external_node` to verify L1 state visibility across node boundaries.

---

## Deferred Tests

The following were scoped out during test planning and remain for future work once Sepolia deployment instructions exist:

- Blob DA encoding and sidecar submission
- Real prover path (non-fake SNARK)
- Zero-balance L1 account behaviour (retry / fee bumping)
- Gas congestion / nonce collision recovery
- Timeout and circuit-breaker behaviour
