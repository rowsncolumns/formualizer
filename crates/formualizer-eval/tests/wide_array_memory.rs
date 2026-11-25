//! An array held in memory reserves for the rows it has.
//!
//! `RangeView::from_owned_rows` builds a temporary Arrow sheet for an array value. Its builders
//! reserve one chunk of rows per column up front, so the chunk size must follow the array: a
//! fixed 32,768-row chunk costs about 850 KB a column whatever the array holds, which is 4 GB for
//! a single row of 5,000 cells and more than a 32-bit build can address.
//!
//! This file is its own test binary because it installs a counting global allocator.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use formualizer_common::LiteralValue;
use formualizer_eval::engine::range_view::RangeView;
use formualizer_eval::engine::{DateSystem, Engine, EvalConfig};
use formualizer_eval::test_workbook::TestWorkbook;
use formualizer_parse::parser::parse;

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn grew(by: usize) {
    let live = LIVE.fetch_add(by, Ordering::Relaxed) + by;
    PEAK.fetch_max(live, Ordering::Relaxed);
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        grew(layout.size());
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        grew(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if new_size >= layout.size() {
            grew(new_size - layout.size());
        } else {
            LIVE.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// The counters are process-wide, so the measured sections run one at a time.
static MEASURING: Mutex<()> = Mutex::new(());

/// What `work` returns, the most bytes it held above what was live when it started, and how long
/// it took.
fn measured<T>(work: impl FnOnce() -> T) -> (T, usize, Duration) {
    let _one_at_a_time = MEASURING.lock().unwrap_or_else(|e| e.into_inner());
    let before = LIVE.load(Ordering::Relaxed);
    PEAK.store(before, Ordering::Relaxed);
    let started = Instant::now();
    let out = work();
    let elapsed = started.elapsed();
    let peak = PEAK.load(Ordering::Relaxed).saturating_sub(before);
    (out, peak, elapsed)
}

const MIB: usize = 1024 * 1024;

fn row_of(cols: usize) -> Vec<Vec<LiteralValue>> {
    vec![
        (0..cols)
            .map(|c| LiteralValue::Number(c as f64 + 1.0))
            .collect(),
    ]
}

/// The widest row a sheet holds (16,384 cells) fits in a small fraction of a 32-bit address
/// space, and reads back cell for cell.
#[test]
fn a_one_row_array_reserves_for_one_row() {
    for cols in [5_000usize, 16_384] {
        let (view, peak, _) =
            measured(|| RangeView::from_owned_rows(row_of(cols), DateSystem::Excel1900));
        assert_eq!(view.dims(), (1, cols));
        assert_eq!(view.get_cell(0, 0), LiteralValue::Number(1.0));
        assert_eq!(
            view.get_cell(0, cols - 1),
            LiteralValue::Number(cols as f64)
        );
        assert!(
            peak < 256 * MIB,
            "a 1x{cols} array held {} MiB at its peak",
            peak / MIB
        );
    }
}

/// A short, wide block (the shape `HSTACK` and `TOROW` build) is held by its rows too.
#[test]
fn a_few_rows_of_many_columns_reserve_for_those_rows() {
    let rows: Vec<Vec<LiteralValue>> = (0..3)
        .map(|r| {
            (0..8_000)
                .map(|c| LiteralValue::Number((r * 8_000 + c) as f64))
                .collect()
        })
        .collect();
    let (view, peak, _) = measured(|| RangeView::from_owned_rows(rows, DateSystem::Excel1900));
    assert_eq!(view.dims(), (3, 8_000));
    assert_eq!(view.get_cell(2, 7_999), LiteralValue::Number(23_999.0));
    assert!(peak < 256 * MIB, "held {} MiB at its peak", peak / MIB);
}

/// An array taller than one chunk still splits at 32,768 rows and reads back across the split.
#[test]
fn a_tall_array_keeps_its_chunks_and_its_cells() {
    let rows: Vec<Vec<LiteralValue>> = (0..70_000)
        .map(|r| {
            vec![
                LiteralValue::Number(r as f64),
                LiteralValue::Text(format!("r{r}")),
            ]
        })
        .collect();
    let view = RangeView::from_owned_rows(rows, DateSystem::Excel1900);
    assert_eq!(view.dims(), (70_000, 2));
    for r in [0usize, 32_767, 32_768, 65_535, 65_536, 69_999] {
        assert_eq!(view.get_cell(r, 0), LiteralValue::Number(r as f64));
        assert_eq!(view.get_cell(r, 1), LiteralValue::Text(format!("r{r}")));
    }
}

/// An array with no rows, or rows with no cells, is still an empty view.
#[test]
fn an_empty_array_is_an_empty_view() {
    assert_eq!(
        RangeView::from_owned_rows(vec![], DateSystem::Excel1900).dims(),
        (0, 0)
    );
    assert_eq!(
        RangeView::from_owned_rows(vec![vec![]], DateSystem::Excel1900).dims(),
        (0, 0)
    );
}

/// `=SEQUENCE(1,5000)` typed into a cell spills its 5,000 numbers promptly and without the
/// gigabytes a chunk per column costs; so does a function that reads that row as a range.
#[test]
fn a_wide_sequence_in_a_cell_spills_within_time_and_memory() {
    for (formula, last_col, last) in [
        ("=SEQUENCE(1,5000)", 5_000u32, 5_000.0),
        ("=SEQUENCE(1,16384)", 16_384, 16_384.0),
        (
            "=HSTACK(SEQUENCE(1,8000),SEQUENCE(1,8000))",
            16_000,
            8_000.0,
        ),
        ("=TOROW(SEQUENCE(1,5000))", 5_000, 5_000.0),
    ] {
        let ((first, end), peak, elapsed) = measured(|| {
            let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
            e.set_cell_formula("Sheet1", 1, 1, parse(formula).unwrap())
                .unwrap();
            e.evaluate_all().unwrap();
            (
                e.get_cell_value("Sheet1", 1, 1),
                e.get_cell_value("Sheet1", 1, last_col),
            )
        });
        assert_eq!(first, Some(LiteralValue::Number(1.0)), "{formula}");
        assert_eq!(end, Some(LiteralValue::Number(last)), "{formula}");
        assert!(
            peak < 512 * MIB,
            "{formula} held {} MiB at its peak",
            peak / MIB
        );
        assert!(
            elapsed < Duration::from_secs(20),
            "{formula} took {elapsed:?}"
        );
    }
}

/// A scalar function over a wide array (`SUM`, `COUNT`) reads it as a range without the
/// per-column chunk either.
#[test]
fn an_aggregate_over_a_wide_array_stays_small() {
    for (formula, expected) in [
        ("=SUM(SEQUENCE(1,16384))", 134_225_920.0),
        ("=COUNT(SEQUENCE(1,5000))", 5_000.0),
        ("=COLUMNS(SEQUENCE(1,16384))", 16_384.0),
    ] {
        let (value, peak, elapsed) = measured(|| {
            let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
            e.set_cell_formula("Sheet1", 1, 1, parse(formula).unwrap())
                .unwrap();
            e.evaluate_all().unwrap();
            e.get_cell_value("Sheet1", 1, 1)
        });
        assert_eq!(value, Some(LiteralValue::Number(expected)), "{formula}");
        assert!(
            peak < 512 * MIB,
            "{formula} held {} MiB at its peak",
            peak / MIB
        );
        assert!(
            elapsed < Duration::from_secs(20),
            "{formula} took {elapsed:?}"
        );
    }
}
