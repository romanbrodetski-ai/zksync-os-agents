#[path = "../../zksync-os-server/node/bin/src/command_source.rs"]
mod command_source;

use alloy::consensus::Sealed;
use alloy::primitives::{Address, B256, TxHash, U256};
use command_source::{ConsensusNodeCommandSource, RebuildOptions};
use chrono::Utc;
use futures::StreamExt;
use num::BigUint;
use num::rational::Ratio;
use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use zksync_os_interface::traits::{PreimageSource, ReadStorage};
use zksync_os_interface::types::{BlockContext, BlockHashes};
use zksync_os_mempool::subpools::{
    interop_fee::InteropFeeSubpool, interop_roots::InteropRootsSubpool, l1::L1Subpool,
    l2, upgrade::UpgradeSubpool,
};
use zksync_os_mempool::{Pool, PoolConfig, TxValidatorConfig};
use zksync_os_pipeline::{PeekableReceiver, PipelineComponent};
use zksync_os_raft::LeadershipSignal;
use zksync_os_reth_compat::provider::ZkProviderFactory;
use zksync_os_sequencer::execution::block_context_provider::BlockContextProvider;
use zksync_os_sequencer::execution::{BlockCanonization, FeeConfig, FeeProvider, NoopCanonization};
use zksync_os_sequencer::model::blocks::{BlockCommand, InvalidTxPolicy, SealPolicy};
use zksync_os_storage_api::{
    ReadReplay, ReadRepository, ReadStateHistory, ReplayRecord, RepositoryResult, StateError,
    StateResult, StoredTxData, TxMeta, ViewState,
};
use zksync_os_types::{
    InteropRootsLogIndex, L1PriorityEnvelope, L1PriorityTx, L1UpgradeEnvelope, L1UpgradeTx,
    ProtocolSemanticVersion, PubdataMode, TokenApiRatio, TokenPricesForFees, ZkEnvelope,
    ZkTransaction,
};

#[derive(Debug, Clone)]
struct FakeReplayStorage {
    records: Arc<BTreeMap<u64, ReplayRecord>>,
}

impl FakeReplayStorage {
    fn new(records: impl IntoIterator<Item = ReplayRecord>) -> Self {
        let records = records
            .into_iter()
            .map(|record| (record.block_context.block_number, record))
            .collect();
        Self {
            records: Arc::new(records),
        }
    }
}

impl ReadReplay for FakeReplayStorage {
    fn get_context(&self, block_number: u64) -> Option<BlockContext> {
        self.records.get(&block_number).map(|record| record.block_context)
    }

    fn get_replay_record_by_key(
        &self,
        block_number: u64,
        _db_key: Option<Vec<u8>>,
    ) -> Option<ReplayRecord> {
        self.records.get(&block_number).cloned()
    }

    fn latest_record(&self) -> u64 {
        *self.records.keys().max().expect("records must not be empty")
    }
}

#[derive(Debug, Clone, Copy)]
struct DummyStateHistory;

#[derive(Debug, Clone, Copy)]
struct DummyStateView;

impl ReadStorage for DummyStateView {
    fn read(&mut self, _key: B256) -> Option<B256> {
        None
    }
}

impl PreimageSource for DummyStateView {
    fn get_preimage(&mut self, _hash: B256) -> Option<Vec<u8>> {
        None
    }
}

impl ReadStateHistory for DummyStateHistory {
    fn state_view_at(&self, block_number: u64) -> StateResult<impl ViewState> {
        if block_number == 0 {
            Ok(DummyStateView)
        } else {
            Err(StateError::NotFound(block_number))
        }
    }

    fn block_range_available(&self) -> std::ops::RangeInclusive<u64> {
        0..=0
    }
}

#[derive(Debug, Clone, Copy)]
struct DummyRepository;

