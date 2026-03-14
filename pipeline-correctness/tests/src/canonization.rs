//! Tests for BlockCanonizer behavior — the consensus fence in the pipeline.
//!
//! BlockCanonizer is the most correctness-critical component in the pipeline.
//! It ensures that only blocks agreed upon by consensus proceed to persistence.
//! These tests exercise the component directly using mock consensus implementations.

use crate::mocks::{make_block_output, make_replay_record};
use async_trait::async_trait;
use tokio::sync::mpsc;
use zksync_os_pipeline::{PeekableReceiver, PipelineComponent};
use zksync_os_sequencer::execution::block_canonizer::{
    BlockCanonization, BlockCanonizer, NoopCanonization,
};
use zksync_os_sequencer::model::blocks::BlockCommandType;
use zksync_os_storage_api::ReplayRecord;

/// Consensus implementation where test code controls what gets canonized.
struct TestConsensus {
    propose_tx: mpsc::Sender<ReplayRecord>,
    canon_rx: mpsc::Receiver<ReplayRecord>,
}

/// Test-side handle to control the consensus.
struct ConsensusControl {
    /// Receives proposed blocks from the canonizer.
    propose_rx: mpsc::Receiver<ReplayRecord>,
    /// Send canonized blocks to the canonizer.
    canon_tx: mpsc::Sender<ReplayRecord>,
}

fn make_test_consensus() -> (TestConsensus, ConsensusControl) {
    let (propose_tx, propose_rx) = mpsc::channel(10);
    let (canon_tx, canon_rx) = mpsc::channel(10);
    (
        TestConsensus {
            propose_tx,
            canon_rx,
        },
        ConsensusControl {
            propose_rx,
            canon_tx,
        },
    )
}

#[async_trait]
impl BlockCanonization for TestConsensus {
    async fn propose(&self, record: ReplayRecord) -> anyhow::Result<()> {
        self.propose_tx.send(record).await?;
        Ok(())
    }

    async fn next_canonized(&mut self) -> anyhow::Result<ReplayRecord> {
        self.canon_rx
            .recv()
            .await
            .ok_or_else(|| anyhow::anyhow!("canon channel closed"))
    }
}

/// Helper to run BlockCanonizer in a task and return handles to its input/output channels.
struct CanonizerHarness {
    /// Send executed blocks into the canonizer (simulating BlockExecutor output).
    input_tx: mpsc::Sender<(
        zksync_os_interface::types::BlockOutput,
        ReplayRecord,
        BlockCommandType,
    )>,
    /// Receive canonized blocks from the canonizer (what would go to BlockApplier).
    output_rx: mpsc::Receiver<(
        zksync_os_interface::types::BlockOutput,
        ReplayRecord,
        BlockCommandType,
    )>,
    /// Receive blocks that the canonizer routes back for re-execution.
    reexecution_rx: mpsc::Receiver<ReplayRecord>,
    /// Handle to the task running the canonizer.
    task: tokio::task::JoinHandle<anyhow::Result<()>>,
}

fn spawn_canonizer(consensus: impl BlockCanonization) -> CanonizerHarness {
    let (reexec_tx, reexec_rx) = mpsc::channel(10);

    let canonizer = BlockCanonizer {
        consensus,
        canonized_blocks_for_execution: reexec_tx,
    };

    // Create the input channel (simulating BlockExecutor -> Canonizer)
    let (input_tx, input_rx) = mpsc::channel(10);
    // Create the output channel (simulating Canonizer -> BlockApplier)
    let (output_tx, output_rx) =
        mpsc::channel(BlockCanonizer::<NoopCanonization>::OUTPUT_BUFFER_SIZE);

    let task = tokio::spawn(async move {
        canonizer
            .run(PeekableReceiver::new(input_rx), output_tx)
            .await
    });

    CanonizerHarness {
        input_tx,
        output_rx,
        reexecution_rx: reexec_rx,
        task,
    }
}

// ─── NoopCanonization Tests ───────────────────────────────────────────────────

/// With NoopCanonization, a Produce block should pass through the canonizer
/// and appear on the output. This is the single-node happy path.
#[tokio::test]
async fn noop_produce_passes_through() {
    let consensus = NoopCanonization::new();
    let mut harness = spawn_canonizer(consensus);

    let record = make_replay_record(1, 1001);
    let output = make_block_output(1, 1001);

    harness
        .input_tx
        .send((output.clone(), record.clone(), BlockCommandType::Produce))
        .await
        .unwrap();

    let (_out_output, out_record, out_cmd) = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        harness.output_rx.recv(),
    )
    .await
    .expect("timed out waiting for canonizer output")
    .expect("output channel closed");

    assert_eq!(out_record.block_context.block_number, 1);
    assert!(matches!(out_cmd, BlockCommandType::Produce));
}

