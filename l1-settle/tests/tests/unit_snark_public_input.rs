/// Category 3 — SNARK public input computation (unit, pure, no L1)
///
/// `snark_public_input` is private in ProofCommand, so we test it indirectly:
/// we build a ProofCommand with a fake proof and known batch data, call solidity_call(),
/// then ABI-decode the suffix to extract the public input from the fake proof U256 array
/// (index [3]).  We compare this to a manually computed expected value.
///
/// The spec (from prove.rs and zksync_tools reference):
///   per_batch_input = keccak256(prev_state || state || commitment)
///   snark_input     = per_batch_input >> 32   (zero the top 4 bytes)
///   For a range: snark_input = keccak256(prev_snark_input || next_snark_input) >> 32
use alloy::primitives::{B256, U256, keccak256};
use alloy::sol_types::SolCall;
use zksync_os_contract_interface::IExecutor;
use zksync_os_l1_sender::batcher_model::SnarkProof;
use zksync_os_l1_sender::commands::SendToL1;
use zksync_os_l1_sender::commands::prove::ProofCommand;
use zksync_os_l1_settle_tests::helpers::signed_envelope;

/// Mirrors `ProofCommand::shift_b256_right`: zero the top 4 bytes, shift the rest right.
fn shift_right(input: &B256) -> B256 {
    let mut bytes = [0u8; 32];
    bytes[4..32].copy_from_slice(&input.as_slice()[0..28]);
    B256::from_slice(&bytes)
}

/// Mirrors `ProofCommand::get_batch_public_input`.
fn per_batch_public_input(prev: &zksync_os_contract_interface::models::StoredBatchInfo, batch: &zksync_os_contract_interface::models::StoredBatchInfo) -> B256 {
    let mut bytes = Vec::with_capacity(96);
    bytes.extend_from_slice(prev.state_commitment.as_slice());
    bytes.extend_from_slice(batch.state_commitment.as_slice());
    bytes.extend_from_slice(batch.commitment.as_slice());
    keccak256(&bytes)
}

/// Extract the fake proof U256 array from prove calldata.
/// Fake proof structure: [type=3, prev_hash=0, magic=13, public_input]
fn extract_fake_proof_public_input(calldata: &[u8]) -> B256 {
    let decoded = IExecutor::proveBatchesSharedBridgeCall::abi_decode(calldata)
        .expect("prove calldata should ABI-decode");

    // suffix: byte[0] = version (0x01), rest is ABI-encoded proofPayloadCall
    let suffix = decoded._proofData;
    assert_eq!(suffix[0], 0x01u8);

    // Decode proofPayloadCall from suffix[1..]
    let proof_payload = IExecutor::proofPayloadCall::abi_decode_raw(&suffix[1..])
        .expect("proof payload should decode");

    // Fake proof array: [type=3, 0, 13, public_input]
    assert_eq!(proof_payload.proof.len(), 4, "fake proof should have 4 elements");
    assert_eq!(proof_payload.proof[0], U256::from(3u32), "fake proof type must be 3");
    assert_eq!(proof_payload.proof[2], U256::from(13u32), "fake proof magic must be 13");

    B256::from(proof_payload.proof[3])
}

// ------------------------------------------------------------------
// T3.1 — Single-batch public input matches manual computation
// ------------------------------------------------------------------
// Mutation: wrong field order in get_batch_public_input (e.g., swap state and commitment)
// → per_batch_input differs → test fails.
#[test]
fn t3_1_single_batch_public_input() {
    let env = signed_envelope(5, 30);
    let prev_info = env.batch.previous_stored_batch_info.clone();

    // Compute expected
    let stored = env.batch.batch_info.clone().into_stored(&env.batch.protocol_version);
    let per_batch = per_batch_public_input(&prev_info, &stored);
    let expected_snark_input = shift_right(&per_batch);

    // Extract from calldata
    let cmd = ProofCommand::new(vec![env], SnarkProof::Fake);
    let calldata = cmd.solidity_call(false);
    let got = extract_fake_proof_public_input(&calldata);

    assert_eq!(
        got, expected_snark_input,
        "single-batch SNARK public input must match keccak(prev_state||state||commitment) >> 32"
    );
}

// ------------------------------------------------------------------
// T3.2 — Two-batch chained public input matches manual computation
// ------------------------------------------------------------------
// Mutation: wrong chaining formula (e.g., use XOR instead of keccak, or skip the second shift)
// → chained result differs → test fails.
#[test]
fn t3_2_two_batch_chained_public_input() {
    let env1 = signed_envelope(1, 30);
    let env2 = signed_envelope(2, 30);

    // Compute expected
    let stored1 = env1.batch.batch_info.clone().into_stored(&env1.batch.protocol_version);
    let stored2 = env2.batch.batch_info.clone().into_stored(&env2.batch.protocol_version);

    let prev1 = &env1.batch.previous_stored_batch_info;
    let pi1 = shift_right(&per_batch_public_input(prev1, &stored1));
    let pi2 = shift_right(&per_batch_public_input(&stored1, &stored2));

    // Chain: keccak(pi1 || pi2) >> 32
    let mut combined = [0u8; 64];
    combined[..32].copy_from_slice(&pi1.0);
    combined[32..].copy_from_slice(&pi2.0);
    let expected_chained = shift_right(&keccak256(combined));

    // Extract from calldata
    let cmd = ProofCommand::new(vec![env1, env2], SnarkProof::Fake);
    let calldata = cmd.solidity_call(false);
    let got = extract_fake_proof_public_input(&calldata);

    assert_eq!(
        got, expected_chained,
        "two-batch chained SNARK public input must match spec"
    );
}

// ------------------------------------------------------------------
// T3.3 — Fake proof magic value is present
// ------------------------------------------------------------------
// Mutation: change FAKE_PROOF_MAGIC_VALUE from 13 to something else
// → assertion on magic value fails.
#[test]
fn t3_3_fake_proof_contains_magic_value() {
    let env = signed_envelope(1, 30);
    let cmd = ProofCommand::new(vec![env], SnarkProof::Fake);
    let calldata = cmd.solidity_call(false);

    let decoded = IExecutor::proveBatchesSharedBridgeCall::abi_decode(&calldata).unwrap();
    let suffix = decoded._proofData;
    let proof_payload = IExecutor::proofPayloadCall::abi_decode_raw(&suffix[1..]).unwrap();

    // proof[0] = type=3, proof[1] = prev_hash=0, proof[2] = magic=13
    assert_eq!(proof_payload.proof[2], U256::from(13u32), "fake proof magic value must be 13");
}
