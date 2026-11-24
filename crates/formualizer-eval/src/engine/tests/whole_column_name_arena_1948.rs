//! rowsncolumns/spreadsheet#1948: a name over a whole column is a reference, not a value. Every
//! evaluation of one used to copy its 1,048,576 cells into the append-only array arena, whose one
//! flat element vector reaches 2^28 entries (1 GiB of 4-byte refs) after 256 of them: the next
//! growth is past what a 32-bit build can allocate and panicked with `capacity overflow`.

use crate::engine::named_range::{NameScope, NamedDefinition};
use crate::engine::{Engine, EvalConfig};
use crate::reference::{CellRef, Coord, RangeRef};
use crate::test_workbook::TestWorkbook;
use formualizer_common::LiteralValue;
use formualizer_parse::parser::parse;

const SHEET_ROWS: u32 = 1_048_576;

fn whole_column(engine: &mut Engine<TestWorkbook>, sheet: &str, col: u32) -> NamedDefinition {
    let sheet_id = engine.graph.sheet_id_mut(sheet);
    NamedDefinition::Range(RangeRef::new(
        CellRef::new(sheet_id, Coord::from_excel(1, col, true, true)),
        CellRef::new(sheet_id, Coord::from_excel(SHEET_ROWS, col, true, true)),
    ))
}

fn arena_arrays(engine: &Engine<TestWorkbook>) -> (usize, usize) {
    let stats = engine.graph.data_store().memory_usage();
    (stats.total_arrays, stats.array_bytes)
}

#[test]
fn evaluating_whole_column_names_does_not_grow_the_array_arena() {
    let mut engine = Engine::new(TestWorkbook::new(), EvalConfig::default());
    engine.graph.add_sheet("Data").unwrap();
    engine
        .set_cell_value("Data", 1, 2, LiteralValue::Number(7.0))
        .unwrap();
    engine
        .set_cell_value("Data", 3, 2, LiteralValue::Number(10.0))
        .unwrap();
    for k in 1..=2 {
        let definition = whole_column(&mut engine, "Data", 2);
        engine
            .define_name(&format!("Whole{k}"), definition, NameScope::Workbook)
            .unwrap();
    }
    engine
        .set_cell_formula("Data", 1, 4, parse("=SUM(Whole1)+ROWS(Whole2)").unwrap())
        .unwrap();
    let before = arena_arrays(&engine);

    engine.evaluate_all().unwrap();
    assert_eq!(
        engine.get_cell_value("Data", 1, 4),
        Some(LiteralValue::Number(17.0 + f64::from(SHEET_ROWS)))
    );
    // Every typed value evaluates the names again.
    for (round, typed) in [100.0, 200.0, 300.0].into_iter().enumerate() {
        engine
            .set_cell_value("Data", 1, 2, LiteralValue::Number(typed))
            .unwrap();
        engine.evaluate_all().unwrap();
        assert_eq!(
            engine.get_cell_value("Data", 1, 4),
            Some(LiteralValue::Number(typed + 10.0 + f64::from(SHEET_ROWS))),
            "round {round}"
        );
    }
    // A direct request for the name's vertex as well.
    let vertex = engine
        .resolve_name_entry("Whole1", engine.graph.sheet_id("Data").unwrap())
        .unwrap()
        .vertex;
    let direct = engine.evaluate_vertex(vertex).unwrap();
    let LiteralValue::Array(rows) = direct else {
        panic!("a range name evaluates to its cells");
    };
    assert_eq!(rows.len(), SHEET_ROWS as usize);
    assert_eq!(rows[0], vec![LiteralValue::Number(300.0)]);

    assert_eq!(
        arena_arrays(&engine),
        before,
        "(arrays, bytes) held by the array arena"
    );
}

#[test]
fn a_range_name_still_answers_with_its_live_cells() {
    let mut engine = Engine::new(TestWorkbook::new(), EvalConfig::default());
    engine.graph.add_sheet("Data").unwrap();
    engine
        .set_cell_value("Data", 2, 3, LiteralValue::Number(7.0))
        .unwrap();
    engine
        .set_cell_value("Data", 3, 3, LiteralValue::Text("ten".into()))
        .unwrap();
    let sheet_id = engine.graph.sheet_id("Data").unwrap();
    engine
        .define_name(
            "Base",
            NamedDefinition::Range(RangeRef::new(
                CellRef::new(sheet_id, Coord::from_excel(2, 3, true, true)),
                CellRef::new(sheet_id, Coord::from_excel(3, 3, true, true)),
            )),
            NameScope::Workbook,
        )
        .unwrap();
    engine.evaluate_all().unwrap();
    let vertex = engine.resolve_name_entry("Base", sheet_id).unwrap().vertex;
    let expected = |first: f64| {
        Some(LiteralValue::Array(vec![
            vec![LiteralValue::Number(first)],
            vec![LiteralValue::Text("ten".into())],
        ]))
    };
    assert_eq!(engine.resolved_name_value("Base", None), expected(7.0));
    assert_eq!(engine.vertex_value(vertex), expected(7.0));

    engine
        .set_cell_value("Data", 2, 3, LiteralValue::Number(8.0))
        .unwrap();
    engine.evaluate_all().unwrap();
    assert_eq!(engine.resolved_name_value("Base", None), expected(8.0));
    assert_eq!(engine.vertex_value(vertex), expected(8.0));
}

#[test]
fn a_range_name_over_a_removed_sheet_answers_ref() {
    let mut engine = Engine::new(TestWorkbook::new(), EvalConfig::default());
    engine.graph.add_sheet("Data").unwrap();
    engine.graph.add_sheet("Gone").unwrap();
    engine
        .set_cell_value("Gone", 1, 1, LiteralValue::Number(7.0))
        .unwrap();
    let gone = engine.graph.sheet_id("Gone").unwrap();
    engine
        .define_name(
            "OverGone",
            NamedDefinition::Range(RangeRef::new(
                CellRef::new(gone, Coord::from_excel(1, 1, true, true)),
                CellRef::new(gone, Coord::from_excel(2, 1, true, true)),
            )),
            NameScope::Workbook,
        )
        .unwrap();
    engine.evaluate_all().unwrap();

    engine.remove_sheet(gone).unwrap();
    let is_ref = |value: Option<LiteralValue>| matches!(value, Some(LiteralValue::Error(e)) if e.kind == formualizer_common::ExcelErrorKind::Ref);
    assert!(is_ref(engine.resolved_name_value("OverGone", None)));
    engine
        .set_cell_formula("Data", 1, 1, parse("=SUM(OverGone)").unwrap())
        .unwrap();
    engine.evaluate_all().unwrap();
    assert!(is_ref(engine.get_cell_value("Data", 1, 1)));
}
