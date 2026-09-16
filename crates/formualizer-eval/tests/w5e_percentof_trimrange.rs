//! Excel-parity pins for `PERCENTOF`, `TRIMRANGE` and `FLOOR`'s zero significance
//! (rowsncolumns/spreadsheet#546, unit W5-E).
//!
//! Seed grid (1-based): A1:A3 = 1,2,3 (A4:A5 blank) · B1:B3 = 10,"x",#N/A · C1:C5 blank ·
//! D2:D3 = 5,6 (D1, D4 blank) · F1 = "" (text), F2 blank · row 8: H8:J8 blank, K8 = 7, L8 = 8,
//! M8:N8 blank.

use formualizer_common::{ExcelError, ExcelErrorKind, LiteralValue};
use formualizer_eval::engine::{Engine, EvalConfig};
use formualizer_eval::test_workbook::TestWorkbook;
use formualizer_parse::parser::parse;

fn seeded() -> Engine<TestWorkbook> {
    let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
    let set = |e: &mut Engine<TestWorkbook>, r: u32, c: u32, v: LiteralValue| {
        e.set_cell_value("Sheet1", r, c, v).unwrap();
    };
    set(&mut e, 1, 1, LiteralValue::Number(1.0));
    set(&mut e, 2, 1, LiteralValue::Number(2.0));
    set(&mut e, 3, 1, LiteralValue::Number(3.0));
    set(&mut e, 1, 2, LiteralValue::Number(10.0));
    set(&mut e, 2, 2, LiteralValue::Text("x".into()));
    set(&mut e, 3, 2, LiteralValue::Error(ExcelError::new_na()));
    set(&mut e, 2, 4, LiteralValue::Number(5.0));
    set(&mut e, 3, 4, LiteralValue::Number(6.0));
    set(&mut e, 1, 6, LiteralValue::Text(String::new()));
    set(&mut e, 8, 11, LiteralValue::Number(7.0));
    set(&mut e, 8, 12, LiteralValue::Number(8.0));
    e
}

