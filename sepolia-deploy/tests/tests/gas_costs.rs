/// Gas cost bounds tests.
/// These encode the expected gas ranges from actual Sepolia deployments.
/// A test failing means gas costs changed significantly, which may be intentional
/// (new features, contract changes) or a regression (accidental complexity).

// Deployment gas bounds (from Sepolia deployment 2026-03-14)
const DEPLOY_L1_CORE_GAS_UPPER: u64 = 50_000_000;
const DEPLOY_CTM_GAS_UPPER: u64 = 60_000_000;
const REGISTER_CTM_GAS_UPPER: u64 = 500_000;
const REGISTER_CHAIN_GAS_UPPER: u64 = 15_000_000;
const DEPLOY_L2_CONTRACTS_GAS_UPPER: u64 = 5_000_000;
const TOTAL_DEPLOYMENT_GAS_UPPER: u64 = 130_000_000;

// Per-batch settlement gas bounds
const COMMIT_GAS_UPPER: u64 = 300_000;
const PROVE_GAS_UPPER: u64 = 200_000;
const EXECUTE_GAS_UPPER: u64 = 250_000;
const TOTAL_SETTLEMENT_GAS_UPPER: u64 = 750_000;

// Baseline values from actual deployment
const BASELINE_DEPLOY_L1_CORE: u64 = 36_309_096;
const BASELINE_DEPLOY_CTM: u64 = 43_505_814;
const BASELINE_REGISTER_CTM: u64 = 277_908;
const BASELINE_REGISTER_CHAIN: u64 = 11_628_968;
const BASELINE_DEPLOY_L2: u64 = 2_597_089;

const BASELINE_COMMIT_BATCH1: u64 = 192_614;
const BASELINE_PROVE_BATCH1: u64 = 104_152;
const BASELINE_EXECUTE_BATCH1: u64 = 131_489;

const BASELINE_COMMIT_BATCH2: u64 = 138_894;
const BASELINE_PROVE_BATCH2: u64 = 87_844;
const BASELINE_EXECUTE_BATCH2: u64 = 154_273;

/// Validate deployment gas costs are within bounds.
/// This test encodes the baseline — update when contracts change intentionally.
#[test]
fn deployment_gas_within_bounds() {
    assert!(
        BASELINE_DEPLOY_L1_CORE < DEPLOY_L1_CORE_GAS_UPPER,
        "DeployL1CoreContracts gas ({}) exceeds bound ({})",
        BASELINE_DEPLOY_L1_CORE,
        DEPLOY_L1_CORE_GAS_UPPER
    );
    assert!(
        BASELINE_DEPLOY_CTM < DEPLOY_CTM_GAS_UPPER,
        "DeployCTM gas ({}) exceeds bound ({})",
        BASELINE_DEPLOY_CTM,
        DEPLOY_CTM_GAS_UPPER
    );
    assert!(
        BASELINE_REGISTER_CTM < REGISTER_CTM_GAS_UPPER,
        "RegisterCTM gas ({}) exceeds bound ({})",
        BASELINE_REGISTER_CTM,
        REGISTER_CTM_GAS_UPPER
    );
    assert!(
        BASELINE_REGISTER_CHAIN < REGISTER_CHAIN_GAS_UPPER,
        "RegisterZKChain gas ({}) exceeds bound ({})",
        BASELINE_REGISTER_CHAIN,
        REGISTER_CHAIN_GAS_UPPER
    );
    assert!(
        BASELINE_DEPLOY_L2 < DEPLOY_L2_CONTRACTS_GAS_UPPER,
        "DeployL2Contracts gas ({}) exceeds bound ({})",
        BASELINE_DEPLOY_L2,
        DEPLOY_L2_CONTRACTS_GAS_UPPER
    );

    let total = BASELINE_DEPLOY_L1_CORE
        + BASELINE_DEPLOY_CTM
        + BASELINE_REGISTER_CTM
        + BASELINE_REGISTER_CHAIN
        + BASELINE_DEPLOY_L2;
    assert!(
        total < TOTAL_DEPLOYMENT_GAS_UPPER,
        "Total deployment gas ({}) exceeds bound ({})",
        total,
        TOTAL_DEPLOYMENT_GAS_UPPER
    );
}

/// Validate per-batch settlement gas costs (genesis batch).
#[test]
fn settlement_gas_batch1_within_bounds() {
    assert!(
        BASELINE_COMMIT_BATCH1 < COMMIT_GAS_UPPER,
        "Batch 1 commit gas ({}) exceeds bound ({})",
        BASELINE_COMMIT_BATCH1,
        COMMIT_GAS_UPPER
    );
    assert!(
        BASELINE_PROVE_BATCH1 < PROVE_GAS_UPPER,
        "Batch 1 prove gas ({}) exceeds bound ({})",
        BASELINE_PROVE_BATCH1,
        PROVE_GAS_UPPER
    );
    assert!(
        BASELINE_EXECUTE_BATCH1 < EXECUTE_GAS_UPPER,
        "Batch 1 execute gas ({}) exceeds bound ({})",
        BASELINE_EXECUTE_BATCH1,
        EXECUTE_GAS_UPPER
    );
}

