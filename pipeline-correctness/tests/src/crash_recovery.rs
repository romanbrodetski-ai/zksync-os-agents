//! Tests for crash recovery via WAL replay.
//!
//! The Sequencer writes to three stores in order:
//!   1. WriteReplay (WAL) — fire-and-forget, return value ignored
//!   2. WriteState — error propagated with ?
//!   3. WriteRepository — error propagated with ?
//!
//! If a crash occurs between step 1 and step 2 (or 3), the block is in the WAL
//! but not in state/repository. On restart, the system replays from WAL and
//! re-runs the write sequence. WriteReplay.write() returns false for the
//! already-existing block (ignored by Sequencer), while WriteState and
//! WriteRepository get their data.
//!
//! These tests simulate this scenario using mock stores.

use crate::mocks::{
    make_block_output, make_replay_record, MockReplayStorage, MockRepository, MockWriteState,
};
use alloy::primitives::B256;
use alloy::consensus::Sealed;
use zksync_os_storage_api::{ReadReplay, WriteReplay, WriteRepository, WriteState};

/// Simulates the Sequencer's three-store write loop for a single block.
/// Returns Ok(()) if all three writes succeed, Err if any fails after WriteReplay.
fn simulate_sequencer_write(
    block_number: u64,
    timestamp: u64,
    replay: &MockReplayStorage,
    state: &MockWriteState,
    repo: &MockRepository,
    override_allowed: bool,
) -> anyhow::Result<()> {
    let replay_record = make_replay_record(block_number, timestamp);
    let block_output = make_block_output(block_number, timestamp);

    // Step 1: WriteReplay — fire-and-forget (Sequencer ignores return value)
    let _written = replay.write(
        Sealed::new_unchecked(replay_record.clone(), block_output.header.hash()),
        override_allowed,
    );

    // Step 2: WriteState — error propagated
    state.add_block_result(
        block_number,
        block_output.storage_writes.clone(),
        block_output
            .published_preimages
            .iter()
            .map(|(k, v)| (*k, v)),
        override_allowed,
    )?;

    // Step 3: WriteRepository — error propagated
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        repo.populate(block_output, replay_record.transactions.clone())
            .await
    })?;

    Ok(())
}

/// If WriteState fails mid-sequence, the WAL has the block but state and repo don't.
/// After "restart" (clearing the failure and re-running), all stores converge.
#[test]
fn crash_after_wal_write_recovers_on_replay() {
    let replay = MockReplayStorage::new().with_genesis();
    let state = MockWriteState::new().with_fail_on_block(3);
    let repo = MockRepository::new();

    // Write blocks 1, 2 successfully
    for i in 1..=2 {
        simulate_sequencer_write(i, 1000 + i, &replay, &state, &repo, false).unwrap();
    }

    // Block 3: WriteReplay succeeds, WriteState fails
    let result = simulate_sequencer_write(3, 1003, &replay, &state, &repo, false);
    assert!(result.is_err(), "Block 3 should fail at WriteState");

    // Verify partial state: WAL has block 3, but state and repo don't
    assert_eq!(replay.latest_record(), 3, "WAL should have block 3");
    assert!(
        replay.get_replay_record(3).is_some(),
        "Block 3 should be in WAL"
    );
    assert_eq!(
        state.write_log().len(),
        2,
        "State should only have blocks 1-2"
    );
    assert_eq!(
        repo.populated_blocks().len(),
        2,
        "Repo should only have blocks 1-2"
    );

    // --- Simulate restart ---
    // Clear failure (machine recovered)
    state.clear_failure();

    // Recovery: replay block 3 from WAL.
    // The Sequencer ignores WriteReplay's return value, so it proceeds to WriteState.
    // On main node, Replay commands use override_allowed=false.
    let result = simulate_sequencer_write(3, 1003, &replay, &state, &repo, false);
    assert!(result.is_ok(), "Recovery write should succeed");

    // WriteReplay returned false (duplicate, no override), but was ignored.
    // The block is not re-written to WAL (still block 3).
    assert_eq!(replay.latest_record(), 3);

    // State and repo now have block 3
    let state_log = state.write_log();
    assert_eq!(state_log.len(), 3, "State should now have blocks 1-3");
    assert_eq!(state_log[2], (3, false));

    let repo_blocks = repo.populated_blocks();
    assert_eq!(repo_blocks.len(), 3, "Repo should now have blocks 1-3");
    assert_eq!(repo_blocks[2], 3);
}

