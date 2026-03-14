/// Category 4 — 2FA / CommitCommand signature validation (unit, pure, no running node)
///
/// Uses the same BATCH_VERIFICATION_KEYS as the integration test harness so that
/// key material is consistent across test suites.
use alloy::primitives::Address;
use alloy::signers::local::PrivateKeySigner;
use std::str::FromStr;
use zksync_os_batch_types::{BatchSignature, BatchSignatureSet};
use zksync_os_contract_interface::l1_discovery::{BatchVerificationSL, BatchVerificationSLConfig};
use zksync_os_l1_sender::batcher_model::BatchSignatureData;
use zksync_os_l1_sender::commands::commit::{BatchVerificationError, CommitCommand};
use zksync_os_l1_settle_tests::helpers::{batch_info, signed_envelope, stored_batch_info};

// Private keys used by the integration test harness (from integration-tests/src/lib.rs)
const KEY_A: &str = "0x7094f4b57ed88624583f68d2f241858f7dafb6d2558bc22d18991690d36b4e47";
const KEY_B: &str = "0xf9306dd03807c08b646d47c739bd51e4d2a25b02bad0efb3d93f095982ac98cd";

fn signer(key: &str) -> PrivateKeySigner {
    PrivateKeySigner::from_str(key).unwrap()
}

fn address(key: &str) -> Address {
    signer(key).address()
}

/// Build a signed `BatchSignatureData` with signatures from the provided keys.
async fn make_signature_data(keys: &[&str], protocol_minor: u64) -> BatchSignatureData {
    let prev = stored_batch_info(0);
    let info = batch_info(1, protocol_minor);
    let multisig = Address::repeat_byte(0xbb);
    let sl_chain_id = 1u64;
    let protocol = zksync_os_l1_settle_tests::helpers::protocol_version(protocol_minor);

    let mut set = BatchSignatureSet::new();
    for key in keys {
        let sig = BatchSignature::sign_batch(
            &prev, &info, sl_chain_id, multisig, &protocol, &signer(key),
        )
        .await;
        let validated = sig
            .verify_signature(&prev, &info, sl_chain_id, multisig, &protocol)
            .expect("signature should be valid");
        set.push(validated).expect("no duplicate signatures");
    }
    BatchSignatureData::Signed { signatures: set }
}

// ------------------------------------------------------------------
// T4.1 — Commit accepted with 2FA disabled (all signature variants)
// ------------------------------------------------------------------
// Mutation: accidentally check signatures when disabled → Ok turned into Err.
#[tokio::test]
async fn t4_1_commit_accepted_when_2fa_disabled() {
    let disabled = BatchVerificationSL::Disabled;

    for sig_data in [
        BatchSignatureData::NotNeeded,
        BatchSignatureData::AlreadyCommitted,
        make_signature_data(&[KEY_A], 30).await,
    ] {
        let mut env = signed_envelope(1, 30);
        env.signature_data = sig_data;
        assert!(
            CommitCommand::try_new(&disabled, env).is_ok(),
            "commit must be accepted when 2FA is disabled regardless of signature data"
        );
    }
}

// ------------------------------------------------------------------
// T4.2 — Commit accepted when threshold exactly met (1 of 1)
// ------------------------------------------------------------------
// Mutation: change `<` to `<=` in threshold check → 1 signature no longer satisfies threshold=1.
#[tokio::test]
async fn t4_2_commit_accepted_threshold_met() {
    let allowed = vec![address(KEY_A)];
    let config = BatchVerificationSL::Enabled(BatchVerificationSLConfig {
        threshold: 1,
        validators: allowed,
    });

    let mut env = signed_envelope(1, 30);
    env.signature_data = make_signature_data(&[KEY_A], 30).await;

    assert!(
        CommitCommand::try_new(&config, env).is_ok(),
        "commit must be accepted when threshold=1 and one valid signature is present"
    );
}

// ------------------------------------------------------------------
// T4.3 — Commit rejected when below threshold
// ------------------------------------------------------------------
// Mutation: change `<` to `>` in threshold check → under-threshold commits accepted.
#[tokio::test]
async fn t4_3_commit_rejected_below_threshold() {
    let allowed = vec![address(KEY_A), address(KEY_B)];
    let config = BatchVerificationSL::Enabled(BatchVerificationSLConfig {
        threshold: 2,
        validators: allowed,
    });

    // Only one of two required signatures
    let mut env = signed_envelope(1, 30);
    env.signature_data = make_signature_data(&[KEY_A], 30).await;

    let result = CommitCommand::try_new(&config, env);
    match result {
        Err(BatchVerificationError::NotEnoughSignatures(got, need)) => {
            assert_eq!(got, 1, "should report 1 signature present");
            assert_eq!(need, 2, "should report threshold of 2");
        }
        other => panic!("expected NotEnoughSignatures, got {other:?}"),
    }
}

