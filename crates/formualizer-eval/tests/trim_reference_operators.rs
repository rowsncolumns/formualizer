//! Excel's trim-reference operators (demo-app-monorepo#49969 FX-10): `A1:.A100` trims trailing
//! blank rows / columns, `A1.:A100` leading ones and `A1.:.A100` both — the same range TRIMRANGE
//! returns with that mode. They also work on whole columns (`A:.A`). A workbook file stores them as
//! `_xlfn._TRO_TRAILING(…)`, `_TRO_LEADING`, `_TRO_ALL`.
//! <https://support.microsoft.com/en-us/office/trimrange-function-d7812248-3bc5-4c6b-901c-1afa9564f999>
//!
//! Seed grid (1-based): B3:B5 = 1,2,3 (B1:B2 and B6+ blank).

use formualizer_common::LiteralValue;
use formualizer_eval::engine::{Engine, EvalConfig};
use formualizer_eval::test_workbook::TestWorkbook;
use formualizer_parse::parser::parse;
use formualizer_parse::pretty::{canonical_formula, excel_formula};

fn eval(formula: &str) -> LiteralValue {
    let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
    for (i, v) in [1.0, 2.0, 3.0].into_iter().enumerate() {
        e.set_cell_value("Sheet1", i as u32 + 3, 2, LiteralValue::Number(v))
            .unwrap();
    }
    e.set_cell_formula("Sheet1", 1, 5, parse(formula).unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    e.get_cell_value("Sheet1", 1, 5)
        .unwrap_or(LiteralValue::Empty)
}

fn num(v: LiteralValue) -> f64 {
    match v {
        LiteralValue::Number(n) => n,
        LiteralValue::Int(i) => i as f64,
        other => panic!("expected a number, got {other:?}"),
    }
}

#[test]
fn trailing_trim_reference() {
    assert_eq!(num(eval("=ROWS(B1:.B100)")), 5.0);
    assert_eq!(num(eval("=SUM(B1:.B100)")), 6.0);
    assert_eq!(num(eval("=ROWS(B:.B)")), 5.0);
    assert_eq!(num(eval("=SUM(B:.B)")), 6.0);
    assert_eq!(num(eval("=ROWS(Sheet1!B1:.B100)")), 5.0);
    assert_eq!(num(eval("=ROWS($B$1:.$B$100)")), 5.0);
}

#[test]
fn leading_and_both_trim_references() {
    assert_eq!(num(eval("=ROWS(B1.:B5)")), 3.0);
    assert_eq!(num(eval("=ROWS(B1.:.B100)")), 3.0);
    assert_eq!(num(eval("=ROWS(B.:.B)")), 3.0);
    assert_eq!(num(eval("=INDEX(B1.:.B100,1)")), 1.0);
}

#[test]
fn trim_references_match_trimrange() {
    assert_eq!(
        num(eval("=ROWS(B1:.B100)")),
        num(eval("=ROWS(TRIMRANGE(B1:B100,2,2))"))
    );
    assert_eq!(
        num(eval("=ROWS(B1.:.B100)")),
        num(eval("=ROWS(TRIMRANGE(B1:B100))"))
    );
}

#[test]
fn stored_function_spelling_evaluates() {
    assert_eq!(num(eval("=ROWS(_xlfn._TRO_ALL(B1:B100))")), 3.0);
    assert_eq!(num(eval("=ROWS(_TRO_TRAILING(B1:B100))")), 5.0);
    assert_eq!(num(eval("=ROWS(_TRO_LEADING(B1:B5))")), 3.0);
}

#[test]
fn trim_references_print_back_as_typed() {
    for f in [
        "=SUM(B1:.B100)",
        "=SUM(B1.:B100)",
        "=SUM(B1.:.B100)",
        "=SUM(B:.B)",
        "=SUM(Sheet1!B1:.B100)",
        "=SUM($B$1:.$B$100)",
    ] {
        assert_eq!(excel_formula(&parse(f).unwrap()), f);
    }
    assert_eq!(
        canonical_formula(&parse("=A1:.A3+1").unwrap()),
        "=A1:.A3 + 1"
    );
}

#[test]
fn ordinary_references_are_unchanged() {
    assert_eq!(num(eval("=ROWS(B1:B100)")), 100.0);
    assert!(parse("=A1:.").is_err());
}
