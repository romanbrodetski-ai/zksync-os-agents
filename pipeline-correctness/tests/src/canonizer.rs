//! Tests for BlockCanonizer — the canonization fence pipeline component.
//!
//! BlockCanonizer invariants:
//! - Replay blocks pass through directly without queuing
//! - Produce/Rebuild blocks are sent to consensus and queued until canonized
//! - Canonized record must match the queued record (equality check, excluding node_version)
//! - When produced_queue is at MAX_PRODUCED_QUEUE_SIZE (2), upstream is blocked
//! - NoopCanonization immediately returns whatever was proposed

use crate::mocks::{make_block_output, make_replay_record};
use tokio::sync::mpsc;
use zksync_os_sequencer::execution::{BlockCanonization, BlockCanonizer, NoopCanonization};
use zksync_os_sequencer::model::blocks::BlockCommandType;
use zksync_os_pipeline::PipelineComponent;
use zksync_os_storage_api::ReplayRecord;
use zksync_os_interface::types::BlockOutput;

/// Helper to run BlockCanonizer in a tokio task and collect `expected` outputs.
/// Keeps the input channel open until after all outputs are collected, because
/// closing the input while Produce blocks are queued for canonization causes the
/// canonizer to bail ("inbound channel closed").
async fn run_canonizer_collect(
    inputs: Vec<(BlockOutput, ReplayRecord, BlockCommandType)>,
    expected: usize,
) -> Vec<(BlockOutput, ReplayRecord, BlockCommandType)> {
    use zksync_os_pipeline::PeekableReceiver;

    let (input_tx, input_rx) = mpsc::channel(32);
    let (output_tx, mut output_rx) = mpsc::channel(32);
    let (replay_tx, _replay_rx) = mpsc::channel(32);

    let canonizer = BlockCanonizer {
        consensus: NoopCanonization::new(),
        canonized_blocks_for_execution: replay_tx,
    };

    let handle = tokio::spawn(async move {
        let _ = canonizer
            .run(PeekableReceiver::new(input_rx), output_tx)
            .await;
    });

    for item in inputs {
        input_tx.send(item).await.unwrap();
    }

    let mut results = vec![];
    for _ in 0..expected {
        let item = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            output_rx.recv(),
        )
        .await
        .expect("timed out waiting for output")
        .expect("output channel closed");
        results.push(item);
    }

    drop(input_tx);
    let _ = handle.await;
    results
}

/// Replay blocks pass directly through without waiting for consensus.
#[tokio::test]
async fn replay_blocks_pass_through_immediately() {
    let record = make_replay_record(1, 1000);
    let output = make_block_output(1, 1000);
    let cmd_type = BlockCommandType::Replay;

    let results = run_canonizer_collect(vec![(output, record.clone(), cmd_type)], 1).await;

    assert_eq!(results.len(), 1);
    assert!(matches!(results[0].2, BlockCommandType::Replay));
    assert_eq!(results[0].1.block_context.block_number, 1);
}

/// NoopCanonization immediately canonizes: Produce blocks are proposed and return,
/// then sent downstream.
#[tokio::test]
async fn produce_blocks_pass_through_via_noop_consensus() {
    let record = make_replay_record(5, 5000);
    let output = make_block_output(5, 5000);
    let cmd_type = BlockCommandType::Produce;

    let results = run_canonizer_collect(vec![(output, record.clone(), cmd_type)], 1).await;

    assert_eq!(results.len(), 1);
    assert!(matches!(results[0].2, BlockCommandType::Produce));
    assert_eq!(results[0].1.block_context.block_number, 5);
}

/// Mixed Replay and Produce blocks are all emitted downstream.
#[tokio::test]
async fn mixed_replay_and_produce_all_pass_through() {
    let inputs = vec![
        (make_block_output(1, 1000), make_replay_record(1, 1000), BlockCommandType::Replay),
        (make_block_output(2, 2000), make_replay_record(2, 2000), BlockCommandType::Replay),
        (make_block_output(3, 3000), make_replay_record(3, 3000), BlockCommandType::Produce),
    ];

    let results = run_canonizer_collect(inputs, 3).await;

    assert_eq!(results.len(), 3);
    assert!(matches!(results[0].2, BlockCommandType::Replay));
    assert!(matches!(results[1].2, BlockCommandType::Replay));
    assert!(matches!(results[2].2, BlockCommandType::Produce));
}

/// NoopCanonization: propose + next_canonized is a round-trip through a channel.
#[tokio::test]
async fn noop_canonization_roundtrip() {
    let mut noop = NoopCanonization::new();
    let record = make_replay_record(42, 42000);

    noop.propose(record.clone()).await.unwrap();
    let canonized = noop.next_canonized().await.unwrap();

    assert_eq!(canonized.block_context.block_number, 42);
    assert_eq!(canonized.block_context.timestamp, 42000);
}
