//! rowsncolumns/spreadsheet#939 K-01: deleting the row / column that holds a dynamic-array anchor
//! trapped the engine on wasm32. `SpillConfig::max_spill_cells` is a `u64` whose default
//! (`EXCEL_SHEET_CELLS` = 2^34) does not fit a 32-bit `usize`; the spill-snapshot code cast it
//! with `as usize`, got 0, truncated the snapshot to nothing and hit
//! `first().expect("non-empty spill cells")`. Excel: deleting the anchor row removes the formula
//! and its spill, dependents of the spill read `#REF!`; deleting a row INSIDE the spill re-spills.
//!
//! The engine cases below pass on a 64-bit host with or without the fix — the truncation only
//! bites where `usize` is 32 bits. Run them under a 32-bit target to see them red before the fix:
//! `cargo test --target wasm32-wasip1 -p formualizer-eval spill_anchor_delete_939`
//! (with `CARGO_TARGET_WASM32_WASIP1_RUNNER=wasmtime`).

use crate::engine::{EXCEL_SHEET_CELLS, EvalConfig, eval::Engine};
use crate::test_workbook::TestWorkbook;
use formualizer_common::LiteralValue;
use formualizer_parse::parser::parse;

const SHEET: &str = "Sheet1";

fn serial_eval_config() -> EvalConfig {
    EvalConfig {
        enable_parallel: false,
        ..Default::default()
    }
}

fn engine() -> Engine<TestWorkbook> {
    Engine::new(TestWorkbook::new(), serial_eval_config())
}

fn set_formula(engine: &mut Engine<TestWorkbook>, row: u32, col: u32, formula: &str) {
    engine
        .set_cell_formula(SHEET, row, col, parse(formula).unwrap())
        .unwrap();
}

fn number(engine: &Engine<TestWorkbook>, row: u32, col: u32) -> Option<f64> {
    match engine.get_cell_value(SHEET, row, col) {
        Some(LiteralValue::Number(n)) => Some(n),
        Some(LiteralValue::Int(i)) => Some(i as f64),
        _ => None,
    }
}

/// The snapshot bound derived from the u64 cap saturates instead of truncating: it is never 0,
/// whatever the pointer width, and a cap above `usize::MAX` clamps to `usize::MAX`.
#[test]
fn snapshot_cell_cap_saturates_and_is_never_zero() {
    let default_cap = EvalConfig::default().spill.snapshot_cell_cap();
    assert!(
        default_cap >= 1,
        "the default cap must keep at least one cell"
    );
    #[cfg(target_pointer_width = "64")]
    assert_eq!(default_cap, EXCEL_SHEET_CELLS as usize);
    #[cfg(target_pointer_width = "32")]
    assert_eq!(
        default_cap,
        usize::MAX,
        "2^34 does not fit a 32-bit usize — saturate, never wrap to 0"
    );
    assert_eq!(
        EvalConfig::default()
            .with_max_spill_cells(u64::MAX)
            .spill
            .snapshot_cell_cap(),
        usize::MAX
    );
    assert_eq!(
        EvalConfig::default()
            .with_max_spill_cells(0)
            .spill
            .snapshot_cell_cap(),
        1,
        "a zero cap must not empty a snapshot of a committed spill"
    );
    assert_eq!(
        EvalConfig::default()
            .with_max_spill_cells(7)
            .spill
            .snapshot_cell_cap(),
        7
    );
}

/// V1: `=SEQUENCE(3)` at A1, delete row 1 (the anchor row). The formula and its whole spill are
/// gone — no trap, no orphan values left in A1:A3.
#[test]
fn deleting_the_anchor_row_of_a_vertical_spill_removes_the_spill() {
    let mut engine = engine();
    set_formula(&mut engine, 1, 1, "=SEQUENCE(3)");
    engine.evaluate_all().unwrap();
    assert_eq!(number(&engine, 3, 1), Some(3.0), "spill materialised");

    engine.delete_rows(SHEET, 1, 1).unwrap();
    engine.evaluate_all().unwrap();

    for row in 1..=3 {
        assert_eq!(
            engine.get_cell_value(SHEET, row, 1),
            None,
            "A{row} keeps no orphan spill value after the anchor row is deleted"
        );
    }
}