fn genesis_repository_block() -> Sealed<alloy::consensus::Block<TxHash>> {
    let header = alloy::consensus::Header {
        number: 0,
        gas_limit: 30_000_000,
        timestamp: 1,
        ..Default::default()
    };
    Sealed::new_unchecked(
        alloy::consensus::Block {
            header,
            body: Default::default(),
        },
        B256::with_last_byte(0x42),
    )
}

impl ReadRepository for DummyRepository {
    fn get_block_by_number(&self, number: u64) -> RepositoryResult<Option<Sealed<alloy::consensus::Block<TxHash>>>> {
        Ok((number == 0).then(genesis_repository_block))
    }

    fn get_block_by_hash(&self, hash: alloy::primitives::BlockHash) -> RepositoryResult<Option<Sealed<alloy::consensus::Block<TxHash>>>> {
        let block = genesis_repository_block();
        Ok((block.hash() == hash).then_some(block))
    }

    fn get_raw_transaction(&self, _hash: alloy::primitives::TxHash) -> RepositoryResult<Option<Vec<u8>>> {
        Ok(None)
    }

    fn get_transaction(&self, _hash: alloy::primitives::TxHash) -> RepositoryResult<Option<ZkTransaction>> {
        Ok(None)
    }

    fn get_transaction_receipt(&self, _hash: alloy::primitives::TxHash) -> RepositoryResult<Option<zksync_os_types::ZkReceiptEnvelope>> {
        Ok(None)
    }

    fn get_transaction_meta(&self, _hash: alloy::primitives::TxHash) -> RepositoryResult<Option<TxMeta>> {
        Ok(None)
    }

    fn get_transaction_hash_by_sender_nonce(
        &self,
        _sender: Address,
        _nonce: u64,
    ) -> RepositoryResult<Option<alloy::primitives::TxHash>> {
        Ok(None)
    }

    fn get_stored_transaction(&self, _hash: alloy::primitives::TxHash) -> RepositoryResult<Option<StoredTxData>> {
        Ok(None)
    }

    fn get_latest_block(&self) -> u64 {
        0
    }
}

fn sample_protocol_version() -> ProtocolSemanticVersion {
    ProtocolSemanticVersion::new(0, 30, 2)
}

fn sample_block_context(block_number: u64, execution_version: u32, timestamp: u64) -> BlockContext {
    BlockContext {
        eip1559_basefee: U256::from(100 + block_number),
        native_price: U256::from(200 + block_number),
        pubdata_price: U256::from(300 + block_number),
        block_number,
        timestamp,
        blob_fee: U256::from(400 + block_number),
        chain_id: 777,
        coinbase: Address::with_last_byte(0x11),
        block_hashes: Default::default(),
        gas_limit: 1_000_000,
        pubdata_limit: 2_000_000,
        mix_hash: B256::ZERO.into(),
        execution_version,
    }
}

fn l1_tx(priority_id: u64) -> ZkTransaction {
    let tx = L1PriorityTx {
        hash: B256::with_last_byte(priority_id as u8),
        initiator: Address::with_last_byte(0x10),
        to: Address::with_last_byte(0x20),
        gas_limit: 200_000,
        gas_per_pubdata_byte_limit: 800,
        max_fee_per_gas: 1,
        max_priority_fee_per_gas: 1,
        nonce: priority_id,
        value: U256::ZERO,
        to_mint: U256::ZERO,
        refund_recipient: Address::with_last_byte(0x30),
        input: Default::default(),
        factory_deps: Vec::new(),
        marker: Default::default(),
    };
    L1PriorityEnvelope { inner: tx }.into()
}

fn upgrade_tx(protocol_version: ProtocolSemanticVersion) -> ZkTransaction {
    let tx = L1UpgradeTx {
        hash: B256::with_last_byte(protocol_version.patch as u8),
        initiator: Address::with_last_byte(0x40),
        to: Address::with_last_byte(0x50),
        gas_limit: 200_000,
        gas_per_pubdata_byte_limit: 800,
        max_fee_per_gas: 1,
        max_priority_fee_per_gas: 1,
        nonce: protocol_version.minor,
        value: U256::ZERO,
        to_mint: U256::ZERO,
        refund_recipient: Address::with_last_byte(0x60),
        input: Default::default(),
        factory_deps: Vec::new(),
        marker: Default::default(),
    };
    L1UpgradeEnvelope { inner: tx }.into()
}

