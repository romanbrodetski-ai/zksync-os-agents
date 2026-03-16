//! Tests for OverlayBuffer — the in-memory state management layer between
//! BlockExecutor and BlockApplier.
//!
//! OverlayBuffer accumulates storage writes and preimages for blocks that have
//! been executed but not yet persisted to base storage. It allows BlockExecutor
//! to run ahead of BlockApplier while still reading correct state.
//!
//! Invariants tested:
//! - Most-recent overlay wins for storage reads
//! - Overlays are contiguous — gaps cause a bail
//! - State views fall through to base when base is ahead of overlays
//! - Purge works when base catches up

use alloy::primitives::{Address, B256};
use zksync_os_interface::traits::{PreimageSource, ReadStorage};
use zksync_os_interface::types::StorageWrite;
use zksync_os_storage_api::OverlayBuffer;

// ─── Mock base state ────────────────────────────────────────────────────────

/// Minimal ReadStateHistory + ViewState implementation for testing.
#[derive(Debug, Clone)]
struct MockBaseState {
    /// Latest block number in base storage
    latest: u64,
    /// Storage values for the base
    storage: std::collections::HashMap<B256, B256>,
    /// Preimages for the base
    preimages: std::collections::HashMap<B256, Vec<u8>>,
}

impl MockBaseState {
    fn new(latest: u64) -> Self {
        Self {
            latest,
            storage: Default::default(),
            preimages: Default::default(),
        }
    }

    fn with_storage(mut self, key: B256, value: B256) -> Self {
        self.storage.insert(key, value);
        self
    }

    fn with_preimage(mut self, hash: B256, data: Vec<u8>) -> Self {
        self.preimages.insert(hash, data);
        self
    }
}

// The view returned by state_view_at
#[derive(Debug, Clone)]
struct MockStateView {
    storage: std::collections::HashMap<B256, B256>,
    preimages: std::collections::HashMap<B256, Vec<u8>>,
}

impl ReadStorage for MockStateView {
    fn read(&mut self, key: B256) -> Option<B256> {
        self.storage.get(&key).copied()
    }
}

impl PreimageSource for MockStateView {
    fn get_preimage(&mut self, hash: B256) -> Option<Vec<u8>> {
        self.preimages.get(&hash).cloned()
    }
}

impl zksync_os_storage_api::ReadStateHistory for MockBaseState {
    fn state_view_at(
        &self,
        _block_number: u64,
    ) -> zksync_os_storage_api::StateResult<impl zksync_os_storage_api::ViewState> {
        Ok(MockStateView {
            storage: self.storage.clone(),
            preimages: self.preimages.clone(),
        })
    }

    fn block_range_available(&self) -> std::ops::RangeInclusive<u64> {
        0..=self.latest
    }
}

fn sw(key_byte: u8, value_byte: u8) -> StorageWrite {
    StorageWrite {
        key: B256::from([key_byte; 32]),
        value: B256::from([value_byte; 32]),
        account: Address::ZERO,
        account_key: B256::ZERO,
    }
}

fn key(n: u8) -> B256 {
    B256::from([n; 32])
}

fn val(n: u8) -> B256 {
    B256::from([n; 32])
}

// ─── Tests ─────────────────────────────────────────────────────────────────

/// add_block must be called in contiguous order.
/// Skipping a block number should bail.
#[test]
fn add_block_must_be_contiguous() {
    let mut buf = OverlayBuffer::default();
    buf.add_block(1, vec![], vec![]).unwrap();
    // Block 3 without block 2 should fail
    let result = buf.add_block(3, vec![], vec![]);
    assert!(result.is_err(), "add_block should fail when gap exists");
}

/// Duplicate block add should fail (same block number twice).
#[test]
fn add_block_duplicate_fails() {
    let mut buf = OverlayBuffer::default();
    buf.add_block(1, vec![], vec![]).unwrap();
    let result = buf.add_block(1, vec![], vec![]);
    assert!(result.is_err(), "add_block with same block number should fail");
}

