//! rowsncolumns/spreadsheet#546 (U10, E-01) — 3-D references. `Sheet1:Sheet3!A1` is the same
//! cell (or range) on every sheet between the two endpoints in TAB order, inclusive; the parser
//! already produced `Cell3D`/`Range3D`, but evaluation answered `#N/IMPL!` and the dependency
//! planner ignored the reference, so an edit on a spanned sheet never re-fired the formula.

use formualizer_common::{ExcelErrorKind, LiteralValue};
use formualizer_eval::engine::{Engine, EvalConfig};
use formualizer_eval::test_workbook::TestWorkbook;
use formualizer_eval::traits::EvaluationContext;
use formualizer_parse::parser::parse;

fn num(n: f64) -> LiteralValue {
    LiteralValue::Number(n)
}

/// North / Mid West / South / Summary in tab order; A1 = 1,2,3 and B1 = 10,20,30 on the first
/// three. Returns the engine with `formula` installed in Summary!A1.
fn workbook(formula: &str) -> Engine<TestWorkbook> {
    let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
    for (i, name) in ["North", "Mid West", "South", "Summary"].iter().enumerate() {
        e.add_sheet(name).unwrap();
        if i < 3 {
            e.set_cell_value(name, 1, 1, num(i as f64 + 1.0)).unwrap();
            e.set_cell_value(name, 1, 2, num((i as f64 + 1.0) * 10.0))
                .unwrap();
        }
    }
    e.set_cell_formula("Summary", 1, 1, parse(formula).unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    e
}

fn summary(e: &Engine<TestWorkbook>) -> Option<LiteralValue> {
    e.get_cell_value("Summary", 1, 1)
}

#[test]
fn three_d_cell_reference_aggregates_every_spanned_sheet() {
    assert_eq!(summary(&workbook("=SUM(North:South!A1)")), Some(num(6.0)));
    assert_eq!(
        summary(&workbook("=AVERAGE(North:South!A1)")),
        Some(num(2.0))
    );
    assert_eq!(summary(&workbook("=COUNT(North:South!A1)")), Some(num(3.0)));
    assert_eq!(summary(&workbook("=MAX(North:South!B1)")), Some(num(30.0)));
}

#[test]
fn three_d_range_reference_stacks_every_sheet() {
    assert_eq!(
        summary(&workbook("=SUM(North:South!A1:B1)")),
        Some(num(66.0))
    );
    assert_eq!(
        summary(&workbook("=SUM('Mid West:South'!A1:B1)")),
        Some(num(55.0))
    );
}

#[test]
fn three_d_span_ignores_endpoint_order_and_case() {
    assert_eq!(summary(&workbook("=SUM(South:North!A1)")), Some(num(6.0)));
    assert_eq!(summary(&workbook("=SUM(north:SOUTH!A1)")), Some(num(6.0)));
    assert_eq!(summary(&workbook("=SUM(North:North!A1)")), Some(num(1.0)));
}

#[test]
fn three_d_unknown_endpoint_is_ref_error() {
    // Like any reference to a sheet that does not exist, the engine rejects the
    // formula at install with #REF! (hosts keep the text and surface the error).
    let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
    e.add_sheet("North").unwrap();
    e.add_sheet("Summary").unwrap();
    let err = e
        .set_cell_formula("Summary", 1, 1, parse("=SUM(North:Nowhere!A1)").unwrap())
        .expect_err("unknown span endpoint must be rejected");
    assert_eq!(err.kind, ExcelErrorKind::Ref);
}

#[test]
fn three_d_reference_is_a_live_dependency() {
    let mut e = workbook("=SUM(North:South!A1)");
    e.set_cell_value("Mid West", 1, 1, num(200.0)).unwrap();
    e.evaluate_all().unwrap();
    assert_eq!(
        summary(&e),
        Some(num(204.0)),
        "edit on a middle sheet re-fires the formula"
    );

    // A sheet that joins the span later (added, then moved to the tab right after North) counts
    // too: the span is resolved from the tab order at evaluation time, not frozen at install.
    // (`EvalConfig::default()` pre-interns a "Sheet1" tab, so positions are looked up, not assumed.)
    e.add_sheet("West").unwrap();
    e.set_cell_value("West", 1, 1, num(1000.0)).unwrap();
    let west = e.sheet_id("West").unwrap();
    let after_north = e.sheet_index_by_name("North").unwrap(); // 1-based → 0-based slot after it
    e.move_sheet(west, after_north).unwrap();
    e.evaluate_all().unwrap();
    assert_eq!(summary(&e), Some(num(1204.0)));

    // ...and leaves it again when moved to the last tab.
    e.move_sheet(west, usize::MAX).unwrap();
    e.evaluate_all().unwrap();
    assert_eq!(summary(&e), Some(num(204.0)));
}