fn replay_record(
    block_number: u64,
    starting_l1_priority_id: u64,
    transactions: Vec<ZkTransaction>,
) -> ReplayRecord {
    ReplayRecord::new(
        sample_block_context(block_number, 3, 1_000 + block_number),
        starting_l1_priority_id,
        transactions,
        999 + block_number,
        semver::Version::new(0, 16, 0),
        sample_protocol_version(),
        B256::with_last_byte(block_number as u8),
        vec![(B256::with_last_byte(0xaa), vec![1, 2, 3])],
        InteropRootsLogIndex {
            block_number,
            index_in_block: 2,
        },
        7,
        11,
    )
}

async fn collect_commands(
    source: ConsensusNodeCommandSource<FakeReplayStorage>,
    count: usize,
) -> Vec<BlockCommand> {
    let (_input_tx, input_rx) = mpsc::channel(1);
    let (output_tx, mut output_rx) = mpsc::channel(32);
    let task = tokio::spawn(async move {
        let _ = source.run(PeekableReceiver::new(input_rx), output_tx).await;
    });

    let mut commands = Vec::with_capacity(count);
    for _ in 0..count {
        commands.push(output_rx.recv().await.expect("command stream ended early"));
    }
    drop(output_rx);
    task.await.unwrap();
    commands
}

async fn run_command_source_to_completion(source: ConsensusNodeCommandSource<FakeReplayStorage>) {
    let (_input_tx, input_rx) = mpsc::channel(1);
    let (output_tx, _output_rx) = mpsc::channel(1);
    source
        .run(PeekableReceiver::new(input_rx), output_tx)
        .await
        .unwrap();
}

fn make_fee_provider() -> FeeProvider {
    let (_, pubdata_price_rx) = watch::channel(Some(U256::ONE));
    let (_, blob_fill_ratio_rx) = watch::channel(Some(Ratio::new(1u64, 1u64)));
    let (_, token_price_rx) = watch::channel(Some(TokenPricesForFees {
        base_token_usd_price: TokenApiRatio {
            ratio: Ratio::from_integer(BigUint::from(1u32)),
            timestamp: Utc::now(),
        },
        sl_token_usd_price: TokenApiRatio {
            ratio: Ratio::from_integer(BigUint::from(1u32)),
            timestamp: Utc::now(),
        },
    }));
    FeeProvider::new(
        FeeConfig {
            native_price_usd: Ratio::from_integer(BigUint::from(1u32)),
            base_fee_override: Some(BigUint::from(1u32)),
            native_per_gas: 1,
            pubdata_price_override: Some(BigUint::from(1u32)),
            pubdata_price_cap: None,
            native_price_override: Some(BigUint::from(1u32)),
        },
        None,
        pubdata_price_rx,
        blob_fill_ratio_rx,
        token_price_rx,
        Some(PubdataMode::Calldata),
    )
}

fn make_provider(next_l1_priority_id: u64) -> BlockContextProvider<impl zksync_os_mempool::subpools::l2::L2Subpool> {
    let zk_provider_factory = ZkProviderFactory::new(DummyStateHistory, DummyRepository, 270);
    let l2_subpool = l2::in_memory(
        zk_provider_factory,
        PoolConfig::default(),
        TxValidatorConfig {
            max_input_bytes: usize::MAX,
        },
    );
    let pool = Pool::new(
        UpgradeSubpool::new(sample_protocol_version()),
        Default::default(),
        InteropFeeSubpool::new(91),
        InteropRootsSubpool::new(100),
        L1Subpool::new(16),
        l2_subpool,
    );
    let (sender, _receiver) = watch::channel(None);
    BlockContextProvider::new(
        next_l1_priority_id,
        InteropRootsLogIndex {
            block_number: 88,
            index_in_block: 5,
        },
        77,
        91,
        pool,
        Default::default(),
        555,         // previous_block_timestamp
        1,           // next_block_number (unused by Rebuild branch)
        Duration::from_millis(500), // block_time
        100,         // max_transactions_in_block
        270,
        10_000_000,
        20_000_000,
        100,
        Duration::from_secs(1),
        sample_protocol_version(),
        Address::with_last_byte(0xfe),
        sender,
        make_fee_provider(),
    )
}

