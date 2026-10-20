//! The schedule builder's per-rectangle memo (`RectDirtyFormulaCache` in `virtual_deps`): a
//! build that shares one cache across every candidate lists exactly the dependencies the
//! per-formula rectangle walk finds (self excluded), and a full evaluation over a template-shaped
//! workbook — hundreds of roll-ups reading the same formula-bearing block — computes every value.

use formualizer_common::LiteralValue;
use formualizer_parse::parser::parse;

use crate::engine::virtual_deps::{RangeVirtualDepProvider, VirtualDepBuilder};
use crate::engine::{Engine, EvalConfig, VertexId, VertexKind};
use crate::test_workbook::TestWorkbook;

const ROWS: u32 = 300;
const ROLLUPS: u32 = 200;
/// Row of the one formula placed INSIDE the rectangle it reads (`=ROWS($B$1:$B$310)`).
const SELF_ROW: u32 = 305;

fn formula(engine: &mut Engine<TestWorkbook>, row: u32, col: u32, text: &str) {
    let ast = parse(text).unwrap_or_else(|err| panic!("parse {text}: {err}"));
    engine.set_cell_formula("Sheet1", row, col, ast).unwrap();
}

/// A1:A300 = the row index, B1:B300 `=A*2` (so the block every roll-up reads holds formulas),
/// D1:D200 `=SUM($B$1:$B$300)`, E1:E200 `=SUMIFS($B$1:$B$300,$A$1:$A$300,">150")` and, when
/// `with_self_reference`, B305 `=ROWS($B$1:$B$310)` — a formula inside its own rectangle, so the
/// builder's self-exclusion is exercised (evaluation reports it as circular, like Excel).
fn template_workbook(with_self_reference: bool) -> Engine<TestWorkbook> {
    let mut engine = Engine::new(TestWorkbook::default(), EvalConfig::default());
    for row in 1..=ROWS {
        engine
            .set_cell_value("Sheet1", row, 1, LiteralValue::Number(f64::from(row)))
            .unwrap();
        formula(&mut engine, row, 2, &format!("=A{row}*2"));
    }
    for i in 1..=ROLLUPS {
        formula(&mut engine, i, 4, "=SUM($B$1:$B$300)");
        formula(&mut engine, i, 5, r#"=SUMIFS($B$1:$B$300,$A$1:$A$300,">150")"#);
    }
    if with_self_reference {
        formula(&mut engine, SELF_ROW, 2, "=ROWS($B$1:$B$310)");
    }
    engine
}

fn vertex_at(engine: &Engine<TestWorkbook>, row: u32, col: u32) -> Option<VertexId> {
    engine
        .graph
        .get_vertex_for_cell(&engine.graph.make_cell_ref("Sheet1", row, col))
}

fn formula_vertices_in(
    engine: &Engine<TestWorkbook>,
    rows: std::ops::RangeInclusive<u32>,
    cols: std::ops::RangeInclusive<u32>,
) -> Vec<VertexId> {
    let mut out = Vec::new();
    for row in rows {
        for col in cols.clone() {
            if let Some(v) = vertex_at(engine, row, col)
                && matches!(
                    engine.graph.get_vertex_kind(v),
                    VertexKind::FormulaScalar | VertexKind::FormulaArray
                )
            {
                out.push(v);
            }
        }
    }
    out.sort_unstable();
    out
}

#[test]
fn shared_rect_memo_lists_the_same_dependencies_as_the_per_formula_walk() {
    let engine = template_workbook(true);
    let candidates = formula_vertices_in(&engine, 1..=320, 1..=6);
    assert_eq!(candidates.len(), (ROWS + 2 * ROLLUPS + 1) as usize);

    let (memo, _) = VirtualDepBuilder::new(&engine).build(&candidates);

    // Independent oracle: every roll-up must be scheduled after every formula in B1:B300.
    let block = formula_vertices_in(&engine, 1..=ROWS, 2..=2);
    assert_eq!(block.len(), ROWS as usize);
    for i in 1..=ROLLUPS {
        for col in [4, 5] {
            let v = vertex_at(&engine, i, col).unwrap();
            assert_eq!(memo.get(&v), Some(&block), "roll-up at r{i}c{col}");
        }
    }
    // The formula inside its own rectangle lists the block but never itself.
    let inner = vertex_at(&engine, SELF_ROW, 2).unwrap();
    assert_eq!(memo.get(&inner), Some(&block), "self-referencing rectangle");
    // The per-formula walk (a fresh cache per call) agrees with the shared build everywhere.
    for &v in &candidates {
        let mut expected = RangeVirtualDepProvider::get_virtual_deps(&engine, v);
        expected.sort_unstable();
        expected.dedup();
        assert_eq!(
            memo.get(&v).cloned().unwrap_or_default(),
            expected,
            "virtual deps of {v:?}"
        );
    }
}

#[test]
fn full_evaluation_over_shared_rectangles_computes_every_rollup() {
    let mut engine = template_workbook(false);
    engine.evaluate_all().unwrap();
    let num = |engine: &Engine<TestWorkbook>, row: u32, col: u32| -> f64 {
        match engine.get_cell_value("Sheet1", row, col) {
            Some(LiteralValue::Number(n)) => n,
            Some(LiteralValue::Int(i)) => i as f64,
            other => panic!("expected a number at r{row}c{col}, got {other:?}"),
        }
    };
    let sum_b: f64 = (1..=ROWS).map(|r| f64::from(r * 2)).sum();
    let sum_b_over_150: f64 = (151..=ROWS).map(|r| f64::from(r * 2)).sum();
    for i in 1..=ROLLUPS {
        assert_eq!(num(&engine, i, 4), sum_b, "D{i}");
        assert_eq!(num(&engine, i, 5), sum_b_over_150, "E{i}");
    }
}
