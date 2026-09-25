//! rowsncolumns/spreadsheet#939 (F-02, F-03) — Excel evaluates every scalar function once per
//! element when it is handed a multi-cell range or an array constant, whatever the argument's
//! coercion policy: `MONTH(D1:D3)` spills `{1;2;3}`, `SUMPRODUCT((MONTH(D1:D5)=1)*C1:C5)` counts
//! the January rows, `SUM(TEXT(C1:C3,"0")+0)` adds the three formatted numbers. `IFERROR` /
//! `IFNA` lift the same way while still short-circuiting per element (`SUM(IFERROR(K1:K5,0))`
//! replaces only the error cells). Functions that legitimately consume a whole range (SUM,
//! VLOOKUP's table, INDEX's array) keep seeing it whole.

use formualizer_common::{ExcelErrorKind, LiteralValue};
use formualizer_eval::engine::{Engine, EvalConfig};
use formualizer_eval::test_workbook::TestWorkbook;
use formualizer_parse::parser::parse;

fn text(s: &str) -> LiteralValue {
    LiteralValue::Text(s.into())
}

fn num(n: f64) -> LiteralValue {
    LiteralValue::Number(n)
}

const OUT_COL: u32 = 30;

/// The functions-a parity probe grid (1-based): C1:C5 = 1..5 · D1:D5 = 2024-01-01, 02-01,
/// 03-01, 04-01, 05-01 as serials · H4:H6 numeric text · K1 = 1/0, K2 = NA(), K3 = 0,
/// K4 = A1 (=1), K5 = 10 · N1:N5 = -2.5, 2.5, -1.5, 1.5, 0. Formula under test in AD1.
fn seeded() -> Engine<TestWorkbook> {
    let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
    e.set_cell_value("Sheet1", 1, 1, num(1.0)).unwrap();
    for (i, c) in [1.0, 2.0, 3.0, 4.0, 5.0].iter().enumerate() {
        e.set_cell_value("Sheet1", i as u32 + 1, 3, num(*c))
            .unwrap();
    }
    for (i, d) in [45292.0, 45323.0, 45352.0, 45383.0, 45413.0]
        .iter()
        .enumerate()
    {
        e.set_cell_value("Sheet1", i as u32 + 1, 4, num(*d))
            .unwrap();
    }
    e.set_cell_value("Sheet1", 4, 8, text("1,000")).unwrap();
    e.set_cell_value("Sheet1", 5, 8, text("$5")).unwrap();
    e.set_cell_value("Sheet1", 6, 8, text("50%")).unwrap();
    e.set_cell_formula("Sheet1", 1, 11, parse("=1/0").unwrap())
        .unwrap();
    e.set_cell_formula("Sheet1", 2, 11, parse("=NA()").unwrap())
        .unwrap();
    e.set_cell_value("Sheet1", 3, 11, num(0.0)).unwrap();
    e.set_cell_formula("Sheet1", 4, 11, parse("=A1").unwrap())
        .unwrap();
    e.set_cell_value("Sheet1", 5, 11, num(10.0)).unwrap();
    for (i, n) in [-2.5, 2.5, -1.5, 1.5, 0.0].iter().enumerate() {
        e.set_cell_value("Sheet1", i as u32 + 1, 14, num(*n))
            .unwrap();
    }
    e
}