/// With NoopCanonization, Replay blocks should bypass canonization entirely
/// and pass through immediately.
#[tokio::test]
async fn noop_replay_bypasses_canonization() {
    let consensus = NoopCanonization::new();
    let mut harness = spawn_canonizer(consensus);

    let record = make_replay_record(1, 1001);
    let output = make_block_output(1, 1001);

    harness
        .input_tx
        .send((output.clone(), record.clone(), BlockCommandType::Replay))
        .await
        .unwrap();

    let (_, out_record, out_cmd) = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        harness.output_rx.recv(),
    )
    .await
    .expect("timed out")
    .expect("closed");

    assert_eq!(out_record.block_context.block_number, 1);
    assert!(matches!(out_cmd, BlockCommandType::Replay));
}

/// Multiple sequential blocks should all pass through in order.
#[tokio::test]
async fn noop_multiple_blocks_in_order() {
    let consensus = NoopCanonization::new();
    let mut harness = spawn_canonizer(consensus);

    for i in 1..=5 {
        let record = make_replay_record(i, 1000 + i);
        let output = make_block_output(i, 1000 + i);

        harness
            .input_tx
            .send((output, record, BlockCommandType::Produce))
            .await
            .unwrap();

        let (_, out_record, _) = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            harness.output_rx.recv(),
        )
        .await
        .expect("timed out")
        .expect("closed");

        assert_eq!(out_record.block_context.block_number, i);
    }
}

/// Rebuild commands should go through canonization just like Produce.
#[tokio::test]
async fn noop_rebuild_goes_through_canonization() {
    let consensus = NoopCanonization::new();
    let mut harness = spawn_canonizer(consensus);

    let record = make_replay_record(1, 1001);
    let output = make_block_output(1, 1001);

    harness
        .input_tx
        .send((output, record, BlockCommandType::Rebuild))
        .await
        .unwrap();

    let (_, _, out_cmd) = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        harness.output_rx.recv(),
    )
    .await
    .expect("timed out")
    .expect("closed");

    assert!(matches!(out_cmd, BlockCommandType::Rebuild));
}

// ─── TestConsensus (multi-node simulation) Tests ──────────────────────────────

/// When the same node proposes and gets canonized, the block should pass through.
/// This simulates a stable leader scenario.
#[tokio::test]
async fn leader_produce_matches_canonized() {
    let (consensus, mut control) = make_test_consensus();
    let mut harness = spawn_canonizer(consensus);

    let record = make_replay_record(1, 1001);
    let output = make_block_output(1, 1001);

    // Node produces a block
    harness
        .input_tx
        .send((output.clone(), record.clone(), BlockCommandType::Produce))
        .await
        .unwrap();

    // Consensus receives the proposal
    let proposed = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        control.propose_rx.recv(),
    )
    .await
    .expect("timed out")
    .expect("closed");

    assert_eq!(proposed.block_context.block_number, 1);

    // Consensus returns the same block as canonized
    control.canon_tx.send(proposed).await.unwrap();

    // Canonizer should emit the block downstream
    let (_, out_record, _) = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        harness.output_rx.recv(),
    )
    .await
    .expect("timed out")
    .expect("closed");

    assert_eq!(out_record.block_context.block_number, 1);
}

/// When consensus returns a block but the produced_queue is empty,
/// the block should be routed to the reexecution channel (for replay
/// by the command source). This simulates receiving a block from another leader.
#[tokio::test]
async fn foreign_block_routed_to_reexecution() {
    let (_consensus, mut control) = make_test_consensus();
    let (consensus2, _control2) = make_test_consensus();

    // Use a fresh consensus that we can control both sides of
    let (propose_tx, _propose_rx) = mpsc::channel(10);
    let (canon_tx, canon_rx) = mpsc::channel(10);
    let consensus = TestConsensus {
        propose_tx,
        canon_rx,
    };

    let mut harness = spawn_canonizer(consensus);

    let foreign_record = make_replay_record(1, 1001);

    // Another node's block arrives via consensus (no local produce)
    canon_tx.send(foreign_record.clone()).await.unwrap();

    // Should appear on the reexecution channel, NOT the output
    let reexec_record = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        harness.reexecution_rx.recv(),
    )
    .await
    .expect("timed out")
    .expect("closed");

    assert_eq!(reexec_record.block_context.block_number, 1);
}

