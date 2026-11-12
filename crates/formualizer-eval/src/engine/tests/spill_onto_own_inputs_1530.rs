//! A result that spills onto cells the formula reads is a circular reference, as in Excel
//! (rowsncolumns/spreadsheet#1530): `=D4:E4` in C4 would spill into D4, its own input.
use crate::engine::{EvalConfig, eval::Engine};
use crate::test_workbook::TestWorkbook;
use formualizer_common::ExcelErrorKind;
use formualizer_parse::LiteralValue;
use formualizer_parse::parser::parse;

fn is_circ(v: Option<LiteralValue>) -> bool {
    matches!(v, Some(LiteralValue::Error(e)) if e.kind == ExcelErrorKind::Circ)
}

#[test]
fn a_spill_onto_its_own_inputs_is_circular_and_spills_nothing() {
    let mut engine = Engine::new(TestWorkbook::new(), EvalConfig::default());
    engine
        .set_cell_value("Sheet1", 4, 5, LiteralValue::Number(7.0))
        .unwrap();
    engine
        .set_cell_formula("Sheet1", 4, 3, parse("=D4:E4").unwrap())
        .unwrap();
    engine.evaluate_all().unwrap();
    assert!(is_circ(engine.get_cell_value("Sheet1", 4, 3)));
    assert_eq!(
        engine.get_cell_value("Sheet1", 4, 4),
        None,
        "nothing spilled"
    );
    assert!(
        engine
            .last_cycle_cells()
            .iter()
            .any(|c| c.coord.row() == 3 && c.coord.col() == 2),
        "C4 reported as a cycle cell"
    );

    // An input edit keeps it circular.
    engine
        .set_cell_value("Sheet1", 4, 5, LiteralValue::Number(9.0))
        .unwrap();
    engine.evaluate_all().unwrap();
    assert!(is_circ(engine.get_cell_value("Sheet1", 4, 3)));
    assert_eq!(engine.get_cell_value("Sheet1", 4, 4), None);

    // The verdict is not the (empty) top-left input read as blank: `=D4:E4+1` is circular too.
    engine
        .set_cell_formula("Sheet1", 4, 3, parse("=D4:E4+1").unwrap())
        .unwrap();
    engine.evaluate_all().unwrap();
    assert!(is_circ(engine.get_cell_value("Sheet1", 4, 3)));
    assert_eq!(engine.get_cell_value("Sheet1", 4, 4), None);

    // Whole columns the spill lands in.
    engine
        .set_cell_formula("Sheet1", 4, 3, parse("=D:E").unwrap())
        .unwrap();
    engine.evaluate_all().unwrap();
    assert!(is_circ(engine.get_cell_value("Sheet1", 4, 3)));

    // A spill clear of its inputs spills again.
    engine
        .set_cell_value("Sheet1", 20, 4, LiteralValue::Number(1.0))
        .unwrap();
    engine
        .set_cell_value("Sheet1", 20, 5, LiteralValue::Number(2.0))
        .unwrap();
    engine
        .set_cell_formula("Sheet1", 4, 3, parse("=D20:E20").unwrap())
        .unwrap();
    engine.evaluate_all().unwrap();
    assert_eq!(
        engine.get_cell_value("Sheet1", 4, 3),
        Some(LiteralValue::Number(1.0))
    );
    assert_eq!(
        engine.get_cell_value("Sheet1", 4, 4),
        Some(LiteralValue::Number(2.0))
    );
}

#[test]
fn a_blocked_spill_onto_its_own_inputs_stays_spill() {
    let mut engine = Engine::new(TestWorkbook::new(), EvalConfig::default());
    engine
        .set_cell_value("Sheet1", 4, 4, LiteralValue::Number(5.0))
        .unwrap();
    engine
        .set_cell_formula("Sheet1", 4, 3, parse("=D4:E4").unwrap())
        .unwrap();
    engine.evaluate_all().unwrap();
    assert!(matches!(
        engine.get_cell_value("Sheet1", 4, 3),
        Some(LiteralValue::Error(e)) if e.kind == ExcelErrorKind::Spill
    ));
}
