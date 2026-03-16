//! Tests for OverlayBuffer — the in-memory state diff buffer used by BlockExecutor
//! to hold executed-but-not-yet-persisted block state between execution and persistence.
//!
//! OverlayBuffer invariants:
//! - Blocks must be added contiguously (non-contiguous add panics)
//! - `sync_with_base_and_build_view_for_block` purges already-persisted overlays
//! - The composite view returns overlay values shadowing the base state
//! - Overlay values from later blocks shadow earlier blocks for the same key

use alloy::primitives::{Address, B256, BlockNumber};
use std::collections::HashMap;
use zksync_os_interface::traits::{PreimageSource, ReadStorage};
use zksync_os_interface::types::StorageWrite;
use zksync_os_storage_api::{OverlayBuffer, ReadStateHistory, StateError, StateResult, ViewState};

// ─── Mock base state ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct MockBaseState {
    storage: HashMap<B256, B256>,
    preimages: HashMap<B256, Vec<u8>>,
    latest_block: u64,
}

impl MockBaseState {
    fn new(latest_block: u64) -> Self {
        Self {
            storage: HashMap::new(),
            preimages: HashMap::new(),
            latest_block,
        }
    }

    fn with_storage(mut self, key: B256, value: B256) -> Self {
        self.storage.insert(key, value);
        self
    }
}

impl ReadStorage for MockBaseState {
    fn read(&mut self, key: B256) -> Option<B256> {
        self.storage.get(&key).copied()
    }
}

impl PreimageSource for MockBaseState {
    fn get_preimage(&mut self, hash: B256) -> Option<Vec<u8>> {
        self.preimages.get(&hash).cloned()
    }
}

impl ReadStateHistory for MockBaseState {
    fn state_view_at(&self, _block_number: BlockNumber) -> StateResult<impl ViewState> {
        Ok(self.clone())
    }

    fn block_range_available(&self) -> std::ops::RangeInclusive<u64> {
        0..=self.latest_block
    }
}

fn make_write(key_byte: u8, value_byte: u8) -> StorageWrite {
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

// ─── Tests ───────────────────────────────────────────────────────────────────

/// Non-contiguous add_block panics.
#[test]
#[should_panic(expected = "Overlay head must be contiguous")]
fn add_non_contiguous_block_panics() {
    let mut buf = OverlayBuffer::default();
    buf.add_block(1, vec![], vec![]).unwrap();
    // Skip block 2
    buf.add_block(3, vec![], vec![]).unwrap();
}

/// Adding the same block number twice panics (not contiguous with last+1).
#[test]
#[should_panic(expected = "Overlay head must be contiguous")]
fn add_duplicate_block_panics() {
    let mut buf = OverlayBuffer::default();
    buf.add_block(1, vec![], vec![]).unwrap();
    buf.add_block(1, vec![], vec![]).unwrap();
}

/// When base is up to date (base_latest >= block-1), overlay returns base state directly.
#[test]
fn view_falls_through_to_base_when_no_overlay_needed() {
    let base = MockBaseState::new(5).with_storage(key(0x01), val(0xAA));
    let mut buf = OverlayBuffer::default();

    // Request view for block 3 — base_latest=5 >= 2, so no overlay needed
    let mut view = buf
        .sync_with_base_and_build_view_for_block(&base, 3)
        .expect("view should build");

    assert_eq!(view.read(key(0x01)), Some(val(0xAA)));
}

/// Overlay writes shadow base state reads.
#[test]
fn overlay_shadows_base_for_same_key() {
    let base = MockBaseState::new(0).with_storage(key(0x01), val(0xBB));
    let mut buf = OverlayBuffer::default();

    // Block 1: overlay writes key(0x01) = val(0xCC)
    buf.add_block(1, vec![make_write(0x01, 0xCC)], vec![]).unwrap();

    // Request view for block 2 (needs overlays up to block 1)
    let mut view = buf
        .sync_with_base_and_build_view_for_block(&base, 2)
        .expect("view should build");

    // Overlay value should shadow the base value
    assert_eq!(view.read(key(0x01)), Some(val(0xCC)));
}

/// Key only in base (not in any overlay) should fall through.
#[test]
fn key_only_in_base_falls_through() {
    let base = MockBaseState::new(0).with_storage(key(0x99), val(0x42));
    let mut buf = OverlayBuffer::default();
    buf.add_block(1, vec![make_write(0x01, 0xFF)], vec![]).unwrap();

    let mut view = buf
        .sync_with_base_and_build_view_for_block(&base, 2)
        .expect("view should build");

    // key(0x99) not in overlay → falls through to base
    assert_eq!(view.read(key(0x99)), Some(val(0x42)));
}

/// Key not in base or any overlay returns None.
#[test]
fn key_in_neither_base_nor_overlay_returns_none() {
    let base = MockBaseState::new(0);
    let mut buf = OverlayBuffer::default();
    buf.add_block(1, vec![], vec![]).unwrap();

    let mut view = buf
        .sync_with_base_and_build_view_for_block(&base, 2)
        .expect("view should build");

    assert_eq!(view.read(key(0xAB)), None);
}

/// Later block's write to same key shadows earlier block's write.
#[test]
fn later_block_overlay_shadows_earlier_block_overlay() {
    let base = MockBaseState::new(0);
    let mut buf = OverlayBuffer::default();

    buf.add_block(1, vec![make_write(0x01, 0xAA)], vec![]).unwrap();
    buf.add_block(2, vec![make_write(0x01, 0xBB)], vec![]).unwrap();

    // View for block 3: needs overlays for blocks 1 and 2
    let mut view = buf
        .sync_with_base_and_build_view_for_block(&base, 3)
        .expect("view should build");

    assert_eq!(view.read(key(0x01)), Some(val(0xBB)), "block 2 must shadow block 1");
}

/// After base advances past the overlay, purge cleans up and reads fall through to base.
#[test]
fn purge_on_sync_drops_persisted_overlays() {
    // Initially base is at block 0; we execute blocks 1 and 2 into overlay
    let mut base = MockBaseState::new(0);
    let mut buf = OverlayBuffer::default();

    buf.add_block(1, vec![make_write(0x01, 0xAA)], vec![]).unwrap();
    buf.add_block(2, vec![make_write(0x01, 0xBB)], vec![]).unwrap();

    // Simulate: base now persisted block 2 (blocks 1 and 2 are persisted)
    base.latest_block = 2;
    base.storage.insert(key(0x01), val(0xBB)); // base now has the final value

    // View for block 3: sync should purge overlays 1 and 2, then return base view
    let mut view = buf
        .sync_with_base_and_build_view_for_block(&base, 3)
        .expect("view should build after sync");

    // Both overlay blocks are purged; base has the value
    assert_eq!(view.read(key(0x01)), Some(val(0xBB)));

    // After purge, adding block 3 must succeed
    drop(view); // Drop the view to release the Arc
    buf.add_block(3, vec![], vec![]).unwrap();
}