async fn drain_txs(command: zksync_os_sequencer::model::blocks::PreparedBlockCommand<'_>) -> Vec<ZkTransaction> {
    command.tx_source.stream.collect::<Vec<_>>().await
}

#[tokio::test]
async fn command_source_replays_then_rebuilds_then_produces() {
    // Fail-first validation: changed `replay_end` to `last_block_in_wal` inside the rebuild branch,
    // which caused block 2 to be replayed instead of rebuilt and made this test fail.
    let storage = FakeReplayStorage::new([
        replay_record(1, 0, vec![]),
        replay_record(2, 5, vec![l1_tx(5)]),
        replay_record(3, 0, vec![upgrade_tx(sample_protocol_version())]),
    ]);
    let (_replays_tx, replays_to_execute) = mpsc::channel(1);
    let source = ConsensusNodeCommandSource {
        block_replay_storage: storage,
        starting_block: 1,
        rebuild_options: Some(RebuildOptions {
            rebuild_from_block: 2,
            blocks_to_empty: HashSet::from([3]),
        }),
        replays_to_execute,
        leadership: LeadershipSignal::AlwaysLeader,
    };

    let commands = collect_commands(source, 4).await;

    assert!(matches!(&commands[0], BlockCommand::Replay(record) if record.block_context.block_number == 1));
    assert!(matches!(&commands[1], BlockCommand::Rebuild(rebuild) if rebuild.replay_record.block_context.block_number == 2 && !rebuild.make_empty));
    assert!(matches!(&commands[2], BlockCommand::Rebuild(rebuild) if rebuild.replay_record.block_context.block_number == 3 && rebuild.make_empty));
    assert!(matches!(&commands[3], BlockCommand::Produce(_)));
}

#[tokio::test]
async fn command_source_rebuild_from_starting_block_skips_replay_phase() {
    // Fail-first validation: changed `replay_end` to `rebuild_from_block` in the rebuild branch,
    // which replayed block 2 before rebuilding it and made this test fail.
    let storage = FakeReplayStorage::new([
        replay_record(2, 0, vec![]),
        replay_record(3, 0, vec![]),
    ]);
    let (_replays_tx, replays_to_execute) = mpsc::channel(1);
    let source = ConsensusNodeCommandSource {
        block_replay_storage: storage,
        starting_block: 2,
        rebuild_options: Some(RebuildOptions {
            rebuild_from_block: 2,
            blocks_to_empty: HashSet::new(),
        }),
        replays_to_execute,
        leadership: LeadershipSignal::AlwaysLeader,
    };

    let commands = collect_commands(source, 3).await;

    assert!(matches!(&commands[0], BlockCommand::Rebuild(rebuild) if rebuild.replay_record.block_context.block_number == 2));
    assert!(matches!(&commands[1], BlockCommand::Rebuild(rebuild) if rebuild.replay_record.block_context.block_number == 3));
    assert!(matches!(&commands[2], BlockCommand::Produce(_)));
}

#[tokio::test]
#[should_panic(expected = "rebuild_from_block must be >= starting_block")]
async fn command_source_rejects_rebuild_before_starting_block() {
    // Fail-first validation: removed the lower-bound assert in `command_source`, which let the
    // source start and made this panic expectation fail.
    let storage = FakeReplayStorage::new([replay_record(3, 0, vec![])]);
    let (_replays_tx, replays_to_execute) = mpsc::channel(1);
    let source = ConsensusNodeCommandSource {
        block_replay_storage: storage,
        starting_block: 3,
        rebuild_options: Some(RebuildOptions {
            rebuild_from_block: 2,
            blocks_to_empty: HashSet::new(),
        }),
        replays_to_execute,
        leadership: LeadershipSignal::AlwaysLeader,
    };

    run_command_source_to_completion(source).await;
}

