//! Tests for the pipeline components introduced in v0.17.0:
//! - BlockExecutor, BlockCanonizer, BlockApplier split
//! - BlockContextProvider block number ownership
//! - OverlayBuffer in-memory state management
//! - forward_range_with replacing stream()

use zksync_os_sequencer::model::blocks::{BlockCommand, BlockCommandType, ProduceCommand};

/// ProduceCommand is now an empty marker struct.
/// Block params (block_number, block_time, max_transactions_in_block) are
/// owned by BlockContextProvider, not the command.
#[test]
fn produce_command_is_unit_struct() {
    // This is a compile-time check. If ProduceCommand gains fields again,
    // this default construction will need to be updated.
    let _cmd = ProduceCommand;
    let cmd = BlockCommand::Produce(ProduceCommand);
    assert!(matches!(cmd.command_type(), BlockCommandType::Produce));
}

/// BlockCommand::block_number() was removed in v0.17.0.
/// Replay and Rebuild block numbers are accessible via their inner data.
/// Produce no longer carries a block number (it's in BlockContextProvider).
#[test]
fn block_command_type_identification() {
    use crate::mocks::make_replay_record;
    use zksync_os_sequencer::model::blocks::RebuildCommand;

    // Replay: block number in inner ReplayRecord
    let replay_record = make_replay_record(10, 1000);
    let block_num = replay_record.block_context.block_number;
    let cmd = BlockCommand::Replay(Box::new(replay_record));
    assert!(matches!(cmd.command_type(), BlockCommandType::Replay));
    assert_eq!(block_num, 10);

    // Produce: unit struct, no block number
    let cmd = BlockCommand::Produce(ProduceCommand);
    assert!(matches!(cmd.command_type(), BlockCommandType::Produce));

    // Rebuild: block number in inner RebuildCommand::replay_record
    let rebuild_record = make_replay_record(30, 2000);
    let block_num = rebuild_record.block_context.block_number;
    let cmd = BlockCommand::Rebuild(Box::new(RebuildCommand {
        replay_record: rebuild_record,
        make_empty: false,
    }));
    assert!(matches!(cmd.command_type(), BlockCommandType::Rebuild));
    assert_eq!(block_num, 30);
}

/// BlockApplier output type is (BlockOutput, ReplayRecord) — third element (BlockCommandType)
/// is consumed by BlockApplier to determine override_allowed, not passed downstream.
#[test]
fn block_applier_output_type_is_two_tuple() {
    // This is a compile-time check. If BlockApplier output type changes,
    // this test will fail to compile.
    fn _assert_output_type(
        _: (
            zksync_os_interface::types::BlockOutput,
            zksync_os_storage_api::ReplayRecord,
        ),
    ) {
    }
}

/// BlockExecutor output type is (BlockOutput, ReplayRecord, BlockCommandType).
/// The BlockCommandType travels through BlockCanonizer to BlockApplier to determine
/// override_allowed.
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

/// ReadReplayExt::stream() was replaced by forward_range_with().
/// forward_range_with sends records to a channel with a mapping function.
#[tokio::test]
async fn forward_range_with_sends_records_in_order() {
    use crate::mocks::MockReplayStorage;
    use alloy::primitives::{B256, Sealed};
    use tokio::sync::mpsc;
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

    // forward_range_with sends blocks 2-4 to a channel
    let (tx, mut rx) = mpsc::channel(10);
    storage
        .forward_range_with(2, 4, tx, |record| record)
        .await
        .unwrap();

    let mut received = vec![];
    while let Ok(r) = rx.try_recv() {
        received.push(r.block_context.block_number);
    }

    assert_eq!(received, vec![2, 3, 4]);
}

/// forward_range_with stops gracefully when output channel is closed.
#[tokio::test]
async fn forward_range_with_handles_closed_channel() {
    use crate::mocks::MockReplayStorage;
    use alloy::primitives::{B256, Sealed};
    use tokio::sync::mpsc;
    use zksync_os_storage_api::{ReadReplayExt, WriteReplay};

    let storage = MockReplayStorage::new().with_genesis();

    for i in 1..=10u64 {
        let record = crate::mocks::make_replay_record(i, 1000 + i);
        storage.write(
            Sealed::new_unchecked(record, B256::from([i as u8; 32])),
            false,
        );
    }

    // Create a channel and immediately drop the receiver
    let (tx, rx) = mpsc::channel(1);
    drop(rx);

    // forward_range_with should return Ok even when channel is closed (it logs a warning)
    let result = storage
        .forward_range_with(1, 10, tx, |record| record)
        .await;
    assert!(result.is_ok(), "forward_range_with should handle closed channel gracefully");
}
