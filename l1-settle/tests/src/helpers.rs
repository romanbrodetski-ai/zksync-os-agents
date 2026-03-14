/// Shared test-data constructors for the l1-settle test suite.
///
/// These build minimal, valid instances of the types involved in L1 settling
/// without requiring a running node or L1 connection.
use alloy::primitives::{Address, B256, U256};
use zksync_os_batch_types::BatchInfo;
use zksync_os_contract_interface::models::{
    CommitBatchInfo, DACommitmentScheme, StoredBatchInfo,
};
use zksync_os_l1_sender::batcher_model::{
    BatchEnvelope, BatchMetadata, BatchSignatureData, FriProof, MissingSignature,
    SignedBatchEnvelope,
};
use zksync_os_types::{ProtocolSemanticVersion, PubdataMode};

/// Build a minimal `StoredBatchInfo` for batch `number`.
pub fn stored_batch_info(number: u64) -> StoredBatchInfo {
    StoredBatchInfo {
        batch_number: number,
        state_commitment: B256::from(U256::from(number * 1000 + 1)),
        number_of_layer1_txs: 0,
        priority_operations_hash: B256::from(U256::from(0xabu64)),
        dependency_roots_rolling_hash: B256::ZERO,
        l2_to_l1_logs_root_hash: B256::from(U256::from(0xcdu64)),
        commitment: B256::from(U256::from(number * 1000 + 2)),
        last_block_timestamp: Some(0),
    }
}

/// Build a minimal `CommitBatchInfo` for batch `number` using the given protocol minor version.
pub fn commit_batch_info(number: u64, protocol_minor: u64) -> CommitBatchInfo {
    CommitBatchInfo {
        batch_number: number,
        new_state_commitment: B256::from(U256::from(number * 1000 + 1)),
        number_of_layer1_txs: 0,
        number_of_layer2_txs: 1,
        priority_operations_hash: B256::from(U256::from(0xabu64)),
        dependency_roots_rolling_hash: B256::ZERO,
        l2_to_l1_logs_root_hash: B256::from(U256::from(0xcdu64)),
        l2_da_commitment_scheme: DACommitmentScheme::BlobsAndPubdataKeccak256,
        da_commitment: B256::ZERO,
        first_block_timestamp: 1_700_000_000,
        first_block_number: if protocol_minor >= 30 {
            Some(number)
        } else {
            None
        },
        last_block_timestamp: 1_700_000_001,
        last_block_number: if protocol_minor >= 30 {
            Some(number)
        } else {
            None
        },
        chain_id: 270,
        operator_da_input: vec![0u8; 32],
        sl_chain_id: if protocol_minor >= 31 { 1 } else { 0 },
    }
}

/// Build a minimal `BatchInfo` from a `CommitBatchInfo`.
pub fn batch_info(number: u64, protocol_minor: u64) -> BatchInfo {
    BatchInfo {
        commit_info: commit_batch_info(number, protocol_minor),
        chain_address: Address::repeat_byte(0x42),
        upgrade_tx_hash: None,
        blob_sidecar: None,
    }
}

/// Build a `BatchMetadata` for the given batch number and protocol minor version.
pub fn batch_metadata(number: u64, protocol_minor: u64) -> BatchMetadata {
    BatchMetadata {
        previous_stored_batch_info: stored_batch_info(number - 1),
        batch_info: batch_info(number, protocol_minor),
        first_block_number: number,
        last_block_number: number,
        pubdata_mode: PubdataMode::Calldata,
        tx_count: 1,
        execution_version: 5,
        protocol_version: protocol_version(protocol_minor),
        computational_native_used: None,
        logs: vec![],
        messages: vec![],
        multichain_root: B256::ZERO,
    }
}

/// Build a `SignedBatchEnvelope<FriProof>` with fake proof and no signatures.
pub fn signed_envelope(number: u64, protocol_minor: u64) -> SignedBatchEnvelope<FriProof> {
    BatchEnvelope {
        batch: batch_metadata(number, protocol_minor),
        data: FriProof::Fake,
        signature_data: BatchSignatureData::NotNeeded,
        latency_tracker: Default::default(),
    }
}

/// Build a `ProtocolSemanticVersion` for the given minor version.
pub fn protocol_version(minor: u64) -> ProtocolSemanticVersion {
    ProtocolSemanticVersion::new(0, minor, 0)
}

/// Build a `BatchEnvelope<(), MissingSignature>` — useful as input before signature step.
pub fn unsigned_envelope(number: u64, protocol_minor: u64) -> BatchEnvelope<FriProof, MissingSignature> {
    BatchEnvelope {
        batch: batch_metadata(number, protocol_minor),
        data: FriProof::Fake,
        signature_data: MissingSignature,
        latency_tracker: Default::default(),
    }
}
