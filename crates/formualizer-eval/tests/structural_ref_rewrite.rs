//! Excel parity for reference rewriting under structural row/column edits
//! (rowsncolumns/spreadsheet#647, unit so-ref-rewrite): whole-row / whole-column references
//! shift along their own axis (REF-04, REF-05), a formula whose text does not change but whose
//! cells moved recomputes, a sheet-qualified reference to a deleted target prints as
//! `Sheet1!#REF!` (REF-15), a spill reference to a deleted anchor (`=#REF!#`, ANC-15) and a
//! structured reference to a deleted table column (`Table1[@#REF!]`, TBL-02) both evaluate to
//! `#REF!`.

use formualizer_common::{ExcelErrorKind, LiteralValue};
use formualizer_eval::engine::{Engine, EvalConfig};
use formualizer_eval::reference::{CellRef, Coord, RangeRef};
use formualizer_eval::test_workbook::TestWorkbook;
use formualizer_parse::parser::parse;
use formualizer_parse::pretty::excel_formula;

fn n(v: f64) -> LiteralValue {
    LiteralValue::Number(v)
}

fn engine() -> Engine<TestWorkbook> {
    let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
    e.add_sheet("Sheet2").unwrap();
    e
}

fn set_formula(e: &mut Engine<TestWorkbook>, sheet: &str, row: u32, col: u32, formula: &str) {
    e.set_cell_formula(sheet, row, col, parse(formula).unwrap())
        .unwrap();
}

fn formula_text(e: &Engine<TestWorkbook>, sheet: &str, row: u32, col: u32) -> String {
    let (ast, _) = e.get_cell(sheet, row, col).expect("cell");
    excel_formula(&ast.expect("formula cell"))
}

fn value(e: &Engine<TestWorkbook>, sheet: &str, row: u32, col: u32) -> LiteralValue {
    e.get_cell_value(sheet, row, col)
        .unwrap_or(LiteralValue::Empty)
}

fn err_kind(v: &LiteralValue) -> ExcelErrorKind {
    match v {
        LiteralValue::Error(e) => e.kind,
        other => panic!("expected an error, got {other:?}"),
    }
}

/// REF-04: A3..C3 = 1,2,3; E1 `=SUM(3:3)`, F1 `=SUM(B:B)`; insert a row at 2 → E1 reads
/// `=SUM(4:4)` (6), F1 stays `=SUM(B:B)` (2).
#[test]
fn whole_row_reference_shifts_on_row_insert_and_whole_column_is_unaffected() {
    let mut e = engine();
    for (c, v) in [(1, 1.0), (2, 2.0), (3, 3.0)] {
        e.set_cell_value("Sheet1", 3, c, n(v)).unwrap();
    }
    set_formula(&mut e, "Sheet1", 1, 5, "=SUM(3:3)");
    set_formula(&mut e, "Sheet1", 1, 6, "=SUM(B:B)");
    e.evaluate_all().unwrap();
    assert_eq!(value(&e, "Sheet1", 1, 5), n(6.0));
    assert_eq!(value(&e, "Sheet1", 1, 6), n(2.0));

    e.insert_rows("Sheet1", 2, 1).unwrap();
    e.evaluate_all().unwrap();
    assert_eq!(formula_text(&e, "Sheet1", 1, 5), "=SUM(4:4)");
    assert_eq!(value(&e, "Sheet1", 1, 5), n(6.0));
    assert_eq!(formula_text(&e, "Sheet1", 1, 6), "=SUM(B:B)");
    assert_eq!(value(&e, "Sheet1", 1, 6), n(2.0));
}

/// REF-05: B1..B3 = 1,2,3; E1 `=SUM(B:B)`; insert a column at A → the formula (now F1) reads
/// `=SUM(C:C)` (6).
#[test]
fn whole_column_reference_shifts_on_column_insert() {
    let mut e = engine();
    for (r, v) in [(1, 1.0), (2, 2.0), (3, 3.0)] {
        e.set_cell_value("Sheet1", r, 2, n(v)).unwrap();
    }
    set_formula(&mut e, "Sheet1", 1, 5, "=SUM(B:B)");
    e.evaluate_all().unwrap();
    assert_eq!(value(&e, "Sheet1", 1, 5), n(6.0));

    e.insert_columns("Sheet1", 1, 1).unwrap();
    e.evaluate_all().unwrap();
    assert_eq!(formula_text(&e, "Sheet1", 1, 6), "=SUM(C:C)");
    assert_eq!(value(&e, "Sheet1", 1, 6), n(6.0));
}

/// A whole-column reference's text is untouched by a row insert, but the cells under it moved:
/// `=INDEX(B:B,2)` must recompute (2 → blank) instead of keeping its pre-edit cached value.
#[test]
fn unchanged_whole_column_reference_recomputes_after_row_insert() {
    let mut e = engine();
    for (r, v) in [(1, 1.0), (2, 2.0), (3, 3.0)] {
        e.set_cell_value("Sheet1", r, 2, n(v)).unwrap();
    }
    set_formula(&mut e, "Sheet1", 1, 5, "=INDEX(B:B,2)");
    e.evaluate_all().unwrap();
    assert_eq!(value(&e, "Sheet1", 1, 5), n(2.0));

    e.insert_rows("Sheet1", 2, 1).unwrap();
    e.evaluate_all().unwrap();
    assert_eq!(formula_text(&e, "Sheet1", 1, 5), "=INDEX(B:B,2)");
    assert_ne!(
        value(&e, "Sheet1", 1, 5),
        n(2.0),
        "the row insert moved B2 down; the stripe dependent must not keep its stale 2"
    );
}

