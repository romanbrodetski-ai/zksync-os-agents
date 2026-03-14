/// Category 5 — ExecuteCommand structure (unit, pure, no L1)
use alloy::sol_types::{SolCall, SolValue};
use zksync_os_contract_interface::IExecutor;
use zksync_os_contract_interface::models::PriorityOpsBatchInfo;
use zksync_os_l1_sender::commands::SendToL1;
use zksync_os_l1_sender::commands::execute::ExecuteCommand;
use zksync_os_l1_settle_tests::helpers::signed_envelope;

// ------------------------------------------------------------------
// T5.1 — Constructor panics when batches.len() != priority_ops.len()
// ------------------------------------------------------------------
// Mutation: remove the assert_eq! in ExecuteCommand::new
// → mismatched input accepted silently, later causing index-out-of-bounds or wrong calldata.
#[test]
fn t5_1_constructor_panics_on_length_mismatch() {
    let env = signed_envelope(1, 30);
    let result = std::panic::catch_unwind(|| {
        ExecuteCommand::new(
            vec![env],
            vec![], // mismatched: 0 priority_ops for 1 batch
            vec![vec![]],
        );
    });
    assert!(result.is_err(), "ExecuteCommand::new must panic when batches.len() != priority_ops.len()");
}

// ------------------------------------------------------------------
// T5.2 — Protocol v29/v30 execute calldata does NOT include logs/messages fields
// ------------------------------------------------------------------
// Mutation: accidentally include logs/messages in v30 encoding path
// → ABI-decode of (stored_infos, priority_ops, interop_roots) fails or produces garbage.
#[test]
fn t5_2_v30_execute_omits_logs_messages() {
    let env = signed_envelope(1, 30);
    let cmd = ExecuteCommand::new(
        vec![env],
        vec![PriorityOpsBatchInfo::default()],
        vec![vec![]],
    );

    let calldata = cmd.solidity_call(false);
    let decoded = IExecutor::executeBatchesSharedBridgeCall::abi_decode(&calldata)
        .expect("execute calldata should ABI-decode");
    let suffix = &decoded._executeData;

    // Skip the version byte and decode the v30 ABI params: (stored_infos, priority_ops, interop_roots)
    let (stored_infos, priority_ops, interop_roots): (
        Vec<IExecutor::StoredBatchInfo>,
        Vec<IExecutor::PriorityOpsBatchInfo>,
        Vec<Vec<zksync_os_contract_interface::InteropRoot>>,
    ) = <(
        Vec<IExecutor::StoredBatchInfo>,
        Vec<IExecutor::PriorityOpsBatchInfo>,
        Vec<Vec<zksync_os_contract_interface::InteropRoot>>,
    )>::abi_decode_params(&suffix[1..])
    .expect("v30 execute suffix should decode as 3-tuple");

    assert_eq!(stored_infos.len(), 1);
    assert_eq!(priority_ops.len(), 1);
    assert_eq!(interop_roots.len(), 1);
}

// ------------------------------------------------------------------
// T5.3 — Protocol v31 non-gateway execute omits logs/messages/multichain_roots
// ------------------------------------------------------------------
// Mutation: ignore the `gateway` flag and always include logs/messages
// → non-gateway v31 calldata has unexpected extra fields.
#[test]
fn t5_3_v31_non_gateway_omits_logs_messages() {
    let env = signed_envelope(1, 31);
    let cmd = ExecuteCommand::new(
        vec![env],
        vec![PriorityOpsBatchInfo::default()],
        vec![vec![]],
    );

    // gateway=false → logs/messages/multichain_roots must all be empty
    let calldata = cmd.solidity_call(false);
    let decoded = IExecutor::executeBatchesSharedBridgeCall::abi_decode(&calldata)
        .expect("execute calldata should ABI-decode");
    let suffix = &decoded._executeData;

    // v31 format: (stored_infos, priority_ops, interop_roots, logs, messages, multichain_roots)
    let (stored_infos, priority_ops, interop_roots, logs, messages, multichain_roots): (
        Vec<IExecutor::StoredBatchInfo>,
        Vec<IExecutor::PriorityOpsBatchInfo>,
        Vec<Vec<zksync_os_contract_interface::InteropRoot>>,
        Vec<Vec<IExecutor::L2Log>>,
        Vec<Vec<alloy::primitives::Bytes>>,
        Vec<alloy::primitives::B256>,
    ) = <(
        Vec<IExecutor::StoredBatchInfo>,
        Vec<IExecutor::PriorityOpsBatchInfo>,
        Vec<Vec<zksync_os_contract_interface::InteropRoot>>,
        Vec<Vec<IExecutor::L2Log>>,
        Vec<Vec<alloy::primitives::Bytes>>,
        Vec<alloy::primitives::B256>,
    )>::abi_decode_params(&suffix[1..])
    .expect("v31 execute suffix should decode as 6-tuple");

    assert_eq!(stored_infos.len(), 1);
    assert_eq!(priority_ops.len(), 1);
    // When gateway=false, logs/messages/multichain_roots must be empty
    assert!(logs.is_empty(), "logs must be empty for non-gateway execute");
    assert!(messages.is_empty(), "messages must be empty for non-gateway execute");
    assert!(multichain_roots.is_empty(), "multichain_roots must be empty for non-gateway execute");
    let _ = interop_roots;
}