#[tokio::test]
#[should_panic(expected = "rebuild_from_block must be <= last_block_in_wal")]
async fn command_source_rejects_rebuild_after_latest_record() {
    // Fail-first validation: changed the upper-bound assert to a strict `< last_block_in_wal`
    // check, which produced a different panic and made this expectation fail.
    let storage = FakeReplayStorage::new([replay_record(3, 0, vec![])]);
    let (_replays_tx, replays_to_execute) = mpsc::channel(1);
    let source = ConsensusNodeCommandSource {
        block_replay_storage: storage,
        starting_block: 3,
        rebuild_options: Some(RebuildOptions {
            rebuild_from_block: 4,
            blocks_to_empty: HashSet::new(),
        }),
        replays_to_execute,
        leadership: LeadershipSignal::AlwaysLeader,
    };

    run_command_source_to_completion(source).await;
}

#[tokio::test]
async fn rebuild_prepare_filters_l1_transactions_after_priority_gap() {
    // Fail-first validation: changed the mismatch branch to keep all replay transactions instead of
    // filtering L1 txs, which made this test fail.
    let mut provider = make_provider(8);
    let rebuild_record = replay_record(
        5,
        5,
        vec![l1_tx(5), upgrade_tx(sample_protocol_version()), l1_tx(6)],
    );

    let prepared = provider
        .prepare_command(BlockCommand::Rebuild(Box::new(
            zksync_os_sequencer::model::blocks::RebuildCommand {
                replay_record: rebuild_record,
                make_empty: false,
            },
        )))
        .await
        .unwrap();

    let txs = drain_txs(prepared).await;
    assert_eq!(txs.len(), 1);
    assert!(matches!(txs[0].envelope(), ZkEnvelope::Upgrade(_)));
}

#[tokio::test]
async fn rebuild_prepare_keeps_transactions_when_first_l1_matches() {
    // Fail-first validation: inverted the priority-id comparison used to decide L1 filtering,
    // which dropped L1 txs even on the aligned case and made this test fail.
    let mut provider = make_provider(5);
    let rebuild_record = replay_record(
        5,
        5,
        vec![l1_tx(5), upgrade_tx(sample_protocol_version()), l1_tx(6)],
    );

    let prepared = provider
        .prepare_command(BlockCommand::Rebuild(Box::new(
            zksync_os_sequencer::model::blocks::RebuildCommand {
                replay_record: rebuild_record,
                make_empty: false,
            },
        )))
        .await
        .unwrap();

    let txs = drain_txs(prepared).await;
    assert_eq!(txs.len(), 3);
    assert!(matches!(txs[0].envelope(), ZkEnvelope::L1(_)));
    assert!(matches!(txs[1].envelope(), ZkEnvelope::Upgrade(_)));
    assert!(matches!(txs[2].envelope(), ZkEnvelope::L1(_)));
}

