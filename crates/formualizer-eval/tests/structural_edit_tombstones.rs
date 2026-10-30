//! A row/column delete followed by an insert must not lose a formula that moved onto the address
//! of a deleted cell (rowsncolumns/spreadsheet#1286).
//!
//! Deleting a row or column tombstones the vertices it held: `VertexEditor::remove_vertex` keeps
//! the slot (vertex ids are stable) and marks it deleted. `DependencyGraph::vertices_in_sheet`
//! used to yield those tombstones, so every later structural edit shifted them with the live
//! vertices and re-registered each one's new address in the cell map. A tombstone allocated
//! after the live vertex shifted onto the same address claimed it: the cell read back with no
//! formula (only its stale value), the orphaned formula vertex kept evaluating into that
//! coordinate, and a value typed there afterwards was overwritten by the orphan's result.

use formualizer_common::LiteralValue;
use formualizer_eval::engine::{Engine, EvalConfig};
use formualizer_eval::test_workbook::TestWorkbook;
use formualizer_parse::parser::parse;
use formualizer_parse::pretty::excel_formula;

fn n(v: f64) -> LiteralValue {
    LiteralValue::Number(v)
}

fn engine() -> Engine<TestWorkbook> {
    Engine::new(TestWorkbook::new(), EvalConfig::default())
}

fn set_value(e: &mut Engine<TestWorkbook>, row: u32, col: u32, v: f64) {
    e.set_cell_value("Sheet1", row, col, n(v)).unwrap();
}

fn set_formula(e: &mut Engine<TestWorkbook>, row: u32, col: u32, formula: &str) {
    e.set_cell_formula("Sheet1", row, col, parse(formula).unwrap())
        .unwrap();
}

fn formula_text(e: &Engine<TestWorkbook>, row: u32, col: u32) -> Option<String> {
    e.get_cell("Sheet1", row, col)
        .and_then(|(ast, _)| ast)
        .map(|ast| excel_formula(&ast))
}

fn value(e: &Engine<TestWorkbook>, row: u32, col: u32) -> LiteralValue {
    e.get_cell_value("Sheet1", row, col)
        .unwrap_or(LiteralValue::Empty)
}

/// A1:A6 = 1..6 and D4 `=A1*2`, evaluated.
fn column_a_and_formula_in_d4() -> Engine<TestWorkbook> {
    let mut e = engine();
    for r in 1..=6 {
        set_value(&mut e, r, 1, r as f64);
    }
    set_formula(&mut e, 4, 4, "=A1*2");
    e.evaluate_all().unwrap();
    assert_eq!(value(&e, 4, 4), n(2.0));
    e
}

/// Delete row 3 (D4's formula moves to D3) and insert a row at 3 (it moves back to D4); D4 then
/// holds the formula, D3 is empty, and D4 recalculates when A1 changes.
fn assert_row_delete_insert_round_trips_d4(mut e: Engine<TestWorkbook>) {
    e.delete_rows("Sheet1", 3, 1).unwrap();
    assert_eq!(formula_text(&e, 3, 4).as_deref(), Some("=A1*2"));

    e.insert_rows("Sheet1", 3, 1).unwrap();
    assert_eq!(formula_text(&e, 4, 4).as_deref(), Some("=A1*2"));
    assert_eq!(formula_text(&e, 3, 4), None);
    e.evaluate_all().unwrap();
    assert_eq!(value(&e, 4, 4), n(2.0));

    set_value(&mut e, 1, 1, 10.0);
    e.evaluate_all().unwrap();
    assert_eq!(value(&e, 4, 4), n(20.0));
    assert_eq!(value(&e, 3, 4), LiteralValue::Empty);
}

/// rowsncolumns/spreadsheet#1286: a literal written into D3 — the row about to be deleted, same
/// column — AFTER the formula. Its tombstone outranked the formula vertex at D4 on the insert.
#[test]
fn row_delete_then_insert_keeps_a_formula_that_moved_over_a_deleted_literal() {
    let mut e = column_a_and_formula_in_d4();
    set_value(&mut e, 3, 4, 16.0);
    assert_row_delete_insert_round_trips_d4(e);
}

