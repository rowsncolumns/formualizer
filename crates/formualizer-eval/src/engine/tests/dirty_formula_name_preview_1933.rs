//! rowsncolumns/spreadsheet#1933: a scratch evaluation (the formula bar's preview) of a named
//! formula no cell reads answers from the cells the name reads now. The name vertex's cached
//! value is only as fresh as the last recalculation that evaluated it; once an edit has dirtied
//! the vertex, the cache is not an answer.

use crate::engine::named_range::{NameScope, NamedDefinition};
use crate::engine::{Engine, EvalConfig};
use crate::interpreter::Interpreter;
use crate::reference::{CellRef, Coord, RangeRef};
use crate::test_workbook::TestWorkbook;
use formualizer_common::{ExcelErrorKind, LiteralValue};
use formualizer_parse::parser::parse;

fn formula_name(engine: &mut Engine<TestWorkbook>, name: &str, formula: &str) {
    engine
        .define_name(
            name,
            NamedDefinition::Formula {
                ast: parse(formula).unwrap(),
                dependencies: Vec::new(),
                range_deps: Vec::new(),
            },
            NameScope::Workbook,
        )
        .unwrap();
}

/// What the formula bar previews for `formula` typed on `Data`.
fn preview(engine: &Engine<TestWorkbook>, formula: &str) -> LiteralValue {
    let ast = parse(formula).unwrap();
    match Interpreter::new(engine, "Data").evaluate_ast(&ast) {
        Ok(value) => value.into_literal(),
        Err(err) => LiteralValue::Error(err),
    }
}

fn workbook() -> Engine<TestWorkbook> {
    let mut engine = Engine::new(TestWorkbook::new(), EvalConfig::default());
    for sheet in ["First", "Data", "Third"] {
        engine.graph.add_sheet(sheet).unwrap();
    }
    engine
        .set_cell_value("Data", 2, 3, LiteralValue::Number(7.0))
        .unwrap();
    engine
        .set_cell_value("Data", 3, 3, LiteralValue::Number(10.0))
        .unwrap();
    for row in 2..=5 {
        engine
            .set_cell_value(
                "Third",
                row,
                1,
                LiteralValue::Number(100.0 + f64::from(row)),
            )
            .unwrap();
    }
    let data = engine.graph.sheet_id("Data").unwrap();
    engine
        .define_name(
            "Base",
            NamedDefinition::Range(RangeRef::new(
                CellRef::new(data, Coord::from_excel(2, 3, true, true)),
                CellRef::new(data, Coord::from_excel(3, 3, true, true)),
            )),
            NameScope::Workbook,
        )
        .unwrap();
    formula_name(&mut engine, "Twice", "=SUM(Base)*2");
    formula_name(&mut engine, "Outer", "=Twice+100");
    formula_name(&mut engine, "OverThird", "=SUM(Third!$A$2:$A$4)+Third!$A$5");
    formula_name(&mut engine, "OverThirdTwice", "=OverThird*2");
    engine
}

#[test]
fn a_dirty_formula_name_previews_from_the_cells_it_reads_now() {
    let mut engine = workbook();
    engine.evaluate_all().unwrap();
    assert_eq!(preview(&engine, "=Twice"), LiteralValue::Number(34.0));
    assert_eq!(preview(&engine, "=Outer+1"), LiteralValue::Number(135.0));

    // An edit nothing recalculates the names after: no cell reads them.
    engine
        .set_cell_value("Data", 2, 3, LiteralValue::Number(6825.0))
        .unwrap();
    assert_eq!(preview(&engine, "=SUM(Base)"), LiteralValue::Number(6835.0));
    assert_eq!(preview(&engine, "=Twice"), LiteralValue::Number(13670.0));
    assert_eq!(preview(&engine, "=Outer+1"), LiteralValue::Number(13771.0));

    engine.evaluate_all().unwrap();
    assert_eq!(preview(&engine, "=Twice"), LiteralValue::Number(13670.0));
    assert_eq!(preview(&engine, "=Outer+1"), LiteralValue::Number(13771.0));
}

#[test]
fn a_name_over_a_name_over_a_deleted_sheet_previews_ref_before_any_recalculation() {
    let mut engine = workbook();
    engine.evaluate_all().unwrap();
    assert_eq!(preview(&engine, "=OverThird"), LiteralValue::Number(414.0));
    assert_eq!(
        preview(&engine, "=OverThirdTwice"),
        LiteralValue::Number(828.0)
    );

    let third = engine.graph.sheet_id("Third").unwrap();
    engine.remove_sheet(third).unwrap();
    for formula in ["=OverThird", "=OverThirdTwice"] {
        match preview(&engine, formula) {
            LiteralValue::Error(err) => assert_eq!(err.kind, ExcelErrorKind::Ref, "{formula}"),
            other => panic!("{formula} previews {other:?} after its sheet is deleted"),
        }
    }
}
