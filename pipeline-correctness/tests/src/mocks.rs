//! Mock implementations of storage traits for testing pipeline components in isolation.
//!
//! These mocks are intentionally simple — they store data in memory and provide
//! hooks for asserting what was written. They are not thread-safe beyond what
//! the trait bounds require.

use alloy::consensus::Header;
use alloy::primitives::{B256, BlockNumber, Sealed};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use zksync_os_interface::types::{BlockContext, BlockHashes, BlockOutput, StorageWrite};
use zksync_os_storage_api::{
    ReadReplay, ReadRepository, ReplayRecord, RepositoryBlock, RepositoryResult, StoredTxData,
    TxMeta, WriteReplay, WriteRepository, WriteState,
};
use zksync_os_types::{
    InteropRootsLogIndex, ProtocolSemanticVersion, ZkReceiptEnvelope, ZkTransaction,
};

/// Creates a minimal ReplayRecord for testing.
/// Block number, timestamp, and chain_id are configurable.
/// Everything else uses sensible defaults.
pub fn make_replay_record(block_number: u64, timestamp: u64) -> ReplayRecord {
    ReplayRecord {
        block_context: BlockContext {
            block_number,
            timestamp,
            chain_id: 270,
            eip1559_basefee: alloy::primitives::U256::from(1000u64),
            native_price: alloy::primitives::U256::from(1u64),
            pubdata_price: alloy::primitives::U256::from(1u64),
            coinbase: alloy::primitives::Address::ZERO,
            block_hashes: BlockHashes::default(),
            gas_limit: 1_000_000_000,
            pubdata_limit: 1_000_000,
            mix_hash: alloy::primitives::U256::ZERO,
            execution_version: 6,
            blob_fee: alloy::primitives::U256::ONE,
        },
        starting_l1_priority_id: 0,
        transactions: vec![],
        previous_block_timestamp: timestamp.saturating_sub(1),
        node_version: semver::Version::new(0, 16, 0),
        protocol_version: ProtocolSemanticVersion::new(0, 30, 2),
        block_output_hash: B256::from([block_number as u8; 32]),
        force_preimages: vec![],
        starting_interop_event_index: InteropRootsLogIndex {
            block_number: 0,
            index_in_block: 0,
        },
        starting_migration_number: 0,
        starting_interop_fee_number: 0,
    }
}

/// Creates a minimal BlockOutput for testing.
/// Uses alloy's Header with the specified block number and timestamp.
pub fn make_block_output(block_number: u64, timestamp: u64) -> BlockOutput {
    let header = Header {
        number: block_number,
        timestamp,
        ..Default::default()
    };
    let sealed_header = Sealed::new_unchecked(header, B256::from([block_number as u8; 32]));

    BlockOutput {
        header: sealed_header,
        tx_results: vec![],
        storage_writes: vec![],
        account_diffs: vec![],
        published_preimages: vec![],
        pubdata: vec![],
        computational_native_used: 0,
    }
}

/// In-memory replay storage. Tracks all writes for assertion.
#[derive(Debug, Clone)]
pub struct MockReplayStorage {
    inner: Arc<Mutex<MockReplayInner>>,
}

#[derive(Debug)]
struct MockReplayInner {
    records: HashMap<u64, ReplayRecord>,
    latest: u64,
    write_log: Vec<(u64, bool)>, // (block_number, override_allowed)
}

impl MockReplayStorage {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(MockReplayInner {
                records: HashMap::new(),
                latest: 0,
                write_log: vec![],
            })),
        }
    }

    /// Pre-populate with a genesis record at block 0.
    pub fn with_genesis(self) -> Self {
        let record = make_replay_record(0, 1000);
        let mut inner = self.inner.lock().unwrap();
        inner.records.insert(0, record);
        inner.latest = 0;
        drop(inner);
        self
    }

    /// Returns all (block_number, override_allowed) pairs that were written.
    pub fn write_log(&self) -> Vec<(u64, bool)> {
        self.inner.lock().unwrap().write_log.clone()
    }

    /// Returns the record at the given block number, if present.
    pub fn get(&self, block_number: u64) -> Option<ReplayRecord> {
        self.inner
            .lock()
            .unwrap()
            .records
            .get(&block_number)
            .cloned()
    }

    /// Number of records stored.
    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().records.len()
    }
}

impl ReadReplay for MockReplayStorage {
    fn get_context(&self, block_number: BlockNumber) -> Option<BlockContext> {
        self.inner
            .lock()
            .unwrap()
            .records
            .get(&block_number)
            .map(|r| r.block_context)
    }

    fn get_replay_record_by_key(
        &self,
        block_number: BlockNumber,
        _db_key: Option<Vec<u8>>,
    ) -> Option<ReplayRecord> {
        self.inner
            .lock()
            .unwrap()
            .records
            .get(&block_number)
            .cloned()
    }

    fn latest_record(&self) -> BlockNumber {
        self.inner.lock().unwrap().latest
    }
}

