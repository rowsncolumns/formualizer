//! Excel-parity pins for error-kind and edge nits (rowsncolumns/spreadsheet#546, unit U24b):
//! `0^0` / `0^-n` domains, FLOOR's negative-significance rules, `TEXT` of a boolean, `ERROR.TYPE`
//! codes for `#SPILL!` / `#CALC!`, `SEQUENCE(0)`, `SORT` order validation, `XLOOKUP` size
//! validation, array `index_num` in `CHOOSE`, 15-significant-digit numeric literals, and a
//! `[#Totals]` structured ref on a table without a totals row.
//!
//! Seed grid (1-based): A1:A5 = 10,20,30,40,50 · B1:B5 = apple,banana,cherry,date,elderberry ·
//! C1:C5 = 5,3,5,1,2 · F1:G2 = [[1,4],[2,5]] · table "Sales" over M1:N3 (header row, no totals):
//! Region/Amount, (N,10), (S,20).

use formualizer_common::{ExcelErrorKind, LiteralValue};
use formualizer_eval::engine::{Engine, EvalConfig};
use formualizer_eval::reference::{CellRef, Coord, RangeRef};
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
    // Table "Sales" at M1:N3 — header row + 2 data rows, NO totals row.
    for (r, c, v) in [
        (1, 13, LiteralValue::Text("Region".into())),
        (1, 14, LiteralValue::Text("Amount".into())),
        (2, 13, LiteralValue::Text("N".into())),
        (2, 14, LiteralValue::Number(10.0)),
        (3, 13, LiteralValue::Text("S".into())),
        (3, 14, LiteralValue::Number(20.0)),
    ] {
        e.set_cell_value("Sheet1", r, c, v).unwrap();
    }
    let sid = e.sheet_id("Sheet1").unwrap();
    e.define_table(
        "Sales",
        RangeRef::new(
            CellRef::new(sid, Coord::from_excel(1, 13, true, true)),
            CellRef::new(sid, Coord::from_excel(3, 14, true, true)),
        ),
        true,
        vec!["Region".into(), "Amount".into()],
        false,
    )
    .unwrap();
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

fn as_number(v: &LiteralValue) -> Option<f64> {
    match v {
        LiteralValue::Number(n) => Some(*n),
        LiteralValue::Int(i) => Some(*i as f64),
        _ => None,
    }
}

fn assert_num(formula: &str, expected: f64) {
    let v = eval_one(formula);
    let got = as_number(&v).unwrap_or_else(|| panic!("{formula}: expected number, got {v:?}"));
    assert!((got - expected).abs() < 1e-9, "{formula}: expected {expected}, got {got}");
}

fn assert_err(formula: &str, kind: ExcelErrorKind) {
    let v = eval_one(formula);
    assert!(
        matches!(&v, LiteralValue::Error(e) if e.kind == kind),
        "{formula}: expected {kind:?}, got {v:?}"
    );
}

// ───────────────────────── B-30: 0^0, FLOOR sign rules, TEXT(bool) ─────────────────────────

#[test]
fn zero_base_power_domain_matches_excel() {
    assert_err("=0^0", ExcelErrorKind::Num);
    assert_err("=POWER(0,0)", ExcelErrorKind::Num);
    assert_err("=0^-1", ExcelErrorKind::Div);
    assert_err("=POWER(0,-2)", ExcelErrorKind::Div);
    assert_num("=0^2", 0.0);
    assert_num("=2^0", 1.0);
    assert_num("=POWER(2,10)", 1024.0);
    // Negative base with a fractional exponent stays #NUM! (Excel does not take real cube roots).
    assert_err("=POWER(-8,1/3)", ExcelErrorKind::Num);
    assert_err("=(-8)^(1/3)", ExcelErrorKind::Num);
}

#[test]
fn floor_negative_significance_follows_excel_2010_rules() {
    // Negative number, positive significance: rounds away from zero.
    assert_num("=FLOOR(-2.5,2)", -4.0);
    assert_num("=FLOOR(-5.9,2)", -6.0);
    // Positive number, negative significance: #NUM!.
    assert_err("=FLOOR(2.5,-2)", ExcelErrorKind::Num);
    // Both negative: rounds toward zero.
    assert_num("=FLOOR(-2.5,-2)", -2.0);
    assert_num("=FLOOR(5.9,2)", 4.0);
    assert_num("=FLOOR(1.58,0.1)", 1.5);
    assert_err("=FLOOR(2.5,0)", ExcelErrorKind::Div);
}

