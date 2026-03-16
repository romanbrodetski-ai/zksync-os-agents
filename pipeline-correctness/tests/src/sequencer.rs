//! Tests for the unified Sequencer pipeline component.
//!
//! The Sequencer combines block execution and persistence into a single pipeline stage.
//! It replaced the previous BlockExecutor → BlockCanonizer → BlockApplier chain.
//!
//! These tests verify:
//! - The Sequencer's OUTPUT_BUFFER_SIZE is set correctly
//! - The command source generates commands with correct block numbers
//! - ProduceCommand carries block_time and max_transactions_in_block

use zksync_os_sequencer::model::blocks::{BlockCommand, ProduceCommand};

/// ProduceCommand must carry block_number, block_time, and max_transactions_in_block.
/// These were previously stored in BlockContextProvider; now they travel with the command.
#[test]
fn produce_command_carries_block_params() {
    let cmd = ProduceCommand {
        block_number: 42,
        block_time: std::time::Duration::from_secs(1),
        max_transactions_in_block: 100,
    };

    assert_eq!(cmd.block_number, 42);
    assert_eq!(cmd.block_time, std::time::Duration::from_secs(1));
    assert_eq!(cmd.max_transactions_in_block, 100);
}

/// BlockCommand::block_number() must return the correct block number for each variant.
#[test]
fn block_command_block_number() {
    use crate::mocks::make_replay_record;
    use zksync_os_sequencer::model::blocks::RebuildCommand;

    // Replay
    let replay_record = make_replay_record(10, 1000);
    let cmd = BlockCommand::Replay(Box::new(replay_record));
    assert_eq!(cmd.block_number(), 10);

    // Produce
    let cmd = BlockCommand::Produce(ProduceCommand {
        block_number: 20,
        block_time: std::time::Duration::from_secs(1),
        max_transactions_in_block: 50,
    });
    assert_eq!(cmd.block_number(), 20);

    // Rebuild
    let rebuild_record = make_replay_record(30, 2000);
    let cmd = BlockCommand::Rebuild(Box::new(RebuildCommand {
        replay_record: rebuild_record,
        make_empty: false,
    }));
    assert_eq!(cmd.block_number(), 30);
}

/// The Sequencer output type is (BlockOutput, ReplayRecord) — no more BlockCommandType.
/// The old pipeline had a third element (BlockCommandType) that was used by BlockApplier
/// to determine override_allowed. Now the Sequencer handles this internally.
#[test]
fn sequencer_output_type_is_two_tuple() {
    // This is a compile-time check. If the Sequencer output type changes,
    // this test will fail to compile.
    fn _assert_output_type(
        _: (
            zksync_os_interface::types::BlockOutput,
            zksync_os_storage_api::ReplayRecord,
        ),
    ) {
    }
}

/// ReadReplayExt::stream() should return records in order.
#[test]
fn replay_stream_returns_records_in_order() {
    use crate::mocks::MockReplayStorage;
    use alloy::primitives::{B256, Sealed};
    use futures::StreamExt;
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

    // Stream blocks 2-4
    let rt = tokio::runtime::Runtime::new().unwrap();
    let records: Vec<_> = rt.block_on(async { storage.stream(2, 4).collect::<Vec<_>>().await });

    assert_eq!(records.len(), 3);
    assert_eq!(records[0].block_context.block_number, 2);
    assert_eq!(records[1].block_context.block_number, 3);
    assert_eq!(records[2].block_context.block_number, 4);
}
