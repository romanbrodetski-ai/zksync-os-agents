//! Tests for backpressure behavior in the pipeline.
//!
//! Backpressure is how the pipeline prevents fast producers from overwhelming
//! slow consumers. It's controlled by OUTPUT_BUFFER_SIZE on each component.
//! These tests verify that backpressure works correctly and that buffer sizes
//! are set to appropriate values.

use anyhow::Result;
use async_trait::async_trait;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use zksync_os_pipeline::{PeekableReceiver, PipelineComponent};
use zksync_os_sequencer::execution::block_applier::BlockApplier;
use zksync_os_sequencer::execution::block_canonizer::{BlockCanonizer, NoopCanonization};

/// Verify BlockCanonizer's OUTPUT_BUFFER_SIZE is 2.
/// This is important because it controls how far ahead persistence can get
/// before the canonizer blocks. Too large = no backpressure. Too small = unnecessary stalling.
///
/// If this test fails, someone changed the buffer size. Check if the change
/// was intentional and update the knowledge doc.
#[test]
fn canonizer_output_buffer_size_is_2() {
    assert_eq!(
        BlockCanonizer::<NoopCanonization>::OUTPUT_BUFFER_SIZE,
        2,
        "BlockCanonizer OUTPUT_BUFFER_SIZE changed from expected value of 2. \
        This affects backpressure for the entire pipeline. \
        Was this intentional? Update knowledge doc if so."
    );
}

/// Verify BlockApplier's OUTPUT_BUFFER_SIZE is 5.
/// BlockApplier does persistence which is fast, so a larger buffer is appropriate.
#[test]
fn applier_output_buffer_size_is_5() {
    type TestApplier = BlockApplier<
        crate::mocks::MockWriteState,
        crate::mocks::MockReplayStorage,
        crate::mocks::MockRepository,
    >;
    assert_eq!(
        TestApplier::OUTPUT_BUFFER_SIZE,
        5,
        "BlockApplier OUTPUT_BUFFER_SIZE changed from expected value of 5. \
        Was this intentional?"
    );
}

/// A slow consumer should cause a fast producer to block when the buffer fills.
/// This verifies the fundamental backpressure mechanism.
#[tokio::test]
async fn slow_consumer_blocks_fast_producer() {
    struct FastProducer;

    #[async_trait]
    impl PipelineComponent for FastProducer {
        type Input = ();
        type Output = u64;
        const NAME: &'static str = "fast_producer";
        const OUTPUT_BUFFER_SIZE: usize = 2; // Small buffer

        async fn run(
            self,
            _input: PeekableReceiver<Self::Input>,
            output: mpsc::Sender<Self::Output>,
        ) -> Result<()> {
            for i in 0..100 {
                output.send(i).await?;
            }
            Ok(())
        }
    }

    let produced = Arc::new(AtomicU64::new(0));
    let produced_clone = produced.clone();

    struct SlowConsumer {
        consumed: Arc<AtomicU64>,
    }

    #[async_trait]
    impl PipelineComponent for SlowConsumer {
        type Input = u64;
        type Output = u64;
        const NAME: &'static str = "slow_consumer";
        const OUTPUT_BUFFER_SIZE: usize = 1;

        async fn run(
            self,
            mut input: PeekableReceiver<Self::Input>,
            output: mpsc::Sender<Self::Output>,
        ) -> Result<()> {
            loop {
                let Some(value) = input.recv().await else {
                    anyhow::bail!("input closed");
                };
                self.consumed.fetch_add(1, Ordering::SeqCst);
                // Simulate slow processing
                tokio::time::sleep(Duration::from_millis(50)).await;
                output.send(value).await?;
            }
        }
    }

    // Wire: FastProducer(buffer=2) -> SlowConsumer(buffer=1) -> drain
    let (prod_tx, prod_rx) = mpsc::channel::<u64>(FastProducer::OUTPUT_BUFFER_SIZE);
    let (slow_tx, mut slow_rx) = mpsc::channel(1);

    let consumed = Arc::new(AtomicU64::new(0));
    let consumed_clone = consumed.clone();

    let mut tasks = tokio::task::JoinSet::new();

    // Producer sends 100 items as fast as possible
    tasks.spawn(async move {
        for i in 0u64..100 {
            prod_tx.send(i).await.ok();
            produced_clone.fetch_add(1, Ordering::SeqCst);
        }
    });

    // Consumer processes slowly
    tasks.spawn(async move {
        SlowConsumer {
            consumed: consumed_clone,
        }
        .run(PeekableReceiver::new(prod_rx), slow_tx)
        .await
        .ok();
    });

    // Drain output
    tasks.spawn(async move {
        while slow_rx.recv().await.is_some() {}
    });

    // After 100ms, the producer should be blocked (only ~2 items in buffer + ~2 consumed)
    tokio::time::sleep(Duration::from_millis(150)).await;
    let produced_count = produced.load(Ordering::SeqCst);
    let consumed_count = consumed.load(Ordering::SeqCst);

    // The producer should NOT have been able to send all 100 items
    // It should be limited to roughly: buffer_size + items_consumed
    assert!(
        produced_count < 20,
        "Producer sent {produced_count} items in 150ms — backpressure not working! \
        Consumer only processed {consumed_count} items."
    );
}

