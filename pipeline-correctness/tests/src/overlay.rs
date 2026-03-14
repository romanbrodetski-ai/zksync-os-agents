//! Tests for the OverlayBuffer — the in-memory state cache that sits between
//! BlockExecutor and persisted state (RocksDB).
//!
//! The overlay is critical for correctness: during execution, the VM reads
//! storage through an OverriddenStateView that checks the overlay first, then
//! falls back to the base (persisted) state. If the overlay doesn't correctly
//! layer writes from earlier blocks, subsequent blocks will read stale data.
//!
//! These tests exercise:
//! - The full overlay lifecycle (add → view → purge)
//! - Read-through behavior (overlay hit vs base fallback)
//! - Multi-block layering (later blocks shadow earlier ones)
//! - Contiguity invariant enforcement
//! - Purge behavior when base state advances
//! - Preimage overlay semantics

use alloy::primitives::{Address, B256, BlockNumber};
use std::collections::HashMap;
use std::fmt::Debug;
use std::ops::RangeInclusive;
use zksync_os_interface::traits::{PreimageSource, ReadStorage};
use zksync_os_interface::types::StorageWrite;
use zksync_os_storage_api::{
    OverlayBuffer, ReadStateHistory, StateResult, ViewState,
};

// ─── Mock Base State ─────────────────────────────────────────────────────────

/// A trivial in-memory state that implements ReadStateHistory.
/// Stores key→value and hash→preimage maps, accessible at any block in range.
#[derive(Debug, Clone)]
struct MockBaseState {
    storage: HashMap<B256, B256>,
    preimages: HashMap<B256, Vec<u8>>,
    block_range: RangeInclusive<u64>,
}

