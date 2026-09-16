//! Re-defining a name (delete + define, which is how hosts move a named range after a structural
//! edit) must keep the formulas that use it wired to the NEW name vertex. Previously `delete_name`
//! detached the dependents and only marked them dirty: the formula re-evaluated once and then
//! never saw edits inside the redefined range (rowsncolumns/spreadsheet#546 E-10).

use formualizer_common::LiteralValue;
use formualizer_eval::engine::named_range::{NameScope, NamedDefinition};
use formualizer_eval::engine::{Engine, EvalConfig};
use formualizer_eval::reference::{CellRef, Coord, RangeRef};
use formualizer_eval::test_workbook::TestWorkbook;
use formualizer_parse::parser::parse;

fn range(
    sheet: formualizer_common::SheetId,
    r1: u32,
    c1: u32,
    r2: u32,
    c2: u32,
) -> NamedDefinition {
    NamedDefinition::Range(RangeRef::new(
        CellRef::new(sheet, Coord::from_excel(r1, c1, true, true)),
        CellRef::new(sheet, Coord::from_excel(r2, c2, true, true)),
    ))
}

#[test]
fn redefined_name_keeps_propagating_edits_to_dependents() {
    let mut engine = Engine::new(TestWorkbook::new(), EvalConfig::default());
    engine
        .set_cell_value("Sheet1", 1, 1, LiteralValue::Number(1.0))
        .unwrap();
    engine
        .set_cell_value("Sheet1", 3, 1, LiteralValue::Number(3.0))
        .unwrap();
    let sheet = engine.sheet_id("Sheet1").expect("sheet registered");
    engine
        .define_name("Data", range(sheet, 1, 1, 3, 1), NameScope::Workbook)
        .unwrap();
    engine
        .set_cell_formula("Sheet1", 1, 2, parse("=SUM(Data)").unwrap())
        .unwrap();
    engine.evaluate_all().unwrap();
    assert_eq!(
        engine.get_cell_value("Sheet1", 1, 2),
        Some(LiteralValue::Number(4.0))
    );

    // Grow the name to A1:A4 the way a host does after inserting a row inside it.
    engine.delete_name("Data", NameScope::Workbook).unwrap();
    engine
        .define_name("Data", range(sheet, 1, 1, 4, 1), NameScope::Workbook)
        .unwrap();
    engine.evaluate_all().unwrap();
    assert_eq!(
        engine.get_cell_value("Sheet1", 1, 2),
        Some(LiteralValue::Number(4.0))
    );

    // An edit inside the redefined range must still reach the dependent.
    engine
        .set_cell_value("Sheet1", 2, 1, LiteralValue::Number(100.0))
        .unwrap();
    engine.evaluate_all().unwrap();
    assert_eq!(
        engine.get_cell_value("Sheet1", 1, 2),
        Some(LiteralValue::Number(104.0)),
        "SUM(Data) must follow edits after the name was redefined"
    );
    engine
        .set_cell_value("Sheet1", 4, 1, LiteralValue::Number(1000.0))
        .unwrap();
    engine.evaluate_all().unwrap();
    assert_eq!(
        engine.get_cell_value("Sheet1", 1, 2),
        Some(LiteralValue::Number(1104.0))
    );
}
