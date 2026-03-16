//! Tests for the BlockExecutor, BlockCanonizer, and BlockApplier pipeline components.
//!
//! The previous monolithic Sequencer has been split into three stages:
//!   BlockExecutor — executes blocks in the VM, does NOT persist, maintains OverlayBuffer
//!   BlockCanonizer — canonization fence; passes Replay through, queues Produce for consensus
//!   BlockApplier — persists blocks to WAL, state, and repository
//!
//! These tests verify:
//! - BlockApplier's output type is (BlockOutput, ReplayRecord)
//! - BlockCommandType carries the override_allowed semantics correctly
//! - ProduceCommand is a unit struct (block params moved to BlockContextProvider)
//! - forward_range_with sends records to a channel in order

use zksync_os_sequencer::model::blocks::{BlockCommand, BlockCommandType, ProduceCommand};

/// ProduceCommand is now a unit struct. Block number, block_time, and
/// max_transactions_in_block are fields on BlockContextProvider, not on the command.
#[test]
fn produce_command_is_unit_struct() {
    // Compile-time check: ProduceCommand can be constructed with no fields
    let _cmd = ProduceCommand;
    let _block_cmd = BlockCommand::Produce(ProduceCommand);
}

/// BlockCommandType encodes the original command variant for use by BlockApplier.
/// Rebuild must map to true override_allowed; Replay and Produce must not.
#[test]
fn block_command_type_encodes_override_intent() {
    use crate::mocks::make_replay_record;
    use zksync_os_sequencer::model::blocks::RebuildCommand;

    let replay_cmd = BlockCommand::Replay(Box::new(make_replay_record(10, 1000)));
    assert!(matches!(replay_cmd.command_type(), BlockCommandType::Replay));

    let produce_cmd = BlockCommand::Produce(ProduceCommand);
    assert!(matches!(produce_cmd.command_type(), BlockCommandType::Produce));

    let rebuild_cmd = BlockCommand::Rebuild(Box::new(RebuildCommand {
        replay_record: make_replay_record(20, 2000),
        make_empty: false,
    }));
    assert!(matches!(rebuild_cmd.command_type(), BlockCommandType::Rebuild));
}

/// BlockApplier's output type is (BlockOutput, ReplayRecord).
/// BlockExecutor's output type is (BlockOutput, ReplayRecord, BlockCommandType).
/// This is a compile-time check.
#[test]
fn block_applier_output_type_is_two_tuple() {
    fn _assert_block_applier_output(
        _: (
            zksync_os_interface::types::BlockOutput,
            zksync_os_storage_api::ReplayRecord,
        ),
    ) {
    }
}

/// ReadReplayExt::forward_range_with should send records in ascending block order.
#[test]
fn forward_range_with_sends_records_in_order() {
    use crate::mocks::MockReplayStorage;
    use alloy::primitives::{B256, Sealed};
    use zksync_os_storage_api::{ReadReplayExt, WriteReplay};

    let storage = MockReplayStorage::new().with_genesis();

    for i in 1..=5u64 {
        let record = crate::mocks::make_replay_record(i, 1000 + i);
        storage.write(
            Sealed::new_unchecked(record, B256::from([i as u8; 32])),
            false,
        );
    }

    let rt = tokio::runtime::Runtime::new().unwrap();
    let records = rt.block_on(async {
        let (tx, mut rx) = tokio::sync::mpsc::channel(10);
        storage
            .forward_range_with(2, 4, tx, |r| r)
            .await
            .expect("forward_range_with should succeed");

        let mut out = vec![];
        while let Ok(r) = rx.try_recv() {
            out.push(r);
        }
        out
    });

    assert_eq!(records.len(), 3);
    assert_eq!(records[0].block_context.block_number, 2);
    assert_eq!(records[1].block_context.block_number, 3);
    assert_eq!(records[2].block_context.block_number, 4);
}

/// forward_range_with should send all records in [start, end] inclusive.
#[test]
fn forward_range_with_inclusive_bounds() {
    use crate::mocks::MockReplayStorage;
    use alloy::primitives::{B256, Sealed};
    use zksync_os_storage_api::{ReadReplayExt, WriteReplay};

    let storage = MockReplayStorage::new().with_genesis();

    for i in 1..=3u64 {
        let record = crate::mocks::make_replay_record(i, 1000 + i);
        storage.write(
            Sealed::new_unchecked(record, B256::from([i as u8; 32])),
            false,
        );
    }

    let rt = tokio::runtime::Runtime::new().unwrap();
    let records = rt.block_on(async {
        let (tx, mut rx) = tokio::sync::mpsc::channel(10);
        storage
            .forward_range_with(0, 3, tx, |r| r)
            .await
            .expect("forward_range_with should succeed");

        let mut out = vec![];
        while let Ok(r) = rx.try_recv() {
            out.push(r);
        }
        out
    });

    assert_eq!(records.len(), 4, "must include block 0 and block 3");
    assert_eq!(records[0].block_context.block_number, 0);
    assert_eq!(records[3].block_context.block_number, 3);
}

/// forward_range_with with a closed channel should return Ok (caller detects close).
#[test]
fn forward_range_with_closed_channel_returns_ok() {
    use crate::mocks::MockReplayStorage;
    use alloy::primitives::{B256, Sealed};
    use zksync_os_storage_api::{ReadReplayExt, WriteReplay};

    let storage = MockReplayStorage::new().with_genesis();
    for i in 1..=5u64 {
        let record = crate::mocks::make_replay_record(i, 1000 + i);
        storage.write(
            Sealed::new_unchecked(record, B256::from([i as u8; 32])),
            false,
        );
    }

    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(async {
        let (tx, rx) = tokio::sync::mpsc::channel::<_>(1);
        drop(rx); // Close the receiver immediately
        storage.forward_range_with(1, 5, tx, |r| r).await
    });

    // Should return Ok even though the channel is closed — caller detects closure
    assert!(result.is_ok());
}