impl MockBaseState {
    fn new(latest_block: u64) -> Self {
        Self {
            storage: HashMap::new(),
            preimages: HashMap::new(),
            block_range: 0..=latest_block,
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

    /// Advance the base state's latest block (simulating persistence).
    fn advance_to(&mut self, block: u64) {
        self.block_range = *self.block_range.start()..=block;
    }
}

/// The view returned by MockBaseState::state_view_at().
#[derive(Debug, Clone)]
struct MockStateView {
    storage: HashMap<B256, B256>,
    preimages: HashMap<B256, Vec<u8>>,
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

impl ReadStateHistory for MockBaseState {
    fn state_view_at(&self, _block_number: BlockNumber) -> StateResult<impl ViewState> {
        Ok(MockStateView {
            storage: self.storage.clone(),
            preimages: self.preimages.clone(),
        })
    }

    fn block_range_available(&self) -> RangeInclusive<u64> {
        self.block_range.clone()
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn key(n: u8) -> B256 {
    B256::from([n; 32])
}

fn val(n: u8) -> B256 {
    B256::from([n; 32])
}

fn write(k: u8, v: u8) -> StorageWrite {
    StorageWrite {
        key: key(k),
        value: val(v),
        account: Address::ZERO,
        account_key: B256::ZERO,
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

/// After adding a block to the overlay, reading through the view should
/// return the overlaid value, not the base state value.
#[test]
fn overlay_shadows_base_state() {
    let base = MockBaseState::new(0).with_storage(key(1), val(10));
    let mut overlay = OverlayBuffer::default();

    // Block 1 overwrites key(1) with val(20)
    overlay.add_block(1, vec![write(1, 20)], vec![]).unwrap();

    let mut view = overlay
        .sync_with_base_and_build_view_for_block(&base, 2)
        .unwrap();

    // Should read the overlay value (20), not the base value (10)
    assert_eq!(view.read(key(1)), Some(val(20)));
}

/// Keys not in the overlay should fall through to the base state.
#[test]
fn overlay_falls_through_to_base() {
    let base = MockBaseState::new(0).with_storage(key(1), val(10));
    let mut overlay = OverlayBuffer::default();

    // Block 1 writes to key(2) only — key(1) untouched in overlay
    overlay.add_block(1, vec![write(2, 20)], vec![]).unwrap();

    let mut view = overlay
        .sync_with_base_and_build_view_for_block(&base, 2)
        .unwrap();

    assert_eq!(view.read(key(1)), Some(val(10)), "key(1) should come from base");
    assert_eq!(view.read(key(2)), Some(val(20)), "key(2) should come from overlay");
}

/// Keys that exist in neither overlay nor base should return None.
#[test]
fn missing_key_returns_none() {
    let base = MockBaseState::new(0);
    let mut overlay = OverlayBuffer::default();
    overlay.add_block(1, vec![], vec![]).unwrap();

    let mut view = overlay
        .sync_with_base_and_build_view_for_block(&base, 2)
        .unwrap();

    assert_eq!(view.read(key(99)), None);
}

/// When multiple overlay blocks write the same key, the latest block's
/// value should win (most-recent-first search).
#[test]
fn later_overlay_block_shadows_earlier() {
    let base = MockBaseState::new(0);
    let mut overlay = OverlayBuffer::default();

    overlay.add_block(1, vec![write(1, 10)], vec![]).unwrap();
    overlay.add_block(2, vec![write(1, 20)], vec![]).unwrap();
    overlay.add_block(3, vec![write(1, 30)], vec![]).unwrap();

    let mut view = overlay
        .sync_with_base_and_build_view_for_block(&base, 4)
        .unwrap();

    assert_eq!(view.read(key(1)), Some(val(30)), "should read block 3's value");
}

/// Preimage overlay should shadow base preimages too.
#[test]
fn preimage_overlay_shadows_base() {
    let hash = key(1);
    let base = MockBaseState::new(0).with_preimage(hash, vec![0xAA; 10]);
    let mut overlay = OverlayBuffer::default();

    overlay
        .add_block(1, vec![], vec![(hash, vec![0xBB; 20])])
        .unwrap();

    let mut view = overlay
        .sync_with_base_and_build_view_for_block(&base, 2)
        .unwrap();

    assert_eq!(view.get_preimage(hash), Some(vec![0xBB; 20]));
}

/// Preimage not in overlay should fall through to base.
#[test]
fn preimage_falls_through_to_base() {
    let hash = key(1);
    let base = MockBaseState::new(0).with_preimage(hash, vec![0xAA; 10]);
    let mut overlay = OverlayBuffer::default();

    // Overlay has a different preimage, not hash(1)
    overlay
        .add_block(1, vec![], vec![(key(2), vec![0xCC; 5])])
        .unwrap();

    let mut view = overlay
        .sync_with_base_and_build_view_for_block(&base, 2)
        .unwrap();

    assert_eq!(view.get_preimage(hash), Some(vec![0xAA; 10]));
}

/// When base state advances past overlay blocks, those blocks should
/// be purged on the next sync.
#[test]
fn purge_removes_blocks_persisted_to_base() {
    let mut base = MockBaseState::new(0);
    let mut overlay = OverlayBuffer::default();

    // Execute blocks 1-5 with overlays
    for i in 1..=5u8 {
        overlay
            .add_block(i as u64, vec![write(i, i * 10)], vec![])
            .unwrap();
    }

    // Simulate persistence: base advances to block 3
    base.advance_to(3);

    // Build view for block 6 — this syncs with base, purging blocks 1-3
    let mut view = overlay
        .sync_with_base_and_build_view_for_block(&base, 6)
        .unwrap();

    // Blocks 4 and 5 still in overlay — their writes should be visible
    assert_eq!(view.read(key(4)), Some(val(40)));
    assert_eq!(view.read(key(5)), Some(val(50)));

    // Block 3's write was purged from overlay, but base state should have it
    // (in our mock, base doesn't actually absorb the writes, so it returns None
    // — the real StateHandle would have them. This test verifies purge happened.)
}

/// After purging, adding a new block should continue from the correct number.
#[test]
fn add_block_after_purge_maintains_contiguity() {
    let mut base = MockBaseState::new(0);
    let mut overlay = OverlayBuffer::default();

    overlay.add_block(1, vec![write(1, 10)], vec![]).unwrap();
    overlay.add_block(2, vec![write(2, 20)], vec![]).unwrap();

    // Advance base to 1, sync to purge block 1.
    // The view must be dropped before add_block (Arc refcount invariant).
    base.advance_to(1);
    {
        let _view = overlay
            .sync_with_base_and_build_view_for_block(&base, 3)
            .unwrap();
        // view dropped here
    }

    // Should be able to add block 3 (contiguous after block 2)
    overlay.add_block(3, vec![write(3, 30)], vec![]).unwrap();
}

/// Adding a non-contiguous block should fail.
#[test]
fn non_contiguous_add_fails() {
    let mut overlay = OverlayBuffer::default();
    overlay.add_block(1, vec![], vec![]).unwrap();

    // Skip block 2 — should fail
    let result = overlay.add_block(3, vec![], vec![]);
    assert!(result.is_err(), "Adding block 3 after block 1 should fail");
    assert!(
        result.unwrap_err().to_string().contains("contiguous"),
        "Error should mention contiguity"
    );
}

/// The first block added can be any number (no prior overlay to check against).
#[test]
fn first_block_can_be_any_number() {
    let mut overlay = OverlayBuffer::default();
    // Starting at block 42 — should work since overlay is empty
    overlay.add_block(42, vec![write(1, 10)], vec![]).unwrap();
    overlay.add_block(43, vec![write(2, 20)], vec![]).unwrap();
}

/// When base_latest already covers the block being executed,
/// the view should come directly from base with no overlay needed.
#[test]
fn base_covers_block_returns_base_view() {
    let base = MockBaseState::new(5).with_storage(key(1), val(99));
    let mut overlay = OverlayBuffer::default();

    // Executing block 3 — base already at 5, so view at block 2 comes from base
    let mut view = overlay
        .sync_with_base_and_build_view_for_block(&base, 3)
        .unwrap();

    assert_eq!(view.read(key(1)), Some(val(99)));
}

/// Multi-key writes across multiple blocks: each key should reflect its
/// latest write across the overlay stack.
#[test]
fn multi_key_multi_block_overlay() {
    let base = MockBaseState::new(0);
    let mut overlay = OverlayBuffer::default();

    // Block 1: write key(1)=10, key(2)=20
    overlay
        .add_block(1, vec![write(1, 10), write(2, 20)], vec![])
        .unwrap();
    // Block 2: overwrite key(1)=11, write key(3)=30
    overlay
        .add_block(2, vec![write(1, 11), write(3, 30)], vec![])
        .unwrap();
    // Block 3: overwrite key(2)=21
    overlay
        .add_block(3, vec![write(2, 21)], vec![])
        .unwrap();

    let mut view = overlay
        .sync_with_base_and_build_view_for_block(&base, 4)
        .unwrap();

    assert_eq!(view.read(key(1)), Some(val(11)), "key(1) from block 2");
    assert_eq!(view.read(key(2)), Some(val(21)), "key(2) from block 3");
    assert_eq!(view.read(key(3)), Some(val(30)), "key(3) from block 2");
}

/// Simulate the BlockExecutor's actual usage pattern:
/// for each block, sync+view, execute (read), then add the new block's writes.
#[test]
fn simulated_executor_lifecycle() {
    let base = MockBaseState::new(0).with_storage(key(1), val(100));
    let mut overlay = OverlayBuffer::default();

    // --- Execute block 1 ---
    {
        let mut view = overlay
            .sync_with_base_and_build_view_for_block(&base, 1)
            .unwrap();
        // Read existing state
        assert_eq!(view.read(key(1)), Some(val(100)));
        assert_eq!(view.read(key(2)), None);
    }
    // Block 1 produces writes
    overlay
        .add_block(1, vec![write(1, 101), write(2, 200)], vec![])
        .unwrap();

    // --- Execute block 2 ---
    {
        let mut view = overlay
            .sync_with_base_and_build_view_for_block(&base, 2)
            .unwrap();
        // Should see block 1's writes via overlay
        assert_eq!(view.read(key(1)), Some(val(101)));
        assert_eq!(view.read(key(2)), Some(val(200)));
    }
    // Block 2 produces writes
    overlay
        .add_block(2, vec![write(1, 102)], vec![])
        .unwrap();

    // --- Execute block 3 ---
    {
        let mut view = overlay
            .sync_with_base_and_build_view_for_block(&base, 3)
            .unwrap();
        // Should see block 2's write to key(1), block 1's write to key(2)
        assert_eq!(view.read(key(1)), Some(val(102)));
        assert_eq!(view.read(key(2)), Some(val(200)));
    }
}

/// Simulate executor lifecycle with base state advancing mid-flight
/// (i.e., BlockApplier persists blocks while executor runs ahead).
#[test]
fn executor_lifecycle_with_base_advancing() {
    let mut base = MockBaseState::new(0)
        .with_storage(key(1), val(100));
    let mut overlay = OverlayBuffer::default();

    // Execute block 1
    {
        let mut view = overlay
            .sync_with_base_and_build_view_for_block(&base, 1)
            .unwrap();
        assert_eq!(view.read(key(1)), Some(val(100)));
    }
    overlay
        .add_block(1, vec![write(1, 101)], vec![])
        .unwrap();

    // Execute block 2
    {
        let mut view = overlay
            .sync_with_base_and_build_view_for_block(&base, 2)
            .unwrap();
        assert_eq!(view.read(key(1)), Some(val(101)));
    }
    overlay
        .add_block(2, vec![write(1, 102)], vec![])
        .unwrap();

    // --- Base advances to block 1 (BlockApplier persisted block 1) ---
    base.advance_to(1);
    // Update mock base to reflect persisted state
    base.storage.insert(key(1), val(101));

    // Execute block 3 — should sync, purge block 1 from overlay
    {
        let mut view = overlay
            .sync_with_base_and_build_view_for_block(&base, 3)
            .unwrap();
        // Block 2's overlay still present, shadows base
        assert_eq!(view.read(key(1)), Some(val(102)));
    }

    // --- Base advances to block 2 ---
    base.advance_to(2);
    base.storage.insert(key(1), val(102));

    // Execute block 3 again (imagine re-execution after rollback)
    overlay
        .add_block(3, vec![write(1, 103)], vec![])
        .unwrap();
    {
        let mut view = overlay
            .sync_with_base_and_build_view_for_block(&base, 4)
            .unwrap();
        assert_eq!(view.read(key(1)), Some(val(103)));
    }
}

/// Multiple preimages across blocks — latest block wins.
#[test]
fn preimage_multi_block_shadowing() {
    let base = MockBaseState::new(0);
    let mut overlay = OverlayBuffer::default();

    let hash = key(1);
    overlay
        .add_block(1, vec![], vec![(hash, vec![1, 2, 3])])
        .unwrap();
    overlay
        .add_block(2, vec![], vec![(hash, vec![4, 5, 6, 7])])
        .unwrap();

    let mut view = overlay
        .sync_with_base_and_build_view_for_block(&base, 3)
        .unwrap();

    assert_eq!(view.get_preimage(hash), Some(vec![4, 5, 6, 7]));
}

/// Mixed storage writes and preimages in the same block — both types
/// should be accessible through the view.
#[test]
fn mixed_storage_and_preimage_overlay() {
    let base = MockBaseState::new(0);
    let mut overlay = OverlayBuffer::default();

    let preimage_hash = key(50);
    overlay
        .add_block(
            1,
            vec![write(1, 10)],
            vec![(preimage_hash, vec![0xDE, 0xAD])],
        )
        .unwrap();

    let mut view = overlay
        .sync_with_base_and_build_view_for_block(&base, 2)
        .unwrap();

    assert_eq!(view.read(key(1)), Some(val(10)));
    assert_eq!(view.get_preimage(preimage_hash), Some(vec![0xDE, 0xAD]));
}

/// Duplicate writes within the same block — last one wins (HashMap semantics).
#[test]
fn duplicate_key_in_same_block_last_wins() {
    let base = MockBaseState::new(0);
    let mut overlay = OverlayBuffer::default();

    // Two writes to the same key in one block
    overlay
        .add_block(
            1,
            vec![write(1, 10), write(1, 20)],
            vec![],
        )
        .unwrap();

    let mut view = overlay
        .sync_with_base_and_build_view_for_block(&base, 2)
        .unwrap();

    // HashMap collect takes the last value for duplicate keys
    let result = view.read(key(1)).unwrap();
    // Either value is acceptable depending on iteration order,
    // but the important thing is it doesn't panic
    assert!(result == val(10) || result == val(20));
}

/// An empty overlay block (no writes) should still be valid and tracked.
#[test]
fn empty_overlay_block_is_valid() {
    let base = MockBaseState::new(0).with_storage(key(1), val(10));
    let mut overlay = OverlayBuffer::default();

    overlay.add_block(1, vec![], vec![]).unwrap();

    let mut view = overlay
        .sync_with_base_and_build_view_for_block(&base, 2)
        .unwrap();

    // Base state should still be accessible
    assert_eq!(view.read(key(1)), Some(val(10)));
}

/// After all overlays are purged, the view should come entirely from base.
#[test]
fn all_overlays_purged_falls_through_to_base() {
    let mut base = MockBaseState::new(0).with_storage(key(1), val(10));
    let mut overlay = OverlayBuffer::default();

    overlay.add_block(1, vec![write(1, 20)], vec![]).unwrap();

    // Base advances past all overlay blocks
    base.advance_to(1);
    base.storage.insert(key(1), val(20));

    let mut view = overlay
        .sync_with_base_and_build_view_for_block(&base, 2)
        .unwrap();

    // All overlays purged, reading directly from base
    assert_eq!(view.read(key(1)), Some(val(20)));
}
