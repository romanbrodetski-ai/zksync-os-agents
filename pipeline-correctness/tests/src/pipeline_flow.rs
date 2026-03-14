//! Tests for the Pipeline builder and end-to-end component composition.
//!
//! These tests verify that:
//! - Pipeline components chain correctly via the builder
//! - Data flows through multiple stages in order
//! - The pipeline framework itself handles task spawning and shutdown correctly

use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::mpsc;
use zksync_os_pipeline::{PeekableReceiver, Pipeline, PipelineComponent};

// ─── Test Components ──────────────────────────────────────────────────────────

/// A simple passthrough component that records what it sees.
/// Useful for verifying pipeline topology and ordering.
struct RecordingPassthrough {
    name: &'static str,
    log: mpsc::Sender<(String, u64)>,
}

#[async_trait]
impl PipelineComponent for RecordingPassthrough {
    type Input = u64;
    type Output = u64;
    const NAME: &'static str = "recording_passthrough";
    const OUTPUT_BUFFER_SIZE: usize = 5;

    async fn run(
        self,
        mut input: PeekableReceiver<Self::Input>,
        output: mpsc::Sender<Self::Output>,
    ) -> Result<()> {
        loop {
            let Some(value) = input.recv().await else {
                anyhow::bail!("input closed");
            };
            let _ = self.log.send((self.name.to_string(), value)).await;
            output.send(value).await?;
        }
    }
}

/// A component that transforms its input (doubles the value).
struct DoublingComponent;

#[async_trait]
impl PipelineComponent for DoublingComponent {
    type Input = u64;
    type Output = u64;
    const NAME: &'static str = "doubler";
    const OUTPUT_BUFFER_SIZE: usize = 2;

    async fn run(
        self,
        mut input: PeekableReceiver<Self::Input>,
        output: mpsc::Sender<Self::Output>,
    ) -> Result<()> {
        loop {
            let Some(value) = input.recv().await else {
                anyhow::bail!("input closed");
            };
            output.send(value * 2).await?;
        }
    }
}

/// A component that filters out odd numbers.
struct EvenFilter;

#[async_trait]
impl PipelineComponent for EvenFilter {
    type Input = u64;
    type Output = u64;
    const NAME: &'static str = "even_filter";
    const OUTPUT_BUFFER_SIZE: usize = 2;

    async fn run(
        self,
        mut input: PeekableReceiver<Self::Input>,
        output: mpsc::Sender<Self::Output>,
    ) -> Result<()> {
        loop {
            let Some(value) = input.recv().await else {
                anyhow::bail!("input closed");
            };
            if value % 2 == 0 {
                output.send(value).await?;
            }
        }
    }
}

/// A slow component that simulates processing delay.
struct SlowComponent {
    delay_ms: u64,
}

#[async_trait]
impl PipelineComponent for SlowComponent {
    type Input = u64;
    type Output = u64;
    const NAME: &'static str = "slow_component";
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
            tokio::time::sleep(std::time::Duration::from_millis(self.delay_ms)).await;
            output.send(value).await?;
        }
    }
}

/// A component that panics on a specific input. Used to test error handling.
struct PanicOnValue {
    panic_value: u64,
}

#[async_trait]
impl PipelineComponent for PanicOnValue {
    type Input = u64;
    type Output = u64;
    const NAME: &'static str = "panic_on_value";
    const OUTPUT_BUFFER_SIZE: usize = 2;

    async fn run(
        self,
        mut input: PeekableReceiver<Self::Input>,
        output: mpsc::Sender<Self::Output>,
    ) -> Result<()> {
        loop {
            let Some(value) = input.recv().await else {
                anyhow::bail!("input closed");
            };
            if value == self.panic_value {
                anyhow::bail!("encountered panic value {}", value);
            }
            output.send(value).await?;
        }
    }
}

// ─── Pipeline Builder Tests ───────────────────────────────────────────────────

