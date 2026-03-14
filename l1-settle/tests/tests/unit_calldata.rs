/// Category 1 — Calldata encoding / decoding (unit, pure, no L1)
use alloy::primitives::{Address, U256};
use alloy::sol_types::SolCall;
use zksync_os_contract_interface::IExecutor;
use zksync_os_contract_interface::calldata::{CommitCalldata, encode_commit_batch_data};
use zksync_os_l1_settle_tests::helpers::{commit_batch_info, signed_envelope, stored_batch_info};

// ------------------------------------------------------------------
// T1.1 — Commit calldata round-trip: v30
// ------------------------------------------------------------------
// Mutation: change V30_ENCODING_VERSION constant (0x03 -> something else), or swap
// v30/v31 ABI struct type in encoder → decode fails or fields mismatch.
#[test]
fn t1_1_commit_calldata_roundtrip_v30() {
    let prev = stored_batch_info(4);
    let info = commit_batch_info(5, 30);

    let raw = encode_commit_batch_data(&prev, info.clone(), 30);

    // Wrap in a real commitBatchesSharedBridgeCall so CommitCalldata::decode can parse it
    let full_call = IExecutor::commitBatchesSharedBridgeCall::new((
        Address::ZERO,
        U256::from(5u64),
        U256::from(5u64),
        raw.into(),
    ))
    .abi_encode();

    let decoded = CommitCalldata::decode(&full_call)
        .expect("v30 calldata should decode successfully");

    assert_eq!(decoded.stored_batch_info, prev, "prev StoredBatchInfo mismatch after v30 round-trip");
    assert_eq!(decoded.commit_batch_info.batch_number, info.batch_number);
    assert_eq!(decoded.commit_batch_info.new_state_commitment, info.new_state_commitment);
    assert_eq!(decoded.commit_batch_info.priority_operations_hash, info.priority_operations_hash);
}

// ------------------------------------------------------------------
// T1.2 — Commit calldata round-trip: v31 (adds sl_chain_id, first/last block numbers)
// ------------------------------------------------------------------
// Mutation: remove sl_chain_id field from v31 struct, wrong ABI tuple order → fields mismatch.
#[test]
fn t1_2_commit_calldata_roundtrip_v31() {
    let prev = stored_batch_info(9);
    let info = commit_batch_info(10, 31);

    let raw = encode_commit_batch_data(&prev, info.clone(), 31);

    let full_call = IExecutor::commitBatchesSharedBridgeCall::new((
        Address::ZERO,
        U256::from(10u64),
        U256::from(10u64),
        raw.into(),
    ))
    .abi_encode();

    let decoded = CommitCalldata::decode(&full_call)
        .expect("v31 calldata should decode successfully");

    assert_eq!(decoded.stored_batch_info, prev);
    assert_eq!(decoded.commit_batch_info.batch_number, info.batch_number);
    assert_eq!(
        decoded.commit_batch_info.sl_chain_id, info.sl_chain_id,
        "sl_chain_id not preserved in v31"
    );
    assert_eq!(
        decoded.commit_batch_info.first_block_number, info.first_block_number,
        "first_block_number not preserved in v31"
    );
    assert_eq!(
        decoded.commit_batch_info.last_block_number, info.last_block_number,
        "last_block_number not preserved in v31"
    );
}

// ------------------------------------------------------------------
// T1.3 — Version byte matches protocol minor version
// ------------------------------------------------------------------
// Mutation: swap version byte constants or wrong match arm in encode_commit_batch_data
// → wrong byte at index 0.
#[test]
fn t1_3_version_byte_per_protocol() {
    let prev = stored_batch_info(0);
    for (minor, expected_byte) in [(29u64, 0x02u8), (30, 0x03), (31, 0x04)] {
        let info = commit_batch_info(1, minor);
        let encoded = encode_commit_batch_data(&prev, info, minor);
        assert_eq!(
            encoded[0], expected_byte,
            "protocol v{minor}: expected version byte 0x{expected_byte:02x}, got 0x{:02x}",
            encoded[0]
        );
    }
}

// ------------------------------------------------------------------
// T1.4 — CommitCalldata decoder rejects V29 and unknown version bytes
// ------------------------------------------------------------------
// Mutation: relax the version guard to accept any byte → Err not returned.
#[test]
fn t1_4_decoder_rejects_v29_and_unknown_versions() {
    let prev = stored_batch_info(0);
    let info = commit_batch_info(1, 29);

    // V29-encoded inner data, wrapped in a real call
    let raw_v29 = encode_commit_batch_data(&prev, info, 29);
    let full_call_v29 = IExecutor::commitBatchesSharedBridgeCall::new((
        Address::ZERO,
        U256::from(1u64),
        U256::from(1u64),
        raw_v29.clone().into(),
    ))
    .abi_encode();
    assert!(
        CommitCalldata::decode(&full_call_v29).is_err(),
        "decoder should reject V29 (0x02) encoding"
    );

    // Completely bogus version byte
    let mut bogus_inner = raw_v29;
    bogus_inner[0] = 0x99;
    let full_call_bogus = IExecutor::commitBatchesSharedBridgeCall::new((
        Address::ZERO,
        U256::from(1u64),
        U256::from(1u64),
        bogus_inner.into(),
    ))
    .abi_encode();
    assert!(
        CommitCalldata::decode(&full_call_bogus).is_err(),
        "decoder should reject unknown version byte 0x99"
    );
}

// ------------------------------------------------------------------
// T1.5 — Prove and execute calldata suffix begins with version byte 0x01
// ------------------------------------------------------------------
// Mutation: change SUPPORTED_ENCODING_VERSION from 1 to something else, or remove the prefix
// → byte at suffix[0] != 0x01.
#[test]
fn t1_5_prove_execute_version_byte_is_one() {
    use zksync_os_contract_interface::models::PriorityOpsBatchInfo;
    use zksync_os_l1_sender::batcher_model::SnarkProof;
    use zksync_os_l1_sender::commands::SendToL1;
    use zksync_os_l1_sender::commands::execute::ExecuteCommand;
    use zksync_os_l1_sender::commands::prove::ProofCommand;

    let env_prove = signed_envelope(1, 30);
    let env_execute = signed_envelope(1, 30);

    // prove
    let prove_cmd = ProofCommand::new(vec![env_prove], SnarkProof::Fake);
    let prove_calldata = prove_cmd.solidity_call(false);
    let decoded_prove =
        IExecutor::proveBatchesSharedBridgeCall::abi_decode(&prove_calldata)
            .expect("prove calldata should ABI-decode");
    assert_eq!(
        decoded_prove._proofData[0], 1u8,
        "prove calldata suffix must start with encoding version byte 0x01"
    );

    // execute
    let execute_cmd = ExecuteCommand::new(
        vec![env_execute],
        vec![PriorityOpsBatchInfo::default()],
        vec![vec![]],
    );
    let execute_calldata = execute_cmd.solidity_call(false);
    let decoded_execute =
        IExecutor::executeBatchesSharedBridgeCall::abi_decode(&execute_calldata)
            .expect("execute calldata should ABI-decode");
    assert_eq!(
        decoded_execute._executeData[0], 1u8,
        "execute calldata suffix must start with encoding version byte 0x01"
    );
}
