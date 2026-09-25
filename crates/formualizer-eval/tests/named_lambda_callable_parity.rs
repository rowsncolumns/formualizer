//! Excel-parity pins for defined names whose value is a `LAMBDA` (rowsncolumns/spreadsheet#939 G-07).
//!
//! Excel lets a name be bound to a lambda — `Dbl` := `=LAMBDA(x,x*2)` — and then treats the name
//! as a callable function everywhere: `=Dbl(21)` is 42, `=MAP(A1:A3,Dbl)` spills `{20;40;60}`,
//! `=LET(f,Dbl,f(3))` is 6, and a recursive name (`Fact` := `=LAMBDA(n,IF(n<=1,1,n*Fact(n-1)))`)
//! calls itself. The fork resolved a call only against builtins and LET/LAMBDA-local bindings, so
//! every non-recursive named lambda was `#NAME?`.
//!
//! Seed grid (1-based): A1:A3 = 10,20,30.

use formualizer_common::{ExcelErrorKind, LiteralValue};
use formualizer_eval::engine::named_range::{NameScope, NamedDefinition};
use formualizer_eval::engine::{Engine, EvalConfig};
use formualizer_eval::test_workbook::TestWorkbook;
use formualizer_parse::parser::parse;

fn n(v: f64) -> LiteralValue {
    LiteralValue::Number(v)
}

fn seeded() -> Engine<TestWorkbook> {
    let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
    for (i, v) in [10.0, 20.0, 30.0].into_iter().enumerate() {
        e.set_cell_value("Sheet1", (i + 1) as u32, 1, n(v)).unwrap();
    }
    e
}

fn define(e: &mut Engine<TestWorkbook>, name: &str, formula: &str, scope: NameScope) {
    e.define_name(
        name,
        NamedDefinition::Formula {
            ast: parse(formula).unwrap(),
            dependencies: Vec::new(),
            range_deps: Vec::new(),
        },
        scope,
    )
    .unwrap();
}

