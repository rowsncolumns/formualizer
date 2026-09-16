//! rowsncolumns/spreadsheet#546 (U23, E-27) — an unknown defined name or an unknown function
//! must evaluate to a `#NAME?` *value*, so IS*/IFERROR/ERROR.TYPE can catch it the way Excel
//! does (`=ISERROR(FOO)` → TRUE, `=IFERROR(FOO,1)` → 1, `=ERROR.TYPE(FOO)` → 5). Previously the
//! name lookup failed as a hard `Err` that bypassed every argument-level error handler.

use formualizer_common::{ExcelErrorKind, LiteralValue};
use formualizer_eval::engine::named_range::{NameScope, NamedDefinition};
use formualizer_eval::engine::{Engine, EvalConfig};
use formualizer_eval::test_workbook::TestWorkbook;
use formualizer_parse::parser::parse;

fn eval(formula: &str) -> LiteralValue {
    let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
    e.set_cell_value("Sheet1", 1, 2, LiteralValue::Number(7.0))
        .unwrap();
    e.set_cell_formula("Sheet1", 1, 1, parse(formula).unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    e.get_cell_value("Sheet1", 1, 1).unwrap()
}

fn assert_name_error(v: &LiteralValue) {
    match v {
        LiteralValue::Error(e) => assert_eq!(e.kind, ExcelErrorKind::Name, "{v:?}"),
        other => panic!("expected #NAME?, got {other:?}"),
    }
}

#[test]
fn unknown_name_is_a_name_error_value() {
    assert_name_error(&eval("=FOO"));
    assert_name_error(&eval("=FOO+1"));
    assert_name_error(&eval("=SUM(FOO)"));
}

#[test]
fn unknown_name_is_catchable() {
    assert_eq!(eval("=ISERROR(FOO)"), LiteralValue::Boolean(true));
    assert_eq!(eval("=ISERR(FOO)"), LiteralValue::Boolean(true));
    assert_eq!(eval("=ISNA(FOO)"), LiteralValue::Boolean(false));
    assert_eq!(eval("=IFERROR(FOO,1)"), LiteralValue::Number(1.0));
    assert_eq!(eval("=ERROR.TYPE(FOO)"), LiteralValue::Number(5.0));
    assert_eq!(eval("=IF(ISERROR(FOO),B1,0)"), LiteralValue::Number(7.0));
}

#[test]
fn unknown_function_is_a_name_error_value() {
    assert_name_error(&eval("=FOOBAR(1)"));
    assert_name_error(&eval("=FOOBAR(1)+1"));
}

#[test]
fn unknown_function_is_catchable() {
    assert_eq!(eval("=ISERROR(FOOBAR(1))"), LiteralValue::Boolean(true));
    assert_eq!(eval("=IFERROR(FOOBAR(1),2)"), LiteralValue::Number(2.0));
    assert_eq!(eval("=ERROR.TYPE(FOOBAR(1))"), LiteralValue::Number(5.0));
    assert_eq!(
        eval("=IFERROR(_xlfn.FOOBAR(1),3)"),
        LiteralValue::Number(3.0)
    );
}

#[test]
fn defining_the_name_later_recomputes_dependents() {
    let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
    e.set_cell_formula("Sheet1", 1, 1, parse("=IFERROR(FOO,1)").unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    assert_eq!(
        e.get_cell_value("Sheet1", 1, 1),
        Some(LiteralValue::Number(1.0))
    );
    e.define_name(
        "FOO",
        NamedDefinition::Literal(LiteralValue::Number(42.0)),
        NameScope::Workbook,
    )
    .unwrap();
    e.evaluate_all().unwrap();
    assert_eq!(
        e.get_cell_value("Sheet1", 1, 1),
        Some(LiteralValue::Number(42.0))
    );
}