impl WriteReplay for MockReplayStorage {
    fn write(&self, record: Sealed<ReplayRecord>, override_allowed: bool) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let block_number = record.block_context.block_number;

        // Enforce: no overwrite without override_allowed
        if inner.records.contains_key(&block_number) {
            if !override_allowed {
                return false;
            }
        } else {
            // New block: enforce sequential ordering (must be next after latest)
            if !inner.records.is_empty() && block_number != inner.latest + 1 && !override_allowed {
                panic!(
                    "WriteReplay: block {} is not next after latest {}",
                    block_number, inner.latest
                );
            }
        }

        inner.write_log.push((block_number, override_allowed));
        inner.records.insert(block_number, record.into_inner());
        if block_number > inner.latest {
            inner.latest = block_number;
        }
        true
    }
}

/// In-memory state storage. Tracks block writes for assertion.
#[derive(Debug, Clone)]
pub struct MockWriteState {
    inner: Arc<Mutex<MockStateInner>>,
}

#[derive(Debug)]
struct MockStateInner {
    /// Tracks which block numbers were written and whether override was allowed.
    write_log: Vec<(u64, bool)>,
    /// If set, add_block_result() returns Err for this block number (simulates crash).
    fail_on_block: Option<u64>,
}

impl MockWriteState {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(MockStateInner {
                write_log: vec![],
                fail_on_block: None,
            })),
        }
    }

    /// Configure this mock to fail when writing a specific block number.
    /// Simulates a crash between WriteReplay and WriteState.
    pub fn with_fail_on_block(self, block_number: u64) -> Self {
        self.inner.lock().unwrap().fail_on_block = Some(block_number);
        self
    }

    /// Clear the failure injection (simulates successful restart).
    pub fn clear_failure(&self) {
        self.inner.lock().unwrap().fail_on_block = None;
    }

    pub fn write_log(&self) -> Vec<(u64, bool)> {
        self.inner.lock().unwrap().write_log.clone()
    }
}

impl WriteState for MockWriteState {
    fn add_block_result<'a, J>(
        &self,
        block_number: u64,
        _storage_diffs: Vec<StorageWrite>,
        _new_preimages: J,
        override_allowed: bool,
    ) -> anyhow::Result<()>
    where
        J: IntoIterator<Item = (B256, &'a Vec<u8>)>,
    {
        let mut inner = self.inner.lock().unwrap();
        if inner.fail_on_block == Some(block_number) {
            return Err(anyhow::anyhow!(
                "simulated crash: WriteState failed on block {block_number}"
            ));
        }
        inner.write_log.push((block_number, override_allowed));
        Ok(())
    }
}

/// In-memory repository. Tracks populated blocks.
#[derive(Debug, Clone)]
pub struct MockRepository {
    inner: Arc<Mutex<MockRepoInner>>,
}

#[derive(Debug)]
struct MockRepoInner {
    populated_blocks: Vec<u64>,
}

impl MockRepository {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(MockRepoInner {
                populated_blocks: vec![],
            })),
        }
    }

    pub fn populated_blocks(&self) -> Vec<u64> {
        self.inner.lock().unwrap().populated_blocks.clone()
    }
}

impl ReadRepository for MockRepository {
    fn get_block_by_number(
        &self,
        _number: BlockNumber,
    ) -> RepositoryResult<Option<RepositoryBlock>> {
        Ok(None)
    }

    fn get_block_by_hash(&self, _hash: B256) -> RepositoryResult<Option<RepositoryBlock>> {
        Ok(None)
    }

    fn get_raw_transaction(&self, _hash: B256) -> RepositoryResult<Option<Vec<u8>>> {
        Ok(None)
    }

    fn get_transaction(&self, _hash: B256) -> RepositoryResult<Option<ZkTransaction>> {
        Ok(None)
    }

    fn get_transaction_receipt(
        &self,
        _hash: B256,
    ) -> RepositoryResult<Option<ZkReceiptEnvelope>> {
        Ok(None)
    }

    fn get_transaction_meta(&self, _hash: B256) -> RepositoryResult<Option<TxMeta>> {
        Ok(None)
    }

    fn get_transaction_hash_by_sender_nonce(
        &self,
        _sender: alloy::primitives::Address,
        _nonce: u64,
    ) -> RepositoryResult<Option<B256>> {
        Ok(None)
    }

    fn get_stored_transaction(&self, _hash: B256) -> RepositoryResult<Option<StoredTxData>> {
        Ok(None)
    }

    fn get_latest_block(&self) -> u64 {
        0
    }
}

impl WriteRepository for MockRepository {
    fn populate(
        &self,
        block_output: BlockOutput,
        _txs: Vec<ZkTransaction>,
    ) -> impl std::future::Future<Output = RepositoryResult<()>> + Send {
        let inner = self.inner.clone();
        async move {
            inner
                .lock()
                .unwrap()
                .populated_blocks
                .push(block_output.header.number);
            Ok(())
        }
    }
}
