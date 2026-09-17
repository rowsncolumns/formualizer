use crate::engine::{Engine, EvalConfig};
use crate::test_workbook::TestWorkbook;
use formualizer_common::LiteralValue;
use formualizer_parse::parser::parse;

fn serial_eval_config() -> EvalConfig {
    EvalConfig {
        enable_parallel: false,
        ..Default::default()
    }
}

#[test]
fn implicit_intersection_column_vector_selects_by_row() {
    let wb = TestWorkbook::new();
    let mut engine = Engine::new(wb, serial_eval_config());

    engine
        .set_cell_value("Sheet1", 5, 1, LiteralValue::Number(42.0))
        .unwrap();

    engine
        .set_cell_formula("Sheet1", 5, 2, parse("=@A1:A10").unwrap())
        .unwrap();
    let _ = engine.evaluate_all().unwrap();

    assert_eq!(
        engine.get_cell_value("Sheet1", 5, 2),
        Some(LiteralValue::Number(42.0))
    );
}

#[test]
fn implicit_intersection_row_vector_selects_by_column() {
    let wb = TestWorkbook::new();
    let mut engine = Engine::new(wb, serial_eval_config());

    engine
        .set_cell_value("Sheet1", 1, 3, LiteralValue::Number(7.0))
        .unwrap();

    engine
        .set_cell_formula("Sheet1", 3, 3, parse("=@A1:E1").unwrap())
        .unwrap();
    let _ = engine.evaluate_all().unwrap();

    assert_eq!(
        engine.get_cell_value("Sheet1", 3, 3),
        Some(LiteralValue::Number(7.0))
    );
}

#[test]
fn implicit_intersection_2d_selects_by_row_and_col_cross_sheet() {
    let wb = TestWorkbook::new();
    let mut engine = Engine::new(wb, serial_eval_config());

    engine
        .set_cell_value("Sheet1", 5, 3, LiteralValue::Number(123.0))
        .unwrap();

    engine
        .set_cell_formula("Sheet2", 5, 3, parse("=@Sheet1!A1:E10").unwrap())
        .unwrap();
    let _ = engine.evaluate_all().unwrap();

    assert_eq!(
        engine.get_cell_value("Sheet2", 5, 3),
        Some(LiteralValue::Number(123.0))
    );
}

#[test]
fn implicit_intersection_out_of_bounds_is_value_error() {
    let wb = TestWorkbook::new();
    let mut engine = Engine::new(wb, serial_eval_config());

    engine
        .set_cell_value("Sheet1", 5, 1, LiteralValue::Number(42.0))
        .unwrap();

    engine
        .set_cell_formula("Sheet1", 20, 2, parse("=@A1:A10").unwrap())
        .unwrap();
    let _ = engine.evaluate_all().unwrap();

    match engine.get_cell_value("Sheet1", 20, 2) {
        Some(LiteralValue::Error(e)) => assert_eq!(e.to_string(), "#VALUE!"),
        other => panic!("expected #VALUE!, got {other:?}"),
    }
}

#[test]
fn implicit_intersection_suppresses_spill_from_array_function() {
    let wb = TestWorkbook::new();
    let mut engine = Engine::new(wb, serial_eval_config());

    engine
        .set_cell_formula("Sheet1", 1, 4, parse("=@SEQUENCE(2,2)").unwrap())
        .unwrap();
    let _ = engine.evaluate_all().unwrap();

    assert_eq!(
        engine.get_cell_value("Sheet1", 1, 4),
        Some(LiteralValue::Number(1.0))
    );

    // No spill should occur.
    assert_eq!(engine.get_cell_value("Sheet1", 1, 5), None);
    assert_eq!(engine.get_cell_value("Sheet1", 2, 4), None);
}

#[test]
fn implicit_intersection_against_spilled_values_requires_at_for_scalar() {
    let wb = TestWorkbook::new();
    let mut engine = Engine::new(wb, serial_eval_config());

    // A1 spills a 3x1 vector: A1:A3 = 1,2,3
    engine
        .set_cell_formula("Sheet1", 1, 1, parse("=SEQUENCE(3,1)").unwrap())
        .unwrap();

    // B2 uses @ to pick the intersecting element (A2)
    engine
        .set_cell_formula("Sheet1", 2, 2, parse("=@A1:A3").unwrap())
        .unwrap();

    let _ = engine.evaluate_all().unwrap();

    assert_eq!(
        engine.get_cell_value("Sheet1", 2, 1),
        Some(LiteralValue::Number(2.0))
    );
    assert_eq!(
        engine.get_cell_value("Sheet1", 2, 2),
        Some(LiteralValue::Number(2.0))
    );

    // B2 should be scalar (no spill).
    assert_eq!(engine.get_cell_value("Sheet1", 3, 2), None);
}

// ── `SINGLE` — the function spelling Excel persists `@` as (`_xlfn.SINGLE(range)`) ──────────────
// Every case mirrors an `@` case above: the two spellings must evaluate identically.

fn engine_with(values: &[(&str, u32, u32, f64)]) -> Engine<TestWorkbook> {
    let mut engine = Engine::new(TestWorkbook::new(), serial_eval_config());
    for (sheet, row, col, v) in values {
        engine
            .set_cell_value(sheet, *row, *col, LiteralValue::Number(*v))
            .unwrap();
    }
    engine
}