fn eval_in(e: &mut Engine<TestWorkbook>, formula: &str) -> LiteralValue {
    e.set_cell_formula("Sheet1", 1, 3, parse(formula).unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    e.get_cell_value("Sheet1", 1, 3).unwrap()
}

/// Evaluate `formula` in C1 with `Dbl := =LAMBDA(x,x*2)` defined workbook-wide.
fn eval_with_dbl(formula: &str) -> LiteralValue {
    let mut e = seeded();
    define(&mut e, "Dbl", "=LAMBDA(x,x*2)", NameScope::Workbook);
    eval_in(&mut e, formula)
}

fn spill_column(e: &Engine<TestWorkbook>, col: u32, rows: u32) -> Vec<LiteralValue> {
    (1..=rows)
        .map(|r| {
            e.get_cell_value("Sheet1", r, col)
                .unwrap_or(LiteralValue::Empty)
        })
        .collect()
}

#[test]
fn named_lambda_is_callable_by_name() {
    assert_eq!(eval_with_dbl("=Dbl(21)"), n(42.0));
    assert_eq!(eval_with_dbl("=Dbl(A2)"), n(40.0));
    assert_eq!(eval_with_dbl("=Dbl(Dbl(5))"), n(20.0));
    assert_eq!(eval_with_dbl("=1+Dbl(21)"), n(43.0));
    assert_eq!(eval_with_dbl("=SUM(Dbl(1),Dbl(2))"), n(6.0));
}

#[test]
fn named_lambda_call_is_case_insensitive() {
    assert_eq!(eval_with_dbl("=DBL(21)"), n(42.0));
    assert_eq!(eval_with_dbl("=dbl(21)"), n(42.0));
}

#[test]
fn named_lambda_body_variants() {
    let mut e = seeded();
    define(&mut e, "Sq", "=LAMBDA(x,x^2)", NameScope::Workbook);
    define(&mut e, "AddOne", "=LAMBDA(a,a+1)", NameScope::Workbook);
    define(
        &mut e,
        "Clamp",
        "=LAMBDA(n,IF(n<=1,1,n*2))",
        NameScope::Workbook,
    );
    define(&mut e, "Add", "=LAMBDA(a,b,a+b)", NameScope::Workbook);
    assert_eq!(eval_in(&mut e, "=Sq(4)"), n(16.0));
    assert_eq!(eval_in(&mut e, "=AddOne(4)"), n(5.0));
    assert_eq!(eval_in(&mut e, "=Clamp(21)"), n(42.0));
    assert_eq!(eval_in(&mut e, "=Clamp(1)"), n(1.0));
    assert_eq!(eval_in(&mut e, "=Add(2,3)"), n(5.0));
}

#[test]
fn named_lambda_passed_as_value_to_higher_order_functions() {
    let mut e = seeded();
    define(&mut e, "Dbl", "=LAMBDA(x,x*2)", NameScope::Workbook);
    define(&mut e, "Add", "=LAMBDA(a,b,a+b)", NameScope::Workbook);
    define(&mut e, "RowTot", "=LAMBDA(r,SUM(r)*2)", NameScope::Workbook);
    assert_eq!(eval_in(&mut e, "=SUM(MAP(A1:A3,Dbl))"), n(120.0));
    assert_eq!(eval_in(&mut e, "=REDUCE(0,A1:A3,Add)"), n(60.0));
    assert_eq!(eval_in(&mut e, "=SUM(BYROW(A1:A3,RowTot))"), n(120.0));
    assert_eq!(eval_in(&mut e, "=SUM(SCAN(0,A1:A3,Add))"), n(100.0));
    // The spilled shape follows the input range.
    e.set_cell_formula("Sheet1", 1, 5, parse("=MAP(A1:A3,Dbl)").unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    assert_eq!(spill_column(&e, 5, 3), vec![n(20.0), n(40.0), n(60.0)]);
}

#[test]
fn named_lambda_bound_through_let() {
    assert_eq!(eval_with_dbl("=LET(f,Dbl,f(3))"), n(6.0));
    assert_eq!(eval_with_dbl("=LET(x,4,Dbl(x))"), n(8.0));
    // A LET binding shadows the workbook name.
    assert_eq!(eval_with_dbl("=LET(Dbl,LAMBDA(x,x*3),Dbl(3))"), n(9.0));
}

#[test]
fn recursive_named_lambda_calls_itself() {
    let mut e = seeded();
    define(
        &mut e,
        "Fact",
        "=LAMBDA(n,IF(n<=1,1,n*Fact(n-1)))",
        NameScope::Workbook,
    );
    assert_eq!(eval_in(&mut e, "=Fact(5)"), n(120.0));
    assert_eq!(eval_in(&mut e, "=Fact(1)"), n(1.0));
}

#[test]
fn sheet_scoped_named_lambda_is_callable() {
    let mut e = seeded();
    let sid = e.sheet_id("Sheet1").unwrap();
    define(&mut e, "Dbl", "=LAMBDA(x,x*2)", NameScope::Sheet(sid));
    assert_eq!(eval_in(&mut e, "=Dbl(21)"), n(42.0));
}

#[test]
fn named_lambda_arity_errors_match_direct_lambda() {
    // Surplus arguments are `#VALUE!` (a direct `LAMBDA(x,x*2)(1,2)` is too).
    match eval_with_dbl("=Dbl(1,2)") {
        LiteralValue::Error(err) => assert_eq!(err.kind, ExcelErrorKind::Value),
        other => panic!("expected #VALUE!, got {other:?}"),
    }
}

#[test]
fn named_lambda_read_as_a_plain_value_is_calc_error() {
    // Excel shows `#CALC!` for a lambda that reaches a cell as a value.
    match eval_with_dbl("=Dbl") {
        LiteralValue::Error(err) => assert_eq!(err.kind, ExcelErrorKind::Calc),
        other => panic!("expected #CALC!, got {other:?}"),
    }
}

#[test]
fn non_lambda_names_still_resolve_and_unknown_calls_stay_name_error() {
    let mut e = seeded();
    define(&mut e, "Rate2", "=0.07", NameScope::Workbook);
    define(&mut e, "Total", "=SUM(1,2,3)", NameScope::Workbook);
    assert_eq!(eval_in(&mut e, "=Rate2*2"), n(0.14));
    assert_eq!(eval_in(&mut e, "=Total*2"), n(12.0));
    // Calling a non-lambda name or an undefined name is `#NAME?` (Excel refuses entry; the
    // value form is what IFERROR sees).
    for f in ["=Total(1)", "=Nope(1)"] {
        match eval_in(&mut e, f) {
            LiteralValue::Error(err) => assert_eq!(err.kind, ExcelErrorKind::Name, "{f}"),
            other => panic!("{f}: expected #NAME?, got {other:?}"),
        }
    }
}
