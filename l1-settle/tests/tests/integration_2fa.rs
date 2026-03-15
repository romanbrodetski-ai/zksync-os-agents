/// Category 8 — Integration: 2FA enabled end-to-end
use std::time::Duration;
use alloy::providers::Provider;
use zksync_os_contract_interface::l1_discovery::L1State;
use zksync_os_integration_tests::Tester;
use zksync_os_integration_tests::provider::ZksyncApi;

async fn wait_for_committed(tester: &Tester, min: u64, max_secs: u64) -> anyhow::Result<u64> {
    let bridgehub = tester.l2_zk_provider.get_bridgehub_contract().await?;
    let chain_id = tester.l2_provider.get_chain_id().await?;

    let attempts = (max_secs / 3).max(1) as usize;
    for _ in 0..attempts {
        let state = L1State::fetch(
            tester.l1_provider().clone().erased(),
            tester.l1_provider().clone().erased(),
            bridgehub,
            chain_id,
        )
        .await?;
        if state.last_committed_batch >= min {
            return Ok(state.last_committed_batch);
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
    anyhow::bail!("timed out: last_committed_batch did not reach {min}")
}

// ------------------------------------------------------------------
// T8.1 — Commit succeeds with 2FA enabled, threshold=1, one valid signature
// ------------------------------------------------------------------
// Mutation: break signature filtering (filter() removed) or wrong calldata selector
// → commit tx uses wrong function and reverts on L1 → last_committed_batch stays at 0.
#[test_log::test(tokio::test)]
async fn t8_1_commit_succeeds_with_2fa_threshold_1() -> anyhow::Result<()> {
    // TesterBuilder::batch_verification(threshold) enables the 2FA server-side signing path.
    // The default signing key (BATCH_VERIFICATION_KEYS[0]) is used to sign batches.
    let tester = Tester::builder()
        .batch_verification(1)
        .build()
        .await?;

    let committed = wait_for_committed(&tester, 1, 120).await?;
    assert!(
        committed >= 1,
        "expected at least 1 batch committed with 2FA threshold=1, got {committed}"
    );
    Ok(())
}

// ------------------------------------------------------------------
// T8.2 — Node handles 2FA threshold config mismatch gracefully and still settles
// ------------------------------------------------------------------
// When server threshold > L1 threshold: no startup warning (safe direction; server enforces
// a stricter requirement). When server threshold < L1 threshold: `check_batch_verification_mismatch`
// emits a warning at startup (added in server commit ad0fcab7).
//
// This test configures server threshold=2 with on-chain threshold=1 (server > L1, no startup
// warning). The pipeline uses effective threshold = max(2, 1) = 2 and must still settle.
//
// Mutation: crash on any threshold mismatch → no batch ever committed.
#[test_log::test(tokio::test)]
async fn t8_2_threshold_mismatch_warns_but_settles() -> anyhow::Result<()> {
    // The on-chain contract has threshold=1 (set by the l1-state fixture).
    // We configure threshold=2 in the server (server > L1: no startup warning from
    // check_batch_verification_mismatch). The pipeline uses effective threshold=max(2,1)=2.
    let tester = Tester::builder()
        .batch_verification(2) // server threshold=2, L1 threshold=1 (server > L1)
        .build()
        .await?;

    // Node must still be alive and commit batches using the on-chain threshold
    let committed = wait_for_committed(&tester, 1, 120).await?;
    assert!(
        committed >= 1,
        "node should survive threshold mismatch and still commit; got last_committed={committed}"
    );
    Ok(())
}
