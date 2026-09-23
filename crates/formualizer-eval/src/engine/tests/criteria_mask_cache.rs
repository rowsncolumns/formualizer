//! The `*IF(S)` criteria-mask cache: one mask per (column, predicate, sheet snapshot), shared by
//! every formula that reads it, rotated by an edit to the sheet.

use formualizer_common::LiteralValue;
use formualizer_parse::parser::parse;

use crate::engine::{Engine, EvalConfig};
use crate::test_workbook::TestWorkbook;

const ROWS: u32 = 400;
const FORMULAS: u32 = 60;

fn engine() -> Engine<TestWorkbook> {
    Engine::new(TestWorkbook::default(), EvalConfig::default())
}

fn formula(engine: &mut Engine<TestWorkbook>, row: u32, col: u32, text: &str) {
    formula_on(engine, "Sheet1", row, col, text);
}

fn formula_on(engine: &mut Engine<TestWorkbook>, sheet: &str, row: u32, col: u32, text: &str) {
    let ast = parse(text).unwrap_or_else(|err| panic!("parse {text}: {err}"));
    engine.set_cell_formula(sheet, row, col, ast).unwrap();
}

fn number(engine: &mut Engine<TestWorkbook>, row: u32, col: u32, n: f64) {
    engine
        .set_cell_value("Sheet1", row, col, LiteralValue::Number(n))
        .unwrap();
}

fn text(engine: &mut Engine<TestWorkbook>, row: u32, col: u32, s: &str) {
    text_on(engine, "Sheet1", row, col, s);
}

fn text_on(engine: &mut Engine<TestWorkbook>, sheet: &str, row: u32, col: u32, s: &str) {
    engine
        .set_cell_value(sheet, row, col, LiteralValue::Text(s.to_string()))
        .unwrap();
}

