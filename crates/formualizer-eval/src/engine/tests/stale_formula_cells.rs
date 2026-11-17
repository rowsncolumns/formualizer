//! `Engine::stale_formula_cells`: the formula cells waiting to be recalculated, for a host that
//! calculates on demand (Excel's Manual calculation) and shows stale cells (Excel's "Format Stale
//! Values"; rowsncolumns/spreadsheet, demo-app-monorepo#57258 D-19). Read-only.

use crate::engine::{Engine, EvalConfig, StaleFormulaCell};
use crate::test_workbook::TestWorkbook;
use formualizer_common::LiteralValue;
use formualizer_parse::parser::parse;

fn engine() -> Engine<TestWorkbook> {
    Engine::new(TestWorkbook::new(), EvalConfig::default())
}

fn formula(engine: &mut Engine<TestWorkbook>, sheet: &str, row: u32, col: u32, f: &str) {
    engine
        .set_cell_formula(sheet, row, col, parse(f).expect("parse"))
        .expect("set formula");
}

fn number(engine: &mut Engine<TestWorkbook>, sheet: &str, row: u32, col: u32, n: f64) {
    engine
        .set_cell_value(sheet, row, col, LiteralValue::Number(n))
        .expect("set value");
}

fn stale(engine: &Engine<TestWorkbook>) -> Vec<(String, u32, u32)> {
    engine
        .stale_formula_cells()
        .into_iter()
        .map(|c| (c.sheet, c.row, c.col))
        .collect()
}

fn at(sheet: &str, row: u32, col: u32) -> (String, u32, u32) {
    (sheet.to_string(), row, col)
}

/// Sheet1: A1 = 1, B1 = A1*2, C1 = B1+1 (reads A1 through B1), D1 = 10 (unrelated), E1 = D1*3;
/// Sheet2: A1 = Sheet1!C1. Evaluated once.
fn workbook() -> Engine<TestWorkbook> {
    let mut e = engine();
    e.add_sheet("Sheet2").unwrap();
    number(&mut e, "Sheet1", 1, 1, 1.0);
    formula(&mut e, "Sheet1", 1, 2, "=A1*2");
    formula(&mut e, "Sheet1", 1, 3, "=B1+1");
    number(&mut e, "Sheet1", 1, 4, 10.0);
    formula(&mut e, "Sheet1", 1, 5, "=D1*3");
    formula(&mut e, "Sheet2", 1, 1, "=Sheet1!C1");
    e.evaluate_all().unwrap();
    e
}

#[test]
fn nothing_is_stale_after_an_evaluation() {
    let e = workbook();
    assert_eq!(stale(&e), Vec::<(String, u32, u32)>::new());
}

#[test]
fn an_edit_left_unevaluated_makes_every_formula_that_reads_it_stale_and_nothing_else() {
    let mut e = workbook();
    number(&mut e, "Sheet1", 1, 1, 5.0);
    // Direct and indirect dependents, on this sheet and another; not E1 (reads D1), not the
    // edited value cell.
    assert_eq!(
        stale(&e),
        vec![at("Sheet1", 1, 2), at("Sheet1", 1, 3), at("Sheet2", 1, 1)]
    );
    // Their values are still the old ones: nothing was evaluated.
    assert_eq!(
        e.get_cell_value("Sheet1", 1, 2),
        Some(LiteralValue::Number(2.0))
    );

    e.evaluate_all().unwrap();
    assert_eq!(stale(&e), Vec::<(String, u32, u32)>::new());
    assert_eq!(
        e.get_cell_value("Sheet2", 1, 1),
        Some(LiteralValue::Number(11.0))
    );
}

#[test]
fn a_formula_entered_and_not_evaluated_is_stale() {
    let mut e = workbook();
    formula(&mut e, "Sheet1", 3, 1, "=E1+1");
    assert_eq!(stale(&e), vec![at("Sheet1", 3, 1)]);
}

#[test]
fn a_forced_recalculation_marks_every_formula_stale() {
    let mut e = workbook();
    e.mark_all_formulas_dirty();
    assert_eq!(
        stale(&e),
        vec![
            at("Sheet1", 1, 2),
            at("Sheet1", 1, 3),
            at("Sheet1", 1, 5),
            at("Sheet2", 1, 1)
        ]
    );
}

#[test]
fn a_formula_replaced_by_a_value_is_no_longer_listed() {
    let mut e = workbook();
    number(&mut e, "Sheet1", 1, 1, 5.0);
    number(&mut e, "Sheet1", 1, 2, 7.0);
    // B1 is a value now; C1 and Sheet2!A1 (which read it) are stale.
    assert_eq!(stale(&e), vec![at("Sheet1", 1, 3), at("Sheet2", 1, 1)]);
}

#[test]
fn volatile_formulas_and_what_they_feed_are_always_waiting_and_flagged() {
    let mut e = workbook();
    formula(&mut e, "Sheet1", 5, 1, "=RAND()");
    formula(&mut e, "Sheet1", 5, 2, "=A5*0");
    e.evaluate_all().unwrap();
    // Every evaluation re-runs a volatile formula and what reads it, so they stay listed.
    let cells = e.stale_formula_cells();
    assert_eq!(
        cells,
        vec![
            StaleFormulaCell {
                sheet: "Sheet1".to_string(),
                row: 5,
                col: 1,
                volatile: true,
            },
            StaleFormulaCell {
                sheet: "Sheet1".to_string(),
                row: 5,
                col: 2,
                volatile: false,
            },
        ]
    );
}

#[test]
fn reading_the_stale_cells_changes_nothing() {
    let mut e = workbook();
    number(&mut e, "Sheet1", 1, 1, 5.0);
    let first = stale(&e);
    assert_eq!(stale(&e), first, "a second read sees the same");
    assert_eq!(
        e.evaluation_vertices().len(),
        first.len(),
        "the engine's own schedule is untouched"
    );
    e.evaluate_all().unwrap();
    assert_eq!(
        e.get_cell_value("Sheet1", 1, 3),
        Some(LiteralValue::Number(11.0))
    );
}
