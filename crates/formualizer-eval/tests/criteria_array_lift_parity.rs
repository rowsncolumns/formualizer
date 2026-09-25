//! rowsncolumns/spreadsheet#939 F-06 — an ARRAY-valued criterion lifts element-wise in the
//! criteria aggregates, the `SUM(COUNTIF(rng,{…}))` / `SUMPRODUCT(SUMIF(…,{…}))` idiom:
//! `COUNTIF(A1:A8,{1,2})` = `{2,1}`, `COUNTIF(C1:C5,C1:C5)` = `{1;1;1;1;1}` (a multi-cell range
//! as criteria is per element too), `SUMIF(E1:E5,{"x","y"},C1:C5)` = `{9,2}`,
//! `SUMIFS(C1:C5,E1:E5,{"x","y"})` = `{9,2}`, `SUMIFS(C1:C5,E1:E5,"x",C1:C5,{">1",">3"})` =
//! `{8,5}`; `MAXIFS`/`MINIFS` follow the same rule. Every one of these answered `0` before.

use formualizer_common::LiteralValue;
use formualizer_eval::engine::{Engine, EvalConfig};
use formualizer_eval::test_workbook::TestWorkbook;
use formualizer_parse::parser::parse;

fn text(s: &str) -> LiteralValue {
    LiteralValue::Text(s.into())
}
fn num(n: f64) -> LiteralValue {
    LiteralValue::Number(n)
}

/// Probe grid: A1:A8 = 1,"x",TRUE,blank,2,"1"(text),blank,3 · C1:C5 = 1..5 · E1:E5 = x,y,x,z,x.
fn engine() -> Engine<TestWorkbook> {
    let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
    let col_a = [
        Some(num(1.0)),
        Some(text("x")),
        Some(LiteralValue::Boolean(true)),
        None,
        Some(num(2.0)),
        Some(text("1")),
        None,
        Some(num(3.0)),
    ];
    for (i, v) in col_a.iter().enumerate() {
        if let Some(v) = v {
            e.set_cell_value("Sheet1", i as u32 + 1, 1, v.clone())
                .unwrap();
        }
    }
    for i in 0..5u32 {
        e.set_cell_value("Sheet1", i + 1, 3, num((i + 1) as f64))
            .unwrap();
    }
    for (i, s) in ["x", "y", "x", "z", "x"].iter().enumerate() {
        e.set_cell_value("Sheet1", i as u32 + 1, 5, text(s))
            .unwrap();
    }
    e
}

const R: u32 = 20;
const C: u32 = 20;

