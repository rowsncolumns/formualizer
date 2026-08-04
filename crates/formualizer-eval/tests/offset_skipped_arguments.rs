//! OFFSET with SKIPPED argument slots — `OFFSET(ref,,,,width)` keeps the commas in Excel and the
//! parser represents each skipped slot as a bare empty-text literal. Excel's semantics: skipped
//! rows/cols default to 0, skipped height/width default to the base reference's dimensions.
//! The schema's strict number coercion previously rejected the empty-text marker with `#VALUE!`
//! (production repro: `=SUM(OFFSET($O2,,,,INDEX(MONTH_NUMBERS,…)))` YTD columns erroring across a
//! customer dashboard, poisoning every RANK/ranking chain downstream).

use formualizer_common::LiteralValue;
use formualizer_eval::engine::{Engine, EvalConfig};
use formualizer_eval::test_workbook::TestWorkbook;
use formualizer_parse::parser::parse;

fn eval_one(formula: &str) -> Option<LiteralValue> {
    let mut engine = Engine::new(TestWorkbook::new(), EvalConfig::default());
    engine
        .set_cell_value("Sheet1", 1, 1, LiteralValue::Number(10.0))
        .unwrap();
    engine
        .set_cell_value("Sheet1", 1, 2, LiteralValue::Number(20.0))
        .unwrap();
    engine
        .set_cell_value("Sheet1", 2, 1, LiteralValue::Number(30.0))
        .unwrap();
    engine
        .set_cell_value("Sheet1", 2, 2, LiteralValue::Number(40.0))
        .unwrap();
    engine
        .set_cell_formula("Sheet1", 5, 5, parse(formula).unwrap())
        .unwrap();
    engine.evaluate_all().unwrap();
    engine.get_cell_value("Sheet1", 5, 5)
}

#[test]
fn skipped_rows_cols_default_to_zero() {
    assert_eq!(
        eval_one("=SUM(OFFSET(A1,,,,2))"),
        Some(LiteralValue::Number(30.0)),
        "skipped rows/cols are 0; skipped height defaults to the base's 1 row"
    );
    assert_eq!(
        eval_one("=SUM(OFFSET(A1,,,1,2))"),
        Some(LiteralValue::Number(30.0)),
    );
}

#[test]
fn skipped_height_and_width_default_to_base_dims() {
    assert_eq!(
        eval_one("=SUM(OFFSET(A1:B2,,,,))"),
        Some(LiteralValue::Number(100.0)),
        "all four trailing slots skipped: the full base rect"
    );
    assert_eq!(
        eval_one("=SUM(OFFSET(A1:B2,1,,,))"),
        Some(LiteralValue::Number(70.0)),
        "shift one row down, keep base dims (A2:B3 → 30+40)"
    );
}

#[test]
fn explicit_arguments_still_work() {
    assert_eq!(
        eval_one("=SUM(OFFSET(A1,0,0,2,2))"),
        Some(LiteralValue::Number(100.0)),
    );
    assert_eq!(
        eval_one("=SUM(OFFSET(A1,1,1))"),
        Some(LiteralValue::Number(40.0)),
    );
}