/// Evaluate `formula` in P1 and return P1..(P1+rows, +cols) as a matrix (spill-aware).
fn eval_grid(formula: &str, rows: u32, cols: u32) -> Vec<Vec<Option<LiteralValue>>> {
    let mut e = seeded();
    e.set_cell_formula("Sheet1", 1, 16, parse(formula).unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    (0..rows)
        .map(|r| {
            (0..cols)
                .map(|c| e.get_cell_value("Sheet1", 1 + r, 16 + c))
                .collect()
        })
        .collect()
}

fn eval_one(formula: &str) -> LiteralValue {
    eval_grid(formula, 1, 1)[0][0]
        .clone()
        .unwrap_or(LiteralValue::Empty)
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

/// Spilled column values (`None` / `Empty` both read as a blank row).
fn column(formula: &str, rows: u32) -> Vec<Option<f64>> {
    eval_grid(formula, rows, 1)
        .into_iter()
        .map(|row| match &row[0] {
            None | Some(LiteralValue::Empty) => None,
            Some(v) => Some(as_number(v).unwrap_or_else(|| panic!("{formula}: {v:?}"))),
        })
        .collect()
}

fn row_values(formula: &str, cols: u32) -> Vec<Option<f64>> {
    eval_grid(formula, 1, cols)[0]
        .iter()
        .map(|v| match v {
            None | Some(LiteralValue::Empty) => None,
            Some(v) => Some(as_number(v).unwrap_or_else(|| panic!("{formula}: {v:?}"))),
        })
        .collect()
}

// ───────────────────────── PERCENTOF ─────────────────────────

#[test]
fn percentof_is_sum_of_subset_over_sum_of_all() {
    assert_num("=PERCENTOF(A1:A2,A1:A3)", 0.5);
    assert_num("=PERCENTOF(A1:A3,A1:A3)", 1.0);
    assert_num("=PERCENTOF(A3,A1:A3)", 0.5);
    assert_num("=PERCENTOF(1,4)", 0.25);
    assert_num("=PERCENTOF({1,2},{1,2,3,4})", 0.3);
}

#[test]
fn percentof_follows_sum_rules_for_text_blank_and_errors() {
    // B2 = "x" is skipped inside a range; blanks A4:A5 contribute nothing.
    assert_num("=PERCENTOF(B1:B2,A1:A5)", 10.0 / 6.0);
    // An error cell in either argument is the result.
    assert_err("=PERCENTOF(B1:B3,A1:A3)", ExcelErrorKind::Na);
    assert_err("=PERCENTOF(A1:A3,B1:B3)", ExcelErrorKind::Na);
    // A scalar that is not a number is #VALUE!, like SUM("abc").
    assert_err("=PERCENTOF(\"abc\",A1:A3)", ExcelErrorKind::Value);
}

#[test]
fn percentof_whole_of_zero_is_div0() {
    assert_err("=PERCENTOF(1,0)", ExcelErrorKind::Div);
    assert_err("=PERCENTOF(A1:A3,C1:C5)", ExcelErrorKind::Div);
    assert_num("=PERCENTOF(C1:C5,A1:A3)", 0.0);
}

#[test]
fn percentof_arity_is_exactly_two() {
    // Too few arguments is the engine-wide #N/A (W4-A: Excel refuses to enter the formula,
    // the JS engine and Sheets evaluate a stored one to #N/A); too many is #VALUE!.
    assert_err("=PERCENTOF(A1:A3)", ExcelErrorKind::Na);
    assert_err("=PERCENTOF(A1,A2,A3)", ExcelErrorKind::Value);
}

#[test]
fn sum_still_propagates_errors_and_skips_text() {
    // The SUM core is shared with PERCENTOF; keep its own contract pinned.
    assert_num("=SUM(A1:A5)", 6.0);
    assert_num("=SUM(B1:B2)", 10.0);
    assert_err("=SUM(B1:B3)", ExcelErrorKind::Na);
    assert_err("=SUM(\"abc\")", ExcelErrorKind::Value);
    assert_num("=SUM(\"3\",A1)", 4.0);
}

// ───────────────────────── TRIMRANGE ─────────────────────────

#[test]
fn trimrange_trims_both_edges_by_default() {
    assert_eq!(
        column("=TRIMRANGE(A1:A5)", 4),
        vec![Some(1.0), Some(2.0), Some(3.0), None]
    );
    assert_eq!(
        column("=TRIMRANGE(D1:D4)", 3),
        vec![Some(5.0), Some(6.0), None]
    );
    // Whole row with blank columns at both ends: H8:N8 → K8:L8.
    assert_eq!(
        row_values("=TRIMRANGE(H8:N8)", 3),
        vec![Some(7.0), Some(8.0), None]
    );
    // Aggregates see the trimmed reference.
    assert_num("=SUM(TRIMRANGE(A1:A5))", 6.0);
    assert_num("=ROWS(TRIMRANGE(A1:A5))", 3.0);
    assert_num("=COLUMNS(TRIMRANGE(H8:N8))", 2.0);
}

#[test]
fn trimrange_modes_select_leading_trailing_or_none() {
    // 1 = leading only: D1 dropped, D4 kept.
    assert_eq!(
        column("=TRIMRANGE(D1:D4,1)", 4),
        vec![Some(5.0), Some(6.0), None, None]
    );
    // 2 = trailing only: D1 kept, D4 dropped.
    assert_eq!(
        column("=TRIMRANGE(D1:D4,2)", 3),
        vec![None, Some(5.0), Some(6.0)]
    );
    // 0 = keep every row; a skipped slot is the default (3).
    assert_num("=ROWS(TRIMRANGE(D1:D4,0))", 4.0);
    assert_num("=ROWS(TRIMRANGE(D1:D4,,0))", 2.0);
    assert_num("=COLUMNS(TRIMRANGE(H8:N8,3,0))", 7.0);
    assert_num("=COLUMNS(TRIMRANGE(H8:N8,3,1))", 4.0);
    assert_num("=COLUMNS(TRIMRANGE(H8:N8,3,2))", 5.0);
    // Row trimming never depends on the column mode and vice versa.
    assert_num("=ROWS(TRIMRANGE(D1:D4,3,0))", 2.0);
}

#[test]
fn trimrange_blank_cell_mode_coerces_to_zero_not_default() {
    // A blank cell in a numeric slot is 0 in Excel (as for TAKE / ROUND), not "omitted":
    // C1 is blank, so TRIMRANGE(D1:D4,C1) keeps every row, matching N(C1) and a literal 0.
    assert_num("=ROWS(TRIMRANGE(D1:D4,C1))", 4.0);
    assert_num("=ROWS(TRIMRANGE(D1:D4,N(C1)))", 4.0);
    assert_num("=ROWS(TRIMRANGE(D1:D4,0))", 4.0);
    assert_eq!(
        column("=TRIMRANGE(D1:D4,C1)", 4),
        vec![None, Some(5.0), Some(6.0), None]
    );
    // Same for the column slot; the row slot still trims by default.
    assert_num("=COLUMNS(TRIMRANGE(H8:N8,3,C1))", 7.0);
    assert_num("=COLUMNS(TRIMRANGE(H8:N8,,C1))", 7.0);
    // Only a truly skipped slot means the default (3).
    assert_num("=ROWS(TRIMRANGE(D1:D4,,C1))", 2.0);
    // The same blank-cell coercion every other numeric slot already applies.
    assert_num("=ROUND(2.567,C1)", 3.0);
}

#[test]
fn trimrange_only_truly_empty_cells_are_blank() {
    // F1 holds the text "" — not blank — so the row survives.
    assert_num("=ROWS(TRIMRANGE(F1:F3))", 1.0);
    // A 0 is not blank either.
    assert_num("=ROWS(TRIMRANGE({0;0}))", 2.0);
}

#[test]
fn trimrange_errors() {
    // Nothing left after trimming is #REF!, as in Excel.
    assert_err("=TRIMRANGE(C1:C5)", ExcelErrorKind::Ref);
    // Modes outside 0-3 are #VALUE!.
    assert_err("=TRIMRANGE(A1:A5,4)", ExcelErrorKind::Value);
    assert_err("=TRIMRANGE(A1:A5,-1)", ExcelErrorKind::Value);
    assert_err("=TRIMRANGE(A1:A5,3,7)", ExcelErrorKind::Value);
    assert_err("=TRIMRANGE(A1:A5,\"both\")", ExcelErrorKind::Value);
    // Too many arguments.
    assert_err("=TRIMRANGE(A1:A5,3,3,3)", ExcelErrorKind::Value);
    // A single-cell result collapses to a scalar.
    assert_num("=TRIMRANGE(A3:A5)", 3.0);
}

// ───────────────────────── FLOOR zero significance ─────────────────────────

#[test]
fn floor_zero_significance_is_div0_but_precise_and_ceiling_are_zero() {
    assert_err("=FLOOR(12,0)", ExcelErrorKind::Div);
    assert_err("=FLOOR(-12,0)", ExcelErrorKind::Div);
    assert_num("=FLOOR(0,0)", 0.0);
    assert_num("=FLOOR.PRECISE(12,0)", 0.0);
    assert_num("=CEILING(12,0)", 0.0);
    // Excel 2010+ sign rules (corpus expectation `#NUM!` for FLOOR(-2.5,2) was Excel 2007).
    assert_num("=FLOOR(-2.5,2)", -4.0);
    assert_num("=FLOOR(-2.5,-2)", -2.0);
    assert_err("=FLOOR(2.5,-2)", ExcelErrorKind::Num);
}
