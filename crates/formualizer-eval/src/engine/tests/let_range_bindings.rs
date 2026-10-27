//! A `LET` name bound to a range stays a *reference* inside the body, as in
//! Excel: `=LET(p,B1:B3,MATCH(5,p,0))` is the match index, not `#NAME?`
//! (Athena-Intel/demo-app-monorepo#44835). Functions that take their argument
//! by reference or as a range (`MATCH`, `INDEX`, `VLOOKUP`, `SUMPRODUCT`,
//! `OFFSET`, `ROWS`, `ROW`) see the bound range; a name bound to a computed
//! array (`SUMIFS(..., {"a","b","c"})`) is still usable wherever an array is
//! accepted; the bound cells remain dependencies of the formula.

use crate::engine::{Engine, EvalConfig};
use crate::test_workbook::TestWorkbook;
use formualizer_common::{ExcelErrorKind, LiteralValue};
use formualizer_parse::parser::parse;

fn engine_with_data() -> Engine<TestWorkbook> {
    let mut engine = Engine::new(TestWorkbook::new(), EvalConfig::default());
    engine.add_sheet("Sheet2").unwrap();
    // A1:A3 = a,b,c   B1:B3 = 3,5,7   C1:C3 = 1,2,3   Sheet2!A1:A3 = 3,5,7
    for (r, (label, b, c)) in [("a", 3.0, 1.0), ("b", 5.0, 2.0), ("c", 7.0, 3.0)]
        .into_iter()
        .enumerate()
    {
        let row = r as u32 + 1;
        engine
            .set_cell_value("Sheet1", row, 1, LiteralValue::Text(label.to_string()))
            .unwrap();
        engine
            .set_cell_value("Sheet1", row, 2, LiteralValue::Number(b))
            .unwrap();
        engine
            .set_cell_value("Sheet1", row, 3, LiteralValue::Number(c))
            .unwrap();
        engine
            .set_cell_value("Sheet2", row, 1, LiteralValue::Number(b))
            .unwrap();
    }
    engine
}

fn set(engine: &mut Engine<TestWorkbook>, row: u32, col: u32, f: &str) {
    engine
        .set_cell_formula("Sheet1", row, col, parse(f).expect("parse"))
        .expect("set formula");
}

fn value(engine: &Engine<TestWorkbook>, row: u32, col: u32) -> LiteralValue {
    engine
        .get_cell_value("Sheet1", row, col)
        .unwrap_or(LiteralValue::Empty)
}

fn num(engine: &Engine<TestWorkbook>, row: u32, col: u32) -> f64 {
    match value(engine, row, col) {
        LiteralValue::Number(n) => n,
        LiteralValue::Int(i) => i as f64,
        other => panic!("expected number at r{row}c{col}, got {other:?}"),
    }
}

#[test]
fn let_range_name_is_a_range_argument_for_lookup_and_aggregate_functions() {
    let mut engine = engine_with_data();
    set(&mut engine, 10, 1, "=LET(p,B1:B3,MATCH(5,p,0))");
    set(&mut engine, 11, 1, "=LET(p,B1:B3,INDEX(p,3))");
    set(&mut engine, 12, 1, "=LET(p,B1:B3,SUM(p))");
    set(&mut engine, 13, 1, "=LET(t,A1:B3,VLOOKUP(\"b\",t,2,FALSE))");
    set(&mut engine, 14, 1, "=LET(p,B1:B3,w,C1:C3,SUMPRODUCT(p,w))");
    set(&mut engine, 15, 1, "=LET(p,Sheet2!A1:A3,MATCH(7,p,0))");
    set(&mut engine, 16, 1, "=LET(p,B1:B3,XLOOKUP(7,p,A1:A3))");
    set(&mut engine, 17, 1, "=LET(p,B1:B3,COUNTIF(p,\">4\"))");
    set(&mut engine, 18, 1, "=LET(p,B1:B3,SUMIF(A1:A3,\"b\",p))");
    engine.evaluate_all().unwrap();

    assert_eq!(num(&engine, 10, 1), 2.0, "MATCH over a LET range");
    assert_eq!(num(&engine, 11, 1), 7.0, "INDEX over a LET range");
    assert_eq!(num(&engine, 12, 1), 15.0, "SUM over a LET range");
    assert_eq!(num(&engine, 13, 1), 5.0, "VLOOKUP over a LET table");
    assert_eq!(num(&engine, 14, 1), 34.0, "SUMPRODUCT over two LET ranges");
    assert_eq!(
        num(&engine, 15, 1),
        3.0,
        "MATCH over a LET range on another sheet"
    );
    assert_eq!(
        value(&engine, 16, 1),
        LiteralValue::Text("c".to_string()),
        "XLOOKUP over a LET range"
    );
    assert_eq!(num(&engine, 17, 1), 2.0, "COUNTIF over a LET range");
    assert_eq!(num(&engine, 18, 1), 5.0, "SUMIF sum_range is a LET range");
}

