use std::time::Duration;

use alloy::network::TransactionBuilder;
use alloy::primitives::{Address, U256};
use alloy::providers::Provider;
use alloy::rpc::types::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;
use rand::Rng;
use serde::Deserialize;
use tokio::sync::watch;
use zksync_os_external_tests::{TestEnvironment, provider_for_rpc_and_signer};

#[tokio::test]
async fn random_arrival_into_already_open_blocks_approaches_half_block_time() -> anyhow::Result<()>
{
    let baselines = load_baselines()?;

    for baseline in baselines {
        let block_time = Duration::from_millis(baseline.block_time_ms);
        let env = TestEnvironment::start_with_block_time(Some(block_time)).await?;
        let filler_signer = PrivateKeySigner::random();
        let filler_address = filler_signer.address();
        env.fund_account(filler_address, U256::from(1_000_000_000_000_000_000u128))
            .await?;
        let filler_provider = provider_for_rpc_and_signer(env.rpc_url.clone(), filler_signer)?;

        let (stop_tx, stop_rx) = watch::channel(false);
        let filler_interval = std::cmp::max(20, (block_time.as_millis() / 5) as u64);
        let filler_handle = tokio::spawn(async move {
            let mut stop_rx = stop_rx;
            loop {
                if *stop_rx.borrow() {
                    break;
                }

                let _ = filler_provider
                    .send_transaction(
                        TransactionRequest::default()
                            .with_to(Address::repeat_byte(0x22))
                            .with_value(U256::from(1_u64)),
                    )
                    .await;

                tokio::select! {
                    _ = stop_rx.changed() => {}
                    _ = tokio::time::sleep(Duration::from_millis(filler_interval)) => {}
                }
            }
        });

        wait_for_block_progress(&env, 3, Duration::from_secs(15)).await?;

        let mut latencies_ms = Vec::with_capacity(baseline.samples);
        let mut rng = rand::rng();
        for attempt in 1..=baseline.samples {
            let send_offset_ms = rng.random_range(0..=block_time.as_millis() as u64);
            tokio::time::sleep(Duration::from_millis(send_offset_ms)).await;
            let report = env.run_basic_transfer_inclusion_only().await?;
            let inclusion_ms = report.inclusion_latency.as_millis();
            println!(
                "open_block block_time_ms={} attempt={} send_offset_ms={} block_number={} inclusion_ms={}",
                block_time.as_millis(),
                attempt,
                send_offset_ms,
                report.block_number,
                inclusion_ms
            );
            latencies_ms.push(inclusion_ms);
        }

        let _ = stop_tx.send(true);
        let _ = filler_handle.await;

        let min = *latencies_ms.iter().min().expect("latencies are present");
        let max = *latencies_ms.iter().max().expect("latencies are present");
        let avg = latencies_ms.iter().sum::<u128>() / latencies_ms.len() as u128;
        let avg_delta_vs_half = avg as i128 - baseline.expected_half_ms as i128;
        let avg_delta_vs_baseline = avg as i128 - baseline.baseline_avg_ms as i128;
        println!(
            "open_block block_time_ms={} summary_min_ms={min}",
            block_time.as_millis()
        );
        println!(
            "open_block block_time_ms={} summary_max_ms={max}",
            block_time.as_millis()
        );
        println!(
            "open_block block_time_ms={} summary_avg_ms={avg}",
            block_time.as_millis()
        );
        println!(
            "open_block block_time_ms={} expected_half_ms={}",
            block_time.as_millis(),
            baseline.expected_half_ms
        );
        println!(
            "open_block block_time_ms={} baseline_avg_ms={}",
            block_time.as_millis(),
            baseline.baseline_avg_ms
        );
        println!(
            "open_block block_time_ms={} delta_vs_expected_half_ms={avg_delta_vs_half}",
            block_time.as_millis()
        );
        println!(
            "open_block block_time_ms={} delta_vs_baseline_avg_ms={avg_delta_vs_baseline}",
            block_time.as_millis()
        );
        println!(
            "open_block block_time_ms={} artifacts_dir={}",
            block_time.as_millis(),
            env.artifacts_dir.display()
        );
    }

    Ok(())
}

#[derive(Debug, Deserialize)]
struct Baseline {
    block_time_ms: u64,
    samples: usize,
    expected_half_ms: u64,
    baseline_avg_ms: u64,
}

fn load_baselines() -> anyhow::Result<Vec<Baseline>> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/baselines/open_block_random_arrival.json"
    );
    let contents = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str(&contents)?)
}

async fn wait_for_block_progress(
    env: &TestEnvironment,
    delta: u64,
    timeout: Duration,
) -> anyhow::Result<()> {
    let provider = env.provider()?;
    let start_block = provider.get_block_number().await?;
    let started = tokio::time::Instant::now();
    loop {
        let current_block = provider.get_block_number().await?;
        if current_block >= start_block + delta {
            return Ok(());
        }
        if started.elapsed() >= timeout {
            anyhow::bail!("timed out waiting for block progress");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