fn eval_grid(formula: &str, rows: u32, cols: u32) -> Vec<Vec<f64>> {
    let mut e = engine();
    e.set_cell_formula("Sheet1", R, C, parse(formula).unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    (R..R + rows)
        .map(|r| {
            (C..C + cols)
                .map(|c| match e.get_cell_value("Sheet1", r, c) {
                    Some(LiteralValue::Number(n)) => n,
                    Some(LiteralValue::Int(i)) => i as f64,
                    other => panic!("{formula} @({r},{c}): expected number, got {other:?}"),
                })
                .collect()
        })
        .collect()
}

fn eval_row(formula: &str, cols: u32) -> Vec<f64> {
    eval_grid(formula, 1, cols).remove(0)
}

fn eval_col(formula: &str, rows: u32) -> Vec<f64> {
    eval_grid(formula, rows, 1)
        .into_iter()
        .map(|r| r[0])
        .collect()
}

fn eval_scalar(formula: &str) -> f64 {
    let mut e = engine();
    e.set_cell_formula("Sheet1", R, C, parse(formula).unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    // A scalar result must not spill.
    assert!(
        e.get_cell_value("Sheet1", R, C + 1).is_none()
            && e.get_cell_value("Sheet1", R + 1, C).is_none(),
        "{formula}: unexpected spill"
    );
    match e.get_cell_value("Sheet1", R, C) {
        Some(LiteralValue::Number(n)) => n,
        Some(LiteralValue::Int(i)) => i as f64,
        other => panic!("{formula}: expected number, got {other:?}"),
    }
}

#[test]
fn countif_with_an_array_criterion_spills_one_count_per_element() {
    // 1 matches A1 (1) and A6 ("1" numeric text); 2 matches A5.
    assert_eq!(eval_row("=COUNTIF(A1:A8,{1,2})", 2), vec![2.0, 1.0]);
    assert_eq!(eval_row("=COUNTIF(E1:E5,{\"x\",\"y\"})", 2), vec![3.0, 1.0]);
    assert_eq!(eval_col("=COUNTIF(E1:E5,{\"x\";\"y\"})", 2), vec![3.0, 1.0]);
    assert_eq!(
        eval_row("=COUNTIF(C1:C5,{\">1\",\">3\"})", 2),
        vec![4.0, 2.0]
    );
    assert_eq!(
        eval_grid("=COUNTIF(C1:C5,{1,2;3,9})", 2, 2),
        vec![vec![1.0, 1.0], vec![1.0, 0.0]]
    );
}

#[test]
fn a_multi_cell_range_as_criteria_is_evaluated_per_cell() {
    assert_eq!(eval_col("=COUNTIF(C1:C5,C1:C5)", 5), vec![1.0; 5]);
    assert_eq!(
        eval_col("=COUNTIF(E1:E5,E1:E5)", 5),
        vec![3.0, 1.0, 3.0, 1.0, 3.0]
    );
    assert_eq!(
        eval_col("=SUMIF(E1:E5,E1:E5,C1:C5)", 5),
        vec![9.0, 2.0, 9.0, 4.0, 9.0]
    );
}

#[test]
fn the_sum_countif_and_sumproduct_sumif_idioms_fold_the_spill() {
    assert_eq!(eval_scalar("=SUM(COUNTIF(A1:A8,{1,2}))"), 3.0);
    assert_eq!(eval_scalar("=SUM(COUNTIF(E1:E5,{\"x\",\"y\"}))"), 4.0);
    assert_eq!(
        eval_scalar("=SUMPRODUCT(SUMIF(E1:E5,{\"x\",\"y\"},C1:C5))"),
        11.0
    );
    assert_eq!(eval_scalar("=SUMPRODUCT(COUNTIF(C1:C5,{1,2,3}))"), 3.0);
    assert_eq!(eval_scalar("=SUM(COUNTIF(C1:C5,C1:C5))"), 5.0);
}

#[test]
fn sumif_averageif_and_the_ifs_family_lift_array_criteria() {
    assert_eq!(
        eval_row("=SUMIF(E1:E5,{\"x\",\"y\"},C1:C5)", 2),
        vec![9.0, 2.0]
    );
    assert_eq!(
        eval_row("=SUMIFS(C1:C5,E1:E5,{\"x\",\"y\"})", 2),
        vec![9.0, 2.0]
    );
    assert_eq!(
        eval_row("=SUMIFS(C1:C5,E1:E5,\"x\",C1:C5,{\">1\",\">3\"})", 2),
        vec![8.0, 5.0]
    );
    assert_eq!(
        eval_row("=COUNTIFS(E1:E5,{\"x\",\"y\"},C1:C5,\">1\")", 2),
        vec![2.0, 1.0]
    );
    assert_eq!(
        eval_row("=AVERAGEIF(E1:E5,{\"x\",\"z\"},C1:C5)", 2),
        vec![3.0, 4.0]
    );
    assert_eq!(
        eval_row("=AVERAGEIFS(C1:C5,E1:E5,{\"x\",\"y\"})", 2),
        vec![3.0, 2.0]
    );
    // Two array criteria broadcast against each other.
    assert_eq!(
        eval_row(
            "=SUMIFS(C1:C5,E1:E5,{\"x\",\"y\"},C1:C5,{\">1\",\">1\"})",
            2
        ),
        vec![8.0, 2.0]
    );
}

#[test]
fn maxifs_and_minifs_lift_array_criteria() {
    assert_eq!(
        eval_row("=MAXIFS(C1:C5,E1:E5,{\"x\",\"y\"})", 2),
        vec![5.0, 2.0]
    );
    assert_eq!(
        eval_row("=MINIFS(C1:C5,E1:E5,{\"x\",\"z\"})", 2),
        vec![1.0, 4.0]
    );
}

#[test]
fn scalar_criteria_keep_the_scalar_path() {
    assert_eq!(eval_scalar("=COUNTIF(C1:C5,\">2\")"), 3.0);
    assert_eq!(eval_scalar("=COUNTIF(C1:C5,{3})"), 1.0);
    assert_eq!(eval_scalar("=COUNTIF(C1:C5,C3)"), 1.0);
    assert_eq!(eval_scalar("=SUMIF(E1:E5,\"x\",C1:C5)"), 9.0);
    assert_eq!(eval_scalar("=SUMIFS(C1:C5,E1:E5,\"x\",C1:C5,\">1\")"), 8.0);
    assert_eq!(eval_scalar("=MAXIFS(C1:C5,E1:E5,\"x\")"), 5.0);
}
