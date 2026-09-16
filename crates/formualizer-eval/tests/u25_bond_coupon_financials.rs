//! rowsncolumns/spreadsheet#546 U25 (A-18, A-20, D-F18 P3): the rest of the Excel securities
//! family on top of U14's coupon schedule.
//!
//! - `ACCRINT` accrued `par × rate × YEARFRAC(issue, settlement)` and, for `calc_method = FALSE`,
//!   from the previous coupon; Excel accrues per quasi-coupon period anchored on `first_interest`
//!   (`Σ A_i / NL_i`) and `calc_method = FALSE` starts at `first_interest`.
//! - The actual/actual year fraction prorated across calendar years; Excel's YEARFRAC divides a
//!   span of one year or less by 365/366 (366 when it straddles a Feb 29) and a longer span by the
//!   average year length — this feeds ACCRINTM, DISC, PRICEDISC, YIELDDISC, INTRATE, RECEIVED,
//!   PRICEMAT, YIELDMAT and the AMOR* functions.
//! - `DISC`, `ODDFPRICE` / `ODDFYIELD` / `ODDLPRICE` / `ODDLYIELD` and `AMORLINC` / `AMORDEGRC`
//!   were `#NAME?`.
//!
//! Expected values are the worked examples from Microsoft's function reference (rounded there to
//! 5–8 significant digits, hence the per-case tolerances). The JS engine pins the same cases in
//! `fast-formula-parser/test/bond-securities-parity.spec.js`.

use formualizer_common::{ExcelErrorKind, LiteralValue};
use formualizer_eval::engine::{Engine, EvalConfig};
use formualizer_eval::test_workbook::TestWorkbook;
use formualizer_parse::parser::parse;

