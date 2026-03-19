use crate::mocks::{
    MockReplayStorage, MockRepository, MockWriteState, make_block_output, make_replay_record,
};
use alloy::primitives::B256;
use async_trait::async_trait;
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::mpsc;
use zksync_os_pipeline::{PeekableReceiver, PipelineComponent};
use zksync_os_sequencer::config::SequencerConfig;
use zksync_os_sequencer::execution::{
    BlockApplier, BlockCanonization, BlockCanonizer, NoopCanonization,
};
use zksync_os_sequencer::model::blocks::{BlockCommand, BlockCommandType, ProduceCommand};
use zksync_os_storage_api::{ReadReplayExt, ReplayRecord, WriteReplay};
use zksync_os_types::NodeRole;

fn test_config(node_role: NodeRole) -> SequencerConfig {
    SequencerConfig {
        node_role,
        block_time: Duration::from_secs(1),
        max_transactions_in_block: 10,
        block_dump_path: PathBuf::new(),
        block_gas_limit: 1_000_000,
        block_pubdata_limit_bytes: 1_000_000,
        max_blocks_to_produce: None,
        interop_roots_per_tx: 1,
    }
}

#[derive(Debug)]
struct MockConsensus {
    proposals: mpsc::UnboundedSender<ReplayRecord>,
    canonized: mpsc::Receiver<ReplayRecord>,
}

#[async_trait]
impl BlockCanonization for MockConsensus {
    async fn propose(&self, record: ReplayRecord) -> anyhow::Result<()> {
        self.proposals.send(record)?;
        Ok(())
    }

    async fn next_canonized(&mut self) -> anyhow::Result<ReplayRecord> {
        self.canonized
            .recv()
            .await
            .ok_or_else(|| anyhow::anyhow!("canonized channel closed"))
    }
}

#[test]
fn produce_command_is_parameterless_and_block_commands_report_type() {
    let produce = BlockCommand::Produce(ProduceCommand);
    let replay = BlockCommand::Replay(Box::new(make_replay_record(3, 1003)));
    let rebuild = BlockCommand::Rebuild(Box::new(
        zksync_os_sequencer::model::blocks::RebuildCommand {
            replay_record: make_replay_record(4, 1004),
            make_empty: false,
        },
    ));

    assert!(matches!(produce, BlockCommand::Produce(ProduceCommand)));
    assert!(matches!(produce.command_type(), BlockCommandType::Produce));
    assert!(matches!(replay.command_type(), BlockCommandType::Replay));
    assert!(matches!(rebuild.command_type(), BlockCommandType::Rebuild));
}

#[tokio::test]
async fn forward_range_with_sends_an_inclusive_ordered_range() {
    let storage = MockReplayStorage::new().with_genesis();
    for block in 1..=4 {
        storage.write(
            alloy::consensus::Sealed::new_unchecked(
                make_replay_record(block, 1000 + block),
                B256::from([block as u8; 32]),
            ),
            false,
        );
    }

    let (tx, mut rx) = mpsc::channel(8);
    storage
        .forward_range_with(1, 3, tx, |record| record.block_context.block_number)
        .await
        .unwrap();

    let mut seen = Vec::new();
    while let Ok(block) = tokio::time::timeout(Duration::from_millis(20), rx.recv()).await {
        let Some(block) = block else {
            break;
        };
        seen.push(block);
        if seen.len() == 3 {
            break;
        }
    }

    assert_eq!(seen, vec![1, 2, 3]);
}

