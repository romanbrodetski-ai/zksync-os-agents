//! Tests for BlockCanonizer — the consensus fence in the pipeline.
//!
//! BlockCanonizer ensures only canonized blocks proceed to BlockApplier.
//! - Replay blocks pass through immediately (already canonized).
//! - Produce/Rebuild blocks are sent to consensus and queued until canonized.
//!
//! Uses NoopCanonization which echoes proposals back immediately.

use crate::mocks::{make_block_output, make_replay_record};
use tokio::sync::mpsc;
use zksync_os_pipeline::{PeekableReceiver, PipelineComponent};
use zksync_os_sequencer::execution::block_canonizer::{BlockCanonizer, NoopCanonization};
use zksync_os_sequencer::model::blocks::BlockCommandType;

/// Replay blocks pass through BlockCanonizer immediately without queuing.
/// They are not sent to consensus.
#[tokio::test]
async fn replay_blocks_pass_through_immediately() {
    let (input_tx, input_rx) = mpsc::channel(4);
    let (output_tx, mut output_rx) = mpsc::channel(4);
    let (_replays_tx, _replays_rx) = mpsc::channel(4);

    let canonizer = BlockCanonizer {
        consensus: NoopCanonization::new(),
        canonized_blocks_for_execution: _replays_tx,
    };

    tokio::spawn(async move {
        canonizer
            .run(PeekableReceiver::new(input_rx), output_tx)
            .await
            .ok();
    });

    // Send 3 Replay blocks
    for i in 1u64..=3 {
        let block_output = make_block_output(i, 1000 + i);
        let replay_record = make_replay_record(i, 1000 + i);
        input_tx
            .send((block_output, replay_record, BlockCommandType::Replay))
            .await
            .unwrap();
    }

    // All 3 should come out in order
    for i in 1u64..=3 {
        let (output, record, cmd_type) = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            output_rx.recv(),
        )
        .await
        .expect("timeout waiting for replay block")
        .expect("channel closed");

        assert_eq!(output.header.number, i);
        assert_eq!(record.block_context.block_number, i);
        assert!(matches!(cmd_type, BlockCommandType::Replay));
    }
}

/// Produce blocks are sent to consensus and come back after canonization.
/// With NoopCanonization, they are canonized immediately.
#[tokio::test]
async fn produce_blocks_wait_for_canonization() {
    let (input_tx, input_rx) = mpsc::channel(4);
    let (output_tx, mut output_rx) = mpsc::channel(4);
    let (_replays_tx, _replays_rx) = mpsc::channel(4);

    let canonizer = BlockCanonizer {
        consensus: NoopCanonization::new(),
        canonized_blocks_for_execution: _replays_tx,
    };

    tokio::spawn(async move {
        canonizer
            .run(PeekableReceiver::new(input_rx), output_tx)
            .await
            .ok();
    });

    // Send a Produce block
    let block_output = make_block_output(1, 1000);
    let replay_record = make_replay_record(1, 1000);
    input_tx
        .send((block_output, replay_record, BlockCommandType::Produce))
        .await
        .unwrap();

    // Block should come out as Produce (cmd_type preserved)
    let (output, record, cmd_type) = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        output_rx.recv(),
    )
    .await
    .expect("timeout waiting for produce block")
    .expect("channel closed");

    assert_eq!(output.header.number, 1);
    assert_eq!(record.block_context.block_number, 1);
    assert!(
        matches!(cmd_type, BlockCommandType::Produce),
        "Command type should be preserved through canonizer"
    );
}

/// Replay and Produce blocks can be interleaved.
/// Replay passes through, Produce waits for canonization.
/// With NoopCanonization, order is: Replay(1), Produce(2) both come out in order.
#[tokio::test]
async fn replay_and_produce_interleaved() {
    let (input_tx, input_rx) = mpsc::channel(8);
    let (output_tx, mut output_rx) = mpsc::channel(8);
    let (_replays_tx, _replays_rx) = mpsc::channel(4);

    let canonizer = BlockCanonizer {
        consensus: NoopCanonization::new(),
        canonized_blocks_for_execution: _replays_tx,
    };

    tokio::spawn(async move {
        canonizer
            .run(PeekableReceiver::new(input_rx), output_tx)
            .await
            .ok();
    });

    // Send Replay(1) then Produce(2)
    input_tx
        .send((
            make_block_output(1, 1000),
            make_replay_record(1, 1000),
            BlockCommandType::Replay,
        ))
        .await
        .unwrap();

    input_tx
        .send((
            make_block_output(2, 1001),
            make_replay_record(2, 1001),
            BlockCommandType::Produce,
        ))
        .await
        .unwrap();

    // Both should arrive; order preserved
    let mut received = vec![];
    for _ in 0..2 {
        let (output, _record, _cmd_type) = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            output_rx.recv(),
        )
        .await
        .expect("timeout")
        .expect("channel closed");
        received.push(output.header.number);
    }

    assert_eq!(received, vec![1, 2]);
}

/// Rebuild blocks are handled like Produce (sent to consensus, queued).
#[tokio::test]
async fn rebuild_blocks_wait_for_canonization() {
    let (input_tx, input_rx) = mpsc::channel(4);
    let (output_tx, mut output_rx) = mpsc::channel(4);
    let (_replays_tx, _replays_rx) = mpsc::channel(4);

    let canonizer = BlockCanonizer {
        consensus: NoopCanonization::new(),
        canonized_blocks_for_execution: _replays_tx,
    };

    tokio::spawn(async move {
        canonizer
            .run(PeekableReceiver::new(input_rx), output_tx)
            .await
            .ok();
    });

    input_tx
        .send((
            make_block_output(5, 2000),
            make_replay_record(5, 2000),
            BlockCommandType::Rebuild,
        ))
        .await
        .unwrap();

    let (_output, _record, cmd_type) = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        output_rx.recv(),
    )
    .await
    .expect("timeout")
    .expect("channel closed");

    assert!(
        matches!(cmd_type, BlockCommandType::Rebuild),
        "Rebuild command type should be preserved"
    );
}
