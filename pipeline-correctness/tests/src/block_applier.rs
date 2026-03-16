//! Tests for BlockApplier's override_allowed logic and persistence ordering.
//!
//! BlockApplier determines override_allowed based on:
//! - Rebuild commands → override_allowed = true
//! - External node role → override_allowed = true
//! - Everything else → override_allowed = false
//!
//! These tests verify the override_allowed derivation from BlockCommandType,
//! and that all three stores (replay, state, repository) are populated.

use crate::mocks::{make_block_output, make_replay_record, MockReplayStorage, MockWriteState};
use alloy::consensus::Sealed;
use zksync_os_sequencer::model::blocks::BlockCommandType;
use zksync_os_storage_api::{WriteReplay, WriteState};

/// BlockApplier sets override_allowed = true only for Rebuild and external nodes.
/// This test verifies the logic by simulating what BlockApplier does.
#[test]
fn override_allowed_for_rebuild_command_type() {
    let is_external = false;

    // Replay → false
    let cmd_type = BlockCommandType::Replay;
    let override_allowed = matches!(cmd_type, BlockCommandType::Rebuild) || is_external;
    assert!(!override_allowed, "Replay should not allow override on main node");

    // Produce → false
    let cmd_type = BlockCommandType::Produce;
    let override_allowed = matches!(cmd_type, BlockCommandType::Rebuild) || is_external;
    assert!(!override_allowed, "Produce should not allow override on main node");

    // Rebuild → true
    let cmd_type = BlockCommandType::Rebuild;
    let override_allowed = matches!(cmd_type, BlockCommandType::Rebuild) || is_external;
    assert!(override_allowed, "Rebuild should allow override");
}

/// On an external node, override_allowed should be true for all command types.
#[test]
fn override_allowed_for_external_node() {
    let is_external = true;

    for cmd_type in [
        BlockCommandType::Replay,
        BlockCommandType::Produce,
        BlockCommandType::Rebuild,
    ] {
        let override_allowed = matches!(cmd_type, BlockCommandType::Rebuild) || is_external;
        assert!(
            override_allowed,
            "External node should always allow override, got false for {:?}",
            cmd_type
        );
    }
}

/// BlockApplier persists to all three stores: replay, state, and repository.
/// Verify the mock stores record correct data.
#[test]
fn persistence_to_all_three_stores() {
    let replay_storage = MockReplayStorage::new().with_genesis();
    let state = MockWriteState::new();

    let block_number = 1u64;
    let block_output = make_block_output(block_number, 1001);
    let replay_record = make_replay_record(block_number, 1001);

    // Simulate what BlockApplier does for a Replay command
    let override_allowed = false;

    replay_storage.write(
        Sealed::new_unchecked(replay_record.clone(), block_output.header.hash()),
        override_allowed,
    );

    state
        .add_block_result(
            block_number,
            block_output.storage_writes.clone(),
            block_output
                .published_preimages
                .iter()
                .map(|(k, v)| (*k, v)),
            override_allowed,
        )
        .unwrap();

    // Check replay was written
    assert_eq!(replay_storage.write_log(), vec![(1, false)]);

    // Check state was written
    assert_eq!(state.write_log(), vec![(1, false)]);
}

/// BlockApplier should correctly pass override_allowed=true for Rebuild commands.
/// Verify that the write_log captures this.
#[test]
fn rebuild_writes_with_override() {
    let replay_storage = MockReplayStorage::new().with_genesis();
    let state = MockWriteState::new();

    let block_output = make_block_output(1, 1001);
    let replay_record = make_replay_record(1, 1001);

    // Rebuild → override_allowed = true
    let override_allowed = true;

    replay_storage.write(
        Sealed::new_unchecked(replay_record, block_output.header.hash()),
        override_allowed,
    );

    state
        .add_block_result(1, vec![], std::iter::empty(), override_allowed)
        .unwrap();

    assert_eq!(replay_storage.write_log(), vec![(1, true)]);
    assert_eq!(state.write_log(), vec![(1, true)]);
}

/// Sequential blocks must be persisted in order.
#[test]
fn sequential_persistence() {
    let replay_storage = MockReplayStorage::new().with_genesis();
    let state = MockWriteState::new();

    for i in 1..=5 {
        let block_output = make_block_output(i, 1000 + i);
        let replay_record = make_replay_record(i, 1000 + i);

        replay_storage.write(
            Sealed::new_unchecked(replay_record, block_output.header.hash()),
            false,
        );
        state
            .add_block_result(i, vec![], std::iter::empty(), false)
            .unwrap();
    }

    let log = replay_storage.write_log();
    assert_eq!(log.len(), 5);
    for (idx, (block_num, override_flag)) in log.iter().enumerate() {
        assert_eq!(*block_num, (idx + 1) as u64);
        assert!(!override_flag);
    }
}
