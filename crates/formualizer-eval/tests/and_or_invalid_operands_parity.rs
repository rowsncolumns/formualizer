//! rowsncolumns/spreadsheet#642 B-08 — Excel never short-circuits `AND`/`OR` past an invalid
//! operand. An error argument propagates whatever the other arguments decide (`=AND(1/0, FALSE)`
//! is `#DIV/0!`, `=OR(TRUE, NA())` is `#N/A`), and a direct text argument that is not `TRUE` /
//! `FALSE` is `#VALUE!` (`=AND("Test", FALSE)`, `=OR("Test", TRUE)`). The first error in argument
//! order wins. Text and blanks inside references are still ignored — `=AND(A2, FALSE)` with A2 = "x"
//! is FALSE — and an error cell inside a referenced range propagates like a direct one.

use formualizer_common::{ExcelErrorKind, LiteralValue};
use formualizer_eval::engine::{Engine, EvalConfig};
use formualizer_eval::test_workbook::TestWorkbook;
use formualizer_parse::parser::parse;

// A1 = 1, A2 = "x", A3 = TRUE, A4 = blank, B1 = =NA() (an error cell), B2 = FALSE
fn eval(formula: &str) -> LiteralValue {
    let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
    e.set_cell_value("Sheet1", 1, 1, LiteralValue::Number(1.0))
        .unwrap();
    e.set_cell_value("Sheet1", 2, 1, LiteralValue::Text("x".into()))
        .unwrap();
    e.set_cell_value("Sheet1", 3, 1, LiteralValue::Boolean(true))
        .unwrap();
    e.set_cell_formula("Sheet1", 1, 2, parse("=NA()").unwrap())
        .unwrap();
    e.set_cell_value("Sheet1", 2, 2, LiteralValue::Boolean(false))
        .unwrap();
    e.set_cell_formula("Sheet1", 10, 4, parse(formula).unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    e.get_cell_value("Sheet1", 10, 4).unwrap()
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
fn a_direct_text_operand_that_is_not_a_boolean_is_value_error_beside_a_decisive_operand() {
    assert_error("=AND(\"Test\", FALSE)", ExcelErrorKind::Value);
    assert_error("=AND(FALSE, \"Test\")", ExcelErrorKind::Value);
    assert_error("=AND(\"Test\", B2)", ExcelErrorKind::Value);
    assert_error("=OR(\"Test\", TRUE)", ExcelErrorKind::Value);
    assert_error("=OR(TRUE, \"Test\")", ExcelErrorKind::Value);
    assert_error("=OR(\"Test\", A3)", ExcelErrorKind::Value);
}

#[test]
fn an_error_operand_propagates_past_a_decisive_operand() {
    assert_error("=AND(1/0, FALSE)", ExcelErrorKind::Div);
    assert_error("=AND(FALSE, 1/0)", ExcelErrorKind::Div);
    assert_error("=AND(FALSE, NA())", ExcelErrorKind::Na);
    assert_error("=OR(1/0, TRUE)", ExcelErrorKind::Div);
    assert_error("=OR(TRUE, 1/0)", ExcelErrorKind::Div);
    assert_error("=OR(TRUE, NA())", ExcelErrorKind::Na);
}

#[test]
fn the_first_error_in_argument_order_wins() {
    assert_error("=AND(1/0, NA())", ExcelErrorKind::Div);
    assert_error("=AND(NA(), 1/0)", ExcelErrorKind::Na);
    assert_error("=OR(1/0, NA())", ExcelErrorKind::Div);
    assert_error("=OR(NA(), 1/0)", ExcelErrorKind::Na);
    // An error ahead of an invalid text operand is the result (the text is never reached).
    assert_error("=AND(1/0, \"Test\")", ExcelErrorKind::Div);
    assert_error("=OR(NA(), \"Test\")", ExcelErrorKind::Na);
}

#[test]
fn an_error_cell_inside_a_reference_propagates_too() {
    assert_error("=AND(B1, FALSE)", ExcelErrorKind::Na);
    assert_error("=AND(FALSE, B1)", ExcelErrorKind::Na);
    assert_error("=OR(TRUE, B1)", ExcelErrorKind::Na);
    assert_error("=OR(A3:B3, B1:B2)", ExcelErrorKind::Na);
    assert_error("=AND(A1:B2)", ExcelErrorKind::Na);
}

#[test]
fn text_and_blanks_inside_references_are_still_ignored_beside_a_decisive_operand() {
    assert_bool("=AND(A2, FALSE)", false);
    assert_bool("=AND(A2:A4, FALSE)", false);
    assert_bool("=AND(A2:A4, TRUE)", true);
    assert_bool("=OR(A2, TRUE)", true);
    assert_bool("=OR(A2:A4, B2)", true);
    assert_bool("=OR(A4, B2)", false);
}

#[test]
fn text_booleans_in_direct_arguments_still_coerce() {
    assert_bool("=AND(\"TRUE\", FALSE)", false);
    assert_bool("=AND(\"true\", TRUE)", true);
    assert_bool("=OR(\"false\", TRUE)", true);
    assert_bool("=OR(\"FALSE\", B2)", false);
}
