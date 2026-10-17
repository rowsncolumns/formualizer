//! `ROWS` / `COLUMNS` read only the shape of their argument, never its cells,
//! so a range that contains the formula's own cell is not a circular
//! reference. Excel calculates `=ROWS($A$8:A8)` in `A8` as `1` (the running
//! row counter idiom filled down a column) and `=COLUMNS(A5:F5)` in `D5` as
//! `6`, with no circular-reference warning — the same as `ROW` / `COLUMN`,
//! which already take their argument by reference. A function that reads the
//! cells (`SUM(G1:G10)` in `G5`) is still circular.

use crate::engine::{CycleConfig, CycleDetection, CyclePolicy, Engine, EvalConfig};
use crate::reference::CellRef;
use crate::test_workbook::TestWorkbook;
use formualizer_common::LiteralValue;
use formualizer_parse::parser::parse;

fn excel_iteration_off() -> Engine<TestWorkbook> {
    let cfg = EvalConfig::default().with_cycle(CycleConfig {
        detection: CycleDetection::Runtime,
        policy: CyclePolicy::Zero,
    });
    Engine::new(TestWorkbook::new(), cfg)
}

fn set_formula(engine: &mut Engine<TestWorkbook>, row: u32, col: u32, f: &str) {
    engine
        .set_cell_formula("Sheet1", row, col, parse(f).expect("parse"))
        .expect("set formula");
}

fn num(engine: &Engine<TestWorkbook>, row: u32, col: u32) -> f64 {
    match engine.get_cell_value("Sheet1", row, col) {
        Some(LiteralValue::Number(n)) => n,
        Some(LiteralValue::Int(i)) => i as f64,
        other => panic!("expected number at r{row}c{col}, got {other:?}"),
    }
}

fn cycle_cells_1based(engine: &Engine<TestWorkbook>) -> Vec<(u32, u32)> {
    let sheet = engine.sheet_id("Sheet1").expect("Sheet1 registered");
    let mut cells: Vec<(u32, u32)> = engine
        .last_cycle_cells()
        .iter()
        .filter(|c: &&CellRef| c.sheet_id == sheet)
        .map(|c| (c.coord.row() + 1, c.coord.col() + 1))
        .collect();
    cells.sort_unstable();
    cells.dedup();
    cells
}

#[test]
fn rows_and_columns_over_their_own_cell_are_not_circular() {
    let mut engine = excel_iteration_off();
    set_formula(&mut engine, 8, 1, "=ROWS($A$8:A8)"); // A8
    set_formula(&mut engine, 9, 1, "=ROWS($A$8:A9)"); // A9 (filled down)
    set_formula(&mut engine, 5, 5, "=ROWS(E5)"); // E5
    set_formula(&mut engine, 5, 4, "=COLUMNS(A5:F5)"); // D5
    set_formula(&mut engine, 5, 6, "=ROW(F5)"); // F5
    set_formula(&mut engine, 9, 8, "=ROWS(H$2:H8)"); // H9
    engine.evaluate_all().expect("evaluate");

    assert_eq!(num(&engine, 8, 1), 1.0, "ROWS($A$8:A8) in A8");
    assert_eq!(num(&engine, 9, 1), 2.0, "ROWS($A$8:A9) in A9");
    assert_eq!(num(&engine, 5, 5), 1.0, "ROWS(E5) in E5");
    assert_eq!(num(&engine, 5, 4), 6.0, "COLUMNS(A5:F5) in D5");
    assert_eq!(num(&engine, 5, 6), 5.0, "ROW(F5) in F5");
    assert_eq!(num(&engine, 9, 8), 7.0, "ROWS(H$2:H8) in H9");
    assert!(
        cycle_cells_1based(&engine).is_empty(),
        "no circular-reference warning"
    );
}

#[test]
fn a_function_that_reads_its_own_cell_is_still_circular() {
    let mut engine = excel_iteration_off();
    set_formula(&mut engine, 5, 7, "=SUM(G1:G10)"); // G5
    set_formula(&mut engine, 8, 1, "=ROWS($A$8:A8)"); // A8
    engine.evaluate_all().expect("evaluate");

    assert_eq!(num(&engine, 5, 7), 0.0, "SUM over its own cell displays 0");
    assert_eq!(num(&engine, 8, 1), 1.0);
    assert_eq!(cycle_cells_1based(&engine), vec![(5, 7)]);
}

#[test]
fn rows_and_columns_still_measure_arrays() {
    let mut engine = excel_iteration_off();
    set_formula(&mut engine, 1, 1, "=ROWS({1;2;3})"); // A1
    set_formula(&mut engine, 1, 2, "=COLUMNS({1,2,3,4})"); // B1
    set_formula(&mut engine, 1, 3, "=ROWS(D1:D4>0)"); // C1
    engine.evaluate_all().expect("evaluate");

    assert_eq!(num(&engine, 1, 1), 3.0);
    assert_eq!(num(&engine, 1, 2), 4.0);
    assert_eq!(num(&engine, 1, 3), 4.0);
}

#[test]
fn isref_over_its_own_cell_is_not_circular() {
    let mut engine = excel_iteration_off();
    set_formula(&mut engine, 3, 2, "=ISREF(B3)"); // B3
    engine.evaluate_all().expect("evaluate");

    assert_eq!(
        engine.get_cell_value("Sheet1", 3, 2),
        Some(LiteralValue::Boolean(true))
    );
    assert!(cycle_cells_1based(&engine).is_empty());
}