#[tokio::test]
async fn rebuild_prepare_empty_block_drops_all_transactions_and_uses_current_cursors() {
    // Fail-first validation: changed `starting_l1_priority_id` to use the replay record value and
    // changed the empty branch to keep replay txs, either of which makes this test fail.
    let mut provider = make_provider(12);
    let rebuild_record = replay_record(
        9,
        3,
        vec![l1_tx(3), l1_tx(4)],
    );

    let prepared = provider
        .prepare_command(BlockCommand::Rebuild(Box::new(
            zksync_os_sequencer::model::blocks::RebuildCommand {
                replay_record: rebuild_record.clone(),
                make_empty: true,
            },
        )))
        .await
        .unwrap();

    assert!(matches!(
        prepared.seal_policy,
        SealPolicy::UntilExhausted {
            allowed_to_finish_early: true
        }
    ));
    assert!(matches!(
        prepared.invalid_tx_policy,
        InvalidTxPolicy::RejectAndContinue
    ));
    assert_eq!(prepared.starting_l1_priority_id, 12);
    assert_eq!(
        prepared.starting_interop_event_index,
        InteropRootsLogIndex {
            block_number: 88,
            index_in_block: 5
        }
    );
    assert_eq!(prepared.starting_migration_number, 77);
    assert_eq!(prepared.starting_interop_fee_number, 91);
    assert_eq!(prepared.expected_block_output_hash, None);
    assert_eq!(prepared.block_context.block_number, 9);
    assert_eq!(prepared.block_context.timestamp, rebuild_record.block_context.timestamp);
    assert_eq!(prepared.block_context.eip1559_basefee, rebuild_record.block_context.eip1559_basefee);
    assert_eq!(prepared.block_context.native_price, rebuild_record.block_context.native_price);
    assert_eq!(prepared.block_context.pubdata_price, rebuild_record.block_context.pubdata_price);
    assert_eq!(prepared.block_context.blob_fee, rebuild_record.block_context.blob_fee);
    assert_eq!(prepared.block_context.chain_id, 270);
    assert_eq!(prepared.block_context.coinbase, Address::with_last_byte(0xfe));

    let txs = drain_txs(prepared).await;
    assert!(txs.is_empty());
}

#[tokio::test]
async fn rebuild_prepare_rejects_empty_upgrade_block() {
    // Fail-first validation: removed the `make_empty && has_upgrade` guard, which made the command
    // prepare successfully and caused this test to fail.
    let mut provider = make_provider(0);
    let error = provider
        .prepare_command(BlockCommand::Rebuild(Box::new(
            zksync_os_sequencer::model::blocks::RebuildCommand {
                replay_record: replay_record(4, 0, vec![upgrade_tx(sample_protocol_version())]),
                make_empty: true,
            },
        )))
        .await
        .expect_err("empty rebuild with an upgrade tx must fail");

    assert!(
        error
            .to_string()
            .contains("Cannot make an empty block when there is an upgrade transaction")
    );
}

// Helper: like make_provider but with a non-default block_hashes value.
fn make_provider_with_block_hashes(
    next_l1_priority_id: u64,
    block_hashes: BlockHashes,
) -> BlockContextProvider<impl zksync_os_mempool::subpools::l2::L2Subpool> {
    let zk_provider_factory = ZkProviderFactory::new(DummyStateHistory, DummyRepository, 270);
    let l2_subpool = l2::in_memory(
        zk_provider_factory,
        PoolConfig::default(),
        TxValidatorConfig {
            max_input_bytes: usize::MAX,
        },
    );
    let pool = Pool::new(
        UpgradeSubpool::new(sample_protocol_version()),
        Default::default(),
        InteropFeeSubpool::new(91),
        InteropRootsSubpool::new(100),
        L1Subpool::new(16),
        l2_subpool,
    );
    let (sender, _receiver) = watch::channel(None);
    BlockContextProvider::new(
        next_l1_priority_id,
        InteropRootsLogIndex {
            block_number: 88,
            index_in_block: 5,
        },
        77,
        91,
        pool,
        block_hashes,
        555,         // previous_block_timestamp
        1,           // next_block_number (unused by Rebuild branch)
        Duration::from_millis(500), // block_time
        100,         // max_transactions_in_block
        270,
        10_000_000,
        20_000_000,
        100,
        Duration::from_secs(1),
        sample_protocol_version(),
        Address::with_last_byte(0xfe),
        sender,
        make_fee_provider(),
    )
}