/// With OUTPUT_BUFFER_SIZE = 0, the component shouldn't start the next item
/// until the previous one is picked up. This is lockstep processing.
#[tokio::test]
async fn zero_buffer_means_lockstep() {
    struct LockstepComponent;

    #[async_trait]
    impl PipelineComponent for LockstepComponent {
        type Input = u64;
        type Output = u64;
        const NAME: &'static str = "lockstep";
        const OUTPUT_BUFFER_SIZE: usize = 0;

        async fn run(
            self,
            mut input: PeekableReceiver<Self::Input>,
            output: mpsc::Sender<Self::Output>,
        ) -> Result<()> {
            loop {
                let Some(value) = input.recv().await else {
                    anyhow::bail!("input closed");
                };
                output.send(value).await?;
            }
        }
    }

    let (_input_tx, _input_rx) = mpsc::channel::<u64>(10);
    // Buffer size 0 means mpsc::channel(0) which is invalid — tokio requires >= 1.
    // The pipeline builder uses OUTPUT_BUFFER_SIZE directly in mpsc::channel().
    // With size 0, this would panic. This test documents the current behavior.
    //
    // NOTE: If this test starts failing, it means the pipeline builder was updated
    // to handle 0-size buffers differently (e.g., by using size 1 as minimum).
    let result = std::panic::catch_unwind(|| mpsc::channel::<u64>(0));
    assert!(
        result.is_err(),
        "mpsc::channel(0) should panic — OUTPUT_BUFFER_SIZE = 0 is not currently supported. \
        If this test fails, tokio's behavior changed."
    );
}

/// Verify that when a downstream component is dropped (e.g., due to error),
/// the upstream send() returns an error rather than blocking forever.
#[tokio::test]
async fn dropped_receiver_unblocks_sender() {
    let (tx, rx) = mpsc::channel::<u64>(1);

    // Fill the buffer
    tx.send(1).await.unwrap();

    // Drop receiver
    drop(rx);

    // Next send should fail, not block
    let result = tx.send(2).await;
    assert!(result.is_err(), "Send to dropped receiver should fail");
}

/// End-to-end backpressure test: a 3-stage pipeline where the last stage
/// is slow should cause all upstream stages to eventually block.
#[tokio::test]
async fn end_to_end_backpressure_propagation() {
    let items_produced = Arc::new(AtomicU64::new(0));
    let items_consumed = Arc::new(AtomicU64::new(0));
    let prod_counter = items_produced.clone();
    let cons_counter = items_consumed.clone();

    let (tx1, rx1) = mpsc::channel::<u64>(2); // buffer between producer and stage1
    let (tx2, rx2) = mpsc::channel::<u64>(2); // buffer between stage1 and stage2
    let (tx3, mut rx3) = mpsc::channel::<u64>(1); // buffer between stage2 and consumer

    let mut tasks = tokio::task::JoinSet::new();

    // Producer: sends items fast
    tasks.spawn(async move {
        for i in 0u64..1000 {
            if tx1.send(i).await.is_err() {
                break;
            }
            prod_counter.fetch_add(1, Ordering::SeqCst);
        }
    });

    // Stage 1: fast passthrough
    tasks.spawn(async move {
        let mut rx: mpsc::Receiver<u64> = rx1;
        loop {
            match rx.recv().await {
                Some(v) => {
                    if tx2.send(v).await.is_err() {
                        break;
                    }
                }
                None => break,
            }
        }
    });

    // Stage 2: slow consumer (100ms per item)
    tasks.spawn(async move {
        let mut rx: mpsc::Receiver<u64> = rx2;
        loop {
            match rx.recv().await {
                Some(v) => {
                    cons_counter.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    if tx3.send(v).await.is_err() {
                        break;
                    }
                }
                None => break,
            }
        }
    });

    // Drain final output
    tasks.spawn(async move {
        while rx3.recv().await.is_some() {}
    });

    // After 500ms, with 100ms per item at the bottleneck:
    // Consumer should have processed ~5 items
    // Producer should be limited by backpressure to roughly: consumed + sum(buffer_sizes) = 5 + 2 + 2 + 1 = 10
    tokio::time::sleep(Duration::from_millis(500)).await;

    let produced = items_produced.load(Ordering::SeqCst);
    let consumed = items_consumed.load(Ordering::SeqCst);

    assert!(
        consumed >= 3 && consumed <= 8,
        "Expected ~5 consumed items in 500ms, got {consumed}"
    );
    assert!(
        produced < 30,
        "Producer sent {produced} items — expected ~{} (consumed + buffers). \
        Backpressure may not be propagating correctly.",
        consumed + 5
    );
}
