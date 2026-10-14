//! rowsncolumns/spreadsheet#1147 — a reference inside an `IF` branch that is
//! never taken is not a runtime dependency.
//!
//! Excel detects circular references at calculation time: a cell is circular
//! only when calculation re-enters a cell that is still being calculated.
//! `B1 = 0`, `A1 = IF(B1>0, C1, 5)`, `C1 = A1+1` therefore calculates to
//! `A1 = 5`, `C1 = 6` with no warning; the cycle exists only in the formula
//! text. When `B1` becomes `1` the untaken branch is taken, calculation
//! re-enters `A1`, and Excel reports the cycle.
//!
//! `CycleDetection::Runtime` is that model: the static SCC is a scheduling
//! unit whose members are evaluated with live-edge recording, and only a
//! witnessed live cycle gets the policy verdict (`CyclePolicy::Zero` = Excel
//! with iterative calculation off: members display 0 and are reported through
//! `Engine::last_cycle_cells`). `CycleDetection::Static` stamps every static
//! SCC — the behavior the issue reports — and is pinned here as the
//! documented difference a host must not opt into for Excel parity.

use crate::engine::{CycleConfig, CycleDetection, CyclePolicy, Engine, EvalConfig};
use crate::reference::CellRef;
use crate::test_workbook::TestWorkbook;
use formualizer_common::{ExcelErrorKind, LiteralValue};
use formualizer_parse::parser::parse;

/// Excel with iterative calculation OFF, cycles detected at calculation time.
fn excel_iteration_off() -> CycleConfig {
    CycleConfig {
        detection: CycleDetection::Runtime,
        policy: CyclePolicy::Zero,
    }
}