fn eval(formula: &str) -> LiteralValue {
    let mut e = seeded();
    e.set_cell_formula("Sheet1", 1, OUT_COL, parse(formula).unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    e.get_cell_value("Sheet1", 1, OUT_COL).unwrap()
}

/// Evaluate `formula` in AD1 and read the AD column spill (rows 1..=rows).
fn eval_col(formula: &str, rows: u32) -> Vec<LiteralValue> {
    let mut e = seeded();
    e.set_cell_formula("Sheet1", 1, OUT_COL, parse(formula).unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    (1..=rows)
        .map(|r| {
            e.get_cell_value("Sheet1", r, OUT_COL)
                .unwrap_or(LiteralValue::Empty)
        })
        .collect()
}

fn n(v: &LiteralValue) -> f64 {
    match v {
        LiteralValue::Number(f) => *f,
        LiteralValue::Int(i) => *i as f64,
        LiteralValue::Boolean(b) => {
            if *b {
                1.0
            } else {
                0.0
            }
        }
        other => panic!("expected a number, got {other:?}"),
    }
}

fn nums(v: &[LiteralValue]) -> Vec<f64> {
    v.iter().map(n).collect()
}

fn err_kind(v: &LiteralValue) -> ExcelErrorKind {
    match v {
        LiteralValue::Error(e) => e.kind,
        other => panic!("expected an error, got {other:?}"),
    }
}

fn assert_num(formula: &str, expected: f64) {
    let got = eval(formula);
    let got_n = match &got {
        LiteralValue::Number(_) | LiteralValue::Int(_) | LiteralValue::Boolean(_) => n(&got),
        other => panic!("{formula}: expected {expected}, got {other:?}"),
    };
    assert!(
        (got_n - expected).abs() < 1e-9,
        "{formula}: expected {expected}, got {got_n}"
    );
}

// ---------------- F-02: coercing scalar functions lift over ranges / arrays ----------------

#[test]
fn date_part_functions_spill_over_a_range() {
    assert_eq!(nums(&eval_col("=MONTH(D1:D3)", 3)), vec![1.0, 2.0, 3.0]);
    assert_eq!(nums(&eval_col("=YEAR(D1:D2)", 2)), vec![2024.0, 2024.0]);
    assert_eq!(nums(&eval_col("=DAY(D1:D5)", 5)), vec![1.0; 5]);
    assert_eq!(
        nums(&eval_col("=WEEKDAY(D1:D5)", 5)),
        vec![2.0, 5.0, 6.0, 2.0, 4.0]
    );
}

#[test]
fn date_part_functions_feed_aggregates() {
    assert_num("=SUMPRODUCT((MONTH(D1:D5)=1)*C1:C5)", 1.0);
    assert_num("=SUMPRODUCT((YEAR(D1:D5)=2024)*C1:C5)", 15.0);
    assert_num("=SUM(IF(MONTH(D1:D5)>2,C1:C5))", 12.0);
    assert_num("=SUM(DAY(D1:D5))", 5.0);
    assert_num("=SUM(WEEKDAY(D1:D5))", 19.0);
}

#[test]
fn edate_eomonth_lift_over_a_range() {
    assert_num("=SUM(EOMONTH(D1:D2,0)-D1:D2)", 58.0);
    assert_num("=SUM(EDATE(D1:D2,1)-D1:D2)", 60.0);
    assert_eq!(
        nums(&eval_col("=EDATE(D1:D2,1)", 2)),
        vec![45323.0, 45352.0]
    );
    // A lifted first argument broadcasts against an array second argument.
    assert_eq!(
        nums(&eval_col("=EDATE(D1,{1;2})", 2)),
        vec![45323.0, 45352.0]
    );
}

#[test]
fn text_and_value_lift_over_a_range() {
    assert_eq!(
        eval_col("=TEXT(D1:D2,\"0\")", 2),
        vec![text("45292"), text("45323")]
    );
    assert_num("=SUMPRODUCT((TEXT(C1:C5,\"0\")=\"3\")*C1:C5)", 3.0);
    assert_num("=SUM(TEXT(C1:C3,\"0\")+0)", 6.0);
    assert_num("=SUM(VALUE(TEXT(C1:C5,\"0\")))", 15.0);
    assert_num("=SUM(VALUE(H4:H6))", 1005.5);
    // Array-constant argument, not just a range.
    assert_eq!(
        eval_col("=TEXT({1;22},\"0\")", 2),
        vec![text("1"), text("22")]
    );
}

#[test]
fn rounding_functions_lift_over_a_range() {
    assert_num("=SUM(CEILING(N1:N5,1))", 2.0);
    assert_num("=SUM(FLOOR.MATH(N1:N5))", -2.0);
    assert_eq!(
        nums(&eval_col("=CEILING(N1:N5,1)", 5)),
        vec![-2.0, 3.0, -1.0, 2.0, 0.0]
    );
    assert_eq!(
        nums(&eval_col("=FLOOR.MATH(N1:N5)", 5)),
        vec![-3.0, 2.0, -2.0, 1.0, 0.0]
    );
}

#[test]
fn lifted_elements_that_fail_coercion_become_per_element_errors() {
    // "abc" cannot become a serial: that element is #VALUE!, the others still evaluate.
    let got = eval_col("=MONTH({45292;\"abc\";45352})", 3);
    assert_eq!(n(&got[0]), 1.0);
    assert_eq!(err_kind(&got[1]), ExcelErrorKind::Value);
    assert_eq!(n(&got[2]), 3.0);
    // ...and an aggregate over the lift sees the element error.
    assert_eq!(
        err_kind(&eval("=SUM(MONTH({45292;\"abc\"}))")),
        ExcelErrorKind::Value
    );
}

#[test]
fn scalar_and_1x1_arguments_keep_the_scalar_path() {
    assert_num("=MONTH(D1)", 1.0);
    assert_num("=MONTH(D2:D2)", 2.0);
    assert_num("=YEAR(D1)", 2024.0);
    assert_eq!(eval("=TEXT(C2,\"0\")"), text("2"));
    assert_num("=EDATE(D1,1)", 45323.0);
    assert_num("=CEILING(N1,1)", -2.0);
    // Lenient text→number coercion still applies to a single argument.
    assert_num("=MONTH(\"2024-03-05\")", 3.0);
}

// ---------------- F-03: IFERROR / IFNA lift over ranges / arrays ----------------

#[test]
fn iferror_lifts_over_a_range() {
    assert_num("=SUM(IFERROR(K1:K5,0))", 11.0);
    assert_num("=MAX(IFERROR(K1:K5,0))", 10.0);
    assert_num("=SUMPRODUCT(IFERROR(K1:K5,0))", 11.0);
    assert_eq!(nums(&eval_col("=IFERROR(K1:K3,0)", 3)), vec![0.0, 0.0, 0.0]);
    assert_num("=COUNT(IFERROR(K1:K5,\"\"))", 3.0);
}

#[test]
fn iferror_lifts_over_an_array_expression() {
    assert_eq!(
        eval_col("=IFERROR(C1:C3/{1;0;1},\"e\")", 3),
        vec![num(1.0), text("e"), num(3.0)]
    );
    assert_num("=SUM(IFERROR(C1:C5/{1;0;1;1;1},0))", 13.0);
    assert_num("=SUM(IFERROR(VALUE(H4:H6),0))", 1005.5);
}

#[test]
fn iferror_fallback_broadcasts_per_element() {
    // The fallback is picked per element (array fallback); K3 = 0 is not an error and passes through.
    assert_eq!(
        nums(&eval_col("=IFERROR(K1:K3,{7;8;9})", 3)),
        vec![7.0, 8.0, 0.0]
    );
    // A 1×1 value with an array fallback broadcasts the value over the fallback's shape.
    assert_eq!(nums(&eval_col("=IFERROR(K1,{7;8})", 2)), vec![7.0, 8.0]);
}

#[test]
fn ifna_lifts_over_a_range_and_leaves_other_errors() {
    let got = eval_col("=IFNA(K1:K3,0)", 3);
    assert_eq!(err_kind(&got[0]), ExcelErrorKind::Div);
    assert_eq!(n(&got[1]), 0.0);
    assert_eq!(n(&got[2]), 0.0);
    assert_num("=SUM(IFNA(K2:K3,0))", 0.0);
    assert_num("=SUM(IFNA(K2:K5,5))", 16.0);
}

#[test]
fn iferror_and_ifna_keep_scalar_semantics() {
    assert_eq!(eval("=IFERROR(K1,\"e\")"), text("e"));
    assert_eq!(eval("=IFERROR(1/0,\"e\")"), text("e"));
    assert_num("=IFERROR(C2,\"e\")", 2.0);
    assert_eq!(err_kind(&eval("=IFNA(K1,\"e\")")), ExcelErrorKind::Div);
    assert_eq!(eval("=IFNA(K2,\"e\")"), text("e"));
    // Short circuit: a non-error value never evaluates the fallback.
    assert_num("=IFERROR(C2,1/0)", 2.0);
    assert_num("=IFNA(C2,1/0)", 2.0);
}

// ---------------- functions that consume a whole range must NOT lift ----------------

#[test]
fn range_consuming_functions_are_not_lifted() {
    assert_num("=SUM(C1:C5)", 15.0);
    assert_num("=COUNT(C1:C5)", 5.0);
    assert_num("=MAX(C1:C5)", 5.0);
    assert_num("=AVERAGE(C1:C5)", 3.0);
    assert_num("=VLOOKUP(3,C1:D5,2,0)", 45352.0);
    assert_num("=INDEX(C1:C5,3)", 3.0);
    assert_num("=MATCH(4,C1:C5,0)", 4.0);
    assert_num("=SUMPRODUCT(C1:C5,C1:C5)", 55.0);
    assert_num("=COUNTIF(C1:C5,\">2\")", 3.0);
    assert_num("=ROWS(C1:C5)", 5.0);
}
