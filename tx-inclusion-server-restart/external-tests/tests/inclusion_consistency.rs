use std::time::Duration;

use zksync_os_external_tests::TestEnvironment;

#[tokio::test]
async fn measures_inclusion_consistency_over_10_transfers() -> anyhow::Result<()> {
    let env = TestEnvironment::start().await?;
    let mut reports = Vec::with_capacity(10);

    for attempt in 1..=10 {
        let report = env.run_basic_transfer_inclusion_only().await?;
        println!(
            "attempt={attempt} tx_hash={} block_number={} inclusion_ms={}",
            report.tx_hash,
            report.block_number,
            report.inclusion_latency.as_millis()
        );
        reports.push(report);
    }

    let latencies: Vec<Duration> = reports.iter().map(|r| r.inclusion_latency).collect();
    let min = latencies.iter().copied().min().expect("latencies are present");
    let max = latencies.iter().copied().max().expect("latencies are present");
    let total_ms: u128 = latencies.iter().map(|d| d.as_millis()).sum();
    let avg_ms = total_ms / latencies.len() as u128;
    let spread_ms = max.as_millis() - min.as_millis();

    println!("artifacts_dir={}", env.artifacts_dir.display());
    println!("server_log={}", env.server_log_path.display());
    println!("anvil_log={}", env.anvil_log_path.display());
    println!("summary_runs={}", latencies.len());
    println!("summary_min_ms={}", min.as_millis());
    println!("summary_max_ms={}", max.as_millis());
    println!("summary_avg_ms={avg_ms}");
    println!("summary_spread_ms={spread_ms}");

    Ok(())
}