#[test]
fn let_range_name_keeps_reference_semantics() {
    let mut engine = engine_with_data();
    set(&mut engine, 10, 1, "=LET(p,B1:B3,ROWS(p))");
    set(&mut engine, 11, 1, "=LET(c,B2,ROW(c))");
    set(&mut engine, 12, 1, "=LET(c,B1,OFFSET(c,1,0))");
    set(&mut engine, 13, 1, "=LET(p,B1:B3,SUM(OFFSET(p,1,0)))");
    set(
        &mut engine,
        14,
        1,
        "=LET(p,B1:B3,SUM(INDEX(p,1):INDEX(p,2)))",
    );
    set(&mut engine, 15, 1, "=LET(p,B1:B3,q,p,MATCH(7,q,0))");
    set(&mut engine, 16, 1, "=LET(p,B1:B3,LET(q,p,INDEX(q,2)))");
    set(&mut engine, 17, 1, "=LET(p,B1:B3,COLUMN(p))");
    engine.evaluate_all().unwrap();

    assert_eq!(num(&engine, 10, 1), 3.0, "ROWS of a LET range");
    assert_eq!(num(&engine, 11, 1), 2.0, "ROW of a LET cell");
    assert_eq!(num(&engine, 12, 1), 5.0, "OFFSET from a LET cell");
    // B2:B4 = 5, 7, blank
    assert_eq!(num(&engine, 13, 1), 12.0, "SUM(OFFSET(LET range))");
    assert_eq!(
        num(&engine, 14, 1),
        8.0,
        "INDEX(p,1):INDEX(p,2) range operator"
    );
    assert_eq!(
        num(&engine, 15, 1),
        3.0,
        "a later LET name bound to an earlier range name"
    );
    assert_eq!(
        num(&engine, 16, 1),
        5.0,
        "nested LET rebinding a range name"
    );
    assert_eq!(num(&engine, 17, 1), 2.0, "COLUMN of a LET range");
}

#[test]
fn let_array_name_is_an_array_argument() {
    let mut engine = engine_with_data();
    // Athena-Intel/demo-app-monorepo#44835: p is the array SUMIFS yields for an array criterion.
    set(
        &mut engine,
        10,
        1,
        "=LET(p,SUMIFS(B1:B3,A1:A3,{\"a\",\"b\",\"c\"}),INDEX(A1:A3,MATCH(MAX(p),p,0)))",
    );
    set(&mut engine, 11, 1, "=LET(p,B1:B3*2,MATCH(10,p,0))");
    set(&mut engine, 12, 1, "=LET(p,B1:B3*2,INDEX(p,1))");
    set(&mut engine, 13, 1, "=LET(p,B1:B3*2,SUM(p))");
    set(&mut engine, 14, 1, "=LET(p,B1:B3*2,ROWS(p))");
    set(
        &mut engine,
        15,
        1,
        "=LET(f,FILTER(B1:B3,B1:B3>3),MATCH(7,f,0))",
    );
    engine.evaluate_all().unwrap();

    assert_eq!(
        value(&engine, 10, 1),
        LiteralValue::Text("c".to_string()),
        "INDEX/MATCH over a SUMIFS array bound by LET"
    );
    assert_eq!(num(&engine, 11, 1), 2.0, "MATCH over a computed array");
    assert_eq!(num(&engine, 12, 1), 6.0, "INDEX over a computed array");
    assert_eq!(num(&engine, 13, 1), 30.0, "SUM over a computed array");
    assert_eq!(num(&engine, 14, 1), 3.0, "ROWS of a computed array");
    assert_eq!(
        num(&engine, 15, 1),
        2.0,
        "MATCH over a FILTER result bound by LET"
    );
}

#[test]
fn let_range_cells_stay_dependencies() {
    let mut engine = engine_with_data();
    set(&mut engine, 10, 1, "=LET(p,B1:B3,MATCH(9,p,0))");
    engine.evaluate_all().unwrap();
    match value(&engine, 10, 1) {
        LiteralValue::Error(e) => assert_eq!(e.kind, ExcelErrorKind::Na),
        other => panic!("expected #N/A before the edit, got {other:?}"),
    }

    engine
        .set_cell_value("Sheet1", 2, 2, LiteralValue::Number(9.0))
        .unwrap();
    engine.evaluate_all().unwrap();
    assert_eq!(
        num(&engine, 10, 1),
        2.0,
        "editing a cell of the bound range recalculates"
    );
}

#[test]
fn an_unbound_name_is_still_name_error() {
    let mut engine = engine_with_data();
    set(&mut engine, 10, 1, "=LET(p,B1:B3,MATCH(5,q,0))");
    engine.evaluate_all().unwrap();
    match value(&engine, 10, 1) {
        LiteralValue::Error(e) => assert_eq!(e.kind, ExcelErrorKind::Name),
        other => panic!("expected #NAME?, got {other:?}"),
    }
}
