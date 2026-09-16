//! rowsncolumns/spreadsheet#546 (U4) — Excel lifts scalar functions and operators element-wise
//! over arrays and ranges: `SUM(IF(rng>25,rng))`, `SUM(LEN(rng))`, `SUM(--(rng>1))`,
//! `SUMPRODUCT((rng="x")*rng2)`, `rng&"x"`. `IF` picks per element and broadcasts scalar
//! branches; `&` must never leak the `Debug` representation of an array into a cell.

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

/// A: 10..50 · B: 1..5 · C: "a","bb","ccc" · D: 5,6,7 · E: "x","y","x"; formula in G1.
fn seeded() -> Engine<TestWorkbook> {
    let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
    for (i, v) in [10.0, 20.0, 30.0, 40.0, 50.0].iter().enumerate() {
        e.set_cell_value("Sheet1", i as u32 + 1, 1, num(*v))
            .unwrap();
        e.set_cell_value("Sheet1", i as u32 + 1, 2, num(i as f64 + 1.0))
            .unwrap();
    }
    for (i, s) in ["a", "bb", "ccc"].iter().enumerate() {
        e.set_cell_value("Sheet1", i as u32 + 1, 3, text(s))
            .unwrap();
        e.set_cell_value("Sheet1", i as u32 + 1, 4, num(5.0 + i as f64))
            .unwrap();
    }
    for (i, s) in ["x", "y", "x"].iter().enumerate() {
        e.set_cell_value("Sheet1", i as u32 + 1, 5, text(s))
            .unwrap();
    }
    e
}

