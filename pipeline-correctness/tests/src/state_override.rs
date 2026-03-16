//! Tests for OverriddenStateView — the mechanism used to inject forced preimages
//! during block execution.
//!
//! When the Sequencer executes a block, it creates a state view at block_number - 1
//! and wraps it with OverriddenStateView::with_preimages() to inject any forced
//! preimages from the block command. This is critical for correctness: forced
//! preimages must be visible to the VM during execution.
//!
//! These tests exercise:
//! - Preimage override behavior (override shadows base)
//! - Fall-through to base state when no override exists
//! - Storage read fall-through (OverriddenStateView with preimages only)
//! - Multiple preimage overrides

use alloy::primitives::B256;
use std::collections::HashMap;
use zksync_os_interface::traits::{PreimageSource, ReadStorage};
use zksync_os_storage_api::OverriddenStateView;

// ─── Mock Base State ─────────────────────────────────────────────────────────

/// A trivial state view for testing OverriddenStateView.
#[derive(Debug, Clone)]
struct MockViewState {
    storage: HashMap<B256, B256>,
    preimages: HashMap<B256, Vec<u8>>,
}

impl MockViewState {
    fn new() -> Self {
        Self {
            storage: HashMap::new(),
            preimages: HashMap::new(),
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

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn hash(n: u8) -> B256 {
    B256::from([n; 32])
}

fn key(n: u8) -> B256 {
    B256::from([n; 32])
}

fn val(n: u8) -> B256 {
    B256::from([n; 32])
}

// ─── Tests ───────────────────────────────────────────────────────────────────

/// Preimage overrides should shadow the base state's preimages.
#[test]
fn preimage_override_shadows_base() {
    let base = MockViewState::new().with_preimage(hash(1), vec![0xAA; 10]);

    let overrides = vec![(hash(1), vec![0xBB; 20])];
    let mut view = OverriddenStateView::with_preimages(base, &overrides);

    assert_eq!(
        view.get_preimage(hash(1)),
        Some(vec![0xBB; 20]),
        "Override preimage should shadow base"
    );
}

/// Preimages not in the override set should fall through to the base state.
#[test]
fn preimage_falls_through_to_base() {
    let base = MockViewState::new().with_preimage(hash(1), vec![0xAA; 10]);

    // Override a different hash
    let overrides = vec![(hash(2), vec![0xCC; 5])];
    let mut view = OverriddenStateView::with_preimages(base, &overrides);

    assert_eq!(
        view.get_preimage(hash(1)),
        Some(vec![0xAA; 10]),
        "Base preimage should be returned when no override exists"
    );
}

/// Missing preimage (neither override nor base) should return None.
#[test]
fn missing_preimage_returns_none() {
    let base = MockViewState::new();
    let overrides: Vec<(B256, Vec<u8>)> = vec![];
    let mut view = OverriddenStateView::with_preimages(base, &overrides);

    assert_eq!(view.get_preimage(hash(99)), None);
}

/// Storage reads should fall through to the base state when using with_preimages().
/// The preimage-only constructor does not inject storage overrides.
#[test]
fn storage_falls_through_with_preimage_override() {
    let base = MockViewState::new().with_storage(key(1), val(10));

    let overrides = vec![(hash(1), vec![0xBB; 20])];
    let mut view = OverriddenStateView::with_preimages(base, &overrides);

    assert_eq!(
        view.read(key(1)),
        Some(val(10)),
        "Storage reads should fall through to base"
    );
}

/// Multiple preimage overrides should all be accessible.
#[test]
fn multiple_preimage_overrides() {
    let base = MockViewState::new();

    let overrides = vec![
        (hash(1), vec![1, 2, 3]),
        (hash(2), vec![4, 5, 6]),
        (hash(3), vec![7, 8, 9]),
    ];
    let mut view = OverriddenStateView::with_preimages(base, &overrides);

    assert_eq!(view.get_preimage(hash(1)), Some(vec![1, 2, 3]));
    assert_eq!(view.get_preimage(hash(2)), Some(vec![4, 5, 6]));
    assert_eq!(view.get_preimage(hash(3)), Some(vec![7, 8, 9]));
    assert_eq!(view.get_preimage(hash(4)), None);
}

/// Combined storage and preimage base state: with_preimages only overrides preimages,
/// not storage.
#[test]
fn preimage_override_does_not_affect_storage() {
    let base = MockViewState::new()
        .with_storage(key(1), val(10))
        .with_preimage(hash(1), vec![0xAA]);

    let overrides = vec![(hash(1), vec![0xBB])];
    let mut view = OverriddenStateView::with_preimages(base, &overrides);

    // Preimage should be overridden
    assert_eq!(view.get_preimage(hash(1)), Some(vec![0xBB]));
    // Storage should be unchanged
    assert_eq!(view.read(key(1)), Some(val(10)));
}

/// Empty overrides should be a no-op — all reads go to base.
#[test]
fn empty_overrides_is_noop() {
    let base = MockViewState::new()
        .with_storage(key(1), val(10))
        .with_preimage(hash(1), vec![0xAA]);

    let overrides: Vec<(B256, Vec<u8>)> = vec![];
    let mut view = OverriddenStateView::with_preimages(base, &overrides);

    assert_eq!(view.read(key(1)), Some(val(10)));
    assert_eq!(view.get_preimage(hash(1)), Some(vec![0xAA]));
}
