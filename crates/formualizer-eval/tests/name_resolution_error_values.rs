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

/// Reviewer follow-up: the `#NAME?` must stay a catchable *value* behind a range / by-ref
/// parameter as well. Builtins reach the unresolved name through `?` on
/// `ArgumentHandle::range_view()` / `resolve_range_view`, which surfaced a hard `Err` that IS*
/// could not see while `IFERROR` (which matches `Err`) could — the same expression answered
/// differently under `ISERROR` and `IFERROR`. `IF(ISERROR(VLOOKUP(...)),...)` is the dominant
/// pre-IFERROR Excel idiom.
#[test]
fn unknown_name_behind_range_arguments_is_catchable() {
    for formula in [
        "=ISERROR(SUM(FOO))",
        "=ISERROR(SUMIF(FOO,1))",
        "=ISERROR(SUMIFS(B1:B2,FOO,1))",
        "=ISERROR(COUNTIF(FOO,1))",
        "=ISERROR(COUNTIFS(FOO,1))",
        "=ISERROR(AVERAGEIF(FOO,1))",
        "=ISERROR(SUMPRODUCT(FOO))",
        "=ISERROR(SUMPRODUCT(B1:B2,FOO))",
        "=ISERROR(INDEX(FOO,1))",
        "=ISERROR(MATCH(1,FOO,0))",
        "=ISERROR(VLOOKUP(1,FOO,1,FALSE))",
        "=ISERROR(XLOOKUP(1,FOO,B1:B2))",
        "=ISERROR(FILTER(FOO,B1:B2=1))",
        "=ISERROR(OFFSET(FOO,0,0))",
        "=ISERROR(ROWS(FOO))",
        "=ISERROR(MAXIFS(B1:B2,FOO,1))",
    ] {
        assert_eq!(eval(formula), LiteralValue::Boolean(true), "{formula}");
    }
    assert_eq!(eval("=ISNA(SUMIF(FOO,1))"), LiteralValue::Boolean(false));
    assert_eq!(
        eval("=ISNA(VLOOKUP(1,FOO,1,FALSE))"),
        LiteralValue::Boolean(false)
    );
    assert_eq!(eval("=ISREF(FOO)"), LiteralValue::Boolean(false));
    assert_eq!(
        eval("=IF(ISERROR(SUMIF(FOO,1)),1,2)"),
        LiteralValue::Number(1.0)
    );
    assert_eq!(eval("=IFERROR(SUM(FOO),9)"), LiteralValue::Number(9.0));
    assert_eq!(
        eval("=IFERROR(VLOOKUP(1,FOO,1,FALSE),9)"),
        LiteralValue::Number(9.0)
    );
    assert_eq!(
        eval("=IFERROR(COUNTIF(FOO,1),9)"),
        LiteralValue::Number(9.0)
    );
    assert_eq!(eval("=IFERROR(ROWS(FOO),9)"), LiteralValue::Number(9.0));
}

/// The criteria aggregates propagate a non-reference range argument's error instead of
/// matching nothing and answering 0; `AREAS` checks that a defined name resolves.
#[test]
fn unknown_name_behind_range_arguments_is_a_name_error_value() {
    for formula in [
        "=SUMIF(FOO,1)",
        "=SUMIF(B1:B2,1,FOO)",
        "=SUMIFS(FOO,B1:B2,1)",
        "=SUMIFS(B1:B2,FOO,1)",
        "=COUNTIF(FOO,1)",
        "=COUNTIFS(FOO,1)",
        "=AVERAGEIF(FOO,1)",
        "=MAXIFS(B1:B2,FOO,1)",
        "=MINIFS(B1:B2,FOO,1)",
        "=MAXIFS(FOO,B1:B2,1)",
        "=VLOOKUP(1,FOO,1,FALSE)",
        "=INDEX(FOO,1)",
        "=AREAS(FOO)",
    ] {
        assert_name_error(&eval(formula));
    }
}

/// A resolvable reference in the same positions is unaffected.
#[test]
fn resolved_ranges_behind_the_same_parameters_still_work() {
    assert_eq!(eval("=SUMIF(B1:B2,7)"), LiteralValue::Number(7.0));
    assert_eq!(eval("=MAXIFS(B1:B2,B1:B2,7)"), LiteralValue::Number(7.0));
    assert_eq!(eval("=COUNTIFS(B1:B2,7)"), LiteralValue::Number(1.0));
    assert_eq!(eval("=AREAS(B1:B2)"), LiteralValue::Number(1.0));
    assert_eq!(eval("=VLOOKUP(7,B1:B2,1,FALSE)"), LiteralValue::Number(7.0));
    assert_eq!(eval("=ISERROR(SUM(B1:B2))"), LiteralValue::Boolean(false));
}
