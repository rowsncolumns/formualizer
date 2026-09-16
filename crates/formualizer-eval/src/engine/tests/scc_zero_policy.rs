//! `CyclePolicy::Zero` — Excel's iterative-calculation-OFF semantics
//! (rowsncolumns/spreadsheet#546 U16, E-30/E-45).
//!
//! Excel never writes an error into a circular cell when iteration is off:
//! the members display `0`, their dependents compute with `0`
//! (`=A1+1` → 1, `ISERROR(A1)` → FALSE), and the workbook raises a
//! circular-reference warning. `Zero` reproduces that: members are stamped
//! `0`, and the stamped cells are reported through
//! `Engine::last_cycle_cells` so a host can raise the warning. `Error` (the
//! default) is unchanged and stays pinned by `scc_runtime_cycles.rs`.

use crate::engine::{CycleConfig, CycleDetection, CyclePolicy, Engine, EvalConfig};
use crate::reference::CellRef;
use crate::test_workbook::TestWorkbook;
use formualizer_common::{ExcelErrorKind, LiteralValue};
use formualizer_parse::parser::parse;

fn zero_engine(detection: CycleDetection) -> Engine<TestWorkbook> {
    let cfg = EvalConfig::default().with_cycle(CycleConfig {
        detection,
        policy: CyclePolicy::Zero,
    });
    Engine::new(TestWorkbook::new(), cfg)
}

fn set_formula(engine: &mut Engine<TestWorkbook>, row: u32, col: u32, f: &str) {
    engine
        .set_cell_formula("Sheet1", row, col, parse(f).expect("parse"))
        .expect("set formula");
}

fn set_number(engine: &mut Engine<TestWorkbook>, row: u32, col: u32, n: f64) {
    engine
        .set_cell_value("Sheet1", row, col, LiteralValue::Number(n))
        .expect("set value");
}

fn num(engine: &Engine<TestWorkbook>, row: u32, col: u32) -> f64 {
    match engine.get_cell_value("Sheet1", row, col) {
        Some(LiteralValue::Number(n)) => n,
        Some(LiteralValue::Int(i)) => i as f64,
        other => panic!("expected number at r{row}c{col}, got {other:?}"),
    }
}

fn cycle_cells_1based(engine: &Engine<TestWorkbook>) -> Vec<(u32, u32)> {
    let sheet = engine.sheet_id("Sheet1").expect("Sheet1 registered");
    let mut cells: Vec<(u32, u32)> = engine
        .last_cycle_cells()
        .iter()
        .filter(|c: &&CellRef| c.sheet_id == sheet)
        .map(|c| (c.coord.row() + 1, c.coord.col() + 1))
        .collect();
    cells.sort_unstable();
    cells.dedup();
    cells
}

/// E-30: `A1 = B1+1`, `B1 = A1+1` with iteration off. Excel: both 0, a
/// dependent `=A1+1` is 1, `ISERROR(A1)` is FALSE — no error value anywhere.
#[test]
fn static_zero_stamps_members_zero_and_dependents_compute_with_zero() {
    for detection in [CycleDetection::Static, CycleDetection::Runtime] {
        let mut engine = zero_engine(detection);
        set_formula(&mut engine, 1, 1, "=B1+1");
        set_formula(&mut engine, 1, 2, "=A1+1");
        set_formula(&mut engine, 1, 3, "=A1+1");
        set_formula(&mut engine, 1, 4, "=ISERROR(A1)");
        engine.evaluate_all().unwrap();

        assert_eq!(num(&engine, 1, 1), 0.0, "{detection:?}: A1 displays 0");
        assert_eq!(num(&engine, 1, 2), 0.0, "{detection:?}: B1 displays 0");
        assert_eq!(
            num(&engine, 1, 3),
            1.0,
            "{detection:?}: =A1+1 computes with 0"
        );
        assert_eq!(
            engine.get_cell_value("Sheet1", 1, 4),
            Some(LiteralValue::Boolean(false)),
            "{detection:?}: ISERROR(A1) is FALSE"
        );
        assert_eq!(
            cycle_cells_1based(&engine),
            vec![(1, 1), (1, 2)],
            "{detection:?}: both members are reported for the warning"
        );
    }
}

