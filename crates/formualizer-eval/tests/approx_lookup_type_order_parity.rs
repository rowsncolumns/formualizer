//! Excel-parity pins for APPROXIMATE lookups over mixed-type data (rowsncolumns/spreadsheet#939 Z-08).
//!
//! Excel's approximate match (`MATCH(…,1|-1)`, `VLOOKUP`/`HLOOKUP(…,TRUE)`, `LOOKUP`, `XLOOKUP` /
//! `XMATCH` match_mode ±1) walks the documented sort order of a range_lookup column —
//! `…, -1, 0, 1, …, A–Z, FALSE, TRUE` — comparing by TYPE first and never coercing across types:
//! every number is smaller than every text, every text smaller than every logical. So
//! `MATCH("1",{1;2;3},1)` is 3 (the text is larger than all the numbers), `MATCH(1,{TRUE;2;3},1)`
//! is `#N/A` (TRUE is not 1) and `VLOOKUP(TRUE,{1,"a";2,"b"},2,TRUE)` is "b". The fork's
//! binary-search comparator coerced `TRUE`→1 and numeric-looking text to numbers, so those three
//! returned 1 / 1 / "a". Exact match (`0` / `FALSE`) is untouched.
//!
//! Seed grid (1-based): A1:A10 = 1..10 · C1:C4 = 1,2,"a","c" · D1:D4 = "w","x","y","z".

use formualizer_common::{ExcelErrorKind, LiteralValue};
use formualizer_eval::engine::{Engine, EvalConfig};
use formualizer_eval::test_workbook::TestWorkbook;
use formualizer_parse::parser::parse;

