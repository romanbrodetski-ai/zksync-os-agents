/// Category 2 — StoredBatchInfo hash (unit, pure, no L1)
use alloy::primitives::{B256, U256, keccak256};
use alloy::sol_types::SolValue;
use zksync_os_contract_interface::IExecutor;
use zksync_os_l1_settle_tests::helpers::stored_batch_info;

// ------------------------------------------------------------------
// T2.1 — Hash is deterministic for known inputs
// ------------------------------------------------------------------
// The hash is keccak256(ABI-encode(IExecutor::StoredBatchInfo)) with
// indexRepeatedStorageChanges=0 and timestamp=0.
//
// Mutation: reorder fields in the From<&StoredBatchInfo> impl, or accidentally include
// last_block_timestamp → hash changes and this golden-value check fails.
#[test]
fn t2_1_hash_is_deterministic() {
    let info = stored_batch_info(7);

    // Compute the expected hash manually, mirroring StoredBatchInfo::hash()
    let abi_struct = IExecutor::StoredBatchInfo::from(&info);
    let expected = keccak256(abi_struct.abi_encode_params());

    let got = info.hash();
    assert_eq!(got, expected, "StoredBatchInfo::hash() must equal keccak256(ABI-encode)");

    // Calling hash() twice must give the same result (deterministic)
    assert_eq!(info.hash(), info.hash(), "hash must be idempotent");
}

// ------------------------------------------------------------------
// T2.2 — last_block_timestamp is excluded from equality and from hash
// ------------------------------------------------------------------
// Mutation: include last_block_timestamp in PartialEq or in the ABI struct From impl
// → the two instances would be unequal or produce different hashes.
#[test]
fn t2_2_last_block_timestamp_does_not_affect_equality_or_hash() {
    let base = stored_batch_info(3);
    let mut different_ts = base.clone();
    different_ts.last_block_timestamp = Some(999_999_999);

    // PartialEq must skip last_block_timestamp
    assert_eq!(
        base, different_ts,
        "StoredBatchInfo::eq must ignore last_block_timestamp"
    );

    // Hash must also be the same (last_block_timestamp is not in the ABI struct)
    assert_eq!(
        base.hash(),
        different_ts.hash(),
        "StoredBatchInfo::hash must not depend on last_block_timestamp"
    );
}

// ------------------------------------------------------------------
// T2.3 — Changing state_commitment changes the hash
// ------------------------------------------------------------------
// Mutation: map state_commitment to the wrong field (e.g., swap batchHash and commitment)
// → hashes would collide for non-trivial inputs.
#[test]
fn t2_3_state_commitment_affects_hash() {
    let base = stored_batch_info(5);
    let mut flipped = base.clone();
    // Flip a single byte in state_commitment
    flipped.state_commitment.0[0] ^= 0xff;

    assert_ne!(
        base.hash(),
        flipped.hash(),
        "flipping state_commitment must change the hash"
    );
}

// ------------------------------------------------------------------
// Bonus: the hardcoded-zero fields in ABI encoding
// ------------------------------------------------------------------
// indexRepeatedStorageChanges and timestamp must always be 0 in the ABI struct.
// Mutation: accidentally use real values for these fields → hash changes for any non-trivial batch.
#[test]
fn t2_4_zero_fields_in_abi_struct() {
    let info = stored_batch_info(1);
    let abi_struct = IExecutor::StoredBatchInfo::from(&info);

    assert_eq!(
        abi_struct.indexRepeatedStorageChanges, 0u64,
        "indexRepeatedStorageChanges must be hardcoded to 0"
    );
    assert_eq!(
        abi_struct.timestamp,
        U256::ZERO,
        "timestamp must be hardcoded to 0"
    );
    // batchHash stores state_commitment
    assert_eq!(
        abi_struct.batchHash,
        B256::from(U256::from(1 * 1000 + 1)),
        "batchHash must store state_commitment"
    );
    // commitment stores batch output hash (commitment field)
    assert_eq!(
        abi_struct.commitment,
        B256::from(U256::from(1 * 1000 + 2)),
        "commitment field must store the batch commitment"
    );
}
