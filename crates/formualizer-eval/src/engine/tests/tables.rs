use crate::engine::{Engine, EvalConfig};
use crate::reference::{CellRef, Coord, RangeRef};
use formualizer_common::{ExcelErrorKind, LiteralValue};

#[test]
fn structured_ref_table_column_tracks_cell_edits_via_table_vertex() {
    let ctx = crate::test_workbook::TestWorkbook::new();
    let mut engine: Engine<_> = Engine::new(ctx, EvalConfig::default());

    engine.add_sheet("Sheet1").unwrap();

    // Table region A1:B3 (header + 2 data rows)
    // Headers: Region, Amount
    // Make the non-selected column numeric so we catch over-wide selection bugs.
    engine
        .set_cell_value("Sheet1", 2, 1, LiteralValue::Number(5.0))
        .unwrap();
    engine
        .set_cell_value("Sheet1", 2, 2, LiteralValue::Number(10.0))
        .unwrap();
    engine
        .set_cell_value("Sheet1", 3, 1, LiteralValue::Number(7.0))
        .unwrap();
    engine
        .set_cell_value("Sheet1", 3, 2, LiteralValue::Number(20.0))
        .unwrap();

    let sheet_id = engine.sheet_id("Sheet1").unwrap();
    let start = CellRef::new(sheet_id, Coord::from_excel(1, 1, true, true));
    let end = CellRef::new(sheet_id, Coord::from_excel(3, 2, true, true));
    let range = RangeRef::new(start, end);
    engine
        .define_table(
            "Sales",
            range,
            true,
            vec!["Region".into(), "Amount".into()],
            false,
        )
        .unwrap();

    let ast = formualizer_parse::parser::parse("=SUM(Sales[Amount])").unwrap();
    engine.set_cell_formula("Sheet1", 1, 4, ast).unwrap();

    let v = engine
        .evaluate_cell("Sheet1", 1, 4)
        .unwrap()
        .expect("computed value");
    assert_eq!(v, LiteralValue::Number(30.0));

    // Edit a precedent cell inside the table and ensure the table-dependent formula is dirtied.
    engine
        .set_cell_value("Sheet1", 2, 2, LiteralValue::Number(100.0))
        .unwrap();
    let v2 = engine
        .evaluate_cell("Sheet1", 1, 4)
        .unwrap()
        .expect("computed value");
    assert_eq!(v2, LiteralValue::Number(120.0));
}

#[test]
fn structured_ref_this_row_column_rewrites_to_concrete_cell() {
    let ctx = crate::test_workbook::TestWorkbook::new();
    let mut engine: Engine<_> = Engine::new(ctx, EvalConfig::default());

    engine.add_sheet("Sheet1").unwrap();

    // Table region A1:C3 (header + 2 data rows)
    // Headers: Region, Amount, Double
    engine
        .set_cell_value("Sheet1", 2, 1, LiteralValue::Text("N".into()))
        .unwrap();
    engine
        .set_cell_value("Sheet1", 2, 2, LiteralValue::Number(10.0))
        .unwrap();
    engine
        .set_cell_value("Sheet1", 3, 1, LiteralValue::Text("S".into()))
        .unwrap();
    engine
        .set_cell_value("Sheet1", 3, 2, LiteralValue::Number(20.0))
        .unwrap();

    let sheet_id = engine.sheet_id("Sheet1").unwrap();
    let start = CellRef::new(sheet_id, Coord::from_excel(1, 1, true, true));
    let end = CellRef::new(sheet_id, Coord::from_excel(3, 3, true, true));
    let range = RangeRef::new(start, end);
    engine
        .define_table(
            "Sales",
            range,
            true,
            vec!["Region".into(), "Amount".into(), "Double".into()],
            false,
        )
        .unwrap();

    // Formula inside the table: Double = [@Amount] * 2
    let f2 = formualizer_parse::parser::parse("=[@Amount]*2").unwrap();
    engine.set_cell_formula("Sheet1", 2, 3, f2).unwrap();
    let f3 = formualizer_parse::parser::parse("=[@[Amount]]*2").unwrap();
    engine.set_cell_formula("Sheet1", 3, 3, f3).unwrap();

    let v2 = engine
        .evaluate_cell("Sheet1", 2, 3)
        .unwrap()
        .expect("computed value");
    assert_eq!(v2, LiteralValue::Number(20.0));
    let v3 = engine
        .evaluate_cell("Sheet1", 3, 3)
        .unwrap()
        .expect("computed value");
    assert_eq!(v3, LiteralValue::Number(40.0));

    // Editing Amount should dirty and recompute the this-row dependents.
    engine
        .set_cell_value("Sheet1", 2, 2, LiteralValue::Number(100.0))
        .unwrap();
    let v2b = engine
        .evaluate_cell("Sheet1", 2, 3)
        .unwrap()
        .expect("computed value");
    assert_eq!(v2b, LiteralValue::Number(200.0));
}