// ------------------------------------------------------------------
// T4.4 — Commit rejected when batch not signed (threshold > 0)
// ------------------------------------------------------------------
// Mutation: skip the guard for non-Signed variant → NotNeeded + threshold=1 accepted.
#[test]
fn t4_4_commit_rejected_when_not_signed() {
    let config = BatchVerificationSL::Enabled(BatchVerificationSLConfig {
        threshold: 1,
        validators: vec![address(KEY_A)],
    });

    let mut env = signed_envelope(1, 30);
    env.signature_data = BatchSignatureData::NotNeeded;

    let result = CommitCommand::try_new(&config, env);
    assert!(
        matches!(result, Err(BatchVerificationError::BatchNotSigned)),
        "expected BatchNotSigned error for NotNeeded data with threshold=1"
    );
}

// ------------------------------------------------------------------
// T4.5 — Threshold=0 with 2FA enabled bypasses signature requirement
// ------------------------------------------------------------------
// Mutation: check count < 0 incorrectly, or apply guard before checking zero threshold.
#[test]
fn t4_5_threshold_zero_bypasses_signatures() {
    let config = BatchVerificationSL::Enabled(BatchVerificationSLConfig {
        threshold: 0,
        validators: vec![address(KEY_A)],
    });

    let mut env = signed_envelope(1, 30);
    env.signature_data = BatchSignatureData::NotNeeded;

    assert!(
        CommitCommand::try_new(&config, env).is_ok(),
        "threshold=0 must accept commits without any signatures"
    );
}

// ------------------------------------------------------------------
// T4.6 — Signatures are sorted by signer address in calldata
// ------------------------------------------------------------------
// Mutation: remove or reverse the sort step in CommitCommand::solidity_call
// → signers in calldata are in an arbitrary (non-ascending) order.
#[tokio::test]
async fn t4_6_signatures_sorted_by_address() {
    use alloy::sol_types::SolCall;
    use zksync_os_contract_interface::IMultisigCommitter;
    use zksync_os_l1_sender::commands::SendToL1;

    let addr_a = address(KEY_A);
    let addr_b = address(KEY_B);
    let allowed = vec![addr_a, addr_b];
    let config = BatchVerificationSL::Enabled(BatchVerificationSLConfig {
        threshold: 1,
        validators: allowed,
    });

    let mut env = signed_envelope(1, 30);
    // Sign with both keys so we have two signers
    env.signature_data = make_signature_data(&[KEY_A, KEY_B], 30).await;

    let cmd = CommitCommand::try_new(&config, env).expect("should succeed with two signatures");
    let calldata = cmd.solidity_call(false);

    // Decode the multisig call
    let decoded = IMultisigCommitter::commitBatchesMultisigCall::abi_decode(&calldata)
        .expect("should decode as multisig call");

    let signers = decoded.signers;
    assert!(signers.len() >= 2, "expected at least 2 signers");
    for window in signers.windows(2) {
        assert!(
            window[0] <= window[1],
            "signers must be in ascending address order; got {:#x} before {:#x}",
            window[0], window[1]
        );
    }
}

// ------------------------------------------------------------------
// T4.7 — Out-of-set signatures are filtered out before threshold check
// ------------------------------------------------------------------
// Mutation: remove the filter() call in CommitCommand::try_new
// → foreign signatures count toward threshold, potentially allowing unauthorized commits.
#[tokio::test]
async fn t4_7_foreign_signatures_filtered_out() {
    // KEY_B is NOT in the allowed set
    let allowed = vec![address(KEY_A)];
    let config = BatchVerificationSL::Enabled(BatchVerificationSLConfig {
        threshold: 1,
        validators: allowed,
    });

    // Provide only KEY_B's signature — should be filtered and leave 0 valid signatures
    let mut env = signed_envelope(1, 30);
    env.signature_data = make_signature_data(&[KEY_B], 30).await;

    let result = CommitCommand::try_new(&config, env);
    match result {
        Err(BatchVerificationError::NotEnoughSignatures(0, 1)) => {
            // correct: filtered to 0, need 1
        }
        other => panic!(
            "expected NotEnoughSignatures(0, 1) after filtering foreign signature, got {other:?}"
        ),
    }
}