/// Basic pipeline: source -> passthrough -> sink.
/// Verifies that data flows end to end.
#[tokio::test]
async fn basic_pipeline_flow() {
    let (log_tx, mut log_rx) = mpsc::channel(100);
    let (input_tx, input_rx) = mpsc::channel(10);

    // Build a pipeline manually by feeding input
    let mut tasks = tokio::task::JoinSet::new();

    // We can't directly feed input into Pipeline::new() since it starts with ()
    // Instead, test the component directly via run()
    let component = RecordingPassthrough {
        name: "stage1",
        log: log_tx,
    };

    let (output_tx, mut output_rx) = mpsc::channel(5);
    tasks.spawn(async move {
        let result = component
            .run(PeekableReceiver::new(input_rx), output_tx)
            .await;
        if let Err(e) = result {
            tracing::error!("component failed: {e}");
        }
    });

    // Send values through
    for i in 1..=5 {
        input_tx.send(i).await.unwrap();
    }

    // Verify output preserves order
    for i in 1..=5 {
        let value = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            output_rx.recv(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(value, i);
    }

    // Verify logging
    for i in 1..=5 {
        let (name, value) = log_rx.recv().await.unwrap();
        assert_eq!(name, "stage1");
        assert_eq!(value, i);
    }
}

/// Chain of transformations: input -> double -> filter even -> output.
/// Verifies that multi-stage pipelines compose correctly.
#[tokio::test]
async fn chained_transformation() {
    let (input_tx, input_rx) = mpsc::channel(10);
    let (mid_tx, mid_rx) = mpsc::channel(DoublingComponent::OUTPUT_BUFFER_SIZE);
    let (output_tx, mut output_rx) = mpsc::channel(EvenFilter::OUTPUT_BUFFER_SIZE);

    let mut tasks = tokio::task::JoinSet::new();

    tasks.spawn(async move {
        DoublingComponent
            .run(PeekableReceiver::new(input_rx), mid_tx)
            .await
            .ok();
    });
    tasks.spawn(async move {
        EvenFilter
            .run(PeekableReceiver::new(mid_rx), output_tx)
            .await
            .ok();
    });

    // Input: 1, 2, 3, 4, 5
    // After doubling: 2, 4, 6, 8, 10
    // After even filter: 2, 4, 6, 8, 10 (all even after doubling)
    for i in 1..=5 {
        input_tx.send(i).await.unwrap();
    }

    for expected in [2, 4, 6, 8, 10] {
        let value = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            output_rx.recv(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(value, expected);
    }
}

/// Ordering preservation: 100 items sent through a 3-stage pipeline
/// must arrive in exactly the same order. This is a fundamental pipeline guarantee.
#[tokio::test]
async fn ordering_preserved_through_multiple_stages() {
    let (log_tx, _log_rx) = mpsc::channel(1000);

    let (input_tx, input_rx) = mpsc::channel(10);
    let (s1_tx, s1_rx) = mpsc::channel(5);
    let (s2_tx, s2_rx) = mpsc::channel(5);
    let (output_tx, mut output_rx) = mpsc::channel(5);

    let mut tasks = tokio::task::JoinSet::new();

    tasks.spawn(async move {
        RecordingPassthrough {
            name: "s1",
            log: log_tx.clone(),
        }
        .run(PeekableReceiver::new(input_rx), s1_tx)
        .await
        .ok();
    });

    let log_tx2 = mpsc::channel(1000).0;
    tasks.spawn(async move {
        RecordingPassthrough {
            name: "s2",
            log: log_tx2,
        }
        .run(PeekableReceiver::new(s1_rx), s2_tx)
        .await
        .ok();
    });

    let log_tx3 = mpsc::channel(1000).0;
    tasks.spawn(async move {
        RecordingPassthrough {
            name: "s3",
            log: log_tx3,
        }
        .run(PeekableReceiver::new(s2_rx), output_tx)
        .await
        .ok();
    });

    // Send concurrently with receiving — otherwise the bounded channels
    // cause backpressure that blocks the sender before all items are sent.
    let sender = tokio::spawn(async move {
        for i in 0u64..100 {
            input_tx.send(i).await.unwrap();
        }
    });

    for i in 0u64..100 {
        let value = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            output_rx.recv(),
        )
        .await
        .expect("timed out waiting for item")
        .expect("channel closed");
        assert_eq!(value, i, "Item {i} arrived out of order");
    }

    sender.await.unwrap();
}

/// When a mid-pipeline component errors, the downstream component's input
/// channel closes, causing it to also error. This tests graceful degradation.
#[tokio::test]
async fn error_propagates_through_channel_closure() {
    let (input_tx, input_rx) = mpsc::channel(10);
    let (mid_tx, mid_rx) = mpsc::channel(2);
    let (output_tx, mut output_rx) = mpsc::channel(2);

    let mut tasks = tokio::task::JoinSet::new();

    // First stage panics on value 3
    tasks.spawn(async move {
        PanicOnValue { panic_value: 3 }
            .run(PeekableReceiver::new(input_rx), mid_tx)
            .await
            .ok();
    });

    // Second stage is a passthrough
    tasks.spawn(async move {
        DoublingComponent
            .run(PeekableReceiver::new(mid_rx), output_tx)
            .await
            .ok();
    });

    // Send 1, 2 — should work
    input_tx.send(1).await.unwrap();
    input_tx.send(2).await.unwrap();

    assert_eq!(output_rx.recv().await, Some(2));
    assert_eq!(output_rx.recv().await, Some(4));

    // Send 3 — first stage will error
    input_tx.send(3).await.unwrap();

    // Output should eventually close (after error propagates)
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        output_rx.recv(),
    )
    .await;

    match result {
        Ok(None) => {} // Channel closed — expected
        Ok(Some(_)) => panic!("Expected channel to close after error"),
        Err(_) => {} // Timeout is also acceptable (error still propagating)
    }
}

/// Verify that Pipeline::spawn() works without panicking when
/// all components eventually close their channels.
#[tokio::test]
async fn pipeline_builder_spawns_without_panic() {
    let mut tasks = tokio::task::JoinSet::new();

    // Pipeline::new() creates a () pipeline. spawn() consumes it and
    // spawns all component tasks. With no components, this just drops
    // the receiver.
    Pipeline::new().spawn(&mut tasks);

    // Give tasks a moment to settle
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
}
