//! Excel never leaves a formula cell blank: a formula whose result is a reference to an empty cell
//! holds the number 0, so `ISBLANK` of that cell is FALSE and references to it read 0. A direct
//! reference to the empty cell inside a formula still reads as blank, and `=""` stays text.

use formualizer_common::LiteralValue;
use formualizer_eval::engine::{Engine, EvalConfig};
use formualizer_eval::test_workbook::TestWorkbook;
use formualizer_parse::parser::parse;

/// Column Y holds "a", "x", "b" in rows 1..=3; column Z is empty. `formulas` go in row 2.
fn eval_row(formulas: &[(u32, &str)]) -> Engine<TestWorkbook> {
    let mut engine = Engine::new(TestWorkbook::new(), EvalConfig::default());
    for (row, v) in [(1, "a"), (2, "x"), (3, "b")] {
        engine
            .set_cell_value("Sheet1", row, 25, LiteralValue::Text(v.into()))
            .unwrap();
    }
    for (col, f) in formulas {
        engine
            .set_cell_formula("Sheet1", 2, *col, parse(f).unwrap())
            .unwrap();
    }
    engine.evaluate_all().unwrap();
    engine
}

fn at(engine: &Engine<TestWorkbook>, col: u32) -> Option<LiteralValue> {
    engine.get_cell_value("Sheet1", 2, col)
}

#[test]
fn blank_formula_results_hold_zero() {
    let e = eval_row(&[
        (1, "=Z2"),
        (2, "=A2"),
        (3, "=ISBLANK(A2)"),
        (4, "=ISBLANK(B2)"),
        (5, "=ISBLANK(Z2)"),
        (6, "=XLOOKUP(\"x\",Y1:Y3,Z1:Z3,\"nf\")"),
        (7, "=ISBLANK(F2)"),
        (8, "=IF(TRUE,Z2)"),
    ]);
    let n = |v: f64| Some(LiteralValue::Number(v));
    let b = |v: bool| Some(LiteralValue::Boolean(v));
    assert_eq!(at(&e, 1), n(0.0), "=Z2 over an empty cell");
    assert_eq!(at(&e, 2), n(0.0), "=A2");
    assert_eq!(at(&e, 3), b(false), "ISBLANK of a formula cell");
    assert_eq!(at(&e, 4), b(false), "ISBLANK of a formula cell");
    assert_eq!(at(&e, 5), b(true), "ISBLANK of the empty cell itself");
    assert_eq!(at(&e, 6), n(0.0), "XLOOKUP landing on an empty cell");
    assert_eq!(at(&e, 7), b(false), "ISBLANK of the XLOOKUP cell");
    assert_eq!(at(&e, 8), n(0.0), "IF returning the empty reference");
}

#[test]
fn blank_reads_inside_a_formula_and_empty_text_are_unchanged() {
    let e = eval_row(&[
        (1, "=Z2=\"\""),
        (2, "=COUNTA(Z2)"),
        (3, "=\"\""),
        (4, "=ISBLANK(C2)"),
    ]);
    assert_eq!(at(&e, 1), Some(LiteralValue::Boolean(true)));
    assert_eq!(at(&e, 2), Some(LiteralValue::Number(0.0)));
    assert_eq!(at(&e, 3), Some(LiteralValue::Text(String::new())));
    assert_eq!(at(&e, 4), Some(LiteralValue::Boolean(false)));
}