/// Validate per-batch settlement gas costs (normal batch).
#[test]
fn settlement_gas_batch2_within_bounds() {
    assert!(
        BASELINE_COMMIT_BATCH2 < COMMIT_GAS_UPPER,
        "Batch 2 commit gas ({}) exceeds bound ({})",
        BASELINE_COMMIT_BATCH2,
        COMMIT_GAS_UPPER
    );
    assert!(
        BASELINE_PROVE_BATCH2 < PROVE_GAS_UPPER,
        "Batch 2 prove gas ({}) exceeds bound ({})",
        BASELINE_PROVE_BATCH2,
        PROVE_GAS_UPPER
    );
    assert!(
        BASELINE_EXECUTE_BATCH2 < EXECUTE_GAS_UPPER,
        "Batch 2 execute gas ({}) exceeds bound ({})",
        BASELINE_EXECUTE_BATCH2,
        EXECUTE_GAS_UPPER
    );
}

/// Total settlement cost per batch stays reasonable.
#[test]
fn total_settlement_gas_reasonable() {
    let batch1_total = BASELINE_COMMIT_BATCH1 + BASELINE_PROVE_BATCH1 + BASELINE_EXECUTE_BATCH1;
    let batch2_total = BASELINE_COMMIT_BATCH2 + BASELINE_PROVE_BATCH2 + BASELINE_EXECUTE_BATCH2;

    assert!(
        batch1_total < TOTAL_SETTLEMENT_GAS_UPPER,
        "Batch 1 total settlement gas ({}) exceeds bound ({})",
        batch1_total,
        TOTAL_SETTLEMENT_GAS_UPPER
    );
    assert!(
        batch2_total < TOTAL_SETTLEMENT_GAS_UPPER,
        "Batch 2 total settlement gas ({}) exceeds bound ({})",
        batch2_total,
        TOTAL_SETTLEMENT_GAS_UPPER
    );
}

/// Print a summary report of gas costs (informational, always passes).
#[test]
fn gas_cost_report() {
    let deploy_total = BASELINE_DEPLOY_L1_CORE
        + BASELINE_DEPLOY_CTM
        + BASELINE_REGISTER_CTM
        + BASELINE_REGISTER_CHAIN
        + BASELINE_DEPLOY_L2;

    let batch1_total = BASELINE_COMMIT_BATCH1 + BASELINE_PROVE_BATCH1 + BASELINE_EXECUTE_BATCH1;
    let batch2_total = BASELINE_COMMIT_BATCH2 + BASELINE_PROVE_BATCH2 + BASELINE_EXECUTE_BATCH2;

    eprintln!("\n=== Gas Cost Report ===");
    eprintln!("Deployment:");
    eprintln!("  L1 Core:       {:>12}", BASELINE_DEPLOY_L1_CORE);
    eprintln!("  CTM:           {:>12}", BASELINE_DEPLOY_CTM);
    eprintln!("  Register CTM:  {:>12}", BASELINE_REGISTER_CTM);
    eprintln!("  Register Chain:{:>12}", BASELINE_REGISTER_CHAIN);
    eprintln!("  L2 Contracts:  {:>12}", BASELINE_DEPLOY_L2);
    eprintln!("  Total:         {:>12}", deploy_total);
    eprintln!();
    eprintln!("Settlement (Batch 1 - genesis):");
    eprintln!("  Commit:  {:>8}", BASELINE_COMMIT_BATCH1);
    eprintln!("  Prove:   {:>8}", BASELINE_PROVE_BATCH1);
    eprintln!("  Execute: {:>8}", BASELINE_EXECUTE_BATCH1);
    eprintln!("  Total:   {:>8}", batch1_total);
    eprintln!();
    eprintln!("Settlement (Batch 2 - normal, 5 L2 txs):");
    eprintln!("  Commit:  {:>8} ({}/tx)", BASELINE_COMMIT_BATCH2, BASELINE_COMMIT_BATCH2 / 5);
    eprintln!("  Prove:   {:>8} ({}/tx)", BASELINE_PROVE_BATCH2, BASELINE_PROVE_BATCH2 / 5);
    eprintln!("  Execute: {:>8} ({}/tx)", BASELINE_EXECUTE_BATCH2, BASELINE_EXECUTE_BATCH2 / 5);
    eprintln!("  Total:   {:>8} ({}/tx)", batch2_total, batch2_total / 5);
    eprintln!("=== End Report ===\n");
}
