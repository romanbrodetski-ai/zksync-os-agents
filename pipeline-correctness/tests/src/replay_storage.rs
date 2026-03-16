//! Tests for the WriteReplay contract — the source of truth for canonical blocks.
//!
//! WriteReplay has specific documented invariants about ordering, atomicity,
//! and override behavior. These tests exhaustively verify those contracts
//! using the mock implementation (which mirrors the expected behavior).
//!
//! When these tests fail, it means either:
//! - The mock diverged from real behavior (update the mock)
//! - The WriteReplay contract changed (update the knowledge doc)
//! - A bug was introduced in a WriteReplay implementation

use crate::mocks::{make_replay_record, MockReplayStorage};
use alloy::primitives::B256;
use alloy::consensus::Sealed;
use zksync_os_storage_api::{ReadReplay, WriteReplay};

/// Genesis block (0) should be writable and become latest.
#[test]
fn write_genesis_block() {
    let storage = MockReplayStorage::new();
    let record = make_replay_record(0, 1000);
    let hash = B256::ZERO;

    let written = storage.write(Sealed::new_unchecked(record.clone(), hash), false);
    assert!(written);
    assert_eq!(storage.latest_record(), 0);

    let retrieved = storage.get_replay_record(0).unwrap();
    assert_eq!(retrieved.block_context.block_number, 0);
}

/// Blocks must be written sequentially. Block N+1 can only be written after block N.
#[test]
fn sequential_writes_succeed() {
    let storage = MockReplayStorage::new().with_genesis();

    for i in 1..=10 {
        let record = make_replay_record(i, 1000 + i);
        let written = storage.write(
            Sealed::new_unchecked(record, B256::from([i as u8; 32])),
            false,
        );
        assert!(written, "Block {i} should be writable");
        assert_eq!(storage.latest_record(), i);
    }
}

/// Writing the same block number twice without override should return false.
#[test]
fn duplicate_write_without_override_returns_false() {
    let storage = MockReplayStorage::new().with_genesis();

    let record = make_replay_record(1, 1001);
    let written = storage.write(Sealed::new_unchecked(record.clone(), B256::ZERO), false);
    assert!(written);

    // Write same block again
    let written = storage.write(Sealed::new_unchecked(record, B256::ZERO), false);
    assert!(!written, "Duplicate write should return false");
}

/// Writing the same block number with override_allowed should succeed.
#[test]
fn duplicate_write_with_override_succeeds() {
    let storage = MockReplayStorage::new().with_genesis();

    let record = make_replay_record(1, 1001);
    let written = storage.write(Sealed::new_unchecked(record.clone(), B256::ZERO), false);
    assert!(written);

    // Write same block with override
    let mut updated_record = make_replay_record(1, 1001);
    updated_record.block_output_hash = B256::from([0xFF; 32]);
    let written = storage.write(
        Sealed::new_unchecked(updated_record.clone(), B256::ZERO),
        true,
    );
    assert!(written, "Override write should succeed");

    // Verify the new record replaced the old one
    let retrieved = storage.get_replay_record(1).unwrap();
    assert_eq!(retrieved.block_output_hash, B256::from([0xFF; 32]));
}

/// Writing a non-sequential block should panic (contract violation).
#[test]
#[should_panic(expected = "not next after latest")]
fn non_sequential_write_panics() {
    let storage = MockReplayStorage::new().with_genesis();

    // Try to write block 5 when latest is 0 — should panic
    let record = make_replay_record(5, 1005);
    storage.write(Sealed::new_unchecked(record, B256::ZERO), false);
}

/// All blocks in range [0, latest] must be retrievable.
#[test]
fn all_blocks_in_range_retrievable() {
    let storage = MockReplayStorage::new().with_genesis();

    for i in 1..=20 {
        let record = make_replay_record(i, 1000 + i);
        storage.write(
            Sealed::new_unchecked(record, B256::from([i as u8; 32])),
            false,
        );
    }

    // Every block from 0 to 20 must be readable
    for i in 0..=20 {
        let record = storage.get_replay_record(i);
        assert!(
            record.is_some(),
            "Block {i} should be retrievable"
        );
        assert_eq!(record.unwrap().block_context.block_number, i);
    }
}

/// get_context should return Some when get_replay_record returns Some.
#[test]
fn get_context_consistent_with_get_replay_record() {
    let storage = MockReplayStorage::new().with_genesis();

    let record = make_replay_record(1, 1001);
    storage.write(
        Sealed::new_unchecked(record, B256::ZERO),
        false,
    );

    // Both methods should agree
    assert!(storage.get_replay_record(1).is_some());
    assert!(storage.get_context(1).is_some());

    let ctx = storage.get_context(1).unwrap();
    let rec = storage.get_replay_record(1).unwrap();
    assert_eq!(ctx.block_number, rec.block_context.block_number);
    assert_eq!(ctx.timestamp, rec.block_context.timestamp);
}

/// latest_record must be monotonically non-decreasing.
#[test]
fn latest_record_monotonic() {
    let storage = MockReplayStorage::new().with_genesis();

    let mut prev_latest = storage.latest_record();

    for i in 1..=10 {
        let record = make_replay_record(i, 1000 + i);
        storage.write(
            Sealed::new_unchecked(record, B256::ZERO),
            false,
        );

        let current_latest = storage.latest_record();
        assert!(
            current_latest >= prev_latest,
            "latest_record decreased from {prev_latest} to {current_latest}"
        );
        prev_latest = current_latest;
    }
}

/// The write_log should accurately capture all writes with their override status.
/// This is useful for verifying that BlockApplier passes correct override flags.
#[test]
fn write_log_tracks_operations() {
    let storage = MockReplayStorage::new().with_genesis();

    // Write blocks 1-3 without override
    for i in 1..=3 {
        let record = make_replay_record(i, 1000 + i);
        storage.write(Sealed::new_unchecked(record, B256::ZERO), false);
    }

    // Override block 3
    let record = make_replay_record(3, 1003);
    storage.write(Sealed::new_unchecked(record, B256::ZERO), true);

    let log = storage.write_log();
    assert_eq!(log.len(), 4);
    assert_eq!(log[0], (1, false));
    assert_eq!(log[1], (2, false));
    assert_eq!(log[2], (3, false));
    assert_eq!(log[3], (3, true));
}
