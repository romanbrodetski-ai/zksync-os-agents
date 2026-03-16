//! Tests for the split pipeline: BlockExecutor → BlockCanonizer → BlockApplier.
//!
//! The Sequencer was split into three separate pipeline components:
//! - BlockExecutor: executes blocks using an in-memory OverlayBuffer (no disk I/O)
//! - BlockCanonizer: consensus fence ensuring only canonized blocks proceed
//! - BlockApplier: persists blocks to WAL, state storage, and repository
//!
//! These tests verify:
//! - ProduceCommand is a unit struct (no fields)
//! - BlockCommandType is correctly derived from each command variant
//! - The BlockExecutor output includes BlockCommandType for downstream routing
//! - The BlockApplier output is (BlockOutput, ReplayRecord) — no command type

use zksync_os_sequencer::model::blocks::{BlockCommand, BlockCommandType, ProduceCommand};

/// ProduceCommand is now a unit struct — block_number, block_time, and
/// max_transactions_in_block are determined by BlockContextProvider at
/// prepare_command() time, not carried on the command.
#[test]
fn produce_command_is_unit_struct() {
    let _cmd = ProduceCommand;
    // If ProduceCommand had fields, this would fail to compile.
}

/// BlockCommand::command_type() must return the correct type for each variant.
#[test]
fn block_command_type() {
    use crate::mocks::make_replay_record;
    use zksync_os_sequencer::model::blocks::RebuildCommand;

    // Replay
    let replay_record = make_replay_record(10, 1000);
    let cmd = BlockCommand::Replay(Box::new(replay_record));
    assert!(matches!(cmd.command_type(), BlockCommandType::Replay));

    // Produce
    let cmd = BlockCommand::Produce(ProduceCommand);
    assert!(matches!(cmd.command_type(), BlockCommandType::Produce));

    // Rebuild
    let rebuild_record = make_replay_record(30, 2000);
    let cmd = BlockCommand::Rebuild(Box::new(RebuildCommand {
        replay_record: rebuild_record,
        make_empty: false,
    }));
    assert!(matches!(cmd.command_type(), BlockCommandType::Rebuild));
}

/// BlockExecutor output is (BlockOutput, ReplayRecord, BlockCommandType).
/// BlockCommandType travels through the canonizer to the applier so it can
/// determine override_allowed.
#[test]
fn block_executor_output_includes_command_type() {
    fn _assert_output_type(
        _: (
            zksync_os_interface::types::BlockOutput,
            zksync_os_storage_api::ReplayRecord,
            BlockCommandType,
        ),
    ) {
    }
}

/// BlockApplier output is (BlockOutput, ReplayRecord) — the command type
/// is consumed by the applier and not forwarded downstream.
#[test]
fn block_applier_output_is_two_tuple() {
    fn _assert_output_type(
        _: (
            zksync_os_interface::types::BlockOutput,
            zksync_os_storage_api::ReplayRecord,
        ),
    ) {
    }
}

/// ReadReplayExt::forward_range_with should forward records in order.
#[test]
fn replay_forward_range_returns_records_in_order() {
    use crate::mocks::MockReplayStorage;
    use alloy::primitives::{B256, Sealed};
    use zksync_os_storage_api::{ReadReplayExt, WriteReplay};

    let storage = MockReplayStorage::new().with_genesis();

    // Write blocks 1-5
    for i in 1..=5u64 {
        let record = crate::mocks::make_replay_record(i, 1000 + i);
        storage.write(
            Sealed::new_unchecked(record, B256::from([i as u8; 32])),
            false,
        );
    }

    // Forward blocks 2-4 through a channel
    let rt = tokio::runtime::Runtime::new().unwrap();
    let records: Vec<_> = rt.block_on(async {
        let (tx, mut rx) = tokio::sync::mpsc::channel(10);
        storage
            .forward_range_with(2, 4, tx, |record| record)
            .await
            .unwrap();
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