#[test]
fn text_of_a_boolean_is_the_boolean_word() {
    assert_eq!(eval_one("=TEXT(TRUE,\"0\")"), text("TRUE"));
    assert_eq!(eval_one("=TEXT(FALSE,\"0.00\")"), text("FALSE"));
    assert_eq!(eval_one("=TEXT(1,\"0\")"), text("1"));
}

// ───────────────────────── E-40: ERROR.TYPE codes ─────────────────────────

#[test]
fn error_type_codes_match_excel() {
    assert_num("=ERROR.TYPE(#NULL!)", 1.0);
    assert_num("=ERROR.TYPE(1/0)", 2.0);
    assert_num("=ERROR.TYPE(#VALUE!)", 3.0);
    assert_num("=ERROR.TYPE(#REF!)", 4.0);
    assert_num("=ERROR.TYPE(#NAME?)", 5.0);
    assert_num("=ERROR.TYPE(#NUM!)", 6.0);
    assert_num("=ERROR.TYPE(#N/A)", 7.0);
    assert_num("=ERROR.TYPE(#SPILL!)", 9.0);
    assert_num("=ERROR.TYPE(#CALC!)", 14.0);
    assert_num("=ERROR.TYPE(SEQUENCE(0))", 14.0);
    assert_err("=ERROR.TYPE(10)", ExcelErrorKind::Na);
}

// ───────────────────────── C-R20: SEQUENCE(0), SORT order validation ─────────────────────────

#[test]
fn sequence_zero_is_calc_and_negative_is_value() {
    assert_err("=SEQUENCE(0)", ExcelErrorKind::Calc);
    assert_err("=SEQUENCE(3,0)", ExcelErrorKind::Calc);
    assert_err("=SEQUENCE(-1)", ExcelErrorKind::Value);
    assert_err("=SEQUENCE(2,-3)", ExcelErrorKind::Value);
    let g = eval_grid("=SEQUENCE(2,2)", 2, 2);
    assert_eq!(g[1][1].as_ref().and_then(as_number), Some(4.0));
}

#[test]
fn sort_order_other_than_plus_minus_one_is_value_error() {
    assert_err("=SORT(A1:A5,1,2)", ExcelErrorKind::Value);
    assert_err("=SORT(A1:A5,1,0)", ExcelErrorKind::Value);
    let desc = eval_grid("=SORT(A1:A5,1,-1)", 5, 1);
    assert_eq!(desc[0][0].as_ref().and_then(as_number), Some(50.0));
    let asc = eval_grid("=SORT(A1:A5,1,1)", 5, 1);
    assert_eq!(asc[0][0].as_ref().and_then(as_number), Some(10.0));
}

// ───────────────────────── C-R18: XLOOKUP array size validation ─────────────────────────

#[test]
fn xlookup_return_array_must_match_lookup_length() {
    assert_err("=XLOOKUP(30,A1:A5,B1:B3)", ExcelErrorKind::Value);
    assert_err("=XLOOKUP(30,A1:A5,B1:C4)", ExcelErrorKind::Value);
    assert_err("=XLOOKUP(4,F1:G1,F2:F2)", ExcelErrorKind::Value);
    assert_eq!(eval_one("=XLOOKUP(30,A1:A5,B1:B5)"), text("cherry"));
    // Multi-column return of the right height spills the matched row.
    let g = eval_grid("=XLOOKUP(30,A1:A5,B1:C5)", 1, 2);
    assert_eq!(g[0][0], Some(text("cherry")));
    assert_eq!(g[0][1].as_ref().and_then(as_number), Some(5.0));
    // Horizontal lookup with a matching width.
    assert_num("=XLOOKUP(4,F1:G1,F2:G2)", 5.0);
    // Array-literal arguments are sized by their values.
    assert_eq!(eval_one("=XLOOKUP(2,{1,2,3},{\"x\",\"y\",\"z\"})"), text("y"));
    assert_err("=XLOOKUP(2,{1,2,3},{\"x\",\"y\"})", ExcelErrorKind::Value);
}