/// The same edit pair with the literal written BEFORE the formula (a lower vertex id) and with
/// nothing in D3 at all: the tombstone order never decides which vertex owns D4.
#[test]
fn row_delete_then_insert_keeps_the_formula_whatever_the_deleted_row_held() {
    let mut e = engine();
    for r in 1..=6 {
        set_value(&mut e, r, 1, r as f64);
    }
    set_value(&mut e, 3, 4, 16.0);
    set_formula(&mut e, 4, 4, "=A1*2");
    e.evaluate_all().unwrap();
    assert_row_delete_insert_round_trips_d4(e);

    assert_row_delete_insert_round_trips_d4(column_a_and_formula_in_d4());
}

/// The issue's write-back: after the loss, a literal typed at D4 landed on the tombstone while
/// the orphaned formula vertex kept D4 as its home, so the next recalc wrote `=A1*2`'s result
/// over the literal. The literal must stand and the formula must be gone.
#[test]
fn a_literal_typed_over_the_round_tripped_formula_is_not_overwritten_on_recalc() {
    let mut e = column_a_and_formula_in_d4();
    set_value(&mut e, 3, 4, 16.0);
    e.delete_rows("Sheet1", 3, 1).unwrap();
    e.insert_rows("Sheet1", 3, 1).unwrap();
    e.evaluate_all().unwrap();

    set_value(&mut e, 4, 4, 16.0);
    set_value(&mut e, 1, 1, 606.0);
    e.evaluate_all().unwrap();
    assert_eq!(formula_text(&e, 4, 4), None);
    assert_eq!(value(&e, 4, 4), n(16.0));
}

/// The column twin: A1:F1 = 1..6, D4 `=A1*2`, a literal written into C4 after the formula;
/// delete column C (the formula moves to C4) and insert a column at C (it moves back to D4).
#[test]
fn column_delete_then_insert_keeps_a_formula_that_moved_over_a_deleted_literal() {
    let mut e = engine();
    for c in 1..=6 {
        set_value(&mut e, 1, c, c as f64);
    }
    set_formula(&mut e, 4, 4, "=A1*2");
    e.evaluate_all().unwrap();
    set_value(&mut e, 4, 3, 16.0);

    e.delete_columns("Sheet1", 3, 1).unwrap();
    assert_eq!(formula_text(&e, 4, 3).as_deref(), Some("=A1*2"));

    e.insert_columns("Sheet1", 3, 1).unwrap();
    assert_eq!(formula_text(&e, 4, 4).as_deref(), Some("=A1*2"));
    assert_eq!(formula_text(&e, 4, 3), None);
    // The row-1 literals shifted over C1's tombstone too.
    assert_eq!(value(&e, 1, 4), n(4.0));
    assert_eq!(value(&e, 1, 5), n(5.0));

    set_value(&mut e, 1, 1, 10.0);
    e.evaluate_all().unwrap();
    assert_eq!(value(&e, 4, 4), n(20.0));
}

/// Two delete/insert cycles: a tombstone left by the first delete must not resurface on the
/// second one either (it used to be shifted, deleted again and re-registered on every edit).
#[test]
fn repeated_row_delete_insert_cycles_keep_the_formula() {
    let mut e = column_a_and_formula_in_d4();
    for _ in 0..2 {
        set_value(&mut e, 3, 4, 16.0);
        e.delete_rows("Sheet1", 3, 1).unwrap();
        e.insert_rows("Sheet1", 3, 1).unwrap();
        assert_eq!(formula_text(&e, 4, 4).as_deref(), Some("=A1*2"));
    }
    set_value(&mut e, 1, 1, 7.0);
    e.evaluate_all().unwrap();
    assert_eq!(value(&e, 4, 4), n(14.0));
}

/// The same tombstone, read by `duplicate_sheet` instead of a second structural edit: after the
/// delete, D3 holds the formula moved up from D4 and the deleted literal's tombstone. The copy
/// must carry the formula at D3, not an empty vertex cloned from the tombstone.
#[test]
fn a_sheet_duplicated_after_a_row_delete_keeps_the_formula_that_moved_over_a_deleted_literal() {
    let mut e = column_a_and_formula_in_d4();
    set_value(&mut e, 3, 4, 16.0);
    e.delete_rows("Sheet1", 3, 1).unwrap();

    e.duplicate_sheet("Sheet1", "Copy").unwrap();
    let (ast, _) = e.get_cell("Copy", 3, 4).expect("copied cell");
    assert!(ast.is_some(), "the copy lost the formula at D3");
    e.set_cell_value("Copy", 1, 1, n(10.0)).unwrap();
    e.evaluate_all().unwrap();
    assert_eq!(e.get_cell_value("Copy", 3, 4), Some(n(20.0)));
}
