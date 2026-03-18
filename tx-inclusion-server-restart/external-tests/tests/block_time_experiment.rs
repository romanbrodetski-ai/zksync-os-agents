use std::time::Duration;

use rand::Rng;
use zksync_os_external_tests::TestEnvironment;

#[tokio::test]
async fn inclusion_latency_changes_with_block_time() -> anyhow::Result<()> {
    let block_times = [
        Duration::from_millis(50),
        Duration::from_millis(100),
        Duration::from_millis(150),
        Duration::from_millis(200),
        Duration::from_millis(250),
        Duration::from_millis(300),
        Duration::from_millis(400),
        Duration::from_millis(500),
        Duration::from_millis(750),
        Duration::from_secs(1),
        Duration::from_millis(1500),
        Duration::from_secs(2),
    ];

    for block_time in block_times {
        let env = TestEnvironment::start_with_block_time(Some(block_time)).await?;
        let mut latencies_ms = Vec::with_capacity(10);
        let mut rng = rand::rng();

        for attempt in 1..=10 {
            let send_offset_ms = rng.random_range(0..=block_time.as_millis() as u64);
            tokio::time::sleep(Duration::from_millis(send_offset_ms)).await;
            let report = env.run_basic_transfer_inclusion_only().await?;
            let inclusion_ms = report.inclusion_latency.as_millis();
            println!(
                "block_time_ms={} attempt={} send_offset_ms={} block_number={} inclusion_ms={}",
                block_time.as_millis(),
                attempt,
                send_offset_ms,
                report.block_number,
                inclusion_ms
            );
            latencies_ms.push(inclusion_ms);
        }

        let min = *latencies_ms.iter().min().expect("latencies are present");
        let max = *latencies_ms.iter().max().expect("latencies are present");
        let avg = latencies_ms.iter().sum::<u128>() / latencies_ms.len() as u128;
        println!("block_time_ms={} summary_min_ms={min}", block_time.as_millis());
        println!("block_time_ms={} summary_max_ms={max}", block_time.as_millis());
        println!("block_time_ms={} summary_avg_ms={avg}", block_time.as_millis());
        println!(
            "block_time_ms={} artifacts_dir={}",
            block_time.as_millis(),
            env.artifacts_dir.display()
        );
        println!(
            "block_time_ms={} server_log={}",
            block_time.as_millis(),
            env.server_log_path.display()
        );
    }

    Ok(())
}
