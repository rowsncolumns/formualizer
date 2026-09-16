//! rowsncolumns/spreadsheet#546 U17b follow-up — misc math / logical Excel parity pinned at the
//! engine level: AND/OR/XOR ignore text and blanks inside references and are `#VALUE!` when a
//! reference leaves nothing to test; IF/NOT coerce text booleans only in direct arguments (a
//! cell that *says* "TRUE" is `#VALUE!`); CEILING/FLOOR families round the 15-digit quotient
//! (`CEILING(0.3,0.1)` is exactly 0.3) and a zero significance is 0 for the CEILING family;
//! POWER overflow is `#NUM!`; PRODUCT of a direct non-numeric text literal is `#VALUE!`.

use formualizer_common::{ExcelErrorKind, LiteralValue};
use formualizer_eval::engine::{Engine, EvalConfig};
use formualizer_eval::test_workbook::TestWorkbook;
use formualizer_parse::parser::parse;

// A1 = 1, A2 = "x", A3 = TRUE, A4 = blank, A5 = 2, B1 = text "TRUE", B3 = "abc"
fn eval(formula: &str) -> LiteralValue {
    let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
    e.set_cell_value("Sheet1", 1, 1, LiteralValue::Number(1.0))
        .unwrap();
    e.set_cell_value("Sheet1", 2, 1, LiteralValue::Text("x".into()))
        .unwrap();
    e.set_cell_value("Sheet1", 3, 1, LiteralValue::Boolean(true))
        .unwrap();
    e.set_cell_value("Sheet1", 5, 1, LiteralValue::Number(2.0))
        .unwrap();
    e.set_cell_value("Sheet1", 1, 2, LiteralValue::Text("TRUE".into()))
        .unwrap();
    e.set_cell_value("Sheet1", 3, 2, LiteralValue::Text("abc".into()))
        .unwrap();
    e.set_cell_formula("Sheet1", 10, 4, parse(formula).unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    e.get_cell_value("Sheet1", 10, 4).unwrap()
}

fn assert_number(formula: &str, want: f64) {
    match eval(formula) {
        LiteralValue::Number(n) => assert_eq!(n, want, "{formula}"),
        LiteralValue::Int(i) => assert_eq!(i as f64, want, "{formula}"),
        other => panic!("{formula}: expected {want}, got {other:?}"),
    }
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
fn and_or_xor_are_value_error_when_a_reference_has_no_logical_values() {
    assert_error("=AND(A2:A2)", ExcelErrorKind::Value);
    assert_error("=AND(B1)", ExcelErrorKind::Value);
    assert_error("=AND(A4:A4)", ExcelErrorKind::Value);
    assert_error("=OR(A2:A2)", ExcelErrorKind::Value);
    assert_error("=OR(B1)", ExcelErrorKind::Value);
    assert_error("=OR(A4:A4)", ExcelErrorKind::Value);
    assert_error("=OR(A2,A4)", ExcelErrorKind::Value);
    assert_error("=XOR(A2:A2)", ExcelErrorKind::Value);
    assert_error("=XOR(B1)", ExcelErrorKind::Value);
}

#[test]
fn and_or_ignore_blanks_and_text_inside_references() {
    // A3 = TRUE, A4 blank: the blank is skipped, not read as FALSE.
    assert_bool("=AND(A3:A4)", true);
    assert_bool("=AND(A1:A5)", true);
    assert_bool("=AND(A3,A4)", true);
    assert_bool("=OR(A2:A4)", true);
    assert_bool("=OR(A4,A3)", true);
    // A1 = 1, A3 = TRUE, A5 = 2: three truthy cells, text and blank skipped.
    assert_bool("=XOR(A1:A5)", true);
    assert_bool("=XOR(A1:A4)", false);
    // A cell holding FALSE / 0 still decides the result.
    assert_bool("=AND(A3,0)", false);
    assert_bool("=OR(A4,0)", false);
}

#[test]
fn direct_text_arguments_still_coerce_or_fail() {
    assert_bool("=AND(\"TRUE\")", true);
    assert_bool("=OR(\"FALSE\",\"true\")", true);
    assert_bool("=XOR(\"TRUE\")", true);
    assert_error("=AND(TRUE,\"a\")", ExcelErrorKind::Value);
    assert_error("=OR(FALSE,\"a\")", ExcelErrorKind::Value);
    assert_error("=AND(\"1\")", ExcelErrorKind::Value);
}

#[test]
fn if_and_not_reject_text_booleans_read_through_a_reference() {
    assert_error("=IF(B1,1,2)", ExcelErrorKind::Value);
    assert_error("=IF(B3,1,2)", ExcelErrorKind::Value);
    assert_error("=NOT(B1)", ExcelErrorKind::Value);
    assert_error("=NOT(B3)", ExcelErrorKind::Value);
    // Direct text booleans and real logicals/numbers in cells are unaffected.
    assert_number("=IF(\"TRUE\",1,2)", 1.0);
    assert_bool("=NOT(\"TRUE\")", false);
    assert_number("=IF(A3,1,2)", 1.0);
    assert_number("=IF(A1,1,2)", 1.0);
    assert_number("=IF(A4,1,2)", 2.0);
    assert_bool("=NOT(A3)", false);
    assert_bool("=NOT(A4)", true);
}

#[test]
fn ceiling_floor_families_round_the_fifteen_digit_quotient() {
    assert_number("=CEILING(-0.3,0.1)", -0.3);
    assert_number("=CEILING(0.3,0.1)", 0.3);
    assert_number("=CEILING(1.5,0.1)", 1.5);
    assert_number("=CEILING(0.234,0.01)", 0.24);
    assert_number("=CEILING(-0.3,-0.1)", -0.3);
    assert_number("=FLOOR(0.3,0.1)", 0.3);
    assert_number("=FLOOR(-0.3,0.1)", -0.3);
    assert_number("=CEILING.MATH(0.3,0.1)", 0.3);
    assert_number("=CEILING.MATH(-0.3,0.1)", -0.3);
    assert_number("=CEILING.MATH(-0.3,0.1,1)", -0.3);
    assert_number("=FLOOR.MATH(0.3,0.1)", 0.3);
    assert_number("=FLOOR.MATH(-0.3,0.1,1)", -0.3);
    assert_number("=CEILING.PRECISE(0.3,0.1)", 0.3);
    assert_number("=FLOOR.PRECISE(-0.3,0.1)", -0.3);
    // Three arguments (mode) are accepted by the schema.
    assert_number("=CEILING.MATH(-5.5,2,-1)", -6.0);
    assert_number("=FLOOR.MATH(-5.5,2,1)", -4.0);
    assert_number("=CEILING.MATH(6.7)", 7.0);
}

#[test]
fn floor_sign_rules_match_excel() {
    assert_error("=FLOOR(2.5,-2)", ExcelErrorKind::Num);
    assert_number("=FLOOR(-2.5,-2)", -2.0);
    assert_number("=FLOOR(-2.5,2)", -4.0);
    assert_number("=FLOOR(2.5,2)", 2.0);
    assert_number("=FLOOR(-0.3,-0.1)", -0.3);
}

#[test]
fn zero_significance_is_zero_for_the_ceiling_family_and_div0_for_floor() {
    assert_number("=CEILING(2.5,0)", 0.0);
    assert_number("=CEILING.MATH(6.7,0)", 0.0);
    assert_number("=CEILING.PRECISE(6.7,0)", 0.0);
    assert_error("=FLOOR(2.5,0)", ExcelErrorKind::Div);
}

#[test]
fn power_overflow_is_num_error_like_the_operator() {
    assert_error("=POWER(2,1024)", ExcelErrorKind::Num);
    assert_error("=2^1024", ExcelErrorKind::Num);
    assert_number("=POWER(2,10)", 1024.0);
    assert_error("=POWER(0,0)", ExcelErrorKind::Num);
}

#[test]
fn product_direct_non_numeric_text_is_value_error() {
    assert_error("=PRODUCT(\"x\")", ExcelErrorKind::Value);
    assert_error("=PRODUCT(1,2,3,4,5,\"2c\")", ExcelErrorKind::Value);
    assert_number("=PRODUCT(\"2\",3)", 6.0);
    assert_number("=PRODUCT(TRUE,2)", 2.0);
    // Text and blanks inside references are ignored; nothing numeric is 0.
    assert_number("=PRODUCT(A2:A2)", 0.0);
    assert_number("=PRODUCT(A4:A4)", 0.0);
    assert_number("=PRODUCT(A1:A5)", 2.0);
}
