//! Snapshot-scoped cache of criteria masks for the `*IF`/`*IFS` family.
//!
//! `SUMIFS(sum, crit_range, crit, …)` evaluates the same `(criteria column, predicate)` pair once
//! per formula cell that references it. A revenue model with 20k `SUMIFS` over a 450-row fact
//! table re-derived every mask (lower-casing the whole text column, Arrow compares, slicing) on
//! every evaluation — 1.5 s per single-cell edit, all of it in `compute_criteria_mask`. The
//! previous `get_criteria_mask` hook built the mask unconditionally; this cache keys the built
//! mask on the view's geometry, the predicate, and the sheet's data snapshot id (the same term the
//! lookup-index cache uses), so within one recalculation every consumer of a column shares one
//! mask and any edit to the sheet naturally rotates the key.
//!
//! Evaluation is parallel, so a cold key is typically requested by many cells at once. Each key
//! owns a [`OnceLock`] cell: the first thread builds, the rest block on that cell and share the
//! result instead of building their own copy — one build per (column, predicate, snapshot).
//!
//! Masks are cheap (one bit per row plus validity) — the byte cap only bounds growth across many
//! snapshots; hitting it clears the whole cache rather than evicting selectively.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock, RwLock};

use arrow_array::Array as _;
use arrow_array::BooleanArray;
use formualizer_common::SheetId;
use rustc_hash::FxHashMap;

use crate::args::CriteriaPredicate;
use crate::engine::lookup_index_cache::LookupHashKey;

/// Default byte budget for cached masks (a 450-row mask is ~120 bytes; a 1M-row mask ~250 KiB).
pub const DEFAULT_CRITERIA_MASK_CACHE_MAX_BYTES: usize = 64 * 1024 * 1024;

/// Hashable projection of a [`CriteriaPredicate`]. `None` when the predicate holds a literal the
/// lookup hash key cannot represent (dates, errors, arrays) — those masks are simply not cached.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub(crate) enum PredicateKey {
    Eq(LookupHashKey),
    Ne(LookupHashKey),
    Gt(u64),
    Ge(u64),
    Lt(u64),
    Le(u64),
    TextLike {
        pattern: Box<str>,
        case_insensitive: bool,
    },
    NotLike(Box<PredicateKey>),
    TextGt(Box<str>),
    TextGe(Box<str>),
    TextLt(Box<str>),
    TextLe(Box<str>),
    IsBlank,
    IsNotBlank,
    IsBlankOrEmptyText,
    IsNumber,
    IsText,
    IsLogical,
}

