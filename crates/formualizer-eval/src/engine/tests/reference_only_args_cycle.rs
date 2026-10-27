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

// ── Athena-Intel/demo-app-monorepo#44763 coverage beyond the own-cell case ──

fn set_num(engine: &mut Engine<TestWorkbook>, sheet: &str, row: u32, col: u32, v: f64) {
    engine
        .set_cell_value(sheet, row, col, LiteralValue::Number(v))
        .expect("set value");
}

#[test]
fn rows_columns_measure_other_sheet_and_named_ranges() {
    use crate::engine::named_range::{NameScope, NamedDefinition};
    use crate::reference::{Coord, RangeRef};

    let mut engine = excel_iteration_off();
    engine.add_sheet("Sheet2").unwrap();
    let sheet2 = engine.sheet_id("Sheet2").unwrap();
    let start = CellRef::new(sheet2, Coord::from_excel(2, 2, true, true));
    let end = CellRef::new(sheet2, Coord::from_excel(5, 4, true, true));
    engine
        .define_name(
            "BLOCK",
            NamedDefinition::Range(RangeRef::new(start, end)),
            NameScope::Workbook,
        )
        .unwrap();

    set_formula(&mut engine, 1, 1, "=ROWS(Sheet2!A1:A5)"); // A1
    set_formula(&mut engine, 1, 2, "=COLUMNS(Sheet2!A1:C1)"); // B1
    set_formula(&mut engine, 1, 3, "=ROWS(BLOCK)"); // C1
    set_formula(&mut engine, 1, 4, "=COLUMNS(BLOCK)"); // D1
    set_formula(&mut engine, 1, 5, "=ROWS(A1:A3 A2:A5)"); // E1 — intersection
    engine.evaluate_all().expect("evaluate");

    assert_eq!(num(&engine, 1, 1), 5.0, "ROWS(Sheet2!A1:A5)");
    assert_eq!(num(&engine, 1, 2), 3.0, "COLUMNS(Sheet2!A1:C1)");
    assert_eq!(num(&engine, 1, 3), 4.0, "ROWS(BLOCK)");
    assert_eq!(num(&engine, 1, 4), 3.0, "COLUMNS(BLOCK)");
    assert_eq!(num(&engine, 1, 5), 2.0, "ROWS(A1:A3 A2:A5)");
    assert!(cycle_cells_1based(&engine).is_empty());
}

#[test]
fn rows_of_a_reference_returning_function_follows_its_inputs() {
    let mut engine = excel_iteration_off();
    set_num(&mut engine, "Sheet1", 1, 2, 7.0); // B1 = 7
    set_formula(&mut engine, 1, 1, "=ROWS(OFFSET(C1,0,0,B1,1))"); // A1
    set_formula(
        &mut engine,
        2,
        1,
        "=COLUMNS(INDEX(C1:H3,0,2):INDEX(C1:H3,0,5))",
    ); // A2
    engine.evaluate_all().expect("evaluate");
    assert_eq!(num(&engine, 1, 1), 7.0, "ROWS(OFFSET(.., B1, 1)) with B1=7");
    assert_eq!(num(&engine, 2, 1), 4.0, "COLUMNS over an INDEX:INDEX span");

    // The height is the by-ref argument, but the cells OFFSET reads to build it are still
    // dependencies: editing B1 recalculates A1.
    set_num(&mut engine, "Sheet1", 1, 2, 3.0);
    engine.evaluate_all().expect("re-evaluate");
    assert_eq!(
        num(&engine, 1, 1),
        3.0,
        "ROWS(OFFSET(.., B1, 1)) after B1=3"
    );
}

#[test]
fn rows_over_own_cell_nested_in_expressions_and_sumproduct() {
    let mut engine = excel_iteration_off();
    for r in 1..=5 {
        set_num(&mut engine, "Sheet1", r, 3, r as f64 * 10.0); // C1:C5 = 10..50
    }
    set_formula(&mut engine, 8, 1, "=ROWS($A$8:A8)*2"); // A8
    set_formula(&mut engine, 9, 1, "=IF(ROWS($A$8:A9)>1,ROWS($A$8:A9),-1)"); // A9
    set_formula(
        &mut engine,
        10,
        1,
        "=EDATE(DATE(2024,1,1),ROWS($A$8:A10)-1)",
    ); // A10
    set_formula(&mut engine, 7, 3, "=SUMPRODUCT(C1:C5)/ROWS(C1:C5)"); // C7
    set_formula(&mut engine, 3, 3, "=SUMPRODUCT(ROW(C1:C5))"); // C3 — row numbers, own cell inside
    set_formula(&mut engine, 5, 4, "=MAX(COLUMN(A5:F5))"); // D5 — own cell inside the range
    engine.evaluate_all().expect("evaluate");

    assert_eq!(num(&engine, 8, 1), 2.0, "ROWS($A$8:A8)*2 in A8");
    assert_eq!(num(&engine, 9, 1), 2.0, "IF(ROWS($A$8:A9)>1, …) in A9");
    // EDATE(2024-01-01, 2) = 2024-03-01 = serial 45352
    assert_eq!(
        num(&engine, 10, 1),
        45352.0,
        "EDATE(.., ROWS($A$8:A10)-1) in A10"
    );
    // C3 is a formula whose own value is not read: average over the five numbers excluding it.
    assert_eq!(num(&engine, 3, 3), 15.0, "SUMPRODUCT(ROW(C1:C5)) in C3");
    assert_eq!(num(&engine, 5, 4), 6.0, "MAX(COLUMN(A5:F5)) in D5");
    assert_eq!(
        num(&engine, 7, 3),
        (10.0 + 20.0 + 15.0 + 40.0 + 50.0) / 5.0,
        "SUMPRODUCT(C1:C5)/ROWS(C1:C5) in C7"
    );
    assert!(cycle_cells_1based(&engine).is_empty());
}