/// E-45: a self-reference (direct, or through a range containing the cell)
/// installs under `Zero` — Excel accepts the entry with iteration off — and
/// displays 0 instead of being rejected at ingest (which left the cell blank).
#[test]
fn zero_accepts_self_references_at_ingest_and_stamps_zero() {
    for detection in [CycleDetection::Static, CycleDetection::Runtime] {
        let mut engine = zero_engine(detection);
        set_formula(&mut engine, 1, 1, "=A1+1");
        set_number(&mut engine, 4, 1, 1.0);
        set_number(&mut engine, 5, 1, 2.0);
        set_formula(&mut engine, 3, 1, "=SUM(A3:A5)");
        set_formula(&mut engine, 6, 1, "=A3+A4");
        engine.evaluate_all().unwrap();

        assert_eq!(num(&engine, 1, 1), 0.0, "{detection:?}: =A1+1 in A1 is 0");
        assert_eq!(
            num(&engine, 3, 1),
            0.0,
            "{detection:?}: =SUM(A3:A5) in A3 is 0"
        );
        assert_eq!(
            num(&engine, 6, 1),
            1.0,
            "{detection:?}: dependent reads A3 as 0"
        );
        assert_eq!(cycle_cells_1based(&engine), vec![(1, 1), (3, 1)]);
    }
}

/// Runtime detection keeps its phantom-vs-live distinction under `Zero`: a
/// guarded pair whose live subgraph is acyclic produces values; the same pair
/// with the guard flipped is a live cycle and gets `0` (not `#CIRC!`).
#[test]
fn runtime_zero_phantom_pair_evaluates_and_live_pair_is_zero() {
    let mut engine = zero_engine(CycleDetection::Runtime);
    engine
        .set_cell_value("Sheet1", 1, 1, LiteralValue::Boolean(true))
        .unwrap();
    set_formula(&mut engine, 2, 1, "=IF(A1,555,A3)");
    set_formula(&mut engine, 3, 1, "=IF(A1,999,A2)");
    engine.evaluate_all().unwrap();
    assert_eq!(num(&engine, 2, 1), 555.0);
    assert_eq!(num(&engine, 3, 1), 999.0);
    assert!(
        cycle_cells_1based(&engine).is_empty(),
        "phantom SCC reports no cycle"
    );

    engine
        .set_cell_value("Sheet1", 1, 1, LiteralValue::Boolean(false))
        .unwrap();
    engine.evaluate_all().unwrap();
    assert_eq!(num(&engine, 2, 1), 0.0, "live cycle member is 0");
    assert_eq!(num(&engine, 3, 1), 0.0, "live cycle member is 0");
    assert_eq!(cycle_cells_1based(&engine), vec![(2, 1), (3, 1)]);
}

/// The cycle stamp participates in evaluation deltas like any other result:
/// the first recalc reports the members (Empty → 0), and re-stamping the same
/// value on the next recalc reports nothing for them.
#[test]
fn zero_stamp_is_reported_in_eval_delta_once() {
    let mut engine = zero_engine(CycleDetection::Static);
    set_formula(&mut engine, 1, 1, "=B1+1");
    set_formula(&mut engine, 1, 2, "=A1+1");
    let (_res, delta) = engine.evaluate_all_with_delta().unwrap();
    let mut changed: Vec<(u32, u32)> = delta
        .changed_cells
        .iter()
        .map(|p| {
            let (_, r, c) = p.to_excel_1based();
            (r, c)
        })
        .collect();
    changed.sort_unstable();
    assert_eq!(
        changed,
        vec![(1, 1), (1, 2)],
        "members land in the delta once"
    );

    // Touch an unrelated cell: the members re-stamp 0 over 0 → no delta for them.
    set_number(&mut engine, 9, 9, 1.0);
    let (_res, delta) = engine.evaluate_all_with_delta().unwrap();
    assert!(
        delta.changed_cells.iter().all(|p| {
            let (_, r, _) = p.to_excel_1based();
            r != 1
        }),
        "unchanged 0 stamps do not re-enter the delta: {:?}",
        delta.changed_cells
    );
}

/// The default `Error` policy is untouched: `#CIRC!` still stamps and direct
/// self-references are still rejected at ingest (the contracts in
/// `scc_runtime_cycles.rs` and the SCC oracle stay green).
#[test]
fn error_policy_still_stamps_circ_and_rejects_self_reference() {
    let mut engine = Engine::new(TestWorkbook::new(), EvalConfig::default());
    let err = engine
        .set_cell_formula("Sheet1", 1, 1, parse("=A1+1").unwrap())
        .unwrap_err();
    assert_eq!(err.kind, ExcelErrorKind::Circ);
    set_formula(&mut engine, 2, 1, "=A3+1");
    set_formula(&mut engine, 3, 1, "=A2+1");
    engine.evaluate_all().unwrap();
    assert!(matches!(
        engine.get_cell_value("Sheet1", 2, 1),
        Some(LiteralValue::Error(e)) if e.kind == ExcelErrorKind::Circ
    ));
    assert_eq!(cycle_cells_1based(&engine), vec![(2, 1), (3, 1)]);
}