#[test]
fn structured_ref_bracket_table_shorthand_selects_data_body() {
    let ctx = crate::test_workbook::TestWorkbook::new();
    let mut engine: Engine<_> = Engine::new(ctx, EvalConfig::default());

    engine.add_sheet("Sheet1").unwrap();

    // Table region A1:B3 (header + 2 data rows)
    // Headers: Region, Amount
    engine
        .set_cell_value("Sheet1", 1, 2, LiteralValue::Number(100.0))
        .unwrap();
    engine
        .set_cell_value("Sheet1", 2, 1, LiteralValue::Number(5.0))
        .unwrap();
    engine
        .set_cell_value("Sheet1", 2, 2, LiteralValue::Number(10.0))
        .unwrap();
    engine
        .set_cell_value("Sheet1", 3, 1, LiteralValue::Number(7.0))
        .unwrap();
    engine
        .set_cell_value("Sheet1", 3, 2, LiteralValue::Number(20.0))
        .unwrap();

    let sheet_id = engine.sheet_id("Sheet1").unwrap();
    let start = CellRef::new(sheet_id, Coord::from_excel(1, 1, true, true));
    let end = CellRef::new(sheet_id, Coord::from_excel(3, 2, true, true));
    let range = RangeRef::new(start, end);
    engine
        .define_table(
            "Sales",
            range,
            true,
            vec!["Region".into(), "Amount".into()],
            false,
        )
        .unwrap();

    let ast = formualizer_parse::parser::parse("=SUM([Sales])").unwrap();
    engine.set_cell_formula("Sheet1", 1, 4, ast).unwrap();

    // Header cell (B1=100) should not be included; only data body should contribute.
    let v = engine
        .evaluate_cell("Sheet1", 1, 4)
        .unwrap()
        .expect("computed value");
    assert_eq!(v, LiteralValue::Number(42.0));
}

#[test]
fn structured_ref_table_name_resolution_is_case_insensitive_by_default() {
    let ctx = crate::test_workbook::TestWorkbook::new();
    let mut engine: Engine<_> = Engine::new(ctx, EvalConfig::default());

    engine.add_sheet("Sheet1").unwrap();

    // Table region A1:B3 (header + 2 data rows)
    engine
        .set_cell_value("Sheet1", 2, 2, LiteralValue::Number(10.0))
        .unwrap();
    engine
        .set_cell_value("Sheet1", 3, 2, LiteralValue::Number(20.0))
        .unwrap();

    let sheet_id = engine.sheet_id("Sheet1").unwrap();
    let start = CellRef::new(sheet_id, Coord::from_excel(1, 1, true, true));
    let end = CellRef::new(sheet_id, Coord::from_excel(3, 2, true, true));
    let range = RangeRef::new(start, end);
    engine
        .define_table(
            "Sales",
            range,
            true,
            vec!["Region".into(), "Amount".into()],
            false,
        )
        .unwrap();

    // Reference the table using different casing.
    let ast = formualizer_parse::parser::parse("=SUM(sales[Amount])").unwrap();
    engine.set_cell_formula("Sheet1", 1, 4, ast).unwrap();
    let v = engine
        .evaluate_cell("Sheet1", 1, 4)
        .unwrap()
        .expect("computed value");
    assert_eq!(v, LiteralValue::Number(30.0));
}

#[test]
fn structured_ref_unicode_table_and_column_resolution_is_case_insensitive_by_default() {
    let ctx = crate::test_workbook::TestWorkbook::new();
    let mut engine: Engine<_> = Engine::new(ctx, EvalConfig::default());

    engine.add_sheet("Sheet1").unwrap();

    engine
        .set_cell_value("Sheet1", 2, 2, LiteralValue::Number(10.0))
        .unwrap();
    engine
        .set_cell_value("Sheet1", 3, 2, LiteralValue::Number(20.0))
        .unwrap();

    let sheet_id = engine.sheet_id("Sheet1").unwrap();
    let start = CellRef::new(sheet_id, Coord::from_excel(1, 1, true, true));
    let end = CellRef::new(sheet_id, Coord::from_excel(3, 2, true, true));
    let range = RangeRef::new(start, end);
    engine
        .define_table(
            "Продажи",
            range,
            true,
            vec!["Регион".into(), "Сумма".into()],
            false,
        )
        .unwrap();

    let ast = formualizer_parse::parser::parse("=SUM(продажи[сумма])").unwrap();
    engine.set_cell_formula("Sheet1", 1, 4, ast).unwrap();
    let v = engine
        .evaluate_cell("Sheet1", 1, 4)
        .unwrap()
        .expect("computed value");
    assert_eq!(v, LiteralValue::Number(30.0));
}