fn eval(formula: &str) -> LiteralValue {
    let mut e = seeded();
    e.set_cell_formula("Sheet1", 1, 7, parse(formula).unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    e.get_cell_value("Sheet1", 1, 7).unwrap()
}

/// Evaluate `formula` in G1 and read the G column spill (rows 1..=rows).
fn eval_col(formula: &str, rows: u32) -> Vec<LiteralValue> {
    let mut e = seeded();
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

fn eval_grid(formula: &str, rows: u32, cols: u32) -> Vec<Vec<LiteralValue>> {
    let mut e = seeded();
    e.set_cell_formula("Sheet1", 1, 7, parse(formula).unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    (1..=rows)
        .map(|r| {
            (7..7 + cols)
                .map(|c| {
                    e.get_cell_value("Sheet1", r, c)
                        .unwrap_or(LiteralValue::Empty)
                })
                .collect()
        })
        .collect()
}

fn n(v: &LiteralValue) -> f64 {
    match v {
        LiteralValue::Number(x) => *x,
        LiteralValue::Int(i) => *i as f64,
        LiteralValue::Boolean(b) => {
            if *b {
                1.0
            } else {
                0.0
            }
        }
        other => panic!("expected number, got {other:?}"),
    }
}

#[test]
fn sum_if_over_range_is_the_classic_array_idiom() {
    assert_eq!(n(&eval("=SUM(IF(A1:A5>25,A1:A5))")), 120.0);
    assert_eq!(n(&eval("=SUM(IF(A1:A5>25,B1:B5))")), 12.0);
    assert_eq!(n(&eval("=SUM(IF({1,2,3}>1,{1,2,3}))")), 5.0);
    assert_eq!(n(&eval("=MAX(IF(A1:A5<30,B1:B5))")), 2.0);
}

#[test]
fn if_over_array_spills_and_broadcasts_scalar_branches() {
    assert_eq!(
        eval_col("=IF(A1:A3>15,\"big\",\"small\")", 3),
        vec![text("small"), text("big"), text("big")]
    );
    let got = eval_col("=IF(A1:A3>15,B1:B3,0)", 3);
    assert_eq!(got.iter().map(n).collect::<Vec<_>>(), vec![0.0, 2.0, 3.0]);
    let got = eval_col("=IF(A1:A3>15,A1:A3)", 3);
    assert_eq!(got[0], LiteralValue::Boolean(false));
    assert_eq!(n(&got[1]), 20.0);
    assert_eq!(n(&got[2]), 30.0);
}

#[test]
fn scalar_if_keeps_short_circuit_semantics() {
    assert_eq!(n(&eval("=IF(A1>5,1,2)")), 1.0);
    assert_eq!(eval("=IF(A1>50,1)"), LiteralValue::Boolean(false));
    assert_eq!(
        n(&eval("=IF(A1>5,A1,1/0)")),
        10.0,
        "untaken branch is not evaluated"
    );
}

#[test]
fn boolean_arrays_lift_into_arithmetic() {
    assert_eq!(n(&eval("=SUM(--(A1:A5>25))")), 3.0);
    assert_eq!(n(&eval("=SUM((A1:A5>25)*B1:B5)")), 12.0);
    assert_eq!(n(&eval("=SUMPRODUCT((E1:E3=\"x\")*D1:D3)")), 12.0);
    assert_eq!(n(&eval("=-TRUE")), -1.0);
    let got = eval_col("=--(A1:A3>15)", 3);
    assert_eq!(got.iter().map(n).collect::<Vec<_>>(), vec![0.0, 1.0, 1.0]);
}

#[test]
fn scalar_functions_lift_element_wise() {
    assert_eq!(n(&eval("=SUM(LEN(C1:C3))")), 6.0);
    let got = eval_col("=LEN(C1:C3)", 3);
    assert_eq!(got.iter().map(n).collect::<Vec<_>>(), vec![1.0, 2.0, 3.0]);
    assert_eq!(
        eval_col("=UPPER(C1:C3)", 3),
        vec![text("A"), text("BB"), text("CCC")]
    );
    let got = eval_col("=ROUND(A1:A3/3,1)", 3);
    assert_eq!(got.iter().map(n).collect::<Vec<_>>(), vec![3.3, 6.7, 10.0]);
    let got = eval_col("=MOD(A1:A3,7)", 3);
    assert_eq!(got.iter().map(n).collect::<Vec<_>>(), vec![3.0, 6.0, 2.0]);
    assert_eq!(
        eval_col("=ISNUMBER(C1:C3)", 3),
        vec![
            LiteralValue::Boolean(false),
            LiteralValue::Boolean(false),
            LiteralValue::Boolean(false)
        ]
    );
    // two lifted arguments broadcast against each other
    let got = eval_grid("=LEFT(C3,{1,2})", 1, 2);
    assert_eq!(got[0], vec![text("c"), text("cc")]);
}

#[test]
fn scalar_functions_still_take_scalars_and_1x1_ranges() {
    assert_eq!(n(&eval("=LEN(C2)")), 2.0);
    assert_eq!(n(&eval("=LEN(C2:C2)")), 2.0);
    assert_eq!(n(&eval("=LEN(123)")), 3.0);
}

#[test]
fn concat_lifts_over_arrays_without_leaking_debug_repr() {
    assert_eq!(
        eval_col("=A1:A3&\"x\"", 3),
        vec![text("10x"), text("20x"), text("30x")]
    );
    assert_eq!(
        eval_col("=\"p\"&A1:A3", 3),
        vec![text("p10"), text("p20"), text("p30")]
    );
    assert_eq!(
        eval_col("=A1:A3&C1:C3", 3),
        vec![text("10a"), text("20bb"), text("30ccc")]
    );
    assert_eq!(eval("=\"a\"&\"b\""), text("ab"));
    let got = eval_grid("={1;2}&{10,20}", 2, 2);
    assert_eq!(got[0], vec![text("110"), text("120")]);
    assert_eq!(got[1], vec![text("210"), text("220")]);
}

#[test]
fn concat_propagates_errors() {
    match eval("=NA()&\"x\"") {
        LiteralValue::Error(e) => assert_eq!(e.kind, ExcelErrorKind::Na),
        other => panic!("expected #N/A, got {other:?}"),
    }
}

#[test]
fn column_vector_against_row_vector_broadcasts() {
    let got = eval_grid("={1;2}+{10,20}", 2, 2);
    assert_eq!(got[0].iter().map(n).collect::<Vec<_>>(), vec![11.0, 21.0]);
    assert_eq!(got[1].iter().map(n).collect::<Vec<_>>(), vec![12.0, 22.0]);
}