fn engine_with(cycle: CycleConfig) -> Engine<TestWorkbook> {
    let cfg = EvalConfig::default()
        .with_cycle(cycle)
        .with_virtual_dep_telemetry(true);
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

fn is_circ(engine: &Engine<TestWorkbook>, row: u32, col: u32) -> bool {
    matches!(
        engine.get_cell_value("Sheet1", row, col),
        Some(LiteralValue::Error(e)) if e.kind == ExcelErrorKind::Circ
    )
}

/// The circular-reference report of the last recalc (what a host folds into
/// its circular-reference warning), as sorted 1-based `(row, col)` pairs.
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

/// The issue's workbook: `B1 = guard`, `A1 = IF(B1>0, C1, 5)`, `C1 = A1+1`.
fn build_1147(engine: &mut Engine<TestWorkbook>, guard: f64) {
    set_number(engine, 1, 2, guard);
    set_formula(engine, 1, 1, "=IF(B1>0,C1,5)");
    set_formula(engine, 1, 3, "=A1+1");
}

/// Excel: `A1 = 5`, `C1 = 6`, no circular-reference warning — `C1` is only
/// referenced from the branch `IF` does not take.
#[test]
fn if_untaken_branch_reference_is_not_a_runtime_dependency() {
    let mut engine = engine_with(excel_iteration_off());
    build_1147(&mut engine, 0.0);
    let res = engine.evaluate_all().unwrap();
    assert_eq!(num(&engine, 1, 1), 5.0, "A1 takes the literal branch");
    assert_eq!(num(&engine, 1, 3), 6.0, "C1 reads A1's calculated value");
    assert_eq!(res.cycle_errors, 0);
    assert!(
        cycle_cells_1based(&engine).is_empty(),
        "no circular-reference warning"
    );
    let t = engine.last_cycle_telemetry();
    assert_eq!(
        t.static_sccs, 1,
        "the static SCC is still a scheduling unit"
    );
    assert_eq!(t.phantom_sccs, 1);
    assert_eq!(t.live_cycles_witnessed, 0);
    assert_eq!(t.circ_cells_stamped, 0);
}

/// Same workbook through the demand path (`evaluate_cell` pulls the whole
/// static SCC as one task).
#[test]
fn if_untaken_branch_reference_is_not_a_runtime_dependency_via_evaluate_cell() {
    let mut engine = engine_with(excel_iteration_off());
    build_1147(&mut engine, 0.0);
    let c1 = engine.evaluate_cell("Sheet1", 1, 3).unwrap();
    assert_eq!(c1, Some(LiteralValue::Number(6.0)));
    assert_eq!(num(&engine, 1, 1), 5.0);
    assert!(cycle_cells_1based(&engine).is_empty());
}

/// When `B1` becomes `1` the branch is taken, calculation re-enters `A1`,
/// and Excel reports the cycle: both members display 0 and are reported.
/// Setting `B1` back to `0` clears it — the cells calculate again and the
/// report is empty.
#[test]
fn guard_flip_makes_the_cycle_live_and_flipping_back_clears_it() {
    let mut engine = engine_with(excel_iteration_off());
    build_1147(&mut engine, 0.0);
    engine.evaluate_all().unwrap();
    assert_eq!(num(&engine, 1, 1), 5.0);
    assert_eq!(num(&engine, 1, 3), 6.0);

    set_number(&mut engine, 1, 2, 1.0);
    let res = engine.evaluate_all().unwrap();
    assert_eq!(num(&engine, 1, 1), 0.0, "live cycle member displays 0");
    assert_eq!(num(&engine, 1, 3), 0.0, "live cycle member displays 0");
    assert_eq!(res.cycle_errors, 1);
    assert_eq!(
        cycle_cells_1based(&engine),
        vec![(1, 1), (1, 3)],
        "both members reported for the circular-reference warning"
    );
    let t = engine.last_cycle_telemetry();
    assert_eq!(t.live_cycles_witnessed, 1);
    assert_eq!(t.circ_cells_stamped, 2);

    set_number(&mut engine, 1, 2, 0.0);
    let res = engine.evaluate_all().unwrap();
    assert_eq!(num(&engine, 1, 1), 5.0, "A1 calculates again");
    assert_eq!(num(&engine, 1, 3), 6.0, "C1 calculates again");
    assert_eq!(res.cycle_errors, 0);
    assert!(
        cycle_cells_1based(&engine).is_empty(),
        "the warning clears with the cycle"
    );
}

/// The same workbook under `CyclePolicy::Error`: `#CIRC!` only while the
/// cycle is live.
#[test]
fn guard_flip_under_error_policy_stamps_circ_only_while_live() {
    let mut engine = engine_with(CycleConfig {
        detection: CycleDetection::Runtime,
        policy: CyclePolicy::Error,
    });
    build_1147(&mut engine, 0.0);
    engine.evaluate_all().unwrap();
    assert_eq!(num(&engine, 1, 1), 5.0);
    assert_eq!(num(&engine, 1, 3), 6.0);

    set_number(&mut engine, 1, 2, 1.0);
    engine.evaluate_all().unwrap();
    assert!(is_circ(&engine, 1, 1));
    assert!(is_circ(&engine, 1, 3));

    set_number(&mut engine, 1, 2, 0.0);
    engine.evaluate_all().unwrap();
    assert_eq!(num(&engine, 1, 1), 5.0);
    assert_eq!(num(&engine, 1, 3), 6.0);
}

/// A three-cell static ring closed only through lazy arms —
/// `A1 = CHOOSE(D1, 10, B1)`, `B1 = IFERROR(A1, C1)`, `C1 = IF(D1=1, B1, A1)`
/// — calculates (10, 10, 10) while `D1 = 1`: `CHOOSE` never materializes the
/// unselected choice and `IFERROR` never evaluates its fallback for a
/// non-error value. With `D1 = 2` `A1` reads `B1` and `B1` reads `A1`: that
/// pair is the live cycle (0, reported); `C1` merely reads the stamped `A1`
/// and is neither stamped nor reported (blast radius = live cycle only).
#[test]
fn three_cell_ring_through_choose_and_iferror_is_circular_only_when_live() {
    let mut engine = engine_with(excel_iteration_off());
    set_number(&mut engine, 1, 4, 1.0); // D1
    set_formula(&mut engine, 1, 1, "=CHOOSE(D1,10,B1)"); // A1
    set_formula(&mut engine, 1, 2, "=IFERROR(A1,C1)"); // B1
    set_formula(&mut engine, 1, 3, "=IF(D1=1,B1,A1)"); // C1
    let res = engine.evaluate_all().unwrap();
    assert_eq!(num(&engine, 1, 1), 10.0);
    assert_eq!(num(&engine, 1, 2), 10.0);
    assert_eq!(num(&engine, 1, 3), 10.0);
    assert_eq!(res.cycle_errors, 0);
    assert!(cycle_cells_1based(&engine).is_empty());
    assert_eq!(engine.last_cycle_telemetry().phantom_sccs, 1);

    set_number(&mut engine, 1, 4, 2.0);
    engine.evaluate_all().unwrap();
    assert_eq!(num(&engine, 1, 1), 0.0, "A1 is in the live cycle");
    assert_eq!(num(&engine, 1, 2), 0.0, "B1 is in the live cycle");
    assert_eq!(num(&engine, 1, 3), 0.0, "C1 computes with A1's 0");
    assert_eq!(
        cycle_cells_1based(&engine),
        vec![(1, 1), (1, 2)],
        "only the live cycle's members are reported"
    );

    set_number(&mut engine, 1, 4, 1.0);
    engine.evaluate_all().unwrap();
    assert_eq!(num(&engine, 1, 1), 10.0);
    assert_eq!(num(&engine, 1, 2), 10.0);
    assert_eq!(num(&engine, 1, 3), 10.0);
    assert!(cycle_cells_1based(&engine).is_empty());
}

/// `IFS` and `SWITCH` arms are lazy the same way.
#[test]
fn ifs_and_switch_untaken_arms_are_not_runtime_dependencies() {
    let mut engine = engine_with(excel_iteration_off());
    set_number(&mut engine, 1, 2, 0.0); // B1
    set_formula(&mut engine, 1, 1, "=IFS(B1>0,C1,TRUE,5)"); // A1
    set_formula(&mut engine, 1, 3, "=A1+1"); // C1
    set_formula(&mut engine, 2, 1, "=SWITCH(B1,1,C2,7)"); // A2
    set_formula(&mut engine, 2, 3, "=A2+1"); // C2
    let res = engine.evaluate_all().unwrap();
    assert_eq!(num(&engine, 1, 1), 5.0);
    assert_eq!(num(&engine, 1, 3), 6.0);
    assert_eq!(num(&engine, 2, 1), 7.0);
    assert_eq!(num(&engine, 2, 3), 8.0);
    assert_eq!(res.cycle_errors, 0);
    assert!(cycle_cells_1based(&engine).is_empty());

    set_number(&mut engine, 1, 2, 1.0);
    engine.evaluate_all().unwrap();
    assert_eq!(
        cycle_cells_1based(&engine),
        vec![(1, 1), (1, 3), (2, 1), (2, 3)],
        "both rings are live once B1 = 1"
    );
}

/// Excel's `AND`/`OR` evaluate every argument (`=AND(FALSE,1/0)` is
/// `#DIV/0!`), so a reference in a later argument IS a runtime dependency
/// and the cycle is real even when an earlier argument already decided the
/// result.
#[test]
fn and_or_arguments_are_runtime_dependencies_like_excel() {
    let mut engine = engine_with(excel_iteration_off());
    set_number(&mut engine, 1, 2, 0.0); // B1
    set_formula(&mut engine, 1, 1, "=AND(B1>0,C1>0)"); // A1
    set_formula(&mut engine, 1, 3, "=IF(A1,1,2)"); // C1
    set_formula(&mut engine, 2, 1, "=OR(B1=0,C2>0)"); // A2
    set_formula(&mut engine, 2, 3, "=IF(A2,1,2)"); // C2
    engine.evaluate_all().unwrap();
    assert_eq!(
        cycle_cells_1based(&engine),
        vec![(1, 1), (1, 3), (2, 1), (2, 3)],
        "the decisive first argument does not make the later reference lazy"
    );
}

/// A genuine cycle is unchanged: `A1 = B1*0.5+10`, `B1 = A1+5` with
/// iteration off displays 0 and is reported; a dependent computes with 0.
#[test]
fn genuine_cycle_is_still_zero_with_warning() {
    let mut engine = engine_with(excel_iteration_off());
    set_formula(&mut engine, 1, 1, "=B1*0.5+10");
    set_formula(&mut engine, 1, 2, "=A1+5");
    set_formula(&mut engine, 1, 3, "=A1+1");
    let res = engine.evaluate_all().unwrap();
    assert_eq!(num(&engine, 1, 1), 0.0);
    assert_eq!(num(&engine, 1, 2), 0.0);
    assert_eq!(num(&engine, 1, 3), 1.0, "dependent computes with 0");
    assert_eq!(res.cycle_errors, 1);
    assert_eq!(cycle_cells_1based(&engine), vec![(1, 1), (1, 2)]);

    // A precedent edit re-stamps the still-circular group and reports it again.
    set_formula(&mut engine, 1, 2, "=A1+6");
    engine.evaluate_all().unwrap();
    assert_eq!(num(&engine, 1, 1), 0.0);
    assert_eq!(cycle_cells_1based(&engine), vec![(1, 1), (1, 2)]);
}

/// Iterative calculation (Excel defaults) still resolves the cycle once it is
/// live — `A1 = IF(B1>0, C1*0.5+10, 5)`, `C1 = A1+5` converges to
/// `A1 = 25`, `C1 = 30` — and does not iterate at all while the cycle is
/// only in the formula text (`A1 = 5`, `C1 = 10` in one pass).
#[test]
fn iterative_calculation_still_resolves_the_cycle_once_live() {
    let mut engine = engine_with(CycleConfig::iterate_excel_defaults());
    set_number(&mut engine, 1, 2, 0.0);
    set_formula(&mut engine, 1, 1, "=IF(B1>0,C1*0.5+10,5)");
    set_formula(&mut engine, 1, 3, "=A1+5");
    engine.evaluate_all().unwrap();
    assert_eq!(num(&engine, 1, 1), 5.0);
    assert_eq!(num(&engine, 1, 3), 10.0);
    let t = engine.last_cycle_telemetry();
    assert_eq!(t.phantom_sccs, 1);
    assert_eq!(
        t.iterated_sccs, 0,
        "nothing to iterate while the cycle is phantom"
    );

    set_number(&mut engine, 1, 2, 1.0);
    engine.evaluate_all().unwrap();
    let a1 = num(&engine, 1, 1);
    let c1 = num(&engine, 1, 3);
    assert!((a1 - 25.0).abs() < 0.02, "A1 converges to 25, got {a1}");
    assert!((c1 - 30.0).abs() < 0.02, "C1 converges to 30, got {c1}");
    assert!(
        cycle_cells_1based(&engine).is_empty(),
        "iteration is not a cycle verdict"
    );
    assert_eq!(engine.last_cycle_telemetry().iterated_sccs, 1);
}

/// `CycleDetection::Static` is the behavior the issue reports: the static SCC
/// is stamped without evaluating anything, so the untaken branch makes both
/// cells 0 with a warning. Pinned as the documented difference — a host that
/// wants Excel's calculation-time detection must opt into `Runtime`.
#[test]
fn static_detection_stamps_the_untaken_branch_cycle_documented_diff() {
    let mut engine = engine_with(CycleConfig {
        detection: CycleDetection::Static,
        policy: CyclePolicy::Zero,
    });
    build_1147(&mut engine, 0.0);
    let res = engine.evaluate_all().unwrap();
    assert_eq!(num(&engine, 1, 1), 0.0);
    assert_eq!(num(&engine, 1, 3), 0.0);
    assert_eq!(res.cycle_errors, 1);
    assert_eq!(cycle_cells_1based(&engine), vec![(1, 1), (1, 3)]);
}
