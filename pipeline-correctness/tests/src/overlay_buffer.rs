//! Tests for OverlayBuffer — the in-memory state overlay used by BlockExecutor.
//!
//! OverlayBuffer bridges the gap between block execution and persistence. It maintains
//! storage writes and preimages for blocks that haven't been persisted yet, allowing
//! BlockExecutor to run ahead of BlockApplier.
//!
//! These tests verify:
//! - Contiguous block number enforcement
//! - add_block succeeds for valid sequences

use alloy::primitives::{Address, B256};
use zksync_os_interface::types::StorageWrite;
use zksync_os_storage_api::OverlayBuffer;

fn storage_write(key_n: u8, val_n: u8) -> StorageWrite {
    StorageWrite {
        key: B256::from([key_n; 32]),
        value: B256::from([val_n; 32]),
        account: Address::ZERO,
        account_key: B256::ZERO,
    }
}

/// Adding blocks must be contiguous. Block N+1 can only follow block N.
#[test]
fn add_block_enforces_contiguity() {
    let mut buffer = OverlayBuffer::default();
    buffer.add_block(1, vec![], vec![]).unwrap();
    buffer.add_block(2, vec![], vec![]).unwrap();

    // Skipping block 3 should fail
    let result = buffer.add_block(4, vec![], vec![]);
    assert!(result.is_err(), "Non-contiguous block should fail");
}

/// Adding the first block to an empty buffer should always succeed.
#[test]
fn add_first_block_to_empty_buffer() {
    let mut buffer = OverlayBuffer::default();
    // Any starting block number should work
    buffer.add_block(42, vec![], vec![]).unwrap();
    buffer.add_block(43, vec![], vec![]).unwrap();
}

/// Adding blocks with storage writes and preimages should succeed.
#[test]
fn add_block_with_data() {
    let mut buffer = OverlayBuffer::default();

    buffer
        .add_block(
            1,
            vec![storage_write(1, 10), storage_write(2, 20)],
            vec![(B256::from([1u8; 32]), vec![0xAA; 10])],
        )
        .unwrap();

    buffer
        .add_block(
            2,
            vec![storage_write(1, 30)], // overwrite key 1
            vec![(B256::from([2u8; 32]), vec![0xBB; 20])],
        )
        .unwrap();
}

/// Duplicate block numbers should fail (not contiguous).
#[test]
fn duplicate_block_number_fails() {
    let mut buffer = OverlayBuffer::default();
    buffer.add_block(1, vec![], vec![]).unwrap();

    let result = buffer.add_block(1, vec![], vec![]);
    assert!(result.is_err(), "Duplicate block number should fail");
}

/// Adding a block with number less than the last should fail.
#[test]
fn backwards_block_number_fails() {
    let mut buffer = OverlayBuffer::default();
    buffer.add_block(5, vec![], vec![]).unwrap();

    let result = buffer.add_block(3, vec![], vec![]);
    assert!(result.is_err(), "Backwards block number should fail");
}