#[test]
fn table_definition_rejects_case_insensitive_collisions() {
    let ctx = crate::test_workbook::TestWorkbook::new();
    let mut engine: Engine<_> = Engine::new(ctx, EvalConfig::default());
    engine.add_sheet("Sheet1").unwrap();

    let sheet_id = engine.sheet_id("Sheet1").unwrap();
    let start = CellRef::new(sheet_id, Coord::from_excel(1, 1, true, true));
    let end = CellRef::new(sheet_id, Coord::from_excel(1, 1, true, true));
    let range = RangeRef::new(start, end);

    engine
        .define_table("Sales", range, true, vec!["A".into()], false)
        .unwrap();

    let err = engine
        .define_table("sales", range, true, vec!["A".into()], false)
        .expect_err("expected collision error");
    assert_eq!(err.kind, ExcelErrorKind::Name);
}

/// A1:C4 table "Sales" — header row + 2 data rows + optional totals row.
/// Headers: Region, Amount, Tax. Data: (N, 10, 1), (S, 20, 2). Totals: (,30,3).
fn sales_table_engine(totals_row: bool) -> Engine<crate::test_workbook::TestWorkbook> {
    let ctx = crate::test_workbook::TestWorkbook::new();
    let mut engine: Engine<_> = Engine::new(ctx, EvalConfig::default());
    engine.add_sheet("Sheet1").unwrap();

    for (col, header) in ["Region", "Amount", "Tax"].iter().enumerate() {
        engine
            .set_cell_value(
                "Sheet1",
                1,
                col as u32 + 1,
                LiteralValue::Text((*header).into()),
            )
            .unwrap();
    }
    engine
        .set_cell_value("Sheet1", 2, 1, LiteralValue::Text("N".into()))
        .unwrap();
    engine
        .set_cell_value("Sheet1", 2, 2, LiteralValue::Number(10.0))
        .unwrap();
    engine
        .set_cell_value("Sheet1", 2, 3, LiteralValue::Number(1.0))
        .unwrap();
    engine
        .set_cell_value("Sheet1", 3, 1, LiteralValue::Text("S".into()))
        .unwrap();
    engine
        .set_cell_value("Sheet1", 3, 2, LiteralValue::Number(20.0))
        .unwrap();
    engine
        .set_cell_value("Sheet1", 3, 3, LiteralValue::Number(2.0))
        .unwrap();

    let end_row = if totals_row {
        engine
            .set_cell_value("Sheet1", 4, 2, LiteralValue::Number(30.0))
            .unwrap();
        engine
            .set_cell_value("Sheet1", 4, 3, LiteralValue::Number(3.0))
            .unwrap();
        4
    } else {
        3
    };

    let sheet_id = engine.sheet_id("Sheet1").unwrap();
    let start = CellRef::new(sheet_id, Coord::from_excel(1, 1, true, true));
    let end = CellRef::new(sheet_id, Coord::from_excel(end_row, 3, true, true));
    engine
        .define_table(
            "Sales",
            RangeRef::new(start, end),
            true,
            vec!["Region".into(), "Amount".into(), "Tax".into()],
            totals_row,
        )
        .unwrap();
    engine
}

fn eval_cell(
    engine: &mut Engine<crate::test_workbook::TestWorkbook>,
    row: u32,
    col: u32,
) -> LiteralValue {
    engine
        .evaluate_cell("Sheet1", row, col)
        .unwrap()
        .expect("computed value")
}

#[test]
fn structured_ref_named_this_row_column() {
    let mut engine = sales_table_engine(false);

    // Named this-row refs in a column OUTSIDE the table (col E), rows 2 and 3.
    let f2 = formualizer_parse::parser::parse("=Sales[[#This Row],[Amount]]*2").unwrap();
    engine.set_cell_formula("Sheet1", 2, 5, f2).unwrap();
    let f3 = formualizer_parse::parser::parse("=Sales[@Amount]*2").unwrap();
    engine.set_cell_formula("Sheet1", 3, 5, f3).unwrap();

    assert_eq!(eval_cell(&mut engine, 2, 5), LiteralValue::Number(20.0));
    assert_eq!(eval_cell(&mut engine, 3, 5), LiteralValue::Number(40.0));

    // Edits to the referenced row dirty the dependents.
    engine
        .set_cell_value("Sheet1", 2, 2, LiteralValue::Number(100.0))
        .unwrap();
    assert_eq!(eval_cell(&mut engine, 2, 5), LiteralValue::Number(200.0));
}

