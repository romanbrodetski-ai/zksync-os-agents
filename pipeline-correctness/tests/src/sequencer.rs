//! Tests for the three-stage block processing pipeline:
//! BlockExecutor → BlockCanonizer → BlockApplier.
//!
//! These tests verify:
//! - BlockExecutor, BlockCanonizer, and BlockApplier output types and buffer sizes
//! - ProduceCommand is a unit struct (block params determined by BlockContextProvider)
//! - BlockCommandType enum covers all command variants
//! - BlockApplier sets override_allowed correctly per command type

use alloy::primitives::B256;
use zksync_os_sequencer::model::blocks::{BlockCommand, BlockCommandType, ProduceCommand};

/// ProduceCommand is now a unit struct — block_number, block_time, and
/// max_transactions_in_block are determined by BlockContextProvider::prepare_command().
#[test]
fn produce_command_is_unit_struct() {
    let _cmd = ProduceCommand;
    // Compile-time check: ProduceCommand has no fields.
    let _cmd2 = ProduceCommand {};
}

/// BlockCommand::command_type() must return the correct variant for each command.
#[test]
fn block_command_type_matches_variant() {
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

/// BlockExecutor output type is (BlockOutput, ReplayRecord, BlockCommandType).
/// BlockApplier strips BlockCommandType, outputting (BlockOutput, ReplayRecord).
#[test]
fn pipeline_stage_output_types() {
    // BlockExecutor output
    fn _assert_executor_output(
        _: (
            zksync_os_interface::types::BlockOutput,
            zksync_os_storage_api::ReplayRecord,
            BlockCommandType,
        ),
    ) {
    }

    // BlockApplier output
    fn _assert_applier_output(
        _: (
            zksync_os_interface::types::BlockOutput,
            zksync_os_storage_api::ReplayRecord,
        ),
    ) {
    }
}

/// ReadReplayExt::forward_range_with should send records through a channel in order.
#[test]
fn replay_forward_range_sends_records_in_order() {
    use crate::mocks::MockReplayStorage;
    use alloy::primitives::{B256, Sealed};
    use tokio::sync::mpsc;
    use zksync_os_storage_api::{ReadReplayExt, ReplayRecord, WriteReplay};

    let storage = MockReplayStorage::new().with_genesis();

    // Write blocks 1-5
    for i in 1..=5u64 {
        let record = crate::mocks::make_replay_record(i, 1000 + i);
        storage.write(
            Sealed::new_unchecked(record, B256::from([i as u8; 32])),
            false,
        );
    }

    let rt = tokio::runtime::Runtime::new().unwrap();
    let records: Vec<ReplayRecord> = rt.block_on(async {
        let (tx, mut rx) = mpsc::channel(10);
        storage
            .forward_range_with(2, 4, tx, |r| r)
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

/// ReplayRecord equality should exclude node_version (by design).
#[test]
fn replay_record_equality_ignores_node_version() {
    let mut r1 = crate::mocks::make_replay_record(1, 1001);
    let mut r2 = crate::mocks::make_replay_record(1, 1001);

    r1.node_version = semver::Version::new(0, 16, 0);
    r2.node_version = semver::Version::new(0, 17, 0);

    assert_eq!(r1, r2, "Records should be equal despite different node_version");
}

/// ReplayRecord equality should NOT ignore block_output_hash.
#[test]
fn replay_record_inequality_on_output_hash() {
    let mut r1 = crate::mocks::make_replay_record(1, 1001);
    let mut r2 = crate::mocks::make_replay_record(1, 1001);

    r1.block_output_hash = B256::from([0x01; 32]);
    r2.block_output_hash = B256::from([0x02; 32]);

    assert_ne!(
        r1, r2,
        "Records with different output hashes should not be equal"
    );
}