/// Column A = client ("Acme" / "Beta" alternating), column B = amount (row index), then
/// `FORMULAS` cells of `=SUMIFS($B$1:$B$400,$A$1:$A$400,"acme",$B$1:$B$400,">10")` in column D and
/// as many `=COUNTIFS($A$1:$A$400,"acme")` in column E — the shape of a roll-up tab over a fact table.
fn fact_table_with_rollups(engine: &mut Engine<TestWorkbook>) {
    for row in 1..=ROWS {
        text(engine, row, 1, if row % 2 == 0 { "Acme" } else { "Beta" });
        number(engine, row, 2, row as f64);
    }
    for i in 0..FORMULAS {
        formula(
            engine,
            i + 1,
            4,
            r#"=SUMIFS($B$1:$B$400,$A$1:$A$400,"acme",$B$1:$B$400,">10")"#,
        );
        formula(engine, i + 1, 5, r#"=COUNTIFS($A$1:$A$400,"acme")"#);
    }
}

fn num(engine: &Engine<TestWorkbook>, row: u32, col: u32) -> f64 {
    match engine.get_cell_value("Sheet1", row, col) {
        Some(LiteralValue::Number(n)) => n,
        Some(LiteralValue::Int(i)) => i as f64,
        other => panic!("expected a number at r{row}c{col}, got {other:?}"),
    }
}

#[test]
fn every_rollup_shares_one_mask_per_column_and_predicate() {
    let mut engine = engine();
    fact_table_with_rollups(&mut engine);
    engine.evaluate_all().unwrap();

    // Even rows 12..=400 → 12+14+…+400 = 195 terms; sum = (12+400)/2 * 195.
    let expected_sum = (12.0 + 400.0) / 2.0 * 195.0;
    for i in 0..FORMULAS {
        assert_eq!(num(&engine, i + 1, 4), expected_sum, "SUMIFS row {}", i + 1);
        assert_eq!(num(&engine, i + 1, 5), 200.0, "COUNTIFS row {}", i + 1);
    }

    let report = engine.criteria_mask_cache_report();
    // Two distinct (column, predicate) pairs: A="acme" (shared by SUMIFS and COUNTIFS), B>10.
    // Without the cache this was 3 × FORMULAS builds; a cold key requested by many threads at
    // once must still build exactly once.
    assert_eq!(report.builds, 2, "{report:?}");
    assert_eq!(report.entries_count, 2, "{report:?}");
    assert!(
        report.hits >= (3 * FORMULAS as usize) - 2,
        "the other {FORMULAS} formulas per predicate hit the cache: {report:?}"
    );
    assert_eq!(report.skipped_volatile, 0, "{report:?}");
}

#[test]
fn an_edit_to_the_criteria_column_rotates_the_mask() {
    let mut engine = engine();
    fact_table_with_rollups(&mut engine);
    engine.evaluate_all().unwrap();
    let before = engine.criteria_mask_cache_report();

    // Row 400 flips Acme → Beta: the largest matching amount drops out of every roll-up.
    text(&mut engine, 400, 1, "Beta");
    engine.evaluate_all().unwrap();

    let expected_sum = (12.0 + 398.0) / 2.0 * 194.0;
    for i in 0..FORMULAS {
        assert_eq!(
            num(&engine, i + 1, 4),
            expected_sum,
            "SUMIFS row {} after edit",
            i + 1
        );
        assert_eq!(
            num(&engine, i + 1, 5),
            199.0,
            "COUNTIFS row {} after edit",
            i + 1
        );
    }
    let after = engine.criteria_mask_cache_report();
    // The A-column masks were rebuilt once each under the new snapshot; nothing was served stale.
    assert!(after.builds > before.builds, "{before:?} → {after:?}");
    assert!(
        after.builds <= before.builds + 2,
        "at most one rebuild per pair: {before:?} → {after:?}"
    );
}

#[test]
fn a_volatile_criteria_column_is_never_cached() {
    let mut engine = engine();
    for row in 1..=64 {
        formula(&mut engine, row, 1, "=IF(RAND()<2,\"x\",\"y\")");
        number(&mut engine, row, 2, row as f64);
    }
    formula(&mut engine, 1, 4, r#"=SUMIFS($B$1:$B$64,$A$1:$A$64,"x")"#);
    formula(&mut engine, 2, 4, r#"=SUMIFS($B$1:$B$64,$A$1:$A$64,"x")"#);
    engine.evaluate_all().unwrap();
    assert_eq!(num(&engine, 1, 4), (1.0 + 64.0) / 2.0 * 64.0);
    let report = engine.criteria_mask_cache_report();
    assert_eq!(report.builds, 0, "{report:?}");
    assert!(report.skipped_volatile >= 2, "{report:?}");
}

/// The criteria column is itself formulas fed from another sheet. The edit lands on Sheet2 only,
/// so Sheet1's snapshot term is untouched by the edit itself — the mask over Sheet1!A must still
/// see the recalculated Sheet1!A64 rather than serve last recalculation's bits.
#[test]
fn criteria_cells_recomputed_from_another_sheet_do_not_serve_a_stale_mask() {
    let mut engine = engine();
    for row in 1..=64 {
        text_on(&mut engine, "Sheet2", row, 1, "x");
        formula(&mut engine, row, 1, &format!("=Sheet2!A{row}"));
        number(&mut engine, row, 2, row as f64);
    }
    formula(&mut engine, 1, 4, r#"=SUMIFS($B$1:$B$64,$A$1:$A$64,"x")"#);
    formula(&mut engine, 2, 4, r#"=SUMIFS($B$1:$B$64,$A$1:$A$64,"x")"#);
    engine.evaluate_all().unwrap();
    assert_eq!(num(&engine, 1, 4), 2080.0);
    assert_eq!(num(&engine, 2, 4), 2080.0);

    text_on(&mut engine, "Sheet2", 64, 1, "y");
    engine.evaluate_all().unwrap();
    assert_eq!(
        num(&engine, 1, 4),
        2016.0,
        "{:?}",
        engine.criteria_mask_cache_report()
    );
    assert_eq!(
        num(&engine, 2, 4),
        2016.0,
        "{:?}",
        engine.criteria_mask_cache_report()
    );
}
