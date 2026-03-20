use std::time::Duration;

use zksync_os_external_tests::TestEnvironment;

#[tokio::test]
async fn external_node_recovers_after_main_and_external_restarts() -> anyhow::Result<()> {
    let mut env = TestEnvironment::start_with_block_time(Some(Duration::from_millis(250))).await?;
    let mut en = env.launch_external_node().await?;

    let tx_before_restart = en.send_basic_transfer().await?;
    let main_receipt_before_restart = env.wait_for_receipt_json(tx_before_restart).await?;
    let en_receipt_before_restart = en.wait_for_receipt_json(tx_before_restart).await?;

    env.restart_main_node().await?;
    en.restart(env.l1_rpc_url(), &env.rpc_url, env.main_node_record())
        .await?;

    let main_receipt_after_restart = env.wait_for_receipt_json(tx_before_restart).await?;
    let en_receipt_after_restart = en.wait_for_receipt_json(tx_before_restart).await?;

    let tx_after_restart = en.send_basic_transfer().await?;
    let main_receipt_for_new_tx = env.wait_for_receipt_json(tx_after_restart).await?;
    let en_receipt_for_new_tx = en.wait_for_receipt_json(tx_after_restart).await?;

    println!("tx_before_restart={tx_before_restart:#x}");
    println!("tx_after_restart={tx_after_restart:#x}");
    println!("main_rpc_url={}", env.rpc_url);
    println!("external_rpc_url={}", en.rpc_url);
    println!("main_artifacts_dir={}", env.artifacts_dir.display());
    println!("external_artifacts_dir={}", en.artifacts_dir.display());

    assert_eq!(main_receipt_before_restart, en_receipt_before_restart);
    assert_eq!(main_receipt_before_restart, main_receipt_after_restart);
    assert_eq!(en_receipt_before_restart, en_receipt_after_restart);
    assert_eq!(main_receipt_after_restart, en_receipt_after_restart);
    assert_eq!(main_receipt_for_new_tx, en_receipt_for_new_tx);

    Ok(())
}
