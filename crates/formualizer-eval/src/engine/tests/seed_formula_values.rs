//! `Engine::seed_formula_values`: a formula loaded with the result its file stores keeps that
//! result until something it reads changes (rowsncolumns/spreadsheet#1794).

use crate::engine::{Engine, EvalConfig};
use crate::test_workbook::TestWorkbook;
use formualizer_common::LiteralValue;
use formualizer_parse::parser::parse;

fn num(n: f64) -> LiteralValue {
    LiteralValue::Number(n)
}

/// F1 = 10, G1 = F1*2 stored as 999, H1 = G1+1 stored as 1000, I1 = RAND() stored as 0.5,
/// J1 = F1+1 with no stored result, K1 = J1*2 stored as 22. Loaded in bulk, nothing evaluated.
fn loaded() -> Engine<TestWorkbook> {
    let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
    e.set_cell_value("Sheet1", 1, 6, num(10.0)).unwrap();
    let formulas = [
        (1, 7, "=F1*2"),
        (1, 8, "=G1+1"),
        (1, 9, "=RAND()"),
        (1, 10, "=F1+1"),
        (1, 11, "=J1*2"),
    ];
    e.bulk_set_formulas(
        "Sheet1",
        formulas
            .iter()
            .map(|&(r, c, f)| (r, c, parse(f).expect("parse"))),
    )
    .unwrap();
    e
}

fn value(e: &Engine<TestWorkbook>, col: u32) -> Option<LiteralValue> {
    e.get_cell_value("Sheet1", 1, col)
}

fn stale(e: &Engine<TestWorkbook>) -> Vec<u32> {
    e.stale_formula_cells().into_iter().map(|c| c.col).collect()
}

#[test]
fn a_seeded_formula_reads_its_stored_result_and_does_not_wait_for_an_evaluation() {
    let mut e = loaded();
    let seeded = e.seed_formula_values(
        "Sheet1",
        [
            (1, 7, num(999.0)),
            (1, 8, num(1000.0)),
            (1, 9, num(0.5)),
            (1, 11, num(22.0)),
            // No formula here.
            (1, 6, num(1.0)),
            (2, 2, num(1.0)),
        ],
    );
    // The volatile formula is not seeded.
    assert_eq!(seeded, vec![(1, 7), (1, 8), (1, 11)]);
    assert_eq!(value(&e, 7), Some(num(999.0)));
    assert_eq!(value(&e, 8), Some(num(1000.0)));
    assert_eq!(stale(&e), vec![9, 10]);
    e.evaluate_all().unwrap();
    assert_eq!(value(&e, 7), Some(num(999.0)), "nothing G1 reads changed");
    assert_eq!(value(&e, 8), Some(num(1000.0)));
    assert_eq!(value(&e, 10), Some(num(11.0)), "J1 had no result");
    assert_eq!(
        value(&e, 11),
        Some(num(22.0)),
        "K1 keeps its seed until told otherwise"
    );
    assert_eq!(value(&e, 6), Some(num(10.0)), "a value cell is not seeded");
}

#[test]
fn an_edit_to_what_a_seeded_formula_reads_computes_it_and_its_readers() {
    let mut e = loaded();
    e.seed_formula_values("Sheet1", [(1, 7, num(999.0)), (1, 8, num(1000.0))]);
    e.evaluate_all().unwrap();
    e.set_cell_value("Sheet1", 1, 26, num(1.0)).unwrap();
    e.evaluate_all().unwrap();
    assert_eq!(value(&e, 7), Some(num(999.0)), "an unrelated edit");
    e.set_cell_value("Sheet1", 1, 6, num(11.0)).unwrap();
    e.evaluate_all().unwrap();
    assert_eq!(value(&e, 7), Some(num(22.0)));
    assert_eq!(value(&e, 8), Some(num(23.0)));
}

#[test]
fn a_formula_reading_a_seeded_one_computes_from_the_seed() {
    let mut e = loaded();
    e.seed_formula_values("Sheet1", [(1, 7, num(999.0))]);
    e.evaluate_all().unwrap();
    assert_eq!(value(&e, 8), Some(num(1000.0)), "H1 = G1 + 1 over the seed");
}

#[test]
fn spreading_reaches_the_seeded_formulas_that_read_a_waiting_one() {
    let mut e = loaded();
    e.seed_formula_values("Sheet1", [(1, 7, num(999.0)), (1, 11, num(5.0))]);
    assert_eq!(stale(&e), vec![8, 9, 10]);
    e.spread_waiting_formulas();
    assert_eq!(stale(&e), vec![8, 9, 10, 11]);
    e.evaluate_all().unwrap();
    assert_eq!(value(&e, 11), Some(num(22.0)), "K1 = J1 * 2, computed");
    assert_eq!(value(&e, 7), Some(num(999.0)));
}

#[test]
fn a_formula_this_engine_computed_is_not_seeded_over() {
    let mut e = loaded();
    e.evaluate_all().unwrap();
    assert_eq!(value(&e, 7), Some(num(20.0)));
    let seeded = e.seed_formula_values("Sheet1", [(1, 7, num(999.0))]);
    assert!(seeded.is_empty());
    assert_eq!(value(&e, 7), Some(num(20.0)));
}

#[test]
fn every_formula_dirty_computes_the_seeded_ones() {
    let mut e = loaded();
    e.seed_formula_values("Sheet1", [(1, 7, num(999.0)), (1, 8, num(1000.0))]);
    e.evaluate_all().unwrap();
    e.mark_all_formulas_dirty();
    e.evaluate_all().unwrap();
    assert_eq!(value(&e, 7), Some(num(20.0)));
    assert_eq!(value(&e, 8), Some(num(21.0)));
}
