//! rowsncolumns/spreadsheet#939 F-05 — the value part of a text criterion that parses as
//! date/time text is coerced to its serial in `COUNTIF` / `SUMIF` / `AVERAGEIF` / the `*IFS`
//! family / `MAXIFS` / `MINIFS`: `COUNTIF(D1:D5,"1/1/2024")` = 1, `COUNTIF(D1:D5,">1/1/2024")`
//! = 4, `SUMIF(D1:D5,">=3/1/2024",C1:C5)` = 12 — the same coercion `"1/1/2024"+0` and
//! `DATEVALUE` apply. The `">"&DATE(2024,1,1)` spelling already worked; the typed text form
//! matched nothing and answered 0.

use formualizer_common::LiteralValue;
use formualizer_eval::engine::{Engine, EvalConfig};
use formualizer_eval::test_workbook::TestWorkbook;
use formualizer_parse::parser::parse;

fn num(n: f64) -> LiteralValue {
    LiteralValue::Number(n)
}
fn text(s: &str) -> LiteralValue {
    LiteralValue::Text(s.into())
}

/// C1:C5 = 1..5 · D1:D5 = 2024-01-01, 02-01, 03-01, 04-01, 05-01 (serials 45292, 45323,
/// 45352, 45383, 45413) · E1:E5 = x,y,x,z,x · B1:B3 = 10, 20, 1000 · T1:T3 = 0.25, 0.5, 0.75.
fn engine() -> Engine<TestWorkbook> {
    let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
    for (i, d) in [45292.0, 45323.0, 45352.0, 45383.0, 45413.0]
        .iter()
        .enumerate()
    {
        e.set_cell_value("Sheet1", i as u32 + 1, 3, num(i as f64 + 1.0))
            .unwrap();
        e.set_cell_value("Sheet1", i as u32 + 1, 4, num(*d))
            .unwrap();
    }
    for (i, s) in ["x", "y", "x", "z", "x"].iter().enumerate() {
        e.set_cell_value("Sheet1", i as u32 + 1, 5, text(s))
            .unwrap();
    }
    for (i, n) in [10.0, 20.0, 1000.0].iter().enumerate() {
        e.set_cell_value("Sheet1", i as u32 + 1, 2, num(*n))
            .unwrap();
    }
    for (i, t) in [0.25, 0.5, 0.75].iter().enumerate() {
        e.set_cell_value("Sheet1", i as u32 + 1, 20, num(*t))
            .unwrap();
    }
    e
}

fn eval(formula: &str) -> LiteralValue {
    let mut e = engine();
    e.set_cell_formula("Sheet1", 30, 30, parse(formula).unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    e.get_cell_value("Sheet1", 30, 30).unwrap()
}

fn assert_num(formula: &str, want: f64) {
    match eval(formula) {
        LiteralValue::Number(n) => {
            assert!((n - want).abs() < 1e-9, "{formula}: got {n}, want {want}")
        }
        LiteralValue::Int(i) => assert_eq!(i as f64, want, "{formula}"),
        other => panic!("{formula}: expected {want}, got {other:?}"),
    }
}

#[test]
fn countif_matches_date_cells_against_date_text_criteria() {
    assert_num("=COUNTIF(D1:D5,\"1/1/2024\")", 1.0);
    assert_num("=COUNTIF(D1:D5,\"=1/1/2024\")", 1.0);
    assert_num("=COUNTIF(D1:D5,\">1/1/2024\")", 4.0);
    assert_num("=COUNTIF(D1:D5,\">=3/1/2024\")", 3.0);
    assert_num("=COUNTIF(D1:D5,\"<2024-03-01\")", 2.0);
    assert_num("=COUNTIF(D1:D5,\"<>1/1/2024\")", 4.0);
    assert_num("=COUNTIF(D1:D5,\"1-Mar-2024\")", 1.0);
    assert_num("=COUNTIF(D1:D5,\"<=\"&\"4/1/2024\")", 4.0);
    // The DATE() spelling keeps working.
    assert_num("=COUNTIF(D1:D5,\">\"&DATE(2024,1,1))", 4.0);
}

#[test]
fn sumif_averageif_and_the_ifs_family_coerce_date_text_too() {
    assert_num("=SUMIF(D1:D5,\">=3/1/2024\",C1:C5)", 12.0);
    assert_num("=SUMIF(D1:D5,\"<3/1/2024\",C1:C5)", 3.0);
    assert_num("=AVERAGEIF(D1:D5,\"<=2/1/2024\",C1:C5)", 1.5);
    assert_num(
        "=SUMIFS(C1:C5,D1:D5,\">=2/1/2024\",D1:D5,\"<=4/1/2024\")",
        9.0,
    );
    assert_num("=COUNTIFS(D1:D5,\">=2/1/2024\",D1:D5,\"<=4/1/2024\")", 3.0);
    assert_num("=AVERAGEIFS(C1:C5,D1:D5,\">3/1/2024\")", 4.5);
    assert_num("=MAXIFS(C1:C5,D1:D5,\"<3/1/2024\")", 2.0);
    assert_num("=MINIFS(C1:C5,D1:D5,\">=4/1/2024\")", 4.0);
}

#[test]
fn time_text_criteria_coerce_to_day_fractions() {
    assert_num("=COUNTIF(T1:T3,\"12:00 PM\")", 1.0);
    assert_num("=COUNTIF(T1:T3,\">6:00 AM\")", 2.0);
    assert_num("=SUMIF(T1:T3,\"<=12:00\",T1:T3)", 0.75);
}

#[test]
fn non_date_text_criteria_are_unchanged() {
    assert_num("=COUNTIF(E1:E5,\"x\")", 3.0);
    assert_num("=COUNTIF(E1:E5,\"<>x\")", 2.0);
    assert_num("=COUNTIF(B1:B3,\"1,000\")", 1.0);
    assert_num("=COUNTIF(B1:B3,\">15\")", 2.0);
    assert_num("=COUNTIF(E1:E5,\"1/1/2024\")", 0.0);
    // A serial typed as text is a number criterion, not a date string.
    assert_num("=COUNTIF(D1:D5,\"45292\")", 1.0);
}
