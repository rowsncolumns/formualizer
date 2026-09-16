//! rowsncolumns/spreadsheet#531 — `TEXTJOIN` over a range joined only the top-left cell, and
//! `FILTER` always evaluated to `#REF!` because its by-ref `include` argument (`B1:B4>0`, a
//! computed boolean array) and nested array-producing functions (`SORT(FILTER(...))`) were
//! forced through reference resolution instead of being materialized as owned range views.

use formualizer_common::{ExcelErrorKind, LiteralValue};
use formualizer_eval::engine::{Engine, EvalConfig};
use formualizer_eval::test_workbook::TestWorkbook;
use formualizer_parse::parser::parse;

fn engine() -> Engine<TestWorkbook> {
    Engine::new(TestWorkbook::new(), EvalConfig::default())
}

fn text(s: &str) -> LiteralValue {
    LiteralValue::Text(s.into())
}

fn num(n: f64) -> LiteralValue {
    LiteralValue::Number(n)
}

/// Seed A1:A3 = a,b,c and evaluate `formula` in E1.
fn eval_textjoin(seed: &[LiteralValue], formula: &str) -> Option<LiteralValue> {
    let mut e = engine();
    for (i, v) in seed.iter().enumerate() {
        e.set_cell_value("Sheet1", i as u32 + 1, 1, v.clone())
            .unwrap();
    }
    e.set_cell_formula("Sheet1", 1, 5, parse(formula).unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    e.get_cell_value("Sheet1", 1, 5)
}

#[test]
fn textjoin_range_joins_every_cell() {
    let abc = [text("a"), text("b"), text("c")];
    assert_eq!(
        eval_textjoin(&abc, "=TEXTJOIN(\",\",TRUE,A1:A3)"),
        Some(text("a,b,c"))
    );
    assert_eq!(
        eval_textjoin(&abc, "=TEXTJOIN(\",\",TRUE,A1,A2,A3)"),
        Some(text("a,b,c")),
        "scalar arguments keep working"
    );
    assert_eq!(
        eval_textjoin(&abc, "=TEXTJOIN(\"-\",TRUE,\"x\",A1:A3,\"y\")"),
        Some(text("x-a-b-c-y")),
        "ranges mix with scalar arguments in argument order"
    );
}

#[test]
fn textjoin_range_of_numbers() {
    let nums = [num(1.0), num(2.0), num(3.0)];
    assert_eq!(
        eval_textjoin(&nums, "=TEXTJOIN(\"-\",TRUE,A1:A3)"),
        Some(text("1-2-3"))
    );
}

#[test]
fn textjoin_range_ignore_empty_flag() {
    let gap = [text("a"), LiteralValue::Empty, text("c")];
    assert_eq!(
        eval_textjoin(&gap, "=TEXTJOIN(\",\",TRUE,A1:A3)"),
        Some(text("a,c"))
    );
    assert_eq!(
        eval_textjoin(&gap, "=TEXTJOIN(\",\",FALSE,A1:A3)"),
        Some(text("a,,c"))
    );
}

#[test]
fn textjoin_array_literal_argument() {
    assert_eq!(
        eval_textjoin(&[], "=TEXTJOIN(\"|\",TRUE,{1,2;3,4})"),
        Some(text("1|2|3|4")),
        "array literals flatten row-major"
    );
}

/// Seed A1:B4 = [[3,1],[1,0],[4,2],[1,0]] and place `formula` at D1. Returns D1:E4 as read back.
fn eval_filter(formula: &str) -> Vec<Vec<Option<LiteralValue>>> {
    let mut e = engine();
    let rows = [[3.0, 1.0], [1.0, 0.0], [4.0, 2.0], [1.0, 0.0]];
    for (r, row) in rows.iter().enumerate() {
        for (c, v) in row.iter().enumerate() {
            e.set_cell_value("Sheet1", r as u32 + 1, c as u32 + 1, num(*v))
                .unwrap();
        }
    }
    e.set_cell_formula("Sheet1", 1, 4, parse(formula).unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    (1..=4)
        .map(|r| (4..=5).map(|c| e.get_cell_value("Sheet1", r, c)).collect())
        .collect()
}

#[test]
fn filter_single_column_with_computed_include() {
    let out = eval_filter("=FILTER(A1:A4,B1:B4>0)");
    assert_eq!(out[0][0], Some(num(3.0)));
    assert_eq!(out[1][0], Some(num(4.0)));
    assert!(
        !matches!(out[2][0], Some(LiteralValue::Error(_))),
        "no error below the spill: {:?}",
        out[2][0]
    );
}

#[test]
fn filter_two_columns_with_computed_include() {
    let out = eval_filter("=FILTER(A1:B4,B1:B4>0)");
    assert_eq!(out[0], vec![Some(num(3.0)), Some(num(1.0))]);
    assert_eq!(out[1], vec![Some(num(4.0)), Some(num(2.0))]);
}

#[test]
fn sort_over_filter_result() {
    let out = eval_filter("=SORT(FILTER(A1:A4,B1:B4>0),1,-1)");
    assert_eq!(out[0][0], Some(num(4.0)));
    assert_eq!(out[1][0], Some(num(3.0)));
}

#[test]
fn filter_no_match_is_calc_error_or_if_empty() {
    let out = eval_filter("=FILTER(A1:A4,B1:B4>100)");
    match &out[0][0] {
        Some(LiteralValue::Error(e)) => assert_eq!(e.kind, ExcelErrorKind::Calc),
        other => panic!("expected #CALC!, got {other:?}"),
    }
    let out = eval_filter("=FILTER(A1:A4,B1:B4>100,\"none\")");
    assert_eq!(out[0][0], Some(text("none")));
}