// ── New tests ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn command_source_no_rebuild_replays_all_then_produces() {
    // Fail-first validation: changed the `else` branch of `rebuild_options` to emit a rebuild
    // stream instead of an empty one, so the WAL records were turned into Rebuild commands rather
    // than Replay commands, causing the first two assertions to fail.
    let storage = FakeReplayStorage::new([
        replay_record(10, 0, vec![]),
        replay_record(11, 0, vec![]),
        replay_record(12, 0, vec![]),
    ]);
    let (_replays_tx, replays_to_execute) = mpsc::channel(1);
    let source = ConsensusNodeCommandSource {
        block_replay_storage: storage,
        starting_block: 10,
        rebuild_options: None,
        replays_to_execute,
        leadership: LeadershipSignal::AlwaysLeader,
    };

    // 3 replay commands + 1 produce command.
    let commands = collect_commands(source, 4).await;

    assert!(matches!(&commands[0], BlockCommand::Replay(r) if r.block_context.block_number == 10));
    assert!(matches!(&commands[1], BlockCommand::Replay(r) if r.block_context.block_number == 11));
    assert!(matches!(&commands[2], BlockCommand::Replay(r) if r.block_context.block_number == 12));
    // ProduceCommand is now a unit struct: block number is tracked inside BlockContextProvider.
    assert!(matches!(&commands[3], BlockCommand::Produce(_)));
}

#[tokio::test]
async fn rebuild_prepare_non_empty_with_no_l1_txs_keeps_all_transactions() {
    // Fail-first validation: set `filter_l1_txs = true` in the `else` branch (None case) AND
    // widened the filter to also drop upgrade txs, which emptied the prepared command and made
    // this test fail with "left: 0, right: 1".
    let mut provider = make_provider(5);
    let rebuild_record = replay_record(
        7,
        0,
        // Only an upgrade tx, no L1 priority txs.
        vec![upgrade_tx(sample_protocol_version())],
    );

    let prepared = provider
        .prepare_command(BlockCommand::Rebuild(Box::new(
            zksync_os_sequencer::model::blocks::RebuildCommand {
                replay_record: rebuild_record,
                make_empty: false,
            },
        )))
        .await
        .unwrap();

    let txs = drain_txs(prepared).await;
    // The upgrade tx must survive; filter_l1_txs == false because first_l1_tx is None.
    assert_eq!(txs.len(), 1);
    assert!(matches!(txs[0].envelope(), ZkEnvelope::Upgrade(_)));
}

#[tokio::test]
async fn rebuild_prepare_preserves_force_preimages() {
    // Fail-first validation: replaced `force_preimages: rebuild.replay_record.force_preimages`
    // with `force_preimages: vec![]`, which caused the assertion below to fail.
    let mut provider = make_provider(0);
    let rebuild_record = replay_record(
        8,
        0,
        vec![],
    );
    let expected_preimages = rebuild_record.force_preimages.clone();

    let prepared = provider
        .prepare_command(BlockCommand::Rebuild(Box::new(
            zksync_os_sequencer::model::blocks::RebuildCommand {
                replay_record: rebuild_record,
                make_empty: false,
            },
        )))
        .await
        .unwrap();

    assert_eq!(prepared.force_preimages, expected_preimages);
    // The helper sets one (B256::with_last_byte(0xaa), vec![1,2,3]) entry; confirm it is non-empty
    // so this test actually exercises the non-trivial case.
    assert!(!prepared.force_preimages.is_empty());
}

#[tokio::test]
async fn rebuild_prepare_preserves_execution_version_and_protocol_version() {
    // Fail-first validation: hardcoded execution_version to 0 in the rebuild branch, which
    // caused the execution_version assertion to fail (left: 0, right: 3).
    // Note: the protocol_version invariant is confirmed structurally by asserting the field
    // equals the replay record value; a source-swap mutation is a no-op because the provider
    // and replay record share the same sample_protocol_version() in this test.
    let mut provider = make_provider(0);
    // sample_block_context uses execution_version=3; sample_protocol_version() is (0,30,2).
    let rebuild_record = replay_record(9, 0, vec![]);
    let expected_exec_version = rebuild_record.block_context.execution_version;
    let expected_proto_version = rebuild_record.protocol_version.clone();

    let prepared = provider
        .prepare_command(BlockCommand::Rebuild(Box::new(
            zksync_os_sequencer::model::blocks::RebuildCommand {
                replay_record: rebuild_record,
                make_empty: false,
            },
        )))
        .await
        .unwrap();

    assert_eq!(
        prepared.block_context.execution_version,
        expected_exec_version,
        "execution_version must be preserved from replay record"
    );
    assert_eq!(
        prepared.protocol_version,
        expected_proto_version,
        "protocol_version must be preserved from replay record"
    );
}

