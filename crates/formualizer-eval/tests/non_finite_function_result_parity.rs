//! rowsncolumns/spreadsheet#939 F-01 — Excel has no infinity or NaN: any worksheet function whose
//! result overflows a double is `#NUM!` (`EXP(710)`, `FACTDOUBLE(301)`, `COMBIN(1030,515)`,
//! `SUMSQ(1E200,1E200)`, `PRODUCT(1E200,1E200)`, `SUMPRODUCT({1E200,…},{1E200,…})`,
//! `POWER(10,400)`), and the error then behaves like any other error value: `ISNUMBER` is FALSE,
//! `ISERROR` is TRUE, `IFERROR` takes the fallback, `1/EXP(710)` is `#NUM!` (not 0), and a lifted
//! call spills `#NUM!` only in the overflowing slots. Only the operators used to guard
//! (`2^1024`, `1E308*10`); a function result leaked `inf` into the cell.

use formualizer_common::{ExcelErrorKind, LiteralValue};
use formualizer_eval::engine::{Engine, EvalConfig};
use formualizer_eval::test_workbook::TestWorkbook;
use formualizer_parse::parser::parse;

fn engine() -> Engine<TestWorkbook> {
    let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
    // A1 = 710 (an EXP argument that overflows), A2 = 1.
    e.set_cell_value("Sheet1", 1, 1, LiteralValue::Number(710.0))
        .unwrap();
    e.set_cell_value("Sheet1", 2, 1, LiteralValue::Number(1.0))
        .unwrap();
    e
}

fn eval(formula: &str) -> LiteralValue {
    let mut e = engine();
    e.set_cell_formula("Sheet1", 1, 7, parse(formula).unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    e.get_cell_value("Sheet1", 1, 7).unwrap()
}

/// Evaluate `formula` in G1 and read the G column spill (rows 1..=rows).
fn eval_col(formula: &str, rows: u32) -> Vec<LiteralValue> {
    let mut e = engine();
    e.set_cell_formula("Sheet1", 1, 7, parse(formula).unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    (1..=rows)
        .map(|r| {
            e.get_cell_value("Sheet1", r, 7)
                .unwrap_or(LiteralValue::Empty)
        })
        .collect()
}

fn assert_num_error(formula: &str) {
    match eval(formula) {
        LiteralValue::Error(e) => assert_eq!(e.kind, ExcelErrorKind::Num, "{formula}"),
        other => panic!("{formula}: expected #NUM!, got {other:?}"),
    }
}

fn assert_number(formula: &str, want: f64) {
    match eval(formula) {
        LiteralValue::Number(n) => {
            assert!(
                (n - want).abs() <= 1e-9 * want.abs().max(1.0),
                "{formula}: got {n}, want {want}"
            );
        }
        LiteralValue::Int(i) => assert_eq!(i as f64, want, "{formula}"),
        other => panic!("{formula}: expected {want}, got {other:?}"),
    }
}

#[test]
fn a_function_result_that_overflows_a_double_is_num_error() {
    assert_num_error("=EXP(710)");
    assert_num_error("=EXP(A1)");
    assert_num_error("=FACTDOUBLE(301)");
    assert_num_error("=COMBIN(1030,515)");
    assert_num_error("=SUMSQ(1E200,1E200)");
    assert_num_error("=PRODUCT(1E200,1E200)");
    assert_num_error("=SUMPRODUCT({1E200,1E200},{1E200,1E200})");
    assert_num_error("=POWER(10,400)");
    assert_num_error("=LN(EXP(710))");
    assert_num_error("=SUM(EXP(710),1)");
}

#[test]
fn the_overflow_error_behaves_like_any_other_error_value() {
    assert_eq!(eval("=ISNUMBER(EXP(710))"), LiteralValue::Boolean(false));
    assert_eq!(eval("=ISERROR(EXP(710))"), LiteralValue::Boolean(true));
    assert_eq!(eval("=ISERR(EXP(710))"), LiteralValue::Boolean(true));
    assert_eq!(
        eval("=IFERROR(EXP(710),\"big\")"),
        LiteralValue::Text("big".into())
    );
    assert_number("=ERROR.TYPE(EXP(710))", 6.0);
    // Dividing by an overflowed value is #NUM!, not 0.
    assert_num_error("=1/EXP(710)");
    assert_num_error("=EXP(710)-EXP(710)");
    assert_num_error("=-EXP(710)");
    assert_eq!(
        eval("=IF(ISERROR(EXP(710)),\"err\",\"num\")"),
        LiteralValue::Text("err".into())
    );
}

#[test]
fn a_lifted_call_spills_num_error_only_in_the_overflowing_slots() {
    let got = eval_col("=EXP(A1:A2)", 2);
    match &got[0] {
        LiteralValue::Error(e) => assert_eq!(e.kind, ExcelErrorKind::Num),
        other => panic!("EXP(710) slot: expected #NUM!, got {other:?}"),
    }
    match &got[1] {
        LiteralValue::Number(n) => assert!((n - std::f64::consts::E).abs() < 1e-12),
        other => panic!("EXP(1) slot: expected e, got {other:?}"),
    }
    let got = eval_col("=EXP({710;1})", 2);
    assert!(matches!(&got[0], LiteralValue::Error(e) if e.kind == ExcelErrorKind::Num));
    assert!(matches!(&got[1], LiteralValue::Number(_)));
}

#[test]
fn finite_results_are_untouched() {
    assert_number("=EXP(1)", std::f64::consts::E);
    assert_number("=EXP(709)", 709f64.exp());
    assert_number("=COMBIN(10,5)", 252.0);
    assert_number("=SUMSQ(1E150,1E150)", 2e300);
    assert_number("=POWER(10,300)", 1e300);
    assert_number("=FACTDOUBLE(7)", 105.0);
    assert_number("=SUMPRODUCT({1,2},{3,4})", 11.0);
}
