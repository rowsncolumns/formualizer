//! Re-spilling a dynamic array must dirty readers of the spilled cells.
//!
//! A formula that reads a spill PROJECTION (a non-anchor cell of a committed
//! spill) has a dependency edge to that cell's vertex, not to the anchor.
//! When the anchor's input changes and the spill recommits with new values,
//! the projection readers must recompute in the same `evaluate_all` pass.

use crate::engine::{Engine, EvalConfig};
use crate::test_workbook::TestWorkbook;
use formualizer_common::LiteralValue;
use formualizer_parse::parser::parse;

fn get(engine: &Engine<TestWorkbook>, row: u32, col: u32) -> Option<LiteralValue> {
    engine.get_cell_value("Sheet1", row, col)
}

#[test]
fn respill_dirties_projection_reader() {
    let mut engine = Engine::new(TestWorkbook::default(), EvalConfig::default());
    engine
        .set_cell_value("Sheet1", 1, 1, LiteralValue::Int(1))
        .unwrap(); // A1
    engine
        .set_cell_formula("Sheet1", 1, 2, parse("=SEQUENCE(3)*A1").unwrap())
        .unwrap(); // B1 spills B1:B3
    engine
        .set_cell_formula("Sheet1", 1, 4, parse("=B3+1").unwrap())
        .unwrap(); // D1 reads a spill projection
    engine.evaluate_all().unwrap();

    assert_eq!(
        get(&engine, 3, 2),
        Some(LiteralValue::Number(3.0)),
        "B3 initial"
    );
    assert_eq!(
        get(&engine, 1, 4),
        Some(LiteralValue::Number(4.0)),
        "D1 initial"
    );

    // Edit the spill's input; the spill recommits over the SAME footprint.
    engine
        .set_cell_value("Sheet1", 1, 1, LiteralValue::Int(2))
        .unwrap();
    engine.evaluate_all().unwrap();

    assert_eq!(
        get(&engine, 3, 2),
        Some(LiteralValue::Number(6.0)),
        "B3 after re-spill"
    );
    assert_eq!(
        get(&engine, 1, 4),
        Some(LiteralValue::Number(7.0)),
        "D1 must recompute from the re-spilled B3"
    );
}

#[test]
fn respill_dirties_transitive_reader_of_projection() {
    let mut engine = Engine::new(TestWorkbook::default(), EvalConfig::default());
    engine
        .set_cell_value("Sheet1", 1, 1, LiteralValue::Int(1))
        .unwrap(); // A1
    engine
        .set_cell_formula("Sheet1", 1, 2, parse("=SEQUENCE(3)*A1").unwrap())
        .unwrap(); // B1 spills B1:B3
    engine
        .set_cell_formula("Sheet1", 1, 4, parse("=B3+1").unwrap())
        .unwrap(); // D1
    engine
        .set_cell_formula("Sheet1", 1, 5, parse("=D1*10").unwrap())
        .unwrap(); // E1: transitive
    engine
        .set_cell_formula("Sheet1", 2, 4, parse("=SUM(B1:B3)").unwrap())
        .unwrap(); // D2: range over the whole spill
    engine.evaluate_all().unwrap();

    assert_eq!(
        get(&engine, 1, 5),
        Some(LiteralValue::Number(40.0)),
        "E1 initial"
    );
    assert_eq!(
        get(&engine, 2, 4),
        Some(LiteralValue::Number(6.0)),
        "D2 initial"
    );

    engine
        .set_cell_value("Sheet1", 1, 1, LiteralValue::Int(3))
        .unwrap();
    engine.evaluate_all().unwrap();

    assert_eq!(
        get(&engine, 1, 4),
        Some(LiteralValue::Number(10.0)),
        "D1 after re-spill"
    );
    assert_eq!(
        get(&engine, 1, 5),
        Some(LiteralValue::Number(100.0)),
        "E1 after re-spill"
    );
    assert_eq!(
        get(&engine, 2, 4),
        Some(LiteralValue::Number(18.0)),
        "D2 after re-spill"
    );
}

#[test]
fn respill_with_shape_change_dirties_readers_of_vacated_cells() {
    let mut engine = Engine::new(TestWorkbook::default(), EvalConfig::default());
    engine
        .set_cell_value("Sheet1", 1, 1, LiteralValue::Int(3))
        .unwrap(); // A1: spill height
    engine
        .set_cell_formula("Sheet1", 1, 2, parse("=SEQUENCE(A1)").unwrap())
        .unwrap(); // B1 spills B1:B3
    engine
        .set_cell_formula("Sheet1", 1, 4, parse("=B3+1").unwrap())
        .unwrap(); // D1 reads the last projection
    engine.evaluate_all().unwrap();

    assert_eq!(
        get(&engine, 1, 4),
        Some(LiteralValue::Number(4.0)),
        "D1 initial"
    );

    // Shrink the spill: B3 is vacated and becomes Empty.
    engine
        .set_cell_value("Sheet1", 1, 1, LiteralValue::Int(2))
        .unwrap();
    engine.evaluate_all().unwrap();

    let b3 = get(&engine, 3, 2);
    assert!(
        matches!(b3, None | Some(LiteralValue::Empty)),
        "B3 vacated, got {b3:?}"
    );
    assert_eq!(
        get(&engine, 1, 4),
        Some(LiteralValue::Number(1.0)),
        "D1 must recompute against the vacated (empty) B3"
    );
}