impl PredicateKey {
    pub(crate) fn from_predicate(pred: &CriteriaPredicate) -> Option<Self> {
        Some(match pred {
            CriteriaPredicate::Eq(v) => Self::Eq(LookupHashKey::from_literal(v)?),
            CriteriaPredicate::Ne(v) => Self::Ne(LookupHashKey::from_literal(v)?),
            CriteriaPredicate::Gt(n) => Self::Gt(n.to_bits()),
            CriteriaPredicate::Ge(n) => Self::Ge(n.to_bits()),
            CriteriaPredicate::Lt(n) => Self::Lt(n.to_bits()),
            CriteriaPredicate::Le(n) => Self::Le(n.to_bits()),
            CriteriaPredicate::TextLike {
                pattern,
                case_insensitive,
            } => Self::TextLike {
                pattern: pattern.as_str().into(),
                case_insensitive: *case_insensitive,
            },
            CriteriaPredicate::NotLike(inner) => {
                Self::NotLike(Box::new(Self::from_predicate(inner)?))
            }
            CriteriaPredicate::TextGt(s) => Self::TextGt(s.as_str().into()),
            CriteriaPredicate::TextGe(s) => Self::TextGe(s.as_str().into()),
            CriteriaPredicate::TextLt(s) => Self::TextLt(s.as_str().into()),
            CriteriaPredicate::TextLe(s) => Self::TextLe(s.as_str().into()),
            CriteriaPredicate::IsBlank => Self::IsBlank,
            CriteriaPredicate::IsNotBlank => Self::IsNotBlank,
            CriteriaPredicate::IsBlankOrEmptyText => Self::IsBlankOrEmptyText,
            CriteriaPredicate::IsNumber => Self::IsNumber,
            CriteriaPredicate::IsText => Self::IsText,
            CriteriaPredicate::IsLogical => Self::IsLogical,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub(crate) struct CriteriaMaskKey {
    pub(crate) sheet_id: SheetId,
    pub(crate) start_row: u32,
    pub(crate) start_col: u32,
    pub(crate) end_row: u32,
    pub(crate) end_col: u32,
    pub(crate) col_in_view: u32,
    pub(crate) pred: PredicateKey,
    /// [`Engine::sheet_data_snapshot_id`] of `sheet_id` when the mask was built.
    pub(crate) snapshot_id: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CriteriaMaskCacheReport {
    pub builds: usize,
    pub hits: usize,
    pub misses: usize,
    pub skipped_volatile: usize,
    pub skipped_unkeyable: usize,
    pub cap_clears: usize,
    pub bytes_in_cache: usize,
    pub entries_count: usize,
}

type MaskCell = Arc<OnceLock<Option<Arc<BooleanArray>>>>;

pub struct CriteriaMaskCache {
    inner: RwLock<FxHashMap<CriteriaMaskKey, MaskCell>>,
    bytes_in_use: AtomicUsize,
    max_bytes: usize,
    builds: AtomicUsize,
    hits: AtomicUsize,
    misses: AtomicUsize,
    skipped_volatile: AtomicUsize,
    skipped_unkeyable: AtomicUsize,
    cap_clears: AtomicUsize,
}

fn mask_bytes(mask: &BooleanArray) -> usize {
    mask.get_buffer_memory_size().saturating_add(64)
}

impl CriteriaMaskCache {
    pub(crate) fn new(max_bytes: usize) -> Self {
        Self {
            inner: RwLock::new(FxHashMap::default()),
            bytes_in_use: AtomicUsize::new(0),
            max_bytes,
            builds: AtomicUsize::new(0),
            hits: AtomicUsize::new(0),
            misses: AtomicUsize::new(0),
            skipped_volatile: AtomicUsize::new(0),
            skipped_unkeyable: AtomicUsize::new(0),
            cap_clears: AtomicUsize::new(0),
        }
    }

    /// Return the mask for `key`, building it with `build` exactly once per key. Concurrent
    /// callers for the same key wait for the first builder. A `None` result is remembered too:
    /// the inputs are fixed for the snapshot, so rebuilding would give `None` again.
    pub(crate) fn get_or_build(
        &self,
        key: CriteriaMaskKey,
        build: impl FnOnce() -> Option<Arc<BooleanArray>>,
    ) -> Option<Arc<BooleanArray>> {
        let cell: MaskCell = {
            let existing = self
                .inner
                .read()
                .ok()
                .and_then(|guard| guard.get(&key).cloned());
            match existing {
                Some(cell) => {
                    self.hits.fetch_add(1, Ordering::Relaxed);
                    cell
                }
                None => {
                    let Ok(mut guard) = self.inner.write() else {
                        return build();
                    };
                    match guard.get(&key) {
                        Some(cell) => {
                            self.hits.fetch_add(1, Ordering::Relaxed);
                            Arc::clone(cell)
                        }
                        None => {
                            self.misses.fetch_add(1, Ordering::Relaxed);
                            let cell: MaskCell = Arc::new(OnceLock::new());
                            guard.insert(key, Arc::clone(&cell));
                            cell
                        }
                    }
                }
            }
        };
        let mut built_bytes = 0usize;
        let value = cell.get_or_init(|| {
            self.builds.fetch_add(1, Ordering::Relaxed);
            let mask = build();
            if let Some(mask) = &mask {
                built_bytes = mask_bytes(mask);
            }
            mask
        });
        if built_bytes > 0 {
            let total = self
                .bytes_in_use
                .fetch_add(built_bytes, Ordering::Relaxed)
                .saturating_add(built_bytes);
            if total > self.max_bytes {
                self.clear_over_cap();
            }
        }
        value.clone()
    }

    /// Over the byte cap the whole cache is dropped — every entry from an older snapshot is dead
    /// weight anyway, and masks are cheap to rebuild. Cells already handed to callers stay valid.
    fn clear_over_cap(&self) {
        if let Ok(mut guard) = self.inner.write() {
            guard.clear();
            self.bytes_in_use.store(0, Ordering::Relaxed);
            self.cap_clears.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn note_skipped_volatile(&self) {
        self.skipped_volatile.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn note_skipped_unkeyable(&self) {
        self.skipped_unkeyable.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn report(&self) -> CriteriaMaskCacheReport {
        let entries_count = self.inner.read().map(|g| g.len()).unwrap_or(0);
        CriteriaMaskCacheReport {
            builds: self.builds.load(Ordering::Relaxed),
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            skipped_volatile: self.skipped_volatile.load(Ordering::Relaxed),
            skipped_unkeyable: self.skipped_unkeyable.load(Ordering::Relaxed),
            cap_clears: self.cap_clears.load(Ordering::Relaxed),
            bytes_in_cache: self.bytes_in_use.load(Ordering::Relaxed),
            entries_count,
        }
    }
}
