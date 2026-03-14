/// Category 7 — Integration: passthrough / restart recovery
///
/// Verifies that:
///   T7.1: L1 batch counters never go backwards (no duplicate commit / execute).
///   T7.2: A second node launched against the same L1 (via `launch_external_node`)
///         observes the same committed batch count as the main node — i.e. state is
///         visible on L1 and not locked inside the main node's in-memory channel.
use std::time::Duration;
use alloy::providers::Provider;
use zksync_os_contract_interface::l1_discovery::L1State;
use zksync_os_integration_tests::Tester;
use zksync_os_integration_tests::provider::ZksyncApi;

async fn fetch_l1_state(tester: &Tester) -> anyhow::Result<L1State> {
    let bridgehub = tester.l2_zk_provider.get_bridgehub_contract().await?;
    let chain_id = tester.l2_provider.get_chain_id().await?;
    L1State::fetch(
        tester.l1_provider().clone().erased(),
        tester.l1_provider().clone().erased(),
        bridgehub,
        chain_id,
    )
    .await
}

// ------------------------------------------------------------------
// T7.1 — L1 batch counters are monotonically non-decreasing
// ------------------------------------------------------------------
// Verifies the passthrough invariant indirectly: if the server were to re-commit an
// already-committed batch the on-chain call would revert (contract rejects duplicate
// batch numbers), which would stall the pipeline and ultimately cause
// `last_committed_batch` to stop advancing.  We sample the counter 10 times over 30 s
// and assert it never decreases.
//
// Mutation: remove passthrough guard in GaplessCommitter → duplicate commit tx reverts
// → pipeline stalls → counter stops increasing and the final value equals the initial
//   non-zero value (the counter does not decrease, but also does not increase past a
//   certain point — a separate assertion in T6.1 / T6.2 catches the progress side).
#[test_log::test(tokio::test)]
async fn t7_1_batch_counters_never_decrease() -> anyhow::Result<()> {
    let tester = Tester::setup().await?;

    let mut prev_committed = 0u64;
    let mut prev_executed = 0u64;

    for i in 0..10 {
        let state = fetch_l1_state(&tester).await?;
        assert!(
            state.last_committed_batch >= prev_committed,
            "last_committed_batch must not decrease: was {prev_committed}, now {} (iteration {i})",
            state.last_committed_batch
        );
        assert!(
            state.last_executed_batch >= prev_executed,
            "last_executed_batch must not decrease: was {prev_executed}, now {} (iteration {i})",
            state.last_executed_batch
        );
        prev_committed = state.last_committed_batch;
        prev_executed = state.last_executed_batch;
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
    Ok(())
}

// ------------------------------------------------------------------
// T7.2 — L1 state is visible on-chain and accessible to a second node
// ------------------------------------------------------------------
// Launches an External Node against the same Anvil L1 (via `launch_external_node`).
// The EN's L1 provider points to the same contract, so it must observe the same
// `last_committed_batch` that the main node published.  This confirms that commits
// are persisted to L1 and not merely held in the main node's memory.
//
// Mutation: commit calldata wrong / tx never mined → L1 counter stays at 0 →
// EN also sees 0 → assertion fails.
#[test_log::test(tokio::test)]
async fn t7_2_en_observes_same_l1_state() -> anyhow::Result<()> {
    let main = Tester::setup().await?;

    // Wait for at least one batch to be committed on L1 by the main node
    let mut main_committed = 0u64;
    for _ in 0..60 {
        let state = fetch_l1_state(&main).await?;
        if state.last_committed_batch >= 1 {
            main_committed = state.last_committed_batch;
            break;
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
    assert!(
        main_committed >= 1,
        "test precondition: main node must commit at least 1 batch before launching EN"
    );

    // Launch an External Node — it shares the same Anvil L1 as the main node
    let en = main.launch_external_node().await?;

    let en_state = fetch_l1_state(&en).await?;
    assert!(
        en_state.last_committed_batch >= main_committed,
        "EN must see the same L1 committed batch count as the main node: \
         main={main_committed}, EN={}",
        en_state.last_committed_batch
    );
    Ok(())
}
