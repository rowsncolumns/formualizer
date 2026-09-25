//! Excel-parity pins for `CHISQ.TEST` / `CHITEST` degrees of freedom (rowsncolumns/spreadsheet#939
//! G-06).
//!
//! Excel derives df from the SHAPE of `actual_range`: `(r−1)(c−1)` when both r > 1 and c > 1, `c−1`
//! for a single row and `r−1` for a single column. The fork used `n−1` for every shape, so a 2×2
//! contingency table was tested with df = 3 instead of 1 (`CHISQ.TEST({10,20;30,40},{15,15;35,35})`
//! → 0.190085 instead of Excel's 0.029096).
//!
//! Seed grid (1-based): A1:B2 = [[10,20],[30,40]] · C1:D2 = [[15,15],[35,35]] ·
//! A4:B6 = [[10,20],[30,40],[50,60]] · C4:D6 = [[15,15],[35,35],[55,55]].

use formualizer_common::{ExcelErrorKind, LiteralValue};
use formualizer_eval::engine::{Engine, EvalConfig};
use formualizer_eval::test_workbook::TestWorkbook;
use formualizer_parse::parser::parse;

fn seeded() -> Engine<TestWorkbook> {
    let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
    let put = |e: &mut Engine<TestWorkbook>, row: u32, col: u32, v: f64| {
        e.set_cell_value("Sheet1", row, col, LiteralValue::Number(v))
            .unwrap();
    };
    for (r, row) in [[10.0, 20.0, 15.0, 15.0], [30.0, 40.0, 35.0, 35.0]]
        .iter()
        .enumerate()
    {
        for (c, v) in row.iter().enumerate() {
            put(&mut e, (r + 1) as u32, (c + 1) as u32, *v);
        }
    }
    for (r, row) in [
        [10.0, 20.0, 15.0, 15.0],
        [30.0, 40.0, 35.0, 35.0],
        [50.0, 60.0, 55.0, 55.0],
    ]
    .iter()
    .enumerate()
    {
        for (c, v) in row.iter().enumerate() {
            put(&mut e, (r + 4) as u32, (c + 1) as u32, *v);
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

fn assert_approx(formula: &str, expected: f64) {
    match eval_one(formula) {
        LiteralValue::Number(n) => assert!(
            (n - expected).abs() < 1e-6,
            "{formula}: expected {expected}, got {n}"
        ),
        other => panic!("{formula}: expected {expected}, got {other:?}"),
    }
}

// chi² = 2·(25/15) + 2·(25/35) = 4.7619; df = (2−1)(2−1) = 1 → p = 0.029096.
const P_2X2: f64 = 0.029096331741252146;
// chi² = 2·(25/15) + 2·(25/35) + 2·(25/55) = 5.6710; df = (3−1)(2−1) = 2 → p = e^(−chi²/2) = 0.058689.
const P_3X2: f64 = 0.058689;

#[test]
fn two_by_two_table_uses_one_degree_of_freedom() {
    assert_approx("=CHISQ.TEST({10,20;30,40},{15,15;35,35})", P_2X2);
    assert_approx("=CHISQ.TEST(A1:B2,C1:D2)", P_2X2);
}

#[test]
fn chitest_alias_matches() {
    assert_approx("=CHITEST({10,20;30,40},{15,15;35,35})", P_2X2);
    assert_approx("=CHITEST(A1:B2,C1:D2)", P_2X2);
}

#[test]
fn three_by_two_table_uses_two_degrees_of_freedom() {
    assert_approx(
        "=CHISQ.TEST({10,20;30,40;50,60},{15,15;35,35;55,55})",
        P_3X2,
    );
    assert_approx("=CHISQ.TEST(A4:B6,C4:D6)", P_3X2);
    // Transposed (2×3) is the same test: (2−1)(3−1) = 2.
    assert_approx(
        "=CHISQ.TEST({10,30,50;20,40,60},{15,35,55;15,35,55})",
        P_3X2,
    );
}

#[test]
fn single_row_and_single_column_keep_k_minus_one() {
    // chi² = 5 + 0 + 5 = 10, df = 2 → p = e^(−5) = 0.006738.
    let p = (-5.0f64).exp();
    assert_approx("=CHISQ.TEST({10,20,30},{20,20,20})", p);
    assert_approx("=CHISQ.TEST({10;20;30},{20;20;20})", p);
    // Two categories: df = 1 (unchanged by the fix).
    assert_approx("=CHISQ.TEST({18,22},{20,20})", 0.5270892568655381);
}

#[test]
fn mismatched_sizes_are_na() {
    match eval_one("=CHISQ.TEST({10,20},{15,15,15})") {
        LiteralValue::Error(e) => assert_eq!(e.kind, ExcelErrorKind::Na),
        other => panic!("expected #N/A, got {other:?}"),
    }
}