fn eval_at(
    engine: &mut Engine<TestWorkbook>,
    sheet: &str,
    row: u32,
    col: u32,
    formula: &str,
) -> Option<LiteralValue> {
    engine
        .set_cell_formula(sheet, row, col, parse(formula).unwrap())
        .unwrap();
    let _ = engine.evaluate_all().unwrap();
    engine.get_cell_value(sheet, row, col)
}

#[test]
fn single_column_vector_selects_by_row_like_at() {
    let mut engine = engine_with(&[("Sheet1", 5, 1, 42.0)]);
    assert_eq!(
        eval_at(&mut engine, "Sheet1", 5, 2, "=SINGLE(A1:A10)"),
        Some(LiteralValue::Number(42.0))
    );
    assert_eq!(
        eval_at(&mut engine, "Sheet1", 5, 3, "=@A1:A10"),
        Some(LiteralValue::Number(42.0))
    );
}

#[test]
fn single_persisted_xlfn_spelling_resolves() {
    let mut engine = engine_with(&[("Sheet1", 1, 3, 7.0)]);
    assert_eq!(
        eval_at(&mut engine, "Sheet1", 3, 3, "=_xlfn.SINGLE(A1:E1)"),
        Some(LiteralValue::Number(7.0))
    );
}

#[test]
fn single_2d_selects_by_row_and_col_cross_sheet() {
    let mut engine = engine_with(&[("Sheet1", 5, 3, 123.0)]);
    assert_eq!(
        eval_at(&mut engine, "Sheet2", 5, 3, "=SINGLE(Sheet1!A1:E10)"),
        Some(LiteralValue::Number(123.0))
    );
}

#[test]
fn single_out_of_bounds_is_value_error() {
    let mut engine = engine_with(&[("Sheet1", 5, 1, 42.0)]);
    match eval_at(&mut engine, "Sheet1", 20, 2, "=SINGLE(A1:A10)") {
        Some(LiteralValue::Error(e)) => assert_eq!(e.to_string(), "#VALUE!"),
        other => panic!("expected #VALUE!, got {other:?}"),
    }
}

#[test]
fn single_cell_range_agrees_with_at_in_both_positions() {
    // A 1x1 range reads as a column vector in both spellings (and in the JS engine): the cell
    // from its own row, `#VALUE!` from any other.
    let mut engine = engine_with(&[("Sheet1", 1, 1, 9.0)]);
    assert_eq!(
        eval_at(&mut engine, "Sheet1", 1, 2, "=SINGLE(A1:A1)"),
        Some(LiteralValue::Number(9.0))
    );
    assert_eq!(
        eval_at(&mut engine, "Sheet1", 1, 3, "=@A1:A1"),
        Some(LiteralValue::Number(9.0))
    );
    for formula in ["=SINGLE(A1:A1)", "=@A1:A1"] {
        match eval_at(&mut engine, "Sheet1", 5, 2, formula) {
            Some(LiteralValue::Error(e)) => assert_eq!(e.to_string(), "#VALUE!", "{formula}"),
            other => panic!("{formula}: expected #VALUE!, got {other:?}"),
        }
    }
}

#[test]
fn single_keeps_a_reference_for_row() {
    let mut engine = engine_with(&[("Sheet1", 5, 1, 42.0)]);
    assert_eq!(
        eval_at(&mut engine, "Sheet1", 5, 2, "=ROW(SINGLE(A1:A10))"),
        Some(LiteralValue::Number(5.0))
    );
}

#[test]
fn single_suppresses_spill_from_array_function() {
    let mut engine = engine_with(&[]);
    assert_eq!(
        eval_at(&mut engine, "Sheet1", 1, 4, "=SINGLE(SEQUENCE(2,2))"),
        Some(LiteralValue::Number(1.0))
    );
    assert_eq!(engine.get_cell_value("Sheet1", 1, 5), None);
    assert_eq!(engine.get_cell_value("Sheet1", 2, 4), None);
}

#[test]
fn single_array_literal_top_left_and_scalar_pass_through() {
    let mut engine = engine_with(&[]);
    assert_eq!(
        eval_at(&mut engine, "Sheet1", 1, 1, "=SINGLE({1,2;3,4})"),
        Some(LiteralValue::Number(1.0))
    );
    assert_eq!(
        eval_at(&mut engine, "Sheet1", 1, 2, "=SINGLE(7)*2"),
        Some(LiteralValue::Number(14.0))
    );
    assert_eq!(
        eval_at(&mut engine, "Sheet1", 1, 3, "=SINGLE(\"x\")"),
        Some(LiteralValue::Text("x".into()))
    );
}

#[test]
fn single_union_and_bad_arity_are_errors_not_name() {
    let mut engine = engine_with(&[("Sheet1", 2, 1, 1.0), ("Sheet1", 2, 3, 2.0)]);
    match eval_at(&mut engine, "Sheet1", 2, 5, "=SINGLE((A1:A5,C1:C5))") {
        Some(LiteralValue::Error(e)) => assert_eq!(e.to_string(), "#VALUE!"),
        other => panic!("expected #VALUE!, got {other:?}"),
    }
    for formula in ["=SINGLE()", "=SINGLE(A1:A5,C1:C5)"] {
        match eval_at(&mut engine, "Sheet1", 3, 5, formula) {
            Some(LiteralValue::Error(e)) => assert_ne!(e.to_string(), "#NAME?", "{formula}"),
            other => panic!("{formula}: expected an error, got {other:?}"),
        }
    }
}
