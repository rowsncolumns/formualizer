//! Excel-parity pins for lookup semantics (rowsncolumns/spreadsheet#546, unit U6):
//! wildcard matching in exact-mode lookups, `MATCH(…,-1)` on descending data, `XLOOKUP`
//! not-found in approximate/wildcard modes, and skipped (`,,`) middle arguments in
//! `INDEX` / `TAKE` / `DROP` / `TOROW`.
//!
//! Seed grid (1-based): A1:A5 = 10,20,30,40,50 · B1:B5 = apple,banana,cherry,date,elderberry ·
//! C1:C5 = 5,3,5,1,2 · F1:G2 = [[1,4],[2,5]].

use formualizer_common::{ExcelErrorKind, LiteralValue};
use formualizer_eval::engine::{Engine, EvalConfig};
use formualizer_eval::test_workbook::TestWorkbook;
use formualizer_parse::parser::parse;

fn seeded() -> Engine<TestWorkbook> {
    let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
    let fruits = ["apple", "banana", "cherry", "date", "elderberry"];
    let c = [5.0, 3.0, 5.0, 1.0, 2.0];
    for (i, fruit) in fruits.iter().enumerate() {
        let row = (i + 1) as u32;
        e.set_cell_value("Sheet1", row, 1, LiteralValue::Number(((i + 1) * 10) as f64))
            .unwrap();
        e.set_cell_value("Sheet1", row, 2, LiteralValue::Text((*fruit).into()))
            .unwrap();
        e.set_cell_value("Sheet1", row, 3, LiteralValue::Number(c[i]))
            .unwrap();
    }
    for (r, row) in [[1.0, 4.0], [2.0, 5.0]].iter().enumerate() {
        for (k, v) in row.iter().enumerate() {
            e.set_cell_value("Sheet1", (r + 1) as u32, (6 + k) as u32, LiteralValue::Number(*v))
                .unwrap();
        }
    }
    e
}

