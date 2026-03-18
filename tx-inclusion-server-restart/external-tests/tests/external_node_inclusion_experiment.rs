use std::time::Duration;

use anyhow::Context;
use serde::Deserialize;
use zksync_os_external_tests::TestEnvironment;

#[tokio::test]
async fn external_node_inclusion_latency_across_block_times() -> anyhow::Result<()> {
    let baselines = load_baselines()?;

    for baseline in baselines {
        let block_time = Duration::from_millis(baseline.block_time_ms);
        let env = TestEnvironment::start_with_block_time(Some(block_time)).await?;
        let en = env.launch_external_node().await?;

        let mut latencies_ms = Vec::with_capacity(baseline.samples);
        for attempt in 1..=baseline.samples {
            let report = en.run_basic_transfer_inclusion_only().await?;
            let inclusion_ms = report.inclusion_latency.as_millis();
            println!(
                "external_node block_time_ms={} attempt={} block_number={} inclusion_ms={}",
                block_time.as_millis(),
                attempt,
                report.block_number,
                inclusion_ms
            );
            latencies_ms.push(inclusion_ms);
        }

        let min = *latencies_ms.iter().min().expect("latencies are present");
        let max = *latencies_ms.iter().max().expect("latencies are present");
        let avg = latencies_ms.iter().sum::<u128>() / latencies_ms.len() as u128;
        let delta_vs_baseline = avg as i128 - baseline.baseline_avg_ms as i128;
        println!(
            "external_node block_time_ms={} summary_min_ms={min}",
            block_time.as_millis()
        );
        println!(
            "external_node block_time_ms={} summary_max_ms={max}",
            block_time.as_millis()
        );
        println!(
            "external_node block_time_ms={} summary_avg_ms={avg}",
            block_time.as_millis()
        );
        println!(
            "external_node block_time_ms={} baseline_avg_ms={}",
            block_time.as_millis(),
            baseline.baseline_avg_ms
        );
        println!(
            "external_node block_time_ms={} delta_vs_baseline_avg_ms={delta_vs_baseline}",
            block_time.as_millis()
        );
        println!(
            "external_node block_time_ms={} artifacts_dir={}",
            block_time.as_millis(),
            en.artifacts_dir.display()
        );
    }

    Ok(())
}

#[derive(Debug, Deserialize)]
struct Baseline {
    block_time_ms: u64,
    samples: usize,
    baseline_avg_ms: u64,
}

fn load_baselines() -> anyhow::Result<Vec<Baseline>> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/baselines/external_node_inclusion.json"
    );
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read baseline file at {path}"))?;
    Ok(serde_json::from_str(&contents)?)
}
