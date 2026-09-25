//! rowsncolumns/spreadsheet#939 F-04 / G-04 — `AND` / `OR` accept the boolean or numeric ARRAY a
//! comparison, an operator or a lifted function produces (`AND(C1:C5>0)`, `OR(E1:E5="x")`,
//! `AND(ISNUMBER(C1:C5))`, `AND(TRUE,A1:A5>5)`, `AND({1,2}>0)`, `AND(--(A1:A5>5))`,
//! `IF(AND(A1:A5>5),"y","n")`, `AND(RANDARRAY(2,2,5,5)=5)`) and test it element by element, the
//! way they test a range. Excel's rules carry over: text and blank ELEMENTS of an array are
//! ignored (a direct text ARGUMENT is still `#VALUE!`), an array with nothing logical in it is
//! `#VALUE!`, and an error element propagates.

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

/// A1:A5 = 10..50 (A6:A8 blank) · C1:C5 = 1..5 · E1:E5 = x,y,x,z,x · K1 = =NA().
fn engine() -> Engine<TestWorkbook> {
    let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
    for i in 0..5u32 {
        e.set_cell_value("Sheet1", i + 1, 1, num(10.0 * (i + 1) as f64))
            .unwrap();
        e.set_cell_value("Sheet1", i + 1, 3, num((i + 1) as f64))
            .unwrap();
    }
    for (i, s) in ["x", "y", "x", "z", "x"].iter().enumerate() {
        e.set_cell_value("Sheet1", i as u32 + 1, 5, text(s))
            .unwrap();
    }
    e.set_cell_formula("Sheet1", 1, 11, parse("=NA()").unwrap())
        .unwrap();
    e
}

fn eval(formula: &str) -> LiteralValue {
    let mut e = engine();
    e.set_cell_formula("Sheet1", 20, 20, parse(formula).unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    e.get_cell_value("Sheet1", 20, 20).unwrap()
}

fn assert_bool(formula: &str, want: bool) {
    assert_eq!(eval(formula), LiteralValue::Boolean(want), "{formula}");
}

fn assert_error(formula: &str, kind: ExcelErrorKind) {
    match eval(formula) {
        LiteralValue::Error(e) => assert_eq!(e.kind, kind, "{formula}"),
        other => panic!("{formula}: expected {kind:?}, got {other:?}"),
    }
}

#[test]
fn and_or_test_the_array_a_comparison_over_a_range_produces() {
    assert_bool("=AND(C1:C5>0)", true);
    assert_bool("=AND(C1:C5>1)", false);
    assert_bool("=OR(C1:C5>4)", true);
    assert_bool("=OR(C1:C5>5)", false);
    assert_bool("=OR(E1:E5=\"x\")", true);
    assert_bool("=AND(E1:E5=\"x\")", false);
    assert_bool("=AND(A1:A5>5)", true);
    assert_bool("=OR(A1:A5>45)", true);
    assert_bool("=AND(A1:A5>45)", false);
}

#[test]
fn and_or_accept_arrays_from_operators_lifted_functions_and_constants() {
    assert_bool("=AND(ISNUMBER(C1:C5))", true);
    assert_bool("=AND(ISNUMBER(E1:E5))", false);
    assert_bool("=OR(ISBLANK(A1:A8))", true);
    assert_bool("=AND(--(A1:A5>5))", true);
    assert_bool("=AND((A1:A5>5)*1)", true);
    assert_bool("=AND({1,2}>0)", true);
    assert_bool("=OR({0,0}>0)", false);
    assert_bool("=AND({TRUE,TRUE})", true);
    assert_bool("=AND(RANDARRAY(2,2,5,5)=5)", true);
    assert_bool("=AND(NOT(A1:A5>45))", false);
}

#[test]
fn array_operands_mix_with_direct_operands_and_feed_other_functions() {
    assert_bool("=AND(C1:C5>0,TRUE)", true);
    assert_bool("=AND(TRUE,A1:A5>5)", true);
    assert_bool("=AND(C1:C5>0,FALSE)", false);
    assert_bool("=OR(FALSE,A1:A5>45)", true);
    assert_bool("=AND(C1:C5>0,E1:E5<>\"q\")", true);
    assert_eq!(eval("=IF(AND(A1:A5>5),\"y\",\"n\")"), text("y"));
    assert_eq!(eval("=IF(AND(A1:A5>15),\"y\",\"n\")"), text("n"));
    assert_eq!(eval("=SUM(IF(AND(C1:C5>0),1,0))"), num(1.0));
}

#[test]
fn text_and_blank_elements_inside_an_array_are_ignored_like_inside_a_range() {
    // Only the boolean elements decide; "x" and blank slots are skipped.
    assert_bool("=AND(IF(C1:C5>2,TRUE,\"x\"))", true);
    assert_bool("=OR(IF(C1:C5>4,TRUE,\"x\"))", true);
    assert_bool("=AND({TRUE,\"x\"})", true);
    assert_bool("=AND(IF(C1:C5>2,FALSE,\"x\"))", false);
    // A direct text argument beside an array is still #VALUE! (Excel never coerces "x").
    assert_error("=AND(\"x\",C1:C5>0)", ExcelErrorKind::Value);
    assert_error("=OR(C1:C5>0,\"x\")", ExcelErrorKind::Value);
}

#[test]
fn an_array_with_nothing_logical_in_it_is_value_error() {
    assert_error("=AND(IF(C1:C5>0,\"t\",\"f\"))", ExcelErrorKind::Value);
    assert_error("=OR({\"a\",\"b\"})", ExcelErrorKind::Value);
}

#[test]
fn an_error_element_inside_the_array_propagates() {
    assert_error("=AND(IF(C1:C5>2,TRUE,NA()))", ExcelErrorKind::Na);
    assert_error("=OR(IF(C1:C5>2,TRUE,NA()))", ExcelErrorKind::Na);
    assert_error("=AND(C1:C5>0,K1)", ExcelErrorKind::Na);
    assert_error("=AND((C1:C5>0)+K1)", ExcelErrorKind::Na);
}

#[test]
fn xor_and_not_over_the_same_arrays_keep_working() {
    // 3, 4, 5 are > 2: an odd count of TRUEs.
    assert_bool("=XOR(C1:C5>2)", true);
    assert_bool("=XOR(C1:C5>1)", false);
    assert_eq!(eval("=SUM(--NOT(C1:C5>2))"), num(2.0));
}