/// Evaluate `formula` in K1 and return K1..(K1+rows, +cols) as a matrix (spill-aware).
fn eval_grid(formula: &str, rows: u32, cols: u32) -> Vec<Vec<Option<LiteralValue>>> {
    let mut e = seeded();
    e.set_cell_formula("Sheet1", 1, 11, parse(formula).unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    (0..rows)
        .map(|r| {
            (0..cols)
                .map(|c| e.get_cell_value("Sheet1", 1 + r, 11 + c))
                .collect()
        })
        .collect()
}

fn eval_one(formula: &str) -> LiteralValue {
    eval_grid(formula, 1, 1)[0][0]
        .clone()
        .unwrap_or(LiteralValue::Empty)
}

fn num(n: f64) -> LiteralValue {
    LiteralValue::Number(n)
}

fn text(s: &str) -> LiteralValue {
    LiteralValue::Text(s.into())
}

fn is_error(v: &LiteralValue, kind: ExcelErrorKind) -> bool {
    matches!(v, LiteralValue::Error(e) if e.kind == kind)
}

fn as_number(v: &LiteralValue) -> Option<f64> {
    match v {
        LiteralValue::Number(n) => Some(*n),
        LiteralValue::Int(i) => Some(*i as f64),
        _ => None,
    }
}

#[test]
fn exact_mode_lookups_honour_wildcards() {
    // C-R05
    assert_eq!(as_number(&eval_one(r#"=VLOOKUP("b*",B1:C5,2,FALSE)"#)), Some(3.0));
    assert_eq!(as_number(&eval_one(r#"=MATCH("c*",B1:B5,0)"#)), Some(3.0));
    assert_eq!(as_number(&eval_one(r#"=XMATCH("b*",B1:B5,2)"#)), Some(2.0));
    assert_eq!(as_number(&eval_one(r#"=XLOOKUP("d*",B1:B5,C1:C5,,2)"#)), Some(1.0));
    assert_eq!(as_number(&eval_one(r#"=XLOOKUP("ch*",B1:B5,C1:C5,,2)"#)), Some(5.0));
    assert_eq!(as_number(&eval_one(r#"=XLOOKUP("dat?",B1:B5,C1:C5,,2)"#)), Some(1.0));
    assert_eq!(as_number(&eval_one(r#"=HLOOKUP("b*",B1:C5,2,FALSE)"#)), None, "row lookup on a column-oriented table has no wildcard row match");
}

#[test]
fn exact_mode_lookups_are_case_insensitive() {
    assert_eq!(as_number(&eval_one(r#"=VLOOKUP("CHERRY",B1:C5,2,FALSE)"#)), Some(5.0));
    assert_eq!(as_number(&eval_one(r#"=MATCH("CHERRY",B1:B5,0)"#)), Some(3.0));
    assert_eq!(as_number(&eval_one(r#"=XLOOKUP("Cherry",B1:B5,C1:C5)"#)), Some(5.0));
}

#[test]
fn match_descending_mode_returns_smallest_value_greater_or_equal() {
    // C-R16: MATCH(35,{50,40,30,20,10},-1) → 2 (40 is the smallest value ≥ 35)
    assert_eq!(as_number(&eval_one("=MATCH(35,{50,40,30,20,10},-1)")), Some(2.0));
    assert_eq!(as_number(&eval_one("=MATCH(40,{50,40,30,20,10},-1)")), Some(2.0));
    assert_eq!(as_number(&eval_one("=MATCH(55,{50,40,30,20,10},-1)")), None);
    assert!(is_error(&eval_one("=MATCH(55,{50,40,30,20,10},-1)"), ExcelErrorKind::Na));
}

#[test]
fn xlookup_not_found_in_approximate_and_wildcard_modes_is_na() {
    // C-R17
    assert!(is_error(&eval_one("=XLOOKUP(5,A1:A5,B1:B5,,-1)"), ExcelErrorKind::Na));
    assert!(is_error(&eval_one("=XLOOKUP(500,A1:A5,B1:B5,,1)"), ExcelErrorKind::Na));
    assert!(is_error(&eval_one(r#"=XLOOKUP("zz*",B1:B5,C1:C5,,2)"#), ExcelErrorKind::Na));
    assert_eq!(eval_one(r#"=XLOOKUP(5,A1:A5,B1:B5,"none",-1)"#), text("none"));
    // regressions: approximate modes that do match
    assert_eq!(eval_one("=XLOOKUP(35,A1:A5,B1:B5,,-1)"), text("cherry"));
    assert_eq!(eval_one("=XLOOKUP(35,A1:A5,B1:B5,,1)"), text("date"));
}

#[test]
fn skipped_middle_arguments_are_treated_as_omitted() {
    // C-R13
    let g = eval_grid("=INDEX(A1:C5,,2)", 5, 1);
    let col: Vec<Option<LiteralValue>> = g.into_iter().map(|r| r[0].clone()).collect();
    assert_eq!(
        col,
        ["apple", "banana", "cherry", "date", "elderberry"]
            .iter()
            .map(|s| Some(text(s)))
            .collect::<Vec<_>>(),
        "INDEX(range,,col) returns the whole column"
    );
    let g = eval_grid("=INDEX(A1:C5,2,)", 1, 3);
    assert_eq!(
        g[0],
        vec![Some(num(20.0)), Some(text("banana")), Some(num(3.0))],
        "INDEX(range,row,) returns the whole row"
    );
    let g = eval_grid("=DROP(A1:C5,,-2)", 5, 1);
    let col: Vec<Option<f64>> = g.iter().map(|r| r[0].as_ref().and_then(as_number)).collect();
    assert_eq!(col, vec![Some(10.0), Some(20.0), Some(30.0), Some(40.0), Some(50.0)]);
    let g = eval_grid("=TAKE(A1:C5,,2)", 2, 2);
    assert_eq!(g[0], vec![Some(num(10.0)), Some(text("apple"))]);
    assert_eq!(g[1], vec![Some(num(20.0)), Some(text("banana"))]);
    let g = eval_grid("=TOROW(F1:G2,,TRUE)", 1, 4);
    let row: Vec<Option<f64>> = g[0].iter().map(|v| v.as_ref().and_then(as_number)).collect();
    assert_eq!(row, vec![Some(1.0), Some(2.0), Some(4.0), Some(5.0)], "scan_by_column");
    let g = eval_grid("=TOROW(F1:G2)", 1, 4);
    let row: Vec<Option<f64>> = g[0].iter().map(|v| v.as_ref().and_then(as_number)).collect();
    assert_eq!(row, vec![Some(1.0), Some(4.0), Some(2.0), Some(5.0)]);
}

#[test]
fn approximate_lookups_still_match_excel() {
    assert_eq!(as_number(&eval_one("=VLOOKUP(25,A1:C5,3,TRUE)")), Some(3.0));
    assert_eq!(eval_one("=VLOOKUP(500,A1:C5,2,TRUE)"), text("elderberry"));
    assert!(is_error(&eval_one("=VLOOKUP(5,A1:C5,2,TRUE)"), ExcelErrorKind::Na));
    assert_eq!(eval_one("=LOOKUP(25,A1:A5,B1:B5)"), text("banana"));
    assert_eq!(eval_one("=LOOKUP(25,A1:B5)"), text("banana"));
    assert_eq!(as_number(&eval_one("=MATCH(35,A1:A5,1)")), Some(3.0));
    assert!(is_error(&eval_one("=MATCH(5,A1:A5,1)"), ExcelErrorKind::Na));
    let g = eval_grid("=XLOOKUP(30,A1:A5,B1:C5)", 1, 2);
    assert_eq!(g[0], vec![Some(text("cherry")), Some(num(5.0))]);
}