#[tokio::test]
async fn block_canonizer_forwards_replay_without_consensus_roundtrip() {
    let (proposal_tx, mut proposal_rx) = mpsc::unbounded_channel();
    let (_canonized_tx, canonized_rx) = mpsc::channel(4);
    let (replay_tx, _replay_rx) = mpsc::channel(4);
    let component = BlockCanonizer {
        consensus: MockConsensus {
            proposals: proposal_tx,
            canonized: canonized_rx,
        },
        canonized_blocks_for_execution: replay_tx,
    };

    let (input_tx, input_rx) = mpsc::channel(4);
    let (output_tx, mut output_rx) = mpsc::channel(4);
    let handle = tokio::spawn(async move {
        component
            .run(PeekableReceiver::new(input_rx), output_tx)
            .await
    });

    let block_output = make_block_output(1, 1001);
    let replay_record = make_replay_record(1, 1001);
    input_tx
        .send((
            block_output.clone(),
            replay_record.clone(),
            BlockCommandType::Replay,
        ))
        .await
        .unwrap();

    let forwarded = tokio::time::timeout(Duration::from_secs(1), output_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(forwarded.0.header.number, 1);
    assert_eq!(forwarded.1, replay_record);
    assert!(matches!(forwarded.2, BlockCommandType::Replay));
    assert!(proposal_rx.try_recv().is_err());

    handle.abort();
}

#[tokio::test]
async fn block_canonizer_holds_produced_blocks_until_matching_canonization() {
    let (proposal_tx, mut proposal_rx) = mpsc::unbounded_channel();
    let (canonized_tx, canonized_rx) = mpsc::channel(4);
    let (replay_tx, _replay_rx) = mpsc::channel(4);
    let component = BlockCanonizer {
        consensus: MockConsensus {
            proposals: proposal_tx,
            canonized: canonized_rx,
        },
        canonized_blocks_for_execution: replay_tx,
    };

    let (input_tx, input_rx) = mpsc::channel(4);
    let (output_tx, mut output_rx) = mpsc::channel(4);
    let handle = tokio::spawn(async move {
        component
            .run(PeekableReceiver::new(input_rx), output_tx)
            .await
    });

    let block_output = make_block_output(7, 1007);
    let replay_record = make_replay_record(7, 1007);
    input_tx
        .send((
            block_output.clone(),
            replay_record.clone(),
            BlockCommandType::Produce,
        ))
        .await
        .unwrap();

    let proposed = tokio::time::timeout(Duration::from_secs(1), proposal_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(proposed, replay_record);
    assert!(
        tokio::time::timeout(Duration::from_millis(50), output_rx.recv())
            .await
            .is_err(),
        "produced block should stay behind the canonization fence"
    );

    canonized_tx.send(replay_record.clone()).await.unwrap();

    let forwarded = tokio::time::timeout(Duration::from_secs(1), output_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(forwarded.0.header.number, 7);
    assert_eq!(forwarded.1, replay_record);
    assert!(matches!(forwarded.2, BlockCommandType::Produce));

    handle.abort();
}

#[tokio::test]
async fn block_canonizer_routes_foreign_canonized_blocks_back_for_replay() {
    let component = BlockCanonizer {
        consensus: NoopCanonization::new(),
        canonized_blocks_for_execution: {
            let (tx, _rx) = mpsc::channel(1);
            tx
        },
    };
    let sender = component.consensus.sender.clone();
    let (replay_tx, mut replay_rx) = mpsc::channel(4);
    let component = BlockCanonizer {
        consensus: component.consensus,
        canonized_blocks_for_execution: replay_tx,
    };

    let (_input_tx, input_rx) = mpsc::channel(4);
    let (output_tx, mut output_rx) = mpsc::channel(4);
    let handle = tokio::spawn(async move {
        component
            .run(PeekableReceiver::new(input_rx), output_tx)
            .await
    });

    let replay_record = make_replay_record(9, 1009);
    sender.send(replay_record.clone()).unwrap();

    let requeued = tokio::time::timeout(Duration::from_secs(1), replay_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(requeued, replay_record);
    assert!(
        tokio::time::timeout(Duration::from_millis(50), output_rx.recv())
            .await
            .is_err(),
        "foreign canonized blocks must go back to the command source, not downstream"
    );

    handle.abort();
}

#[tokio::test]
async fn block_applier_keeps_main_node_replays_non_overriding() {
    let replay = MockReplayStorage::new().with_genesis();
    let state = MockWriteState::new();
    let repo = MockRepository::new();
    let component = BlockApplier {
        state: state.clone(),
        replay: replay.clone(),
        repositories: repo.clone(),
        config: test_config(NodeRole::MainNode),
    };

    let (input_tx, input_rx) = mpsc::channel(4);
    let (output_tx, mut output_rx) = mpsc::channel(4);
    let handle = tokio::spawn(async move {
        component
            .run(PeekableReceiver::new(input_rx), output_tx)
            .await
    });

    input_tx
        .send((
            make_block_output(1, 1001),
            make_replay_record(1, 1001),
            BlockCommandType::Replay,
        ))
        .await
        .unwrap();

    tokio::time::timeout(Duration::from_secs(1), output_rx.recv())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(replay.write_log(), vec![(1, false)]);
    assert_eq!(state.write_log(), vec![(1, false)]);
    assert_eq!(repo.populated_blocks(), vec![1]);

    handle.abort();
}

/// When the canonized record doesn't match the locally produced record, BlockCanonizer
/// must bail immediately. This guards against split-brain scenarios where another node
/// has become leader and is producing different blocks.
#[tokio::test]
async fn block_canonizer_bails_on_canonization_mismatch() {
    let (proposal_tx, mut proposal_rx) = mpsc::unbounded_channel();
    let (canonized_tx, canonized_rx) = mpsc::channel(4);
    let (replay_tx, _replay_rx) = mpsc::channel(4);
    let component = BlockCanonizer {
        consensus: MockConsensus {
            proposals: proposal_tx,
            canonized: canonized_rx,
        },
        canonized_blocks_for_execution: replay_tx,
    };

    let (input_tx, input_rx) = mpsc::channel(4);
    let (output_tx, mut output_rx) = mpsc::channel(4);
    let handle = tokio::spawn(async move {
        component
            .run(PeekableReceiver::new(input_rx), output_tx)
            .await
    });

    let block_output = make_block_output(5, 1005);
    let produced_record = make_replay_record(5, 1005);

    // Submit a Produce block
    input_tx
        .send((block_output, produced_record.clone(), BlockCommandType::Produce))
        .await
        .unwrap();

    // Wait for it to be proposed
    let _ = tokio::time::timeout(Duration::from_secs(1), proposal_rx.recv())
        .await
        .unwrap()
        .unwrap();

    // Return a DIFFERENT record (different block_output_hash simulates a competing leader)
    let mut mismatched_record = produced_record.clone();
    mismatched_record.block_output_hash = alloy::primitives::B256::from([0xFF; 32]);
    canonized_tx.send(mismatched_record).await.unwrap();

    // BlockCanonizer must bail with an error
    let result = tokio::time::timeout(Duration::from_secs(1), handle).await;
    assert!(
        result.is_ok(),
        "BlockCanonizer should have terminated within timeout"
    );
    let join_result = result.unwrap();
    assert!(
        join_result.is_ok(),
        "JoinHandle should complete without panic"
    );
    let inner = join_result.unwrap();
    assert!(
        inner.is_err(),
        "BlockCanonizer must return Err on canonization mismatch, got Ok"
    );

    // Downstream must not receive any block output: either the channel closes
    // immediately (canonizer bailed and dropped output_tx) or times out.
    // Either way, no `Some(item)` must arrive.
    let downstream_result =
        tokio::time::timeout(Duration::from_millis(50), output_rx.recv()).await;
    assert!(
        !matches!(downstream_result, Ok(Some(_))),
        "Mismatched block must never reach BlockApplier"
    );
}

#[tokio::test]
async fn block_applier_allows_overrides_for_rebuilds_and_external_replays() {
    let cases = [
        (NodeRole::MainNode, BlockCommandType::Rebuild, 2),
        (NodeRole::ExternalNode, BlockCommandType::Replay, 3),
    ];

    for (role, cmd_type, block_number) in cases {
        let replay = MockReplayStorage::new().with_genesis();
        let state = MockWriteState::new();
        let repo = MockRepository::new();
        let component = BlockApplier {
            state: state.clone(),
            replay: replay.clone(),
            repositories: repo.clone(),
            config: test_config(role),
        };

        let (input_tx, input_rx) = mpsc::channel(4);
        let (output_tx, mut output_rx) = mpsc::channel(4);
        let handle = tokio::spawn(async move {
            component
                .run(PeekableReceiver::new(input_rx), output_tx)
                .await
        });

        input_tx
            .send((
                make_block_output(block_number, 1000 + block_number),
                make_replay_record(block_number, 1000 + block_number),
                cmd_type,
            ))
            .await
            .unwrap();

        tokio::time::timeout(Duration::from_secs(1), output_rx.recv())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(replay.write_log(), vec![(block_number, true)]);
        assert_eq!(state.write_log(), vec![(block_number, true)]);
        assert_eq!(repo.populated_blocks(), vec![block_number]);

        handle.abort();
    }
}