/// When the canonized block doesn't match the produced block,
/// the canonizer should error (bail). This simulates a leadership change
/// where another node became leader and produced a different block.
#[tokio::test]
async fn mismatched_canonized_block_causes_error() {
    let (consensus, mut control) = make_test_consensus();
    let mut harness = spawn_canonizer(consensus);

    // Node produces block 1 with one hash
    let record = make_replay_record(1, 1001);
    let output = make_block_output(1, 1001);

    harness
        .input_tx
        .send((output, record, BlockCommandType::Produce))
        .await
        .unwrap();

    // Wait for proposal
    let _proposed = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        control.propose_rx.recv(),
    )
    .await
    .expect("timed out")
    .expect("closed");

    // Return a DIFFERENT block as canonized (different output hash = different block)
    let mut different_record = make_replay_record(1, 1001);
    different_record.block_output_hash = alloy::primitives::B256::from([0xFF; 32]);

    control.canon_tx.send(different_record).await.unwrap();

    // The canonizer task should error
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), harness.task).await;
    match result {
        Ok(Ok(Err(e))) => {
            let msg = e.to_string();
            assert!(
                msg.contains("mismatch"),
                "Expected mismatch error, got: {msg}"
            );
        }
        other => panic!("Expected error from canonizer, got: {other:?}"),
    }
}

/// Replay blocks should pass through even when using TestConsensus,
/// without being proposed to consensus.
#[tokio::test]
async fn replay_bypasses_consensus_even_with_real_consensus() {
    let (consensus, mut control) = make_test_consensus();
    let mut harness = spawn_canonizer(consensus);

    let record = make_replay_record(1, 1001);
    let output = make_block_output(1, 1001);

    harness
        .input_tx
        .send((output, record, BlockCommandType::Replay))
        .await
        .unwrap();

    // Should appear on output immediately
    let (_, out_record, _) = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        harness.output_rx.recv(),
    )
    .await
    .expect("timed out")
    .expect("closed");

    assert_eq!(out_record.block_context.block_number, 1);

    // Nothing should have been proposed
    assert!(control.propose_rx.try_recv().is_err());
}

/// Test that the canonizer's backpressure works: when the produced_queue is full
/// (MAX_PRODUCED_QUEUE_SIZE = 2), the canonizer should stop accepting new input
/// until consensus confirms a block.
#[tokio::test]
async fn backpressure_when_produced_queue_full() {
    let (consensus, mut control) = make_test_consensus();
    let mut harness = spawn_canonizer(consensus);

    // Send 2 produce blocks (fills the queue to MAX_PRODUCED_QUEUE_SIZE)
    for i in 1..=2 {
        let record = make_replay_record(i, 1000 + i);
        let output = make_block_output(i, 1000 + i);

        harness
            .input_tx
            .send((output, record, BlockCommandType::Produce))
            .await
            .unwrap();

        // Drain proposals
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            control.propose_rx.recv(),
        )
        .await;
    }

    // Try to send a 3rd block — this should block because the queue is full
    let record = make_replay_record(3, 1003);
    let output = make_block_output(3, 1003);

    let _send_result = tokio::time::timeout(
        std::time::Duration::from_millis(200),
        harness
            .input_tx
            .send((output, record, BlockCommandType::Produce)),
    )
    .await;

    // The send itself succeeds (goes into the input channel buffer),
    // but the canonizer won't read it until the queue drains.
    // Let's verify by checking that no new proposal arrives:
    let propose_result = tokio::time::timeout(
        std::time::Duration::from_millis(200),
        control.propose_rx.recv(),
    )
    .await;

    // Should timeout — canonizer won't read more input until queue drains
    assert!(
        propose_result.is_err(),
        "Expected timeout (backpressure), but got a proposal"
    );

    // Now canonize block 1 to free a slot
    let canonized = make_replay_record(1, 1001);
    control.canon_tx.send(canonized).await.unwrap();

    // Drain the output (block 1)
    let _ = harness.output_rx.recv().await;

    // Now the canonizer should process the queued block 3
    let propose_result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        control.propose_rx.recv(),
    )
    .await;

    assert!(
        propose_result.is_ok(),
        "Expected block 3 to be proposed after queue drained"
    );
}

/// Interleaved Replay and Produce commands should work correctly:
/// Replays pass through immediately, Produces go through consensus.
#[tokio::test]
async fn interleaved_replay_and_produce() {
    let (consensus, mut control) = make_test_consensus();
    let mut harness = spawn_canonizer(consensus);

    // Replay block 1
    let record1 = make_replay_record(1, 1001);
    let output1 = make_block_output(1, 1001);
    harness
        .input_tx
        .send((output1, record1, BlockCommandType::Replay))
        .await
        .unwrap();

    // Should pass through immediately
    let (_, out, _) = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        harness.output_rx.recv(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out.block_context.block_number, 1);

    // Produce block 2
    let record2 = make_replay_record(2, 1002);
    let output2 = make_block_output(2, 1002);
    harness
        .input_tx
        .send((output2, record2, BlockCommandType::Produce))
        .await
        .unwrap();

    // Wait for proposal, then canonize
    let proposed = control.propose_rx.recv().await.unwrap();
    control.canon_tx.send(proposed).await.unwrap();

    let (_, out, _) = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        harness.output_rx.recv(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out.block_context.block_number, 2);
}
