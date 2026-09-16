//! rowsncolumns/spreadsheet#546 W5-D (B-recheck `AREAS((A1,B2))`, C R-23 `INDEX(union, …, area_num)`):
//! a parenthesised union `(A1:A5,C1:C5)` is a multi-area reference. `AREAS` counts its operands
//! and `INDEX`'s fourth argument picks the area to index; both were `#VALUE!` because the union
//! operator only existed in value context (stacked for aggregates).

use formualizer_common::{ExcelErrorKind, LiteralValue};
use formualizer_eval::engine::{Engine, EvalConfig};
use formualizer_eval::test_workbook::TestWorkbook;
use formualizer_parse::parser::parse;

/// A1:A5 = 1..5, C1:C5 = 2,3,4,5,6; the formula goes in E1.
fn eval(formula: &str) -> LiteralValue {
    let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
    for r in 1..=5u32 {
        e.set_cell_value("Sheet1", r, 1, LiteralValue::Int(r as i64))
            .unwrap();
        e.set_cell_value("Sheet1", r, 3, LiteralValue::Int(r as i64 + 1))
            .unwrap();
    }
    e.set_cell_formula("Sheet1", 1, 5, parse(formula).unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    e.get_cell_value("Sheet1", 1, 5)
        .unwrap_or(LiteralValue::Empty)
}

fn num(v: &LiteralValue) -> f64 {
    match v {
        LiteralValue::Number(n) => *n,
        LiteralValue::Int(i) => *i as f64,
        other => panic!("expected number, got {other:?}"),
    }
}

fn err_kind(v: &LiteralValue) -> ExcelErrorKind {
    match v {
        LiteralValue::Error(e) => e.kind,
        other => panic!("expected error, got {other:?}"),
    }
}

#[test]
fn areas_counts_union_operands() {
    assert_eq!(num(&eval("=AREAS((A1,B2))")), 2.0);
    assert_eq!(num(&eval("=AREAS((A1:B2,C3,D4:D6))")), 3.0);
    assert_eq!(num(&eval("=AREAS(((A1,B2),C3))")), 3.0);
    assert_eq!(num(&eval("=AREAS(A1:B2)")), 1.0);
    assert_eq!(num(&eval("=AREAS(A1)")), 1.0);
    assert_eq!(err_kind(&eval("=AREAS(1)")), ExcelErrorKind::Value);
}

#[test]
fn index_area_num_selects_the_union_area() {
    assert_eq!(num(&eval("=INDEX((A1:A5,C1:C5),2,1,2)")), 3.0);
    assert_eq!(num(&eval("=INDEX((A1:A5,C1:C5),2,1,1)")), 2.0);
    // area_num defaults to 1
    assert_eq!(num(&eval("=INDEX((A1:A5,C1:C5),4,1)")), 4.0);
    assert_eq!(num(&eval("=INDEX((A1:A5,C1:C5),5,,2)")), 6.0);
    // INDEX stays a reference: a whole-column pick feeds SUM
    assert_eq!(num(&eval("=SUM(INDEX((A1:A5,C1:C5),0,1,2))")), 20.0);
    // a single-area reference still accepts area_num 1
    assert_eq!(num(&eval("=INDEX(C1:C5,3,1,1)")), 4.0);
}

#[test]
fn index_area_num_out_of_range_is_ref_error() {
    assert_eq!(
        err_kind(&eval("=INDEX((A1:A5,C1:C5),2,1,3)")),
        ExcelErrorKind::Ref
    );
    assert_eq!(
        err_kind(&eval("=INDEX((A1:A5,C1:C5),2,1,0)")),
        ExcelErrorKind::Ref
    );
    assert_eq!(err_kind(&eval("=INDEX(A1:A5,2,1,2)")), ExcelErrorKind::Ref);
    // an array constant is one area
    assert_eq!(num(&eval("=INDEX({1,2;3,4},2,2,1)")), 4.0);
    assert_eq!(
        err_kind(&eval("=INDEX({1,2;3,4},2,2,2)")),
        ExcelErrorKind::Ref
    );
}

#[test]
fn union_in_value_context_still_stacks_for_aggregates() {
    assert_eq!(num(&eval("=SUM((A1:A5,C1:C5))")), 35.0);
    assert_eq!(num(&eval("=COUNT((A1,C1,C5))")), 3.0);
}
