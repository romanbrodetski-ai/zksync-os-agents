use zksync_os_external_tests::TestEnvironment;

#[tokio::test]
async fn receipts_match_between_main_node_and_external_node() -> anyhow::Result<()> {
    let env = TestEnvironment::start().await?;
    let en = env.launch_external_node().await?;

    let tx_hash = en.send_basic_transfer().await?;
    let main_receipt = env.wait_for_receipt_json(tx_hash).await?;
    let en_receipt = en.wait_for_receipt_json(tx_hash).await?;

    println!("tx_hash={tx_hash:#x}");
    println!("main_rpc_url={}", env.rpc_url);
    println!("external_rpc_url={}", en.rpc_url);
    println!("main_artifacts_dir={}", env.artifacts_dir.display());
    println!("external_artifacts_dir={}", en.artifacts_dir.display());

    assert_eq!(main_receipt, en_receipt);

    Ok(())
}