#[tokio::test]
async fn rebuild_prepare_uses_provider_block_hashes_not_replay_record() {
    // Fail-first validation: changed the rebuild branch to use
    // `rebuild.replay_record.block_context.block_hashes` instead of
    // `self.block_hashes_for_next_block`, which caused the assertion to fail because
    // the replay record carries Default::default() while the provider carries a distinct value.
    let mut distinct_hashes = [U256::ZERO; 256];
    distinct_hashes[0] = U256::from(0xdeadbeef_u64);
    let provider_block_hashes = BlockHashes(distinct_hashes);

    // Confirm the replay record has the default (all-zero) block hashes.
    let rebuild_record = replay_record(10, 0, vec![]);
    assert_eq!(rebuild_record.block_context.block_hashes, BlockHashes::default(),
        "replay_record must have default block_hashes for this test to be meaningful");

    let mut provider = make_provider_with_block_hashes(0, provider_block_hashes);

    let prepared = provider
        .prepare_command(BlockCommand::Rebuild(Box::new(
            zksync_os_sequencer::model::blocks::RebuildCommand {
                replay_record: rebuild_record,
                make_empty: false,
            },
        )))
        .await
        .unwrap();

    assert_eq!(
        prepared.block_context.block_hashes,
        provider_block_hashes,
        "block_hashes must come from provider, not replay record"
    );
    assert_ne!(
        prepared.block_context.block_hashes,
        BlockHashes::default(),
        "sanity: the two values must be distinct"
    );
}

/// Confirms that NoopCanonization uses an unbounded channel, so any number of proposals
/// can be buffered without blocking. This is important because BlockCanonizer's
/// MAX_PRODUCED_QUEUE_SIZE = 2 provides the actual backpressure, not the channel itself.
///
/// Fail-first validation: replaced `mpsc::unbounded_channel()` with `mpsc::channel(1)`
/// in NoopCanonization::new(), which caused propose(block2).await to block and this test to timeout.
#[tokio::test]
async fn noop_canonization_channel_supports_max_produced_queue_size_proposals() {
    let noop = NoopCanonization::new();

    // Propose 3 blocks without draining between them.
    // If channel were bounded to MAX_PRODUCED_QUEUE_SIZE (2), the third propose would block.
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        noop.propose(replay_record(1, 0, vec![])).await.unwrap();
        noop.propose(replay_record(2, 0, vec![])).await.unwrap();
        noop.propose(replay_record(3, 0, vec![])).await.unwrap();
    })
    .await;

    result.expect(
        "NoopCanonization channel must be unbounded (was changed from bounded in consensus refactor). \
         propose(block3) deadlocked — the channel has insufficient capacity.",
    );
}

/// Confirms that BlockContextProvider tracks next_block_number and checks it on Replay commands.
/// This is a new invariant added in the consensus refactor to detect out-of-order block delivery.
///
/// Fail-first validation: removed the `self.next_block_number == record.block_context.block_number`
/// check from the Replay branch, which let a mismatched Replay through and made this test fail.
#[tokio::test]
async fn replay_rejects_out_of_order_block_number() {
    let mut provider = make_provider(0);
    // Provider starts with next_block_number = 1, but we send a replay for block 5.
    let record = replay_record(5, 0, vec![]);
    let err = provider
        .prepare_command(BlockCommand::Replay(Box::new(record)))
        .await
        .expect_err("Replay with wrong block number must fail");

    assert!(
        err.to_string().contains("blocks received"),
        "error must mention block ordering: {err}"
    );
}
