use std::sync::LazyLock;
use std::time::{Duration, Instant};

use alloy::primitives::{Address, U256};
use tokio::sync::Mutex;
use zksync_os_external_tests::TestEnvironment;

static RESTART_TEST_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

#[tokio::test]
async fn included_tx_survives_main_node_restart() -> anyhow::Result<()> {
    let _guard = RESTART_TEST_LOCK.lock().await;
    let mut env = TestEnvironment::start_with_block_time(Some(Duration::from_millis(250))).await?;
    let recipient = Address::repeat_byte(0x11);
    let balance_before = env.recipient_balance(recipient).await?;

    let tx_hash = env.send_basic_transfer().await?;
    let receipt_before = env.wait_for_receipt_json(tx_hash).await?;

    env.restart_main_node().await?;

    let receipt_after = env.wait_for_receipt_json(tx_hash).await?;
    let balance_after = env.recipient_balance(recipient).await?;

    println!("tx_hash={tx_hash:#x}");
    println!("balance_before={balance_before}");
    println!("balance_after={balance_after}");
    println!("artifacts_dir={}", env.artifacts_dir.display());

    assert_eq!(receipt_before, receipt_after);
    assert_eq!(balance_after, balance_before + U256::from(1_u64));

    Ok(())
}

#[tokio::test]
async fn restart_after_inclusion_before_finality_still_settles() -> anyhow::Result<()> {
    let _guard = RESTART_TEST_LOCK.lock().await;
    let mut env = TestEnvironment::start_with_block_time(Some(Duration::from_millis(250))).await?;

    let report = env.run_basic_transfer_inclusion_only().await?;
    let started = Instant::now();
    env.restart_main_node().await?;
    let restart_elapsed = started.elapsed();

    env.wait_for_safe(report.block_number, Duration::from_secs(180))
        .await?;
    let safe_elapsed = started.elapsed();
    env.wait_for_finalized(report.block_number, Duration::from_secs(180))
        .await?;
    let finalized_elapsed = started.elapsed();

    println!("tx_hash={}", report.tx_hash);
    println!("block_number={}", report.block_number);
    println!("inclusion_ms={}", report.inclusion_latency.as_millis());
    println!("restart_ms={}", restart_elapsed.as_millis());
    println!("safe_after_restart_ms={}", safe_elapsed.as_millis());
    println!("finalized_after_restart_ms={}", finalized_elapsed.as_millis());
    println!("artifacts_dir={}", env.artifacts_dir.display());

    assert!(restart_elapsed <= safe_elapsed);
    assert!(safe_elapsed <= finalized_elapsed);

    Ok(())
}

#[tokio::test]
async fn main_node_can_include_new_tx_after_restart_replay() -> anyhow::Result<()> {
    let _guard = RESTART_TEST_LOCK.lock().await;
    let mut env = TestEnvironment::start_with_block_time(Some(Duration::from_millis(250))).await?;
    let recipient = Address::repeat_byte(0x11);
    let balance_before = env.recipient_balance(recipient).await?;

    let first_tx_hash = env.send_basic_transfer().await?;
    let first_receipt = env.wait_for_receipt_json(first_tx_hash).await?;

    env.restart_main_node().await?;

    let first_receipt_after_restart = env.wait_for_receipt_json(first_tx_hash).await?;
    let second_tx_hash = env.send_basic_transfer().await?;
    let second_receipt = env.wait_for_receipt_json(second_tx_hash).await?;
    let balance_after = env.recipient_balance(recipient).await?;

    println!("first_tx_hash={first_tx_hash:#x}");
    println!("second_tx_hash={second_tx_hash:#x}");
    println!("balance_before={balance_before}");
    println!("balance_after={balance_after}");
    println!("artifacts_dir={}", env.artifacts_dir.display());

    assert_eq!(first_receipt, first_receipt_after_restart);
    assert_ne!(first_receipt["blockHash"], second_receipt["blockHash"]);
    assert_eq!(balance_after, balance_before + U256::from(2_u64));

    Ok(())
}
