//! `Engine::cell_dependents_closure` — the read-only transitive-dependents
//! query hosts use to find every cell derived from an asynchronous input
//! (so its in-flight state can shadow them). Must cover direct references,
//! range references, and transitive chains, without mutating dirty state.

use crate::engine::{Engine, EvalConfig};
use crate::test_workbook::TestWorkbook;
use formualizer_common::LiteralValue;
use formualizer_parse::parser::parse;

fn closure_of(engine: &Engine<TestWorkbook>, cells: &[(&str, u32, u32)]) -> Vec<(String, u32, u32)> {
    let cells: Vec<(String, u32, u32)> = cells
        .iter()
        .map(|(s, r, c)| (s.to_string(), *r, *c))
        .collect();
    let mut out = engine.cell_dependents_closure(&cells);
    out.sort();
    out
}

#[test]
fn direct_range_and_transitive_dependents() {
    let mut engine = Engine::new(TestWorkbook::default(), EvalConfig::default());
    engine
        .set_cell_value("Sheet1", 1, 1, LiteralValue::Int(1))
        .unwrap(); // A1 (the async input)
    engine
        .set_cell_formula("Sheet1", 1, 2, parse("=A1*2").unwrap())
        .unwrap(); // B1: direct
    engine
        .set_cell_formula("Sheet1", 1, 3, parse("=B1+1").unwrap())
        .unwrap(); // C1: transitive
    engine
        .set_cell_formula("Sheet1", 7, 3, parse("=SUM(A1:A5)").unwrap())
        .unwrap(); // C7: range over the input
    engine
        .set_cell_formula("Sheet1", 8, 3, parse("=SUM(D1:D5)").unwrap())
        .unwrap(); // C8: unrelated range
    engine.evaluate_all().unwrap();

    let closure = closure_of(&engine, &[("Sheet1", 1, 1)]);
    assert!(closure.contains(&("Sheet1".to_string(), 1, 2)), "direct dependent missing: {closure:?}");
    assert!(closure.contains(&("Sheet1".to_string(), 1, 3)), "transitive dependent missing: {closure:?}");
    assert!(closure.contains(&("Sheet1".to_string(), 7, 3)), "range dependent missing: {closure:?}");
    assert!(!closure.contains(&("Sheet1".to_string(), 8, 3)), "unrelated range must not appear: {closure:?}");
    assert!(!closure.contains(&("Sheet1".to_string(), 1, 1)), "the root itself must not appear: {closure:?}");
}

#[test]
fn closure_is_read_only_and_multi_source() {
    let mut engine = Engine::new(TestWorkbook::default(), EvalConfig::default());
    engine
        .set_cell_value("Sheet1", 1, 1, LiteralValue::Int(1))
        .unwrap(); // A1
    engine
        .set_cell_value("Sheet1", 5, 5, LiteralValue::Int(2))
        .unwrap(); // E5
    engine
        .set_cell_formula("Sheet1", 1, 2, parse("=A1*2").unwrap())
        .unwrap(); // B1
    engine
        .set_cell_formula("Sheet1", 5, 6, parse("=E5*2").unwrap())
        .unwrap(); // F5
    engine.evaluate_all().unwrap();

    // Multi-source: both inputs' dependents come back from one call.
    let closure = closure_of(&engine, &[("Sheet1", 1, 1), ("Sheet1", 5, 5)]);
    assert!(closure.contains(&("Sheet1".to_string(), 1, 2)), "{closure:?}");
    assert!(closure.contains(&("Sheet1".to_string(), 5, 6)), "{closure:?}");

    // Read-only: querying dirtied nothing, so a recalc re-evaluates nothing new
    // (evaluate_all on a clean graph must not change any value).
    engine.evaluate_all().unwrap();
    let after = closure_of(&engine, &[("Sheet1", 1, 1), ("Sheet1", 5, 5)]);
    assert_eq!(closure, after);

    // Unknown sheet / unmaterialized cell: contributes nothing, no panic.
    assert!(closure_of(&engine, &[("Nope", 1, 1)]).is_empty());
    assert!(closure_of(&engine, &[("Sheet1", 100, 100)]).is_empty());
}