fn eval(formula: &str) -> LiteralValue {
    let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
    e.set_cell_formula("Sheet1", 1, 1, parse(formula).unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    e.get_cell_value("Sheet1", 1, 1).expect("value")
}

fn num(formula: &str) -> f64 {
    match eval(formula) {
        LiteralValue::Number(n) => n,
        LiteralValue::Int(i) => i as f64,
        other => panic!("{formula}: expected a number, got {other:?}"),
    }
}

fn assert_close(formula: &str, expected: f64, tolerance: f64) {
    let actual = num(formula);
    assert!(
        (actual - expected).abs() <= tolerance,
        "{formula}: expected {expected}, got {actual}"
    );
}

fn assert_num_error(formula: &str) {
    match eval(formula) {
        LiteralValue::Error(e) => assert_eq!(e.kind, ExcelErrorKind::Num, "{formula}"),
        other => panic!("{formula}: expected #NUM!, got {other:?}"),
    }
}

#[test]
fn accrint_accrues_per_quasi_coupon_period_from_issue() {
    // Microsoft reference: 60 30/360 days of a 10% semiannual coupon on 1000.
    assert_close(
        "=ACCRINT(DATE(2008,3,1), DATE(2008,8,31), DATE(2008,5,1), 0.1, 1000, 2, 0)",
        16.666667,
        5e-7,
    );
    // Microsoft reference: calc_method FALSE with settlement before first_interest still starts at issue.
    assert_close(
        "=ACCRINT(DATE(2008,3,5), DATE(2008,8,31), DATE(2008,5,1), 0.1, 1000, 2, 0, FALSE)",
        15.555556,
        5e-7,
    );
    // Actual/actual: 61 of the 184 days in the 29-Feb → 31-Aug-2008 quasi period, not 61/366.
    assert_close(
        "=ACCRINT(DATE(2008,3,1), DATE(2008,8,31), DATE(2008,5,1), 0.1, 1000, 2, 1)",
        50.0 * (61.0 / 184.0),
        1e-9,
    );
    // A blank basis / calc_method slot takes the default.
    assert_close(
        "=ACCRINT(DATE(2008,3,1), DATE(2008,8,31), DATE(2008,5,1), 0.1, 1000, 2,)",
        16.666667,
        5e-7,
    );
}

#[test]
fn accrint_calc_method_false_starts_at_first_interest_once_settlement_passes_it() {
    assert_close(
        "=ACCRINT(DATE(2008,1,1), DATE(2008,7,1), DATE(2008,10,1), 0.08, 1000, 2, 0, FALSE)",
        20.0,
        1e-9,
    );
    assert_close(
        "=ACCRINT(DATE(2008,1,1), DATE(2008,7,1), DATE(2008,10,1), 0.08, 1000, 2, 0)",
        60.0,
        1e-9,
    );
    // Two coupons past first_interest: FALSE still measures from first_interest (Excel's doc).
    assert_close(
        "=ACCRINT(DATE(2008,1,1), DATE(2008,7,1), DATE(2009,4,1), 0.08, 1000, 2, 0, FALSE)",
        60.0,
        1e-9,
    );
    assert_num_error("=ACCRINT(DATE(2008,5,1), DATE(2008,8,31), DATE(2008,3,1), 0.1, 1000, 2, 0)");
}

#[test]
fn actual_actual_year_fraction_follows_excel_yearfrac() {
    // ACCRINTM on a ≤ 1-year span that straddles Feb 29 2008 divides by 366.
    assert_close(
        "=ACCRINTM(DATE(2007,10,1), DATE(2008,4,1), 0.1, 1000, 1)",
        1000.0 * 0.1 * 183.0 / 366.0,
        1e-9,
    );
    // …and by 365 when it does not.
    assert_close(
        "=ACCRINTM(DATE(2022,10,1), DATE(2023,4,1), 0.1, 1000, 1)",
        1000.0 * 0.1 * 182.0 / 365.0,
        1e-9,
    );
    // A span longer than a year uses the average length of the years touched (2008 is leap).
    assert_close(
        "=ACCRINTM(DATE(2008,1,1), DATE(2010,1,1), 0.1, 1000, 1)",
        1000.0 * 0.1 * 731.0 / (1096.0 / 3.0),
        1e-9,
    );
    // Same-year discount security on actual/actual: Microsoft reference for DISC.
    assert_close(
        "=DISC(DATE(2007,1,25), DATE(2007,6,15), 97.975, 100, 1)",
        0.052420213,
        5e-10,
    );
    assert_close(
        "=ACCRINTM(DATE(2008,4,1), DATE(2008,6,15), 0.1, 1000, 3)",
        20.54795,
        5e-6,
    );
}

#[test]
fn disc_counts_days_per_basis() {
    // 28-Feb-2023 → 31-Aug-2023: NASD 180 days, European 182, actual 184.
    assert_close(
        "=DISC(DATE(2023,2,28), DATE(2023,8,31), 97, 100, 0)",
        0.03 / (180.0 / 360.0),
        1e-12,
    );
    assert_close(
        "=DISC(DATE(2023,2,28), DATE(2023,8,31), 97, 100, 4)",
        0.03 / (182.0 / 360.0),
        1e-12,
    );
    assert_close(
        "=DISC(DATE(2023,2,28), DATE(2023,8,31), 97, 100, 2)",
        0.03 / (184.0 / 360.0),
        1e-12,
    );
    assert_close(
        "=DISC(DATE(2023,2,28), DATE(2023,8,31), 97, 100)",
        0.03 / (180.0 / 360.0),
        1e-12,
    );
    assert_num_error("=DISC(DATE(2007,1,25), DATE(2007,6,15), 0, 100, 1)");
    assert_num_error("=DISC(DATE(2007,6,15), DATE(2007,1,25), 97.975, 100, 1)");
    assert_num_error("=DISC(DATE(2007,1,25), DATE(2007,6,15), 97.975, 100, 5)");
}

#[test]
fn odd_coupon_functions_match_the_microsoft_reference() {
    assert_close(
        "=ODDFPRICE(DATE(2008,11,11), DATE(2021,3,1), DATE(2008,10,15), DATE(2009,3,1), 0.0785, 0.0625, 100, 2, 1)",
        113.5977,
        5e-5,
    );
    assert_close(
        "=ODDFYIELD(DATE(2008,11,11), DATE(2021,3,1), DATE(2008,10,15), DATE(2009,3,1), 0.0575, 84.5, 100, 2, 0)",
        0.07725,
        5e-6,
    );
    assert_close(
        "=ODDLPRICE(DATE(2008,2,7), DATE(2008,6,15), DATE(2007,10,15), 0.0375, 0.0405, 100, 2, 0)",
        99.87829,
        5e-6,
    );
    assert_close(
        "=ODDLYIELD(DATE(2008,4,20), DATE(2008,6,15), DATE(2007,12,24), 0.0375, 99.875, 100, 2, 0)",
        0.04519,
        5e-6,
    );
    // Basis defaults to 0 when omitted or blank.
    assert_close(
        "=ODDLPRICE(DATE(2008,2,7), DATE(2008,6,15), DATE(2007,10,15), 0.0375, 0.0405, 100, 2)",
        99.87829,
        5e-6,
    );
    assert_close(
        "=ODDLPRICE(DATE(2008,2,7), DATE(2008,6,15), DATE(2007,10,15), 0.0375, 0.0405, 100, 2,)",
        99.87829,
        5e-6,
    );
}

#[test]
fn odd_coupon_price_and_yield_are_inverses_including_a_long_first_period() {
    let first = num(
        "=ODDFPRICE(DATE(2008,11,11), DATE(2021,3,1), DATE(2008,10,15), DATE(2009,3,1), 0.0785, 0.0625, 100, 2, 1)",
    );
    assert_close(
        &format!(
            "=ODDFYIELD(DATE(2008,11,11), DATE(2021,3,1), DATE(2008,10,15), DATE(2009,3,1), 0.0785, {first}, 100, 2, 1)"
        ),
        0.0625,
        1e-10,
    );
    let last = num(
        "=ODDLPRICE(DATE(2008,4,20), DATE(2008,6,15), DATE(2007,12,24), 0.0375, 0.045, 100, 2, 3)",
    );
    assert_close(
        &format!(
            "=ODDLYIELD(DATE(2008,4,20), DATE(2008,6,15), DATE(2007,12,24), 0.0375, {last}, 100, 2, 3)"
        ),
        0.045,
        1e-10,
    );
    // Long odd first period (first coupon ≈16.5 months after issue): hand-expanded Excel formula
    // on 30/360 — 23 coupons from 1-Mar-2010, the first worth DFC/E = 496/180 regular coupons,
    // discounted from DSC/E = 470/180 periods out, less A/E = 26/180 of a coupon accrued.
    let bond = "DATE(2008,11,11), DATE(2021,3,1), DATE(2008,10,15), DATE(2010,3,1), 0.0785";
    let c = 100.0 * 0.0785 / 2.0;
    let f: f64 = 1.03125;
    let mut expected =
        100.0 / f.powf(22.0 + 470.0 / 180.0) + c * (496.0 / 180.0) / f.powf(470.0 / 180.0);
    for k in 2..=23 {
        expected += c / f.powf((k - 1) as f64 + 470.0 / 180.0);
    }
    expected -= c * (26.0 / 180.0);
    assert_close(
        &format!("=ODDFPRICE({bond}, 0.0625, 100, 2, 0)"),
        expected,
        1e-9,
    );
    let price_actual = num(&format!("=ODDFPRICE({bond}, 0.0625, 100, 2, 1)"));
    assert!(
        (price_actual - expected).abs() < 0.5,
        "basis 1 = {price_actual}, basis 0 = {expected}"
    );
    assert_close(
        &format!("=ODDFYIELD({bond}, {price_actual}, 100, 2, 1)"),
        0.0625,
        1e-10,
    );
}

#[test]
fn odd_coupon_functions_enforce_excel_date_ordering_and_guards() {
    assert_num_error(
        "=ODDFPRICE(DATE(2008,10,1), DATE(2021,3,1), DATE(2008,10,15), DATE(2009,3,1), 0.0785, 0.0625, 100, 2, 1)",
    );
    assert_num_error(
        "=ODDFYIELD(DATE(2009,3,1), DATE(2021,3,1), DATE(2008,10,15), DATE(2009,3,1), 0.0575, 84.5, 100, 2, 0)",
    );
    assert_num_error(
        "=ODDLPRICE(DATE(2007,10,1), DATE(2008,6,15), DATE(2007,10,15), 0.0375, 0.0405, 100, 2, 0)",
    );
    assert_num_error(
        "=ODDLYIELD(DATE(2008,4,20), DATE(2008,6,15), DATE(2007,12,24), 0.0375, 0, 100, 2, 0)",
    );
    assert_num_error(
        "=ODDLPRICE(DATE(2008,2,7), DATE(2008,6,15), DATE(2007,10,15), 0.0375, 0.0405, 100, 3, 0)",
    );
    assert_num_error(
        "=ODDFPRICE(DATE(2008,11,11), DATE(2021,3,1), DATE(2008,10,15), DATE(2009,3,1), -0.01, 0.0625, 100, 2, 1)",
    );
}

#[test]
fn amorlinc_and_amordegrc_match_the_microsoft_reference() {
    assert_close(
        "=AMORLINC(2400, DATE(2008,8,19), DATE(2008,12,31), 300, 1, 0.15, 1)",
        360.0,
        1e-9,
    );
    assert_eq!(
        num("=AMORDEGRC(2400, DATE(2008,8,19), DATE(2008,12,31), 300, 1, 0.15, 1)"),
        776.0
    );
    // Period 0 is prorated (134 of 366 days); the schedule stops at the salvage value.
    let first = 134.0 / 366.0 * 0.15 * 2400.0;
    assert_close(
        "=AMORLINC(2400, DATE(2008,8,19), DATE(2008,12,31), 300, 0, 0.15, 1)",
        first,
        1e-9,
    );
    assert_close(
        "=AMORLINC(2400, DATE(2008,8,19), DATE(2008,12,31), 300, 5, 0.15, 1)",
        360.0,
        1e-9,
    );
    assert_close(
        "=AMORLINC(2400, DATE(2008,8,19), DATE(2008,12,31), 300, 6, 0.15, 1)",
        2100.0 - 5.0 * 360.0 - first,
        1e-9,
    );
    assert_eq!(
        num("=AMORLINC(2400, DATE(2008,8,19), DATE(2008,12,31), 300, 7, 0.15, 1)"),
        0.0
    );
    assert_eq!(
        num("=AMORDEGRC(2400, DATE(2008,8,19), DATE(2008,12,31), 300, 0, 0.15, 1)"),
        330.0
    );
    let tail: Vec<f64> = (2..8)
        .map(|p| {
            num(&format!(
                "=AMORDEGRC(2400, DATE(2008,8,19), DATE(2008,12,31), 300, {p}, 0.15, 1)"
            ))
        })
        .collect();
    assert!(tail.iter().all(|v| *v >= 0.0), "{tail:?}");
    assert_eq!(*tail.last().unwrap(), 0.0);
    // Basis defaults to 0 when omitted; basis 2 is rejected, as are inverted / negative inputs.
    assert_close(
        "=AMORLINC(2400, DATE(2008,8,19), DATE(2008,12,31), 300, 1, 0.15)",
        360.0,
        1e-9,
    );
    assert_num_error("=AMORLINC(2400, DATE(2008,8,19), DATE(2008,12,31), 300, 1, 0.15, 2)");
    assert_num_error("=AMORDEGRC(2400, DATE(2008,8,19), DATE(2008,12,31), 300, 1, 0.15, 2)");
    assert_num_error("=AMORLINC(2400, DATE(2008,8,19), DATE(2008,12,31), 2500, 1, 0.15, 1)");
    assert_num_error("=AMORDEGRC(2400, DATE(2008,12,31), DATE(2008,8,19), 300, 1, 0.15, 1)");
}
