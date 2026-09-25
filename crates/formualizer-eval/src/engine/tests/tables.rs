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

/// A structured reference over a CALCULATED column must be scheduled after the column's own
/// formulas. The formula's graph edge points at the table vertex (stripe range deps — dirty
/// propagation only), so without table-aware virtual deps `=SUM(Sales[Price])` installed in the
/// same batch as the `[@Amount]*2` cells (a graph rebuild) evaluates first and reads blanks, then
/// lags one edit behind forever.
#[test]
fn structured_ref_over_calculated_column_evaluates_after_the_column_formulas() {
    let ctx = crate::test_workbook::TestWorkbook::new();
    let mut engine: Engine<_> = Engine::new(ctx, EvalConfig::default());
    engine.add_sheet("Sheet1").unwrap();
    for (col, header) in ["Item", "Amount", "Price"].iter().enumerate() {
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
        .set_cell_value("Sheet1", 2, 1, LiteralValue::Text("a".into()))
        .unwrap();
    engine
        .set_cell_value("Sheet1", 2, 2, LiteralValue::Number(10.0))
        .unwrap();
    engine
        .set_cell_value("Sheet1", 3, 1, LiteralValue::Text("b".into()))
        .unwrap();
    engine
        .set_cell_value("Sheet1", 3, 2, LiteralValue::Number(15.0))
        .unwrap();
    let sheet_id = engine.sheet_id("Sheet1").unwrap();
    let start = CellRef::new(sheet_id, Coord::from_excel(1, 1, true, true));
    let end = CellRef::new(sheet_id, Coord::from_excel(3, 3, true, true));
    engine
        .define_table(
            "Sales",
            RangeRef::new(start, end),
            true,
            vec!["Item".into(), "Amount".into(), "Price".into()],
            false,
        )
        .unwrap();

    // One batch, structured-ref consumers FIRST (row-major order of a rebuilt graph).
    let parse = |f: &str| formualizer_parse::parser::parse(f).unwrap();
    engine
        .bulk_set_formulas(
            "Sheet1",
            vec![
                (1, 5, parse("=SUM(Sales[Price])")),
                (1, 6, parse("=SUM(Sales[[#Data],[Price]])")),
                (1, 7, parse("=SUM(C2:C3)")),
                (2, 3, parse("=[@Amount]*2")),
                (3, 3, parse("=[@Amount]*2")),
            ],
        )
        .unwrap();
    engine.evaluate_all().unwrap();
    let value = |engine: &Engine<_>, row: u32, col: u32| engine.get_cell_value("Sheet1", row, col);
    assert_eq!(value(&engine, 1, 5), Some(LiteralValue::Number(50.0)));
    assert_eq!(value(&engine, 1, 6), Some(LiteralValue::Number(50.0)));
    assert_eq!(value(&engine, 1, 7), Some(LiteralValue::Number(50.0)));

    // An edit that re-fires the calculated column re-fires the structured refs AFTER it.
    engine
        .set_cell_value("Sheet1", 2, 2, LiteralValue::Number(20.0))
        .unwrap();
    engine.evaluate_all().unwrap();
    assert_eq!(value(&engine, 2, 3), Some(LiteralValue::Number(40.0)));
    assert_eq!(value(&engine, 1, 5), Some(LiteralValue::Number(70.0)));
    assert_eq!(value(&engine, 1, 6), Some(LiteralValue::Number(70.0)));
    assert_eq!(value(&engine, 1, 7), Some(LiteralValue::Number(70.0)));
}

/// Chained calculated columns (`Total` reads `Price`, `Price` reads `Amount`) plus a totals-row
/// `[#Totals]` consumer: every hop orders after its precedents, and a totals-row formula over the
/// table body does not see itself (the data body excludes the totals row — no false cycle).
#[test]
fn structured_ref_chain_and_totals_row_order_after_calculated_columns() {
    let ctx = crate::test_workbook::TestWorkbook::new();
    let mut engine: Engine<_> = Engine::new(ctx, EvalConfig::default());
    engine.add_sheet("Sheet1").unwrap();
    for (col, header) in ["Item", "Amount", "Price", "Total"].iter().enumerate() {
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
        .set_cell_value("Sheet1", 2, 2, LiteralValue::Number(10.0))
        .unwrap();
    engine
        .set_cell_value("Sheet1", 3, 2, LiteralValue::Number(15.0))
        .unwrap();
    let sheet_id = engine.sheet_id("Sheet1").unwrap();
    let start = CellRef::new(sheet_id, Coord::from_excel(1, 1, true, true));
    let end = CellRef::new(sheet_id, Coord::from_excel(4, 4, true, true));
    engine
        .define_table(
            "Sales",
            RangeRef::new(start, end),
            true,
            vec![
                "Item".into(),
                "Amount".into(),
                "Price".into(),
                "Total".into(),
            ],
            true,
        )
        .unwrap();
    let parse = |f: &str| formualizer_parse::parser::parse(f).unwrap();
    engine
        .bulk_set_formulas(
            "Sheet1",
            vec![
                // Consumers first: an outside cell over Total, the totals-row SUBTOTAL over
                // Price, and an outside cell over the totals row itself.
                (1, 6, parse("=SUM(Sales[Total])")),
                (4, 3, parse("=SUBTOTAL(109,Sales[Price])")),
                (1, 7, parse("=Sales[[#Totals],[Price]]")),
                (2, 4, parse("=[@Price]+1")),
                (3, 4, parse("=[@Price]+1")),
                (2, 3, parse("=[@Amount]*2")),
                (3, 3, parse("=[@Amount]*2")),
            ],
        )
        .unwrap();
    engine.evaluate_all().unwrap();
    let value = |engine: &Engine<_>, row: u32, col: u32| engine.get_cell_value("Sheet1", row, col);
    assert_eq!(value(&engine, 1, 6), Some(LiteralValue::Number(52.0)));
    assert_eq!(value(&engine, 4, 3), Some(LiteralValue::Number(50.0)));
    assert_eq!(value(&engine, 1, 7), Some(LiteralValue::Number(50.0)));

    engine
        .set_cell_value("Sheet1", 3, 2, LiteralValue::Number(25.0))
        .unwrap();
    engine.evaluate_all().unwrap();
    assert_eq!(value(&engine, 1, 6), Some(LiteralValue::Number(72.0)));
    assert_eq!(value(&engine, 4, 3), Some(LiteralValue::Number(70.0)));
    assert_eq!(value(&engine, 1, 7), Some(LiteralValue::Number(70.0)));
}

/// Unqualified structured references without `@` — Excel's own totals-row form
/// `=SUBTOTAL(109,[Qty])`, plus `[[#Totals],[Qty]]` / `[#Totals]` — resolve against the table the
/// evaluating cell sits in (totals row included); outside any table they are `#NAME?`
/// (rowsncolumns/spreadsheet#939 K-07). Covers both ingest paths: `set_cell_formula` (graph) and
/// `bulk_set_formulas` (ingest pipeline).
#[test]
fn unqualified_column_and_item_specifiers_resolve_against_the_containing_table() {
    let ctx = crate::test_workbook::TestWorkbook::new();
    let mut engine: Engine<_> = Engine::new(ctx, EvalConfig::default());
    engine.add_sheet("Sheet1").unwrap();
    for (col, header) in ["Item", "Qty", "Price"].iter().enumerate() {
        engine
            .set_cell_value("Sheet1", 1, col as u32 + 1, LiteralValue::Text((*header).into()))
            .unwrap();
    }
    for (row, qty, price) in [(2u32, 1.0, 10.0), (3, 2.0, 20.0), (4, 3.0, 30.0)] {
        engine.set_cell_value("Sheet1", row, 2, LiteralValue::Number(qty)).unwrap();
        engine.set_cell_value("Sheet1", row, 3, LiteralValue::Number(price)).unwrap();
    }
    let sheet_id = engine.sheet_id("Sheet1").unwrap();
    // Table1 over A1:C5: header row 1, body rows 2..4, totals row 5.
    engine
        .define_table(
            "Table1",
            RangeRef::new(
                CellRef::new(sheet_id, Coord::from_excel(1, 1, true, true)),
                CellRef::new(sheet_id, Coord::from_excel(5, 3, true, true)),
            ),
            true,
            vec!["Item".into(), "Qty".into(), "Price".into()],
            true,
        )
        .unwrap();
    let parse = |f: &str| formualizer_parse::parser::parse(f).unwrap();

    // Outside any table an unqualified reference is refused at ingest with `#NAME?` (the host
    // engine surfaces the rejection as the cell's error value), through both ingest paths.
    for f in ["=[[#Totals],[Qty]]", "=SUM([Qty])", "=SUM([#Totals])"] {
        let err = engine
            .set_cell_formula("Sheet1", 1, 8, parse(f))
            .expect_err("unqualified structured reference outside a table is refused");
        assert_eq!(err.kind, ExcelErrorKind::Name, "{f}: {err:?}");
        let err = engine
            .bulk_set_formulas("Sheet1", vec![(1, 9, parse(f))])
            .expect_err("bulk: unqualified structured reference outside a table is refused");
        assert_eq!(err.kind, ExcelErrorKind::Name, "bulk {f}: {err:?}");
    }

    // Graph ingest path (single-cell installs) — Excel's generated totals-row formulas.
    engine.set_cell_formula("Sheet1", 5, 2, parse("=SUBTOTAL(109,[Qty])")).unwrap();
    // Ingest-pipeline path (bulk installs).
    engine
        .bulk_set_formulas(
            "Sheet1",
            vec![
                (5, 3, parse("=SUBTOTAL(109,[Price])")),
                (1, 10, parse("=Table1[[#Totals],[Qty]]")),
                (1, 11, parse("=SUM(Table1[#Totals])")),
            ],
        )
        .unwrap();
    engine.evaluate_all().unwrap();

    let value = |engine: &Engine<_>, row: u32, col: u32| engine.get_cell_value("Sheet1", row, col);
    assert_eq!(value(&engine, 5, 2), Some(LiteralValue::Number(6.0)), "=SUBTOTAL(109,[Qty]) in the totals row");
    assert_eq!(value(&engine, 5, 3), Some(LiteralValue::Number(60.0)), "=SUBTOTAL(109,[Price]) in the totals row");
    assert_eq!(value(&engine, 1, 10), Some(LiteralValue::Number(6.0)), "=Table1[[#Totals],[Qty]] reads the totals cell");
    assert_eq!(value(&engine, 1, 11), Some(LiteralValue::Number(66.0)), "=SUM(Table1[#Totals]) = 6 + 60");

    // Unqualified item forms INSIDE the table body (column A, rows 2..4) through both paths.
    engine.set_cell_formula("Sheet1", 2, 1, parse("=[[#Totals],[Qty]]")).unwrap();
    engine.set_cell_formula("Sheet1", 3, 1, parse("=SUM([Price])")).unwrap();
    engine
        .bulk_set_formulas("Sheet1", vec![(4, 1, parse("=SUM([#Totals])"))])
        .unwrap();
    engine.evaluate_all().unwrap();
    assert_eq!(value(&engine, 2, 1), Some(LiteralValue::Number(6.0)), "in-table =[[#Totals],[Qty]]");
    assert_eq!(value(&engine, 3, 1), Some(LiteralValue::Number(60.0)), "in-table =SUM([Price])");
    assert_eq!(value(&engine, 4, 1), Some(LiteralValue::Number(66.0)), "in-table =SUM([#Totals])");

    // A body edit flows through the unqualified totals formula.
    engine.set_cell_value("Sheet1", 2, 2, LiteralValue::Number(11.0)).unwrap();
    engine.evaluate_all().unwrap();
    assert_eq!(value(&engine, 5, 2), Some(LiteralValue::Number(16.0)));
    assert_eq!(value(&engine, 1, 10), Some(LiteralValue::Number(16.0)));
}