/// V7: `=SEQUENCE(1,3)` at A1 (horizontal spill), delete column A (the anchor column).
#[test]
fn deleting_the_anchor_column_of_a_horizontal_spill_removes_the_spill() {
    let mut engine = engine();
    set_formula(&mut engine, 1, 1, "=SEQUENCE(1,3)");
    engine.evaluate_all().unwrap();
    assert_eq!(number(&engine, 1, 3), Some(3.0), "spill materialised");

    engine.delete_columns(SHEET, 1, 1).unwrap();
    engine.evaluate_all().unwrap();

    for col in 1..=3 {
        assert_eq!(
            engine.get_cell_value(SHEET, 1, col),
            None,
            "R1C{col} keeps no orphan spill value after the anchor column is deleted"
        );
    }
}

/// V19 / V20 / V16: array literals, range spills and 2-D spills take the same snapshot path.
#[test]
fn deleting_the_anchor_row_of_literal_range_and_two_d_spills_removes_them() {
    for formula in ["={1;2;3}", "=C1:C3", "=SEQUENCE(2,2)"] {
        let mut engine = engine();
        for row in 1..=3 {
            engine
                .set_cell_value(SHEET, row, 3, LiteralValue::Int(row as i64))
                .unwrap();
        }
        set_formula(&mut engine, 1, 1, formula);
        engine.evaluate_all().unwrap();
        assert_eq!(
            number(&engine, 1, 1),
            Some(1.0),
            "{formula}: anchor evaluated"
        );

        engine.delete_rows(SHEET, 1, 1).unwrap();
        engine.evaluate_all().unwrap();

        assert_eq!(
            engine.get_cell_value(SHEET, 1, 1),
            None,
            "{formula}: A1 empty after the anchor row is deleted"
        );
        assert_eq!(
            engine.get_cell_value(SHEET, 1, 2),
            None,
            "{formula}: B1 holds no orphan projection"
        );
        // Row 2 slid into row 1: for the vertical spills that row was a projection (gone with its
        // anchor), never a literal 2 left behind.
        assert_ne!(
            number(&engine, 1, 1),
            Some(2.0),
            "{formula}: no orphan literal 2 in A1"
        );
    }
}

/// V17 / V18: a multi-row delete covering the anchor and every projection (and one row more).
#[test]
fn deleting_every_row_of_a_spill_at_once_removes_it() {
    for count in [3u32, 4] {
        let mut engine = engine();
        set_formula(&mut engine, 1, 1, "=SEQUENCE(3)");
        engine.evaluate_all().unwrap();

        engine.delete_rows(SHEET, 1, count).unwrap();
        engine.evaluate_all().unwrap();

        for row in 1..=3 {
            assert_eq!(
                engine.get_cell_value(SHEET, row, 1),
                None,
                "delete rows 1..={count}: A{row} empty"
            );
        }
    }
}

/// V2: deleting a row INSIDE the spill (not the anchor) keeps the anchor and must not trap. The
/// engine shifts cells but leaves re-planning the spill to its host (rnc-store rebuilds the array
/// so it re-spills into the slid row, Excel's behaviour) — the assertion here is the anchor's.
#[test]
fn deleting_a_row_inside_the_spill_keeps_the_anchor() {
    let mut engine = engine();
    set_formula(&mut engine, 1, 1, "=SEQUENCE(3)");
    engine.evaluate_all().unwrap();

    engine.delete_rows(SHEET, 2, 1).unwrap();
    engine.evaluate_all().unwrap();

    assert_eq!(
        number(&engine, 1, 1),
        Some(1.0),
        "the anchor survives a delete inside its spill"
    );
}

/// V22: a dependent of the spill (`=SUM(A1#)`) below the spill reads `#REF!` once the anchor row
/// is deleted — never the stale 6.
#[test]
fn dependents_of_a_deleted_spill_anchor_read_ref_error() {
    let mut engine = engine();
    set_formula(&mut engine, 1, 1, "=SEQUENCE(3)");
    set_formula(&mut engine, 6, 3, "=SUM(A1#)");
    engine.evaluate_all().unwrap();
    assert_eq!(
        number(&engine, 6, 3),
        Some(6.0),
        "SUM over the spill before the delete"
    );

    engine.delete_rows(SHEET, 1, 1).unwrap();
    engine.evaluate_all().unwrap();

    match engine.get_cell_value(SHEET, 5, 3) {
        Some(LiteralValue::Error(e)) => assert_eq!(e, "#REF!"),
        other => panic!("expected #REF! in the slid dependent (C5), got {other:?}"),
    }
}