#[test]
fn structured_ref_named_this_row_column_range() {
    let mut engine = sales_table_engine(false);

    let f = formualizer_parse::parser::parse("=SUM(Sales[[#This Row],[Amount]:[Tax]])").unwrap();
    engine.set_cell_formula("Sheet1", 2, 5, f).unwrap();
    assert_eq!(eval_cell(&mut engine, 2, 5), LiteralValue::Number(11.0));
}

#[test]
fn structured_ref_named_bare_at_selects_whole_current_row() {
    let mut engine = sales_table_engine(false);

    // Sales[@] = the full current row across the table's columns.
    let f = formualizer_parse::parser::parse("=SUM(Sales[@])").unwrap();
    engine.set_cell_formula("Sheet1", 3, 5, f).unwrap();
    assert_eq!(eval_cell(&mut engine, 3, 5), LiteralValue::Number(22.0));
}

#[test]
fn structured_ref_unnamed_this_row_column_range() {
    let mut engine = sales_table_engine(false);

    // Unnamed shorthand with a column range, evaluated from INSIDE the table.
    let f = formualizer_parse::parser::parse("=SUM([@[Amount]:[Tax]])").unwrap();
    engine.set_cell_formula("Sheet1", 3, 1, f).unwrap();
    assert_eq!(eval_cell(&mut engine, 3, 1), LiteralValue::Number(22.0));
}

#[test]
fn structured_ref_named_this_row_outside_table_rows_is_value_error() {
    let mut engine = sales_table_engine(false);

    let f = formualizer_parse::parser::parse("=Sales[@Amount]").unwrap();
    let err = engine
        .set_cell_formula("Sheet1", 10, 5, f)
        .expect_err("this-row outside the table's rows must fail");
    assert_eq!(err.kind, ExcelErrorKind::Value);
}

#[test]
fn structured_ref_combination_data_column() {
    let mut engine = sales_table_engine(false);

    // [[#Data],[Amount]] == [Amount]
    let f = formualizer_parse::parser::parse("=SUM(Sales[[#Data],[Amount]])").unwrap();
    engine.set_cell_formula("Sheet1", 1, 5, f).unwrap();
    assert_eq!(eval_cell(&mut engine, 1, 5), LiteralValue::Number(30.0));
}

#[test]
fn structured_ref_combination_headers_column() {
    let mut engine = sales_table_engine(false);

    let f = formualizer_parse::parser::parse("=Sales[[#Headers],[Amount]]").unwrap();
    engine.set_cell_formula("Sheet1", 1, 5, f).unwrap();
    assert_eq!(
        eval_cell(&mut engine, 1, 5),
        LiteralValue::Text("Amount".into())
    );
}

#[test]
fn structured_ref_combination_data_and_totals() {
    let mut engine = sales_table_engine(true);

    let f = formualizer_parse::parser::parse("=SUM(Sales[[#Data],[#Totals],[Amount]])").unwrap();
    engine.set_cell_formula("Sheet1", 1, 5, f).unwrap();
    assert_eq!(eval_cell(&mut engine, 1, 5), LiteralValue::Number(60.0));
}

#[test]
fn structured_ref_combination_headers_and_totals_is_ref_error() {
    let mut engine = sales_table_engine(true);

    // Headers + Totals without Data is a non-adjacent row union.
    let f = formualizer_parse::parser::parse("=SUM(Sales[[#Headers],[#Totals],[Amount]])").unwrap();
    engine.set_cell_formula("Sheet1", 1, 5, f).unwrap();
    let v = eval_cell(&mut engine, 1, 5);
    assert_eq!(
        v,
        LiteralValue::Error(formualizer_common::ExcelError::new(ExcelErrorKind::Ref))
    );
}

#[test]
fn structured_ref_this_row_via_direct_interpreter_eval() {
    // The rnc conditional-format path: parse + evaluate an AST directly with a
    // cell context, without ingesting the formula into the graph.
    let engine = sales_table_engine(false);

    let sheet_id = engine.sheet_id("Sheet1").unwrap();
    let ast = formualizer_parse::parser::parse("=Sales[@Amount]*2").unwrap();
    let interp = crate::interpreter::Interpreter::new_with_cell(
        &engine,
        "Sheet1",
        CellRef::new_absolute(sheet_id, 2, 1), // 0-based: row 3, col B
    );
    let v = interp.evaluate_ast(&ast).unwrap().into_literal();
    assert_eq!(v, LiteralValue::Number(40.0));
}
