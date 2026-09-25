//! Excel-parity pins for the `range_lookup` argument of `VLOOKUP` / `HLOOKUP`
//! (rowsncolumns/spreadsheet#939 G-03).
//!
//! Excel: an OMITTED `range_lookup` (`VLOOKUP(v,tbl,c)`) is `TRUE` → approximate match; a slot that is
//! PRESENT but EMPTY (`VLOOKUP(v,tbl,c,)`, or a reference to a blank cell) evaluates to `0` → `FALSE`
//! → exact match. The fork used to invert both ("this engine defaults range_lookup to FALSE") — an
//! omitted flag did an exact match (`#N/A` for a between-keys value) and an empty slot did an
//! approximate one.
//!
//! Seed grid (1-based): A1:A5 = 10,20,30,40,50 · B1:B5 = apple,banana,cherry,date,elderberry ·
//! C1:C5 = 5,3,5,1,2 · F1:G2 = [[1,4],[2,5]] · Z1 blank.

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
        e.set_cell_value(
            "Sheet1",
            row,
            1,
            LiteralValue::Number(((i + 1) * 10) as f64),
        )
        .unwrap();
        e.set_cell_value("Sheet1", row, 2, LiteralValue::Text((*fruit).into()))
            .unwrap();
        e.set_cell_value("Sheet1", row, 3, LiteralValue::Number(c[i]))
            .unwrap();
    }
    for (r, row) in [[1.0, 4.0], [2.0, 5.0]].iter().enumerate() {
        for (k, v) in row.iter().enumerate() {
            e.set_cell_value(
                "Sheet1",
                (r + 1) as u32,
                (6 + k) as u32,
                LiteralValue::Number(*v),
            )
            .unwrap();
        }
    }
    e
}

fn eval_one(formula: &str) -> LiteralValue {
    let mut e = seeded();
    e.set_cell_formula("Sheet1", 1, 11, parse(formula).unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    e.get_cell_value("Sheet1", 1, 11)
        .unwrap_or(LiteralValue::Empty)
}

fn text(s: &str) -> LiteralValue {
    LiteralValue::Text(s.into())
}

fn assert_na(formula: &str) {
    match eval_one(formula) {
        LiteralValue::Error(e) => assert_eq!(e.kind, ExcelErrorKind::Na, "{formula}: {e:?}"),
        other => panic!("{formula}: expected #N/A, got {other:?}"),
    }
}

fn assert_num(formula: &str, expected: f64) {
    match eval_one(formula) {
        LiteralValue::Number(n) => assert_eq!(n, expected, "{formula}"),
        LiteralValue::Int(i) => assert_eq!(i as f64, expected, "{formula}"),
        other => panic!("{formula}: expected {expected}, got {other:?}"),
    }
}

#[test]
fn vlookup_omitted_range_lookup_is_approximate() {
    // 35 is between the 30 and 40 keys: approximate → the largest key ≤ 35 → row 3.
    assert_eq!(eval_one("=VLOOKUP(35,A1:C5,2)"), text("cherry"));
    // Past the last key → last row; below the first key → #N/A (same as Excel).
    assert_eq!(eval_one("=VLOOKUP(99,A1:C5,2)"), text("elderberry"));
    assert_na("=VLOOKUP(5,A1:C5,2)");
    // An exact key still resolves to its own row.
    assert_eq!(eval_one("=VLOOKUP(30,A1:C5,2)"), text("cherry"));
}

#[test]
fn vlookup_empty_range_lookup_slot_is_exact() {
    // `VLOOKUP(v,tbl,c,)` — the fourth slot is present but empty → 0 → FALSE → exact.
    assert_na("=VLOOKUP(35,A1:C5,2,)");
    assert_eq!(eval_one("=VLOOKUP(30,A1:C5,2,)"), text("cherry"));
    // A reference to a blank cell is the same empty value.
    assert_na("=VLOOKUP(35,A1:C5,2,Z1)");
    assert_eq!(eval_one("=VLOOKUP(30,A1:C5,2,Z1)"), text("cherry"));
}

#[test]
fn vlookup_explicit_flags_are_unchanged() {
    assert_eq!(eval_one("=VLOOKUP(35,A1:C5,2,TRUE)"), text("cherry"));
    assert_na("=VLOOKUP(35,A1:C5,2,FALSE)");
    // Numeric coercion: 0 is FALSE, any other number is TRUE.
    assert_na("=VLOOKUP(35,A1:C5,2,0)");
    assert_eq!(eval_one("=VLOOKUP(35,A1:C5,2,1)"), text("cherry"));
}

#[test]
fn hlookup_omitted_range_lookup_is_approximate() {
    // First row of F1:G2 is 1,4: approximate lookup of 2.5 → the 1 column → F2 = 2.
    assert_num("=HLOOKUP(2.5,F1:G2,2)", 2.0);
    assert_num("=HLOOKUP(9,F1:G2,2)", 5.0);
    assert_na("=HLOOKUP(0.5,F1:G2,2)");
}

#[test]
fn hlookup_empty_range_lookup_slot_is_exact() {
    assert_na("=HLOOKUP(2.5,F1:G2,2,)");
    assert_num("=HLOOKUP(4,F1:G2,2,)", 5.0);
    assert_na("=HLOOKUP(2.5,F1:G2,2,Z1)");
    assert_na("=HLOOKUP(2.5,F1:G2,2,0)");
    assert_num("=HLOOKUP(2.5,F1:G2,2,1)", 2.0);
}
