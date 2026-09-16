//! Dynamic-array spill contention settles by anchor POSITION, never by evaluation or entry order
//! (Excel parity, rowsncolumns/spreadsheet#546 U20 E-28/E-29/E-32):
//! - a formula cell inside an array's range is content and blocks it (`#SPILL!`);
//! - two arrays contending for free cells resolve to the anchor earlier in row-major order;
//! - host-declared blockers (merged cells) block a spill like content;
//! - the default spill cap is the sheet itself.

use crate::engine::{Engine, EvalConfig};
use crate::test_workbook::TestWorkbook;
use formualizer_common::{ExcelErrorKind, LiteralValue};
use formualizer_parse::parser::parse;

fn configs() -> [(&'static str, EvalConfig); 2] {
    [
        (
            "sequential",
            EvalConfig {
                enable_parallel: false,
                ..EvalConfig::default()
            },
        ),
        (
            "parallel",
            EvalConfig {
                enable_parallel: true,
                max_threads: Some(4),
                ..EvalConfig::default()
            },
        ),
    ]
}

fn get(engine: &Engine<TestWorkbook>, row: u32, col: u32) -> Option<LiteralValue> {
    engine.get_cell_value("Sheet1", row, col)
}

fn is_spill(v: &Option<LiteralValue>) -> bool {
    matches!(v, Some(LiteralValue::Error(e)) if e.kind == ExcelErrorKind::Spill)
}

fn is_empty(v: &Option<LiteralValue>) -> bool {
    matches!(v, None | Some(LiteralValue::Empty))
}

fn set(engine: &mut Engine<TestWorkbook>, row: u32, col: u32, formula: &str) {
    engine
        .set_cell_formula("Sheet1", row, col, parse(formula).unwrap())
        .unwrap();
    engine.evaluate_all().unwrap();
}

/// A1 `SEQUENCE(3)` wants A1:A3; A3 holds `SEQUENCE(2)`. A3 is a formula — content — so A1 is
/// `#SPILL!` and A3 spills, whichever was entered first and however the layer is scheduled.
#[test]
fn anchor_inside_another_arrays_range_blocks_the_outer_array_in_either_entry_order() {
    for (mode, cfg) in configs() {
        for a1_first in [true, false] {
            let ctx = format!(
                "{mode}, A1 entered {}",
                if a1_first { "first" } else { "second" }
            );
            let mut engine = Engine::new(TestWorkbook::default(), cfg.clone());
            if a1_first {
                set(&mut engine, 1, 1, "=SEQUENCE(3)");
                set(&mut engine, 3, 1, "=SEQUENCE(2)");
            } else {
                set(&mut engine, 3, 1, "=SEQUENCE(2)");
                set(&mut engine, 1, 1, "=SEQUENCE(3)");
            }
            for pass in 0..2 {
                assert!(
                    is_spill(&get(&engine, 1, 1)),
                    "{ctx} pass {pass}: A1 blocked by the formula in A3, got {:?}",
                    get(&engine, 1, 1)
                );
                assert!(
                    is_empty(&get(&engine, 2, 1)),
                    "{ctx} pass {pass}: A2 not projected"
                );
                assert_eq!(
                    get(&engine, 3, 1),
                    Some(LiteralValue::Number(1.0)),
                    "{ctx} pass {pass}: A3 spills"
                );
                assert_eq!(
                    get(&engine, 4, 1),
                    Some(LiteralValue::Number(2.0)),
                    "{ctx} pass {pass}: A4"
                );
                engine.evaluate_all().unwrap();
            }
        }
    }
}

/// Both formulas present before the first evaluation (a workbook rebuilt from its document): they
/// share a layer, A1 plans first and is blocked — and must hand its region lock back, or A3 finds
/// "Region reserved by another spill" and both end up `#SPILL!`.
#[test]
fn anchor_inside_another_arrays_range_settles_within_one_layer() {
    for (mode, cfg) in configs() {
        let mut engine = Engine::new(TestWorkbook::default(), cfg);
        engine
            .set_cell_formula("Sheet1", 1, 1, parse("=SEQUENCE(3)").unwrap())
            .unwrap();
        engine
            .set_cell_formula("Sheet1", 3, 1, parse("=SEQUENCE(2)").unwrap())
            .unwrap();
        engine.evaluate_all().unwrap();
        assert!(
            is_spill(&get(&engine, 1, 1)),
            "{mode}: A1 blocked, got {:?}",
            get(&engine, 1, 1)
        );
        assert_eq!(
            get(&engine, 3, 1),
            Some(LiteralValue::Number(1.0)),
            "{mode}: A3 spills"
        );
        assert_eq!(
            get(&engine, 4, 1),
            Some(LiteralValue::Number(2.0)),
            "{mode}: A4"
        );
    }
}

/// B1 `SEQUENCE(3)` (B1:B3) and A2 `SEQUENCE(1,3)` (A2:C2) meet at B2 with neither anchor inside
/// the other's range. The anchor earlier in row-major order — B1 — wins; A2 is `#SPILL!`. Entering
/// A2 first commits its spill, so B1's later evaluation must PREEMPT it (and A2 must settle on
/// `#SPILL!` within the same `evaluate_all`).
#[test]
fn mutual_conflict_settles_on_the_earlier_anchor_in_row_major_order() {
    for (mode, cfg) in configs() {
        for b1_first in [true, false] {
            let ctx = format!(
                "{mode}, B1 entered {}",
                if b1_first { "first" } else { "second" }
            );
            let mut engine = Engine::new(TestWorkbook::default(), cfg.clone());
            if b1_first {
                set(&mut engine, 1, 2, "=SEQUENCE(3)");
                set(&mut engine, 2, 1, "=SEQUENCE(1,3)");
            } else {
                set(&mut engine, 2, 1, "=SEQUENCE(1,3)");
                set(&mut engine, 1, 2, "=SEQUENCE(3)");
            }
            for pass in 0..2 {
                assert_eq!(
                    get(&engine, 1, 2),
                    Some(LiteralValue::Number(1.0)),
                    "{ctx} pass {pass}: B1 wins"
                );
                assert_eq!(
                    get(&engine, 2, 2),
                    Some(LiteralValue::Number(2.0)),
                    "{ctx} pass {pass}: B2 is B1's"
                );
                assert_eq!(
                    get(&engine, 3, 2),
                    Some(LiteralValue::Number(3.0)),
                    "{ctx} pass {pass}: B3"
                );
                assert!(
                    is_spill(&get(&engine, 2, 1)),
                    "{ctx} pass {pass}: A2 loses → #SPILL!, got {:?}",
                    get(&engine, 2, 1)
                );
                assert!(
                    is_empty(&get(&engine, 2, 3)),
                    "{ctx} pass {pass}: C2 not projected by A2"
                );
                engine.evaluate_all().unwrap();
            }
        }
    }
}

/// Both arrays entered before the first evaluation land in one layer: the outcome must not depend
/// on vertex (entry) order there either.
#[test]
fn mutual_conflict_in_one_layer_is_order_independent() {
    for (mode, cfg) in configs() {
        for b1_first in [true, false] {
            let ctx = format!(
                "{mode}, B1 entered {}",
                if b1_first { "first" } else { "second" }
            );
            let mut engine = Engine::new(TestWorkbook::default(), cfg.clone());
            let (first, second) = if b1_first {
                ((1, 2, "=SEQUENCE(3)"), (2, 1, "=SEQUENCE(1,3)"))
            } else {
                ((2, 1, "=SEQUENCE(1,3)"), (1, 2, "=SEQUENCE(3)"))
            };
            engine
                .set_cell_formula("Sheet1", first.0, first.1, parse(first.2).unwrap())
                .unwrap();
            engine
                .set_cell_formula("Sheet1", second.0, second.1, parse(second.2).unwrap())
                .unwrap();
            engine.evaluate_all().unwrap();
            assert_eq!(
                get(&engine, 2, 2),
                Some(LiteralValue::Number(2.0)),
                "{ctx}: B2 is B1's"
            );
            assert!(
                is_spill(&get(&engine, 2, 1)),
                "{ctx}: A2 → #SPILL!, got {:?}",
                get(&engine, 2, 1)
            );
        }
    }
}

/// A scalar formula typed into a projection is content too: the array over it re-plans to
/// `#SPILL!` and the formula keeps its own value.
#[test]
fn scalar_formula_on_a_projection_blocks_the_array() {
    for (mode, cfg) in configs() {
        let mut engine = Engine::new(TestWorkbook::default(), cfg);
        set(&mut engine, 1, 1, "=SEQUENCE(3)");
        set(&mut engine, 2, 1, "=5+1");
        assert!(
            is_spill(&get(&engine, 1, 1)),
            "{mode}: A1 blocked by the formula in A2, got {:?}",
            get(&engine, 1, 1)
        );
        assert_eq!(
            get(&engine, 2, 1),
            Some(LiteralValue::Number(6.0)),
            "{mode}: A2 keeps its value"
        );
        assert!(is_empty(&get(&engine, 3, 1)), "{mode}: A3 vacated");
    }
}

/// A preempted anchor's readers see the change: C1 reads C2 (A2's projection) and must recompute
/// once B1 preempts A2 and C2 empties.
#[test]
fn preemption_recomputes_readers_of_the_vacated_projection() {
    let mut engine = Engine::new(TestWorkbook::default(), EvalConfig::default());
    set(&mut engine, 2, 1, "=SEQUENCE(1,3)"); // A2:C2 = 1,2,3
    set(&mut engine, 1, 3, "=C2*10"); // C1 reads the projection C2
    assert_eq!(get(&engine, 1, 3), Some(LiteralValue::Number(30.0)));
    set(&mut engine, 1, 2, "=SEQUENCE(3)"); // B1 preempts A2
    assert!(is_spill(&get(&engine, 2, 1)));
    assert_eq!(
        get(&engine, 1, 3),
        Some(LiteralValue::Number(0.0)),
        "C1 recomputed from the emptied C2"
    );
}

/// Host-declared blockers (a spreadsheet's merged cells) stop a spill like content does; clearing
/// them frees the region. The anchor's own cell is exempt — a 1×1 result there is not a spill.
#[test]
fn host_spill_blockers_block_projection_cells_only() {
    for (mode, cfg) in configs() {
        let mut engine = Engine::new(TestWorkbook::default(), cfg);
        let sheet = engine.sheet_id("Sheet1").unwrap();
        // "Merge" C1:C2 (0-based rows 0..=1, col 2) and A1:B1 (row 0, cols 0..=1).
        engine.set_spill_blockers(sheet, &[(0, 1, 2, 2), (0, 0, 0, 1)]);
        set(&mut engine, 1, 2, "=SEQUENCE(1,2)"); // B1:C1 → C1 is merged
        assert!(
            is_spill(&get(&engine, 1, 2)),
            "{mode}: B1 blocked by the merged C1, got {:?}",
            get(&engine, 1, 2)
        );
        assert!(is_empty(&get(&engine, 1, 3)), "{mode}: C1 untouched");
        set(&mut engine, 1, 1, "=SEQUENCE(3)"); // A1:A3 — only the anchor A1 lies in the merge A1:B1
        assert_eq!(
            get(&engine, 3, 1),
            Some(LiteralValue::Number(3.0)),
            "{mode}: anchor-only overlap spills"
        );
        set(&mut engine, 1, 4, "=SEQUENCE(2,2)"); // D1:E2 — no blocker there
        assert_eq!(
            get(&engine, 2, 5),
            Some(LiteralValue::Number(4.0)),
            "{mode}: blockers are per-region, not per-sheet"
        );
        // Unmerge: the blockers go and B1 re-plans on its next evaluation.
        engine.set_spill_blockers(sheet, &[]);
        set(&mut engine, 1, 2, "=SEQUENCE(1,2)");
        assert_eq!(
            get(&engine, 1, 2),
            Some(LiteralValue::Number(1.0)),
            "{mode}: B1 spills once unmerged"
        );
        assert_eq!(
            get(&engine, 1, 3),
            Some(LiteralValue::Number(2.0)),
            "{mode}: C1"
        );
    }
}

/// The default cap is the sheet itself: an array of 20,000 cells (over the old 10,000 cap) spills;
/// one that would run off the sheet's edge is `#SPILL!`, never `#NUM!`.
#[test]
fn default_spill_cap_is_the_sheet() {
    let mut engine = Engine::new(TestWorkbook::default(), EvalConfig::default());
    set(&mut engine, 1, 1, "=SEQUENCE(20000)");
    assert_eq!(get(&engine, 20000, 1), Some(LiteralValue::Number(20000.0)));
    set(&mut engine, 1, 2, "=SEQUENCE(2000000)");
    assert!(
        is_spill(&get(&engine, 1, 2)),
        "taller than the sheet → #SPILL!, got {:?}",
        get(&engine, 1, 2)
    );
    set(&mut engine, 2, 3, "=SEQUENCE(1048576)");
    assert!(
        is_spill(&get(&engine, 2, 3)),
        "runs off the bottom edge → #SPILL!, got {:?}",
        get(&engine, 2, 3)
    );
    set(&mut engine, 1, 4, "=SEQUENCE(1,16385)");
    assert!(
        is_spill(&get(&engine, 1, 4)),
        "wider than the sheet → #SPILL!, got {:?}",
        get(&engine, 1, 4)
    );
}