fn eval(formula: &str) -> LiteralValue {
    let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
    for i in 1..=10u32 {
        e.set_cell_value("Sheet1", i, 1, LiteralValue::Number(i as f64))
            .unwrap();
    }
    let c = [
        LiteralValue::Number(1.0),
        LiteralValue::Number(2.0),
        LiteralValue::Text("a".into()),
        LiteralValue::Text("c".into()),
    ];
    for (i, v) in c.iter().enumerate() {
        e.set_cell_value("Sheet1", (i + 1) as u32, 3, v.clone())
            .unwrap();
        e.set_cell_value(
            "Sheet1",
            (i + 1) as u32,
            4,
            LiteralValue::Text(["w", "x", "y", "z"][i].into()),
        )
        .unwrap();
    }
    e.set_cell_formula("Sheet1", 1, 26, parse(formula).unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    e.get_cell_value("Sheet1", 1, 26).unwrap()
}

fn assert_num(formula: &str, expected: f64) {
    match eval(formula) {
        LiteralValue::Number(n) => assert!((n - expected).abs() < 1e-9, "{formula}: {n}"),
        LiteralValue::Int(i) => assert_eq!(i as f64, expected, "{formula}"),
        other => panic!("{formula}: expected {expected}, got {other:?}"),
    }
}

fn assert_text(formula: &str, expected: &str) {
    assert_eq!(
        eval(formula),
        LiteralValue::Text(expected.into()),
        "{formula}"
    );
}

fn assert_na(formula: &str) {
    match eval(formula) {
        LiteralValue::Error(e) => assert_eq!(e.kind, ExcelErrorKind::Na, "{formula}"),
        other => panic!("{formula}: expected #N/A, got {other:?}"),
    }
}

#[test]
fn match_ascending_never_coerces_across_types() {
    assert_na("=MATCH(1,{TRUE;2;3},1)");
    assert_num("=MATCH(\"1\",{1;2;3},1)", 3.0);
    assert_na("=MATCH(1,{\"a\";\"b\"},1)");
    assert_num("=MATCH(TRUE,{1;\"a\";FALSE;TRUE},1)", 4.0);
    assert_num("=MATCH(FALSE,{1;\"a\";TRUE},1)", 2.0);
    assert_num("=MATCH(\"b\",{1;2;\"a\";\"c\"},1)", 3.0);
    assert_num("=MATCH(5,{1;2;\"a\";\"c\"},1)", 2.0);
    assert_num("=MATCH(\"b\",C1:C4,1)", 3.0);
    assert_num("=MATCH(5,C1:C4,1)", 2.0);
}

#[test]
fn match_ascending_binary_search_path_follows_the_same_order() {
    // Eight or more entries take the binary-search path.
    assert_num("=MATCH(\"m\",A1:A10,1)", 10.0);
    assert_num("=MATCH(TRUE,A1:A10,1)", 10.0);
    assert_num("=MATCH(TRUE,{1;2;3;4;5;6;7;8;\"a\";\"b\"},1)", 10.0);
    assert_na("=MATCH(0,{\"a\";\"b\";\"c\";\"d\";\"e\";\"f\";\"g\";\"h\"},1)");
    assert_num("=MATCH(\"1\",A1:A10,1)", 10.0);
}

#[test]
fn match_descending_follows_the_reversed_order() {
    // Descending data: TRUE, FALSE, Z–A, …, 2, 1. Smallest value >= the lookup value.
    assert_num("=MATCH(5,{\"c\";\"b\";\"a\";3;2;1},-1)", 3.0);
    assert_num("=MATCH(\"b\",{TRUE;\"c\";\"b\";\"a\";3},-1)", 3.0);
    assert_na("=MATCH(TRUE,{\"c\";\"b\";3;2},-1)");
    assert_num("=MATCH(\"1\",{TRUE;\"c\";\"b\";3;2},-1)", 3.0);
}

#[test]
fn numeric_approximate_matches_are_unchanged() {
    assert_num("=MATCH(2.5,{1;2;3},1)", 2.0);
    assert_num("=MATCH(7.5,A1:A10,1)", 7.0);
    assert_na("=MATCH(0.5,A1:A10,1)");
    assert_num("=MATCH(2.5,{3;2;1},-1)", 1.0);
    assert_text("=VLOOKUP(2.5,{1,\"a\";2,\"b\";3,\"c\"},2,TRUE)", "b");
}

#[test]
fn vlookup_hlookup_lookup_follow_the_same_order() {
    assert_text("=VLOOKUP(TRUE,{1,\"a\";2,\"b\"},2,TRUE)", "b");
    assert_text("=VLOOKUP(\"1\",{1,\"a\";2,\"b\"},2,TRUE)", "b");
    assert_na("=VLOOKUP(1,{TRUE,\"a\";2,\"b\"},2,TRUE)");
    assert_na("=VLOOKUP(1,{\"a\",1;\"b\",2},2,TRUE)");
    assert_text("=VLOOKUP(\"b\",C1:D4,2,TRUE)", "y");
    assert_text("=VLOOKUP(5,C1:D4,2,TRUE)", "x");
    assert_text("=HLOOKUP(\"x\",{1,2;\"a\",\"b\"},2,TRUE)", "b");
    assert_na("=HLOOKUP(1,{TRUE,2;\"a\",\"b\"},2,TRUE)");
    assert_num("=LOOKUP(\"z\",{1;2;3})", 3.0);
    assert_text("=LOOKUP(TRUE,{1;2;3},{\"a\";\"b\";\"c\"})", "c");
    assert_na("=LOOKUP(1,{\"a\";\"b\"})");
    assert_na("=LOOKUP(1,{TRUE;2;3})");
}

#[test]
fn xlookup_xmatch_approximate_modes_follow_the_same_order() {
    // Binary search (search_mode 2 / -2).
    assert_text("=XLOOKUP(\"1\",{1;2;3},{\"a\";\"b\";\"c\"},,-1,2)", "c");
    assert_text("=XLOOKUP(TRUE,{1;2;3},{\"a\";\"b\";\"c\"},,-1,2)", "c");
    assert_na("=XLOOKUP(1,{TRUE;2;3},{\"a\";\"b\";\"c\"},,-1,2)");
    assert_na("=XLOOKUP(1,{\"a\";\"b\"},{1;2},,-1,2)");
    assert_text("=XLOOKUP(0,{1;2;\"a\"},{\"x\";\"y\";\"z\"},,1,2)", "x");
    assert_na("=XLOOKUP(\"zz\",{1;2;\"a\"},{\"x\";\"y\";\"z\"},,1,2)");
    assert_num("=XMATCH(\"z\",{1;2;3},-1,2)", 3.0);
    assert_num("=XMATCH(5,{3;2;1},-1,-2)", 1.0);
    // Linear search modes compare the same way — search_mode only changes the walk.
    assert_text("=XLOOKUP(\"1\",{1;2;3},{\"a\";\"b\";\"c\"},,-1)", "c");
    assert_na("=XLOOKUP(1,{TRUE;2;3},{\"a\";\"b\";\"c\"},,-1)");
    assert_num("=XMATCH(\"b\",{1;2;\"a\";\"c\"},-1)", 3.0);
    assert_num("=XMATCH(\"b\",{1;2;\"a\";\"c\"},1)", 4.0);
    assert_num("=XMATCH(2.5,{1;2;3},-1)", 2.0);
    assert_num("=XMATCH(2.5,{1;2;3},1)", 3.0);
}