/// REF-15: Sheet2!B1 `=Sheet1!A3`; delete Sheet1 row 3 → `=Sheet1!#REF!` → #REF!.
#[test]
fn sheet_qualified_reference_to_deleted_row_prints_qualified_ref_error() {
    let mut e = engine();
    e.set_cell_value("Sheet1", 3, 1, n(9.0)).unwrap();
    set_formula(&mut e, "Sheet2", 1, 2, "=Sheet1!A3");
    e.evaluate_all().unwrap();
    assert_eq!(value(&e, "Sheet2", 1, 2), n(9.0));

    e.delete_rows("Sheet1", 3, 1).unwrap();
    e.evaluate_all().unwrap();
    assert_eq!(formula_text(&e, "Sheet2", 1, 2), "=Sheet1!#REF!");
    assert_eq!(err_kind(&value(&e, "Sheet2", 1, 2)), ExcelErrorKind::Ref);
    // The printed text re-parses to the same value.
    set_formula(&mut e, "Sheet2", 2, 2, "=Sheet1!#REF!");
    e.evaluate_all().unwrap();
    assert_eq!(err_kind(&value(&e, "Sheet2", 2, 2)), ExcelErrorKind::Ref);
}

/// REF-12 stays: an unqualified reference to a deleted row is the bare `#REF!` token.
#[test]
fn unqualified_reference_to_deleted_row_prints_bare_ref_error() {
    let mut e = engine();
    e.set_cell_value("Sheet1", 3, 1, n(9.0)).unwrap();
    set_formula(&mut e, "Sheet1", 1, 3, "=A3+1");
    e.evaluate_all().unwrap();
    e.delete_rows("Sheet1", 3, 1).unwrap();
    e.evaluate_all().unwrap();
    assert_eq!(formula_text(&e, "Sheet1", 1, 3), "=#REF!+1");
    assert_eq!(err_kind(&value(&e, "Sheet1", 1, 3)), ExcelErrorKind::Ref);
}

/// ANC-15: the spill reference a deleted anchor leaves behind, `=#REF!#`, evaluates to `#REF!`.
#[test]
fn spill_reference_to_deleted_anchor_is_ref_error() {
    let mut e = engine();
    set_formula(&mut e, "Sheet1", 5, 6, "=#REF!#");
    set_formula(&mut e, "Sheet1", 6, 6, "=SUM(#REF!#)");
    e.evaluate_all().unwrap();
    assert_eq!(err_kind(&value(&e, "Sheet1", 5, 6)), ExcelErrorKind::Ref);
    assert_eq!(err_kind(&value(&e, "Sheet1", 6, 6)), ExcelErrorKind::Ref);
    assert_eq!(formula_text(&e, "Sheet1", 5, 6), "=#REF!#");
}

/// TBL-02 / TBL-06: a structured reference to a deleted table column — Excel writes the column
/// specifier as `#REF!` — is `#REF!` in both the this-row and the whole-column form.
#[test]
fn structured_reference_to_ref_error_column_is_ref_error() {
    let mut e = engine();
    e.set_cell_value("Sheet1", 1, 1, LiteralValue::Text("Id".into()))
        .unwrap();
    e.set_cell_value("Sheet1", 1, 2, LiteralValue::Text("Price".into()))
        .unwrap();
    e.set_cell_value("Sheet1", 2, 1, n(1.0)).unwrap();
    e.set_cell_value("Sheet1", 2, 2, n(3.0)).unwrap();
    let sid = e.sheet_id("Sheet1").unwrap();
    e.define_table(
        "Table1",
        RangeRef::new(
            CellRef::new(sid, Coord::from_excel(1, 1, true, true)),
            CellRef::new(sid, Coord::from_excel(2, 2, true, true)),
        ),
        true,
        vec!["Id".into(), "Price".into()],
        false,
    )
    .unwrap();
    set_formula(&mut e, "Sheet1", 2, 5, "=SUM(Table1[Price])");
    e.evaluate_all().unwrap();
    assert_eq!(value(&e, "Sheet1", 2, 5), n(3.0));

    set_formula(&mut e, "Sheet1", 2, 6, "=SUM(Table1[#REF!])");
    e.evaluate_all().unwrap();
    assert_eq!(err_kind(&value(&e, "Sheet1", 2, 6)), ExcelErrorKind::Ref);

    // The this-row form resolves at ingest; a missing column is a `#REF!` rejection (never
    // `#NAME?` / `#VALUE!`), so the host can surface it as the cell's value.
    match e.set_cell_formula("Sheet1", 2, 4, parse("=Table1[@#REF!]*2").unwrap()) {
        Err(err) => assert_eq!(err.kind, ExcelErrorKind::Ref),
        Ok(()) => {
            e.evaluate_all().unwrap();
            assert_eq!(err_kind(&value(&e, "Sheet1", 2, 4)), ExcelErrorKind::Ref);
        }
    }
}
