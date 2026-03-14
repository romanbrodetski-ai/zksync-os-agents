/// Category 6 — Integration: batch settling happy path (fake provers, Anvil L1)
///
/// Verifies that the full Commit → Prove → Execute pipeline completes end-to-end
/// and that L1 counters advance in the correct order.
use std::time::Duration;
use alloy::network::TransactionBuilder;
use alloy::providers::Provider;
use zksync_os_contract_interface::l1_discovery::L1State;
use zksync_os_integration_tests::Tester;
use zksync_os_integration_tests::provider::ZksyncApi;

/// Poll `L1State` until `predicate` is true or `max_attempts` are exhausted.
async fn wait_for_l1_state(
    tester: &Tester,
    max_attempts: usize,
    interval: Duration,
    predicate: impl Fn(&L1State) -> bool,
) -> anyhow::Result<L1State> {
    for attempt in 0..max_attempts {
        let bridgehub = tester.l2_zk_provider.get_bridgehub_contract().await?;
        let chain_id = tester.l2_provider.get_chain_id().await?;
        let state = L1State::fetch(
            tester.l1_provider().clone().erased(),
            tester.l1_provider().clone().erased(),
            bridgehub,
            chain_id,
        )
        .await?;
        if predicate(&state) {
            return Ok(state);
        }
        tracing::info!(attempt, "L1 state not ready yet, waiting...");
        tokio::time::sleep(interval).await;
    }
    anyhow::bail!("timed out waiting for L1 state condition after {max_attempts} attempts")
}

// ------------------------------------------------------------------
// T6.1 — At least one batch is committed, proved, and executed on L1 (v30 genesis)
// ------------------------------------------------------------------
// Mutation: break CommitCommand calldata encoding (e.g., wrong version byte)
// → L1 commit transaction reverts → last_committed_batch stays at 0.
#[test_log::test(tokio::test)]
async fn t6_1_single_batch_settles_v30() -> anyhow::Result<()> {
    let tester = Tester::setup().await?;

    let state = wait_for_l1_state(&tester, 60, Duration::from_secs(5), |s| {
        s.last_executed_batch >= 1
    })
    .await?;

    assert!(
        state.last_committed_batch >= 1,
        "expected at least 1 committed batch, got {}",
        state.last_committed_batch
    );
    assert!(
        state.last_proved_batch >= 1,
        "expected at least 1 proved batch, got {}",
        state.last_proved_batch
    );
    assert!(
        state.last_executed_batch >= 1,
        "expected at least 1 executed batch, got {}",
        state.last_executed_batch
    );
    Ok(())
}

// ------------------------------------------------------------------
// T6.2 — Batch counter ordering: committed >= proved >= executed
// ------------------------------------------------------------------
// Mutation: send execute before prove (wrong pipeline order)
// → ordering invariant violated → assertion fails.
#[test_log::test(tokio::test)]
async fn t6_2_batch_counter_ordering() -> anyhow::Result<()> {
    let tester = Tester::setup().await?;

    // Sample L1 state several times while the pipeline is running
    let bridgehub = tester.l2_zk_provider.get_bridgehub_contract().await?;
    let chain_id = tester.l2_provider.get_chain_id().await?;

    for _ in 0..10 {
        let state = L1State::fetch(
            tester.l1_provider().clone().erased(),
            tester.l1_provider().clone().erased(),
            bridgehub,
            chain_id,
        )
        .await?;

        assert!(
            state.last_committed_batch >= state.last_proved_batch,
            "committed({}) must be >= proved({})",
            state.last_committed_batch,
            state.last_proved_batch
        );
        assert!(
            state.last_proved_batch >= state.last_executed_batch,
            "proved({}) must be >= executed({})",
            state.last_proved_batch,
            state.last_executed_batch
        );

        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    Ok(())
}

// ------------------------------------------------------------------
// T6.3 — Multiple batches are all eventually executed
// ------------------------------------------------------------------
// Mutation: pipeline stalls after first batch (e.g., nonce collision or channel closed)
// → last_executed_batch never reaches 3.
#[test_log::test(tokio::test)]
async fn t6_3_multiple_batches_settle() -> anyhow::Result<()> {
    use alloy::primitives::U256;

    let tester = Tester::setup().await?;

    // Generate some L2 transactions to trigger additional batches
    for _ in 0..5 {
        tester
            .l2_provider
            .send_transaction(
                alloy::rpc::types::TransactionRequest::default()
                    .with_from(tester.l2_wallet.default_signer().address())
                    .with_to(tester.l2_wallet.default_signer().address())
                    .with_value(U256::from(1u64)),
            )
            .await?
            .watch()
            .await?;
    }

    let state = wait_for_l1_state(&tester, 120, Duration::from_secs(5), |s| {
        s.last_executed_batch >= 3
    })
    .await?;

    assert!(
        state.last_executed_batch >= 3,
        "expected at least 3 executed batches after producing L2 transactions, got {}",
        state.last_executed_batch
    );
    Ok(())
}