#[test]
fn xlookup_whole_column_references_are_not_size_checked() {
    // `A:A` / `B:B` views are trimmed to the used region (which can differ per column) — the
    // declared extents are equal, so the size rule must not fire.
    assert_eq!(eval_one("=XLOOKUP(30,A:A,B:B)"), text("cherry"));
    assert_num("=XLOOKUP(40,A:A,C:C)", 1.0);
}

// ───────────────────────── C-R19: CHOOSE with an array index ─────────────────────────

#[test]
fn choose_array_index_spills_element_wise() {
    let g = eval_grid("=CHOOSE({1,3},\"a\",\"b\",\"c\")", 1, 2);
    assert_eq!(g[0][0], Some(text("a")));
    assert_eq!(g[0][1], Some(text("c")));
    let v = eval_grid("=CHOOSE({2;1},\"a\",\"b\")", 2, 1);
    assert_eq!(v[0][0], Some(text("b")));
    assert_eq!(v[1][0], Some(text("a")));
    // Out-of-range picks are #VALUE! per element; scalar CHOOSE is unchanged.
    let bad = eval_grid("=CHOOSE({1,4},\"a\",\"b\")", 1, 2);
    assert_eq!(bad[0][0], Some(text("a")));
    assert!(matches!(&bad[0][1], Some(LiteralValue::Error(e)) if e.kind == ExcelErrorKind::Value));
    assert_eq!(eval_one("=CHOOSE(2,\"a\",\"b\",\"c\")"), text("b"));
    assert_num("=SUM(CHOOSE({1,2},A1,A2))", 30.0);
}

// ───────────────────────── E-46: 15 significant digits in numeric literals ─────────────────────────

#[test]
fn numeric_literals_keep_fifteen_significant_digits() {
    assert_eq!(eval_one("=123456789012345678"), num(123456789012345000.0));
    assert_eq!(eval_one("=1234567890.123456789"), num(1234567890.12346));
    // Literals Excel can already represent are bit-identical.
    assert_eq!(eval_one("=0.1"), num(0.1));
    assert_eq!(eval_one("=1E-300"), num(1e-300));
    assert_eq!(eval_one("=0.1+0.2"), num(0.1 + 0.2));
    assert_eq!(eval_one("=-2.5"), num(-2.5));
}

// ───────────────────────── E-34: [#Totals] without a totals row ─────────────────────────

#[test]
fn totals_specifier_on_a_table_without_totals_row_is_ref_error() {
    assert_err("=SUM(Sales[#Totals])", ExcelErrorKind::Ref);
    assert_err("=Sales[[#Totals],[Amount]]", ExcelErrorKind::Ref);
    assert_num("=SUM(Sales[Amount])", 30.0);
}

// ───────────── E-33: bare `[@Col]` outside any table ─────────────

#[test]
fn bare_this_row_reference_outside_any_table_is_name_error() {
    let mut e = seeded();
    let res = e.set_cell_formula("Sheet1", 10, 11, parse("=[@Amount]").unwrap());
    match res {
        Err(err) => assert_eq!(err.kind, ExcelErrorKind::Name, "ingest rejects with #NAME?"),
        Ok(()) => {
            e.evaluate_all().unwrap();
            let v = e.get_cell_value("Sheet1", 10, 11).unwrap_or(LiteralValue::Empty);
            assert!(
                matches!(&v, LiteralValue::Error(er) if er.kind == ExcelErrorKind::Name),
                "installed formula must evaluate to #NAME?, got {v:?}"
            );
        }
    }
    // A named this-row ref outside the table's rows is #VALUE! (E-33 second form).
    let res = e.set_cell_formula("Sheet1", 10, 12, parse("=Sales[@Amount]").unwrap());
    match res {
        Err(err) => assert_eq!(err.kind, ExcelErrorKind::Value),
        Ok(()) => {
            e.evaluate_all().unwrap();
            let v = e.get_cell_value("Sheet1", 10, 12).unwrap_or(LiteralValue::Empty);
            assert!(
                matches!(&v, LiteralValue::Error(er) if er.kind == ExcelErrorKind::Value),
                "installed formula must evaluate to #VALUE!, got {v:?}"
            );
        }
    }
}
