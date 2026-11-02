//! Excel calculation options the host drives (demo-app-monorepo#49969 FX-07 / FX-08 / FX-18):
//! a circular group that iterates only when dirty, `INFO("recalc")` reporting Manual mode, and
//! Excel's 32,767-character cell text limit.
//! <https://support.microsoft.com/en-us/office/change-formula-recalculation-iteration-or-precision-in-excel-73fc7dac-91cf-4d36-86e8-67124f6bcce4>

use crate::engine::{CycleConfig, Engine, EvalConfig};
use crate::test_workbook::TestWorkbook;
use formualizer_common::{ExcelErrorKind, LiteralValue};
use formualizer_parse::parser::parse;

fn engine(cfg: EvalConfig) -> Engine<TestWorkbook> {
    Engine::new(TestWorkbook::new(), cfg)
}

fn formula(engine: &mut Engine<TestWorkbook>, row: u32, col: u32, f: &str) {
    engine
        .set_cell_formula("Sheet1", row, col, parse(f).expect("parse"))
        .expect("set formula");
}

fn value(engine: &Engine<TestWorkbook>, row: u32, col: u32) -> Option<LiteralValue> {
    engine.get_cell_value("Sheet1", row, col)
}

fn num(engine: &Engine<TestWorkbook>, row: u32, col: u32) -> f64 {
    match value(engine, row, col) {
        Some(LiteralValue::Number(n)) => n,
        Some(LiteralValue::Int(i)) => i as f64,
        other => panic!("expected a number at r{row}c{col}, got {other:?}"),
    }
}

fn dirty_only_iteration(max_iterations: u32) -> EvalConfig {
    let mut cfg = EvalConfig::default().with_cycle(CycleConfig::iterate(max_iterations, 0.001));
    cfg.iterate_circular_on_every_recalc = false;
    cfg
}

#[test]
fn circular_group_iterates_only_when_dirty() {
    let mut e = engine(dirty_only_iteration(100));
    e.set_cell_value("Sheet1", 1, 2, LiteralValue::Number(1.0))
        .unwrap();
    formula(&mut e, 1, 1, "=A1+B1");
    e.evaluate_all().unwrap();
    assert_eq!(num(&e, 1, 1), 100.0, "one recalc runs exactly 100 passes");

    // An edit the group does not read: the recalc leaves A1 alone.
    e.set_cell_value("Sheet1", 5, 5, LiteralValue::Number(42.0))
        .unwrap();
    formula(&mut e, 6, 5, "=E5*2");
    e.evaluate_all().unwrap();
    assert_eq!(num(&e, 1, 1), 100.0, "unrelated edit does not iterate A1");
    assert_eq!(num(&e, 6, 5), 84.0);

    // An edit the group reads makes it dirty: it iterates again, from its last value.
    e.set_cell_value("Sheet1", 1, 2, LiteralValue::Number(2.0))
        .unwrap();
    e.evaluate_all().unwrap();
    assert_eq!(num(&e, 1, 1), 300.0, "B1 changed: 100 more passes of +2");

    // A forced recalculation (F9) iterates the circular group again; nothing else.
    e.mark_circular_groups_dirty();
    e.evaluate_all().unwrap();
    assert_eq!(num(&e, 1, 1), 500.0, "F9: 100 more passes");
    assert_eq!(num(&e, 6, 5), 84.0);
}

#[test]
fn mark_all_formulas_dirty_recalculates_every_formula() {
    let mut e = engine(dirty_only_iteration(10));
    formula(&mut e, 1, 1, "=A1+1");
    formula(&mut e, 2, 1, "=1+1");
    e.evaluate_all().unwrap();
    assert_eq!(num(&e, 1, 1), 10.0);
    e.evaluate_all().unwrap();
    assert_eq!(num(&e, 1, 1), 10.0, "nothing dirty, nothing iterates");
    e.mark_all_formulas_dirty();
    e.evaluate_all().unwrap();
    assert_eq!(
        num(&e, 1, 1),
        20.0,
        "Ctrl+Alt+F9 recalculates the circular cell too"
    );
    assert_eq!(num(&e, 2, 1), 2.0);
}

#[test]
fn default_still_iterates_on_every_recalc() {
    let mut e = engine(EvalConfig::default().with_cycle(CycleConfig::iterate(100, 0.001)));
    formula(&mut e, 1, 1, "=A1+1");
    e.evaluate_all().unwrap();
    assert_eq!(num(&e, 1, 1), 100.0);
    e.set_cell_value("Sheet1", 5, 5, LiteralValue::Number(42.0))
        .unwrap();
    e.evaluate_all().unwrap();
    assert_eq!(
        num(&e, 1, 1),
        200.0,
        "the accumulator contract is the default"
    );
}

#[test]
fn info_recalc_reports_the_calculation_mode() {
    let mut e = engine(EvalConfig::default());
    formula(&mut e, 1, 1, "=INFO(\"recalc\")");
    e.evaluate_all().unwrap();
    assert_eq!(
        value(&e, 1, 1),
        Some(LiteralValue::Text("Automatic".into()))
    );
    e.set_manual_calculation(true);
    formula(&mut e, 1, 1, "=INFO(\"recalc\")");
    e.evaluate_all().unwrap();
    assert_eq!(value(&e, 1, 1), Some(LiteralValue::Text("Manual".into())));
}

fn is_value_error(v: Option<LiteralValue>) -> bool {
    matches!(v, Some(LiteralValue::Error(e)) if e.kind == ExcelErrorKind::Value)
}

#[test]
fn text_results_over_32767_characters_are_value_errors() {
    let mut e = engine(EvalConfig::default());
    formula(&mut e, 1, 1, "=REPT(\"a\",20000)");
    formula(&mut e, 1, 2, "=A1&A1");
    formula(&mut e, 1, 3, "=CONCAT(A1,A1)");
    formula(&mut e, 1, 4, "=LEN(A1&A1)");
    formula(&mut e, 1, 5, "=TEXTJOIN(\",\",TRUE,A1,A1)");
    formula(&mut e, 1, 6, "=LEN(REPT(\"a\",16384)&REPT(\"b\",16383))");
    formula(&mut e, 1, 7, "=SUBSTITUTE(A1,\"a\",\"bb\")");
    formula(&mut e, 1, 8, "=CONCATENATE(A1,A1)");
    e.evaluate_all().unwrap();
    assert_eq!(
        num(&e, 1, 6),
        32767.0,
        "exactly 32,767 characters is allowed"
    );
    for col in [2, 3, 4, 5, 7, 8] {
        assert!(
            is_value_error(value(&e, 1, col)),
            "col {col}: {:?}",
            value(&e, 1, col).map(|v| format!("{v:?}").chars().take(60).collect::<String>())
        );
    }
}
