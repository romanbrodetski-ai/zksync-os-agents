# Open Questions

These ambiguities were identified during Stage 1 recon and have not yet been answered.
Engineers reviewing this agent's work are invited to annotate or answer these below.
(Future: may be surfaced to Slack automatically.)

---

## Q1 — Protocol v32 gap in commit encoding

`execute.rs` handles protocol versions `31 | 32` with the same calldata layout, but
`encode_commit_batch_data` in `lib/contract_interface/src/calldata.rs` only handles 29, 30, 31
and panics on v32.

Is v32 intentionally absent from commit encoding (i.e. v32 uses the same V31 encoding path),
or is this a latent panic waiting to happen when v32 batches are committed?

**Answer:** *(not yet provided)*

---

## Q2 — `CommitCalldata::decode` rejects V29

The decoder in `lib/contract_interface/src/calldata.rs` errors if encoding version ≠ V30/V31.
The `L1CommitWatcher` calls `fetch_commit_calldata` to re-read committed batch data from L1.

If a chain has V29-era batches and the node restarts, does the watcher ever attempt to decode
V29 commit transactions? If yes, this silently fails. Is V29 considered permanently retired
and safe to drop here?

**Answer:** *(not yet provided)*

---

## Q3 — `shift_b256_right` semantics in SNARK public input

`ProofCommand::shift_b256_right` zeroes the first 4 bytes of the B256 and shifts the remaining
28 bytes to positions [4..32], effectively computing `value >> 32` in big-endian.

Is this exactly the right-shift that the on-chain verifier contract expects for the chained
public input? A mismatch here would cause valid proofs to be rejected on-chain.

**Answer:** *(not yet provided)*

---

## Q4 — `last_block_timestamp` excluded from `StoredBatchInfo::PartialEq`

The field exists in the struct and is serialized, but is explicitly skipped in the `PartialEq`
implementation with a comment "skip `last_block_timestamp` check".

Is this an in-progress deprecation? Is the field still written/read from storage anywhere
meaningful, or is it safe to remove in a future breaking version?

**Answer:** *(not yet provided)*

---

## Q5 — Fake proof magic value

`FAKE_PROOF_MAGIC_VALUE = 13` is included in the fake proof payload.
Is this value checked by the on-chain fake verifier contract, or is it only a local sanity
marker in the sequencer? If it is checked on-chain, a change to this constant would break
fake-proof submissions silently.

**Answer:** *(not yet provided)*
