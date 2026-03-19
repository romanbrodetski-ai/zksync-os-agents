use alloy::primitives::B256;
use std::collections::HashMap;
use zksync_os_interface::traits::{PreimageSource, ReadStorage};
use zksync_os_interface::types::StorageWrite;
use zksync_os_storage_api::{OverlayBuffer, ReadStateHistory, StateError, StateResult};

#[derive(Debug, Clone, Default)]
struct MockViewState {
    storage: HashMap<B256, B256>,
    preimages: HashMap<B256, Vec<u8>>,
}

impl ReadStorage for MockViewState {
    fn read(&mut self, key: B256) -> Option<B256> {
        self.storage.get(&key).copied()
    }
}

impl PreimageSource for MockViewState {
    fn get_preimage(&mut self, hash: B256) -> Option<Vec<u8>> {
        self.preimages.get(&hash).cloned()
    }
}

#[derive(Debug, Clone)]
struct MockStateHistory {
    latest: u64,
    views: HashMap<u64, MockViewState>,
}

impl ReadStateHistory for MockStateHistory {
    fn state_view_at(
        &self,
        block_number: u64,
    ) -> StateResult<impl zksync_os_storage_api::ViewState> {
        self.views
            .get(&block_number)
            .cloned()
            .ok_or(StateError::NotFound(block_number))
    }

    fn block_range_available(&self) -> std::ops::RangeInclusive<u64> {
        0..=self.latest
    }
}

fn hash(byte: u8) -> B256 {
    B256::from([byte; 32])
}

fn write(key: B256, value: B256) -> StorageWrite {
    StorageWrite {
        key,
        value,
        account: alloy::primitives::Address::ZERO,
        account_key: B256::ZERO,
    }
}

#[test]
fn overlay_buffer_builds_execution_view_from_base_plus_unpersisted_blocks() {
    let key = hash(1);
    let preimage_hash = hash(9);

    let history = MockStateHistory {
        latest: 0,
        views: HashMap::from([(
            0,
            MockViewState {
                storage: HashMap::from([(key, hash(10))]),
                preimages: HashMap::new(),
            },
        )]),
    };

    let mut overlays = OverlayBuffer::default();
    overlays
        .add_block(
            1,
            vec![write(key, hash(11))],
            vec![(preimage_hash, vec![1, 2, 3])],
        )
        .unwrap();

    let mut view = overlays
        .sync_with_base_and_build_view_for_block(&history, 2)
        .unwrap();

    assert_eq!(view.read(key), Some(hash(11)));
    assert_eq!(view.get_preimage(preimage_hash), Some(vec![1, 2, 3]));
}

#[test]
fn overlay_buffer_purges_blocks_already_persisted_in_base_state() {
    let key = hash(2);

    let history = MockStateHistory {
        latest: 1,
        views: HashMap::from([(
            1,
            MockViewState {
                storage: HashMap::from([(key, hash(21))]),
                preimages: HashMap::new(),
            },
        )]),
    };

    let mut overlays = OverlayBuffer::default();
    overlays
        .add_block(1, vec![write(key, hash(99))], vec![])
        .unwrap();
    overlays
        .add_block(2, vec![write(hash(3), hash(30))], vec![])
        .unwrap();

    let mut view = overlays
        .sync_with_base_and_build_view_for_block(&history, 3)
        .unwrap();

    assert_eq!(
        view.read(key),
        Some(hash(21)),
        "stale overlay for block 1 must be purged once block 1 is persisted in base state"
    );
    assert_eq!(view.read(hash(3)), Some(hash(30)));
}

/// OverlayBuffer must reject add_block when the new block number is not contiguous with
/// the last entry. This enforces the sequential-processing invariant: if block N+2 is
/// added without N+1, the overlay would have a gap and execution would read stale state.
///
/// Mutation tested: if the contiguity check were removed from add_block, this test would
/// panic on the subsequent sync_with_base_and_build_view_for_block call (which checks
/// first/last overlay == expected range), or silently produce wrong state reads.
#[test]
fn overlay_buffer_rejects_non_contiguous_add_block() {
    let mut overlays = OverlayBuffer::default();

    // Add block 1 successfully
    overlays.add_block(1, vec![], vec![]).unwrap();

    // Attempt to add block 3 (skipping block 2) — must fail
    let result = overlays.add_block(3, vec![], vec![]);
    assert!(
        result.is_err(),
        "add_block must reject non-contiguous block numbers; \
         adding block 3 after block 1 should fail"
    );

    // Block 2 (the expected next) must still succeed
    overlays.add_block(2, vec![], vec![]).unwrap();
}

/// When base has already persisted all overlays, sync_with_base_and_build_view_for_block
/// should return an empty-overlay view backed purely by base (fast path).
/// This ensures we don't hold onto purged overlay data after persistence catches up.
#[test]
fn overlay_buffer_returns_base_only_view_when_fully_persisted() {
    let key = hash(7);

    // Base has blocks 0-3 persisted
    let history = MockStateHistory {
        latest: 3,
        views: HashMap::from([(
            3,
            MockViewState {
                storage: HashMap::from([(key, hash(77))]),
                preimages: HashMap::new(),
            },
        )]),
    };

    let mut overlays = OverlayBuffer::default();
    // Pre-populate with overlays 1-3 (all now persisted in base)
    for block_num in 1u64..=3 {
        overlays.add_block(block_num, vec![], vec![]).unwrap();
    }

    // Execute block 4 — base covers up to 3, no overlays needed
    let mut view = overlays
        .sync_with_base_and_build_view_for_block(&history, 4)
        .unwrap();

    // Value should come from base, not from (now-purged) overlays
    assert_eq!(
        view.read(key),
        Some(hash(77)),
        "view must read from base after overlays are purged"
    );

    // Drop the view before mutating the buffer — the Arc must be at refcount 1.
    drop(view);

    // After the view is dropped and the buffer was synced, overlays should be empty
    // (all purged since base has caught up). Verify we can add block 4 from empty state.
    overlays.add_block(4, vec![], vec![]).unwrap();
}
