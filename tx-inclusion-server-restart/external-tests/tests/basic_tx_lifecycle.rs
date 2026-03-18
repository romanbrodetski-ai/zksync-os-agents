use zksync_os_external_tests::TestEnvironment;

#[tokio::test]
async fn measures_basic_transaction_lifecycle() -> anyhow::Result<()> {
    let env = TestEnvironment::start().await?;
    let report = env.run_basic_transfer().await?;

    println!("rpc_url={}", env.rpc_url);
    println!("status_url={}", env.status_url);
    println!("artifacts_dir={}", env.artifacts_dir.display());
    println!("logs_dir={}", env.logs_dir.display());
    println!("anvil_log={}", env.anvil_log_path.display());
    println!("server_log={}", env.server_log_path.display());
    println!("tx_hash={}", report.tx_hash);
    println!("block_number={}", report.block_number);
    println!("inclusion_ms={}", report.inclusion_latency.as_millis());
    println!("safe_ms={}", report.safe_latency.as_millis());
    println!("finalized_ms={}", report.finalized_latency.as_millis());

    assert!(report.inclusion_latency <= report.safe_latency);
    assert!(report.safe_latency <= report.finalized_latency);

    Ok(())
}
