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
        formula(
            &mut engine,
            i,
            5,
            r#"=SUMIFS($B$1:$B$300,$A$1:$A$300,">150")"#,
        );
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

    let (vdeps, _) = VirtualDepBuilder::new(&engine).build(&candidates);
    let memo = vdeps.expand();

    // Independent oracle: every roll-up must be scheduled after every formula in B1:B300 — and
    // all 400 of them share ONE group for it (the second group is B305's own rectangle).
    let block = formula_vertices_in(&engine, 1..=ROWS, 2..=2);
    assert_eq!(block.len(), ROWS as usize);
    assert_eq!(vdeps.group_count(), 2);
    for i in 1..=ROLLUPS {
        for col in [4, 5] {
            let v = vertex_at(&engine, i, col).unwrap();
            assert_eq!(memo.get(&v), Some(&block), "roll-up at r{i}c{col}");
            assert_eq!(
                vdeps.groups_of(v).len(),
                1,
                "roll-up at r{i}c{col} shares the group"
            );
            assert!(
                vdeps.owned_of(v).is_empty(),
                "roll-up at r{i}c{col} owns nothing"
            );
        }
    }
    assert_eq!(
        vdeps.edge_count(),
        2 * ROLLUPS as usize * ROWS as usize + ROWS as usize
    );
    // The formula inside its own rectangle lists the block but never itself — as owned extras,
    // since a group never contains a vertex that depends on it.
    let inner = vertex_at(&engine, SELF_ROW, 2).unwrap();
    assert_eq!(memo.get(&inner), Some(&block), "self-referencing rectangle");
    assert!(vdeps.groups_of(inner).is_empty());
    assert_eq!(vdeps.owned_of(inner), block.as_slice());
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

/// Cycles meet the rectangle groups in the condensation path: a dependent that shares a cycle
/// node with a member of its group (B1 ↔ A1, and B1 reads $A$1:$A$10) must not wait on that
/// group — it would be waiting on its own node — while an outside reader (C1) still orders
/// after the cycle. And a cycle THROUGH a group (D1 reads $E$1:$E$5, E1 = D1) is a cycle among
/// the real vertices. Neither may deadlock the schedule.
#[test]
fn cycles_through_and_beside_rectangle_groups_still_schedule() {
    let mut engine = Engine::new(TestWorkbook::default(), EvalConfig::default());
    for row in 2..=10 {
        engine
            .set_cell_value("Sheet1", row, 1, LiteralValue::Number(f64::from(row)))
            .unwrap();
    }
    formula(&mut engine, 1, 1, "=B1");
    formula(&mut engine, 1, 2, "=A1+SUM($A$1:$A$10)");
    formula(&mut engine, 1, 3, "=SUM($A$2:$A$10)+SUM($A$1:$A$10)");
    formula(&mut engine, 1, 4, "=SUM($E$1:$E$5)");
    formula(&mut engine, 1, 5, "=D1");
    for row in 2..=5 {
        engine
            .set_cell_value("Sheet1", row, 5, LiteralValue::Number(f64::from(row)))
            .unwrap();
    }

    engine
        .evaluate_all()
        .expect("a schedule with cycles beside and through groups");

    // The cycles are reported as circular; the outside reader still computed.
    for (row, col) in [(1, 1), (1, 2), (1, 4), (1, 5)] {
        assert!(
            matches!(
                engine.get_cell_value("Sheet1", row, col),
                Some(LiteralValue::Error(_))
            ),
            "r{row}c{col} is part of a cycle"
        );
    }
    match engine.get_cell_value("Sheet1", 1, 3) {
        // SUM($A$2:$A$10) = 54; SUM($A$1:$A$10) carries A1's circular error.
        Some(LiteralValue::Error(_)) | Some(LiteralValue::Number(_)) => {}
        other => panic!("C1 should have been scheduled after the cycle, got {other:?}"),
    }
}