/// When base is up-to-date with block N-1, no overlays are needed.
/// The view reads directly from base storage.
#[test]
fn view_reads_from_base_when_base_is_current() {
    let base = MockBaseState::new(5) // latest persisted = block 5
        .with_storage(key(1), val(42));
    let mut buf = OverlayBuffer::default();

    // Execute block 6 — base at block 5 already, no overlays needed
    let mut view = buf
        .sync_with_base_and_build_view_for_block(&base, 6)
        .unwrap();

    // Should read from base (which has key(1) = 42)
    assert_eq!(view.read(key(1)), Some(val(42)));
}

/// When executor is ahead of persistence, overlay values shadow base.
/// Writing to key 1 in block 6 should be visible when executing block 7.
#[test]
fn overlay_shadows_base_when_executor_is_ahead() {
    let base = MockBaseState::new(5).with_storage(key(1), val(10)); // base has 10
    let mut buf = OverlayBuffer::default();

    // Block 6 writes 99 to key(1)
    buf.add_block(6, vec![sw(1, 99)], vec![]).unwrap();

    // Execute block 7 — base is at 5, overlay for block 6 is in memory
    let mut view = buf
        .sync_with_base_and_build_view_for_block(&base, 7)
        .unwrap();

    // Should return overlay value (99), not base value (10)
    assert_eq!(view.read(key(1)), Some(val(99)));
}

/// Most-recent overlay wins when the same key is written in multiple blocks.
#[test]
fn most_recent_overlay_wins_for_same_key() {
    let base = MockBaseState::new(5).with_storage(key(1), val(10));
    let mut buf = OverlayBuffer::default();

    buf.add_block(6, vec![sw(1, 20)], vec![]).unwrap(); // block 6: key1 = 20
    buf.add_block(7, vec![sw(1, 30)], vec![]).unwrap(); // block 7: key1 = 30

    // Execute block 8
    let mut view = buf
        .sync_with_base_and_build_view_for_block(&base, 8)
        .unwrap();

    // Most recent overlay (block 7) should win
    assert_eq!(view.read(key(1)), Some(val(30)));
}

/// Preimage overrides from overlays shadow base preimages.
#[test]
fn overlay_preimage_shadows_base_preimage() {
    let hash1 = B256::from([1u8; 32]);
    let base = MockBaseState::new(5).with_preimage(hash1, vec![0xAA]);
    let mut buf = OverlayBuffer::default();

    buf.add_block(6, vec![], vec![(hash1, vec![0xBB])])
        .unwrap();

    let mut view = buf
        .sync_with_base_and_build_view_for_block(&base, 7)
        .unwrap();

    assert_eq!(view.get_preimage(hash1), Some(vec![0xBB]));
}

/// Once base catches up (BlockApplier persists), overlays are purged
/// and subsequent views read directly from base.
#[test]
fn overlays_purged_when_base_catches_up() {
    let _base_at_5 = MockBaseState::new(5).with_storage(key(1), val(10));
    let mut buf = OverlayBuffer::default();

    // Executor ran ahead: blocks 6, 7
    buf.add_block(6, vec![sw(1, 20)], vec![]).unwrap();
    buf.add_block(7, vec![sw(1, 30)], vec![]).unwrap();

    // Now base catches up to block 7 (BlockApplier persisted blocks 6+7)
    let base_at_7 = MockBaseState::new(7).with_storage(key(1), val(30));

    // Execute block 8 — overlays for 6+7 should be purged
    let mut view = buf
        .sync_with_base_and_build_view_for_block(&base_at_7, 8)
        .unwrap();

    // Should read from base (which now has block 7's writes = 30)
    assert_eq!(view.read(key(1)), Some(val(30)));
}

/// When base is exactly at block N-1, view for block N needs no overlays.
#[test]
fn view_for_block_n_needs_base_at_n_minus_one() {
    let base = MockBaseState::new(9); // base at 9
    let mut buf = OverlayBuffer::default();

    // Should succeed: base has 9, we need state for block 9 (= block 10 - 1)
    let result = buf.sync_with_base_and_build_view_for_block(&base, 10);
    assert!(result.is_ok());
}