/// Recovery replays multiple blocks when crash happens early in a sequence.
/// Blocks already in all three stores are idempotently re-processed.
#[test]
fn recovery_replays_multiple_blocks_idempotently() {
    let replay = MockReplayStorage::new().with_genesis();
    let state = MockWriteState::new().with_fail_on_block(2);
    let repo = MockRepository::new();

    // Block 1 succeeds in all stores
    simulate_sequencer_write(1, 1001, &replay, &state, &repo, false).unwrap();

    // Block 2: WAL succeeds, state fails
    let result = simulate_sequencer_write(2, 1002, &replay, &state, &repo, false);
    assert!(result.is_err());

    assert_eq!(replay.latest_record(), 2);
    assert_eq!(state.write_log().len(), 1); // only block 1
    assert_eq!(repo.populated_blocks().len(), 1); // only block 1

    // --- Recovery: replay blocks 1 and 2 ---
    state.clear_failure();

    // Re-run block 1: WriteReplay returns false (already exists), state and repo
    // get duplicate writes (idempotent in real implementation).
    simulate_sequencer_write(1, 1001, &replay, &state, &repo, false).unwrap();

    // Re-run block 2: now succeeds
    simulate_sequencer_write(2, 1002, &replay, &state, &repo, false).unwrap();

    // Verify convergence
    assert_eq!(replay.latest_record(), 2);
    let state_log = state.write_log();
    // Block 1 written twice (original + recovery), block 2 once (recovery only)
    assert_eq!(state_log.len(), 3);
    assert_eq!(state_log[0], (1, false));
    assert_eq!(state_log[1], (1, false)); // idempotent re-write
    assert_eq!(state_log[2], (2, false));

    let repo_blocks = repo.populated_blocks();
    assert_eq!(repo_blocks.len(), 3); // block 1 twice, block 2 once
}

/// Rebuild commands use override_allowed=true, which allows WriteReplay to overwrite.
/// This simulates the recovery path for Rebuild commands.
#[test]
fn rebuild_recovery_uses_override() {
    let replay = MockReplayStorage::new().with_genesis();
    let state = MockWriteState::new();
    let repo = MockRepository::new();

    // Write block 1 normally
    simulate_sequencer_write(1, 1001, &replay, &state, &repo, false).unwrap();

    // Rebuild block 1 with override_allowed=true
    simulate_sequencer_write(1, 1001, &replay, &state, &repo, true).unwrap();

    // Both writes should be in the log
    let state_log = state.write_log();
    assert_eq!(state_log.len(), 2);
    assert_eq!(state_log[0], (1, false));
    assert_eq!(state_log[1], (1, true)); // override_allowed=true for Rebuild

    // Replay storage also got the override write
    let replay_log = replay.write_log();
    assert_eq!(replay_log.len(), 2);
    assert_eq!(replay_log[0], (1, false));
    assert_eq!(replay_log[1], (1, true));
}

/// WriteReplay returning false does not block recovery.
/// This is the critical property: the Sequencer ignores WriteReplay's return value.
#[test]
fn wal_duplicate_write_does_not_block_downstream_stores() {
    let replay = MockReplayStorage::new().with_genesis();
    let state = MockWriteState::new();
    let repo = MockRepository::new();

    // Write block 1 to WAL only (simulate by writing directly)
    let record = make_replay_record(1, 1001);
    replay.write(
        Sealed::new_unchecked(record, B256::from([1u8; 32])),
        false,
    );
    assert_eq!(replay.latest_record(), 1);
    assert!(state.write_log().is_empty(), "State should be empty");

    // Now simulate the Sequencer re-processing block 1 during recovery.
    // WriteReplay.write() returns false (already exists), but the Sequencer
    // ignores this and proceeds to WriteState and WriteRepository.
    let result = simulate_sequencer_write(1, 1001, &replay, &state, &repo, false);
    assert!(result.is_ok());

    // State and repo now have the block
    assert_eq!(state.write_log().len(), 1);
    assert_eq!(repo.populated_blocks(), vec![1]);
}
