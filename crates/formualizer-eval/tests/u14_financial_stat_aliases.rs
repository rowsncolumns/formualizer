//! rowsncolumns/spreadsheet#546 (U14) — financial functions and Excel 2007 statistical
//! compatibility names: `CUMIPMT` / `CUMPRINC` in Excel's sign convention, `DB`'s partial last
//! period, `VDB` / `FVSCHEDULE`, the `COUP*` coupon-schedule family, `DURATION` / `MDURATION`,
//! the discounted-security functions, and the legacy distribution aliases.
//!
//! Expected values are Excel's documented function-reference examples unless noted.

use formualizer_common::{ExcelErrorKind, LiteralValue};
use formualizer_eval::engine::{Engine, EvalConfig};
use formualizer_eval::test_workbook::TestWorkbook;
use formualizer_parse::parser::parse;

fn eval(formula: &str) -> LiteralValue {
    let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
    e.set_cell_formula("Sheet1", 1, 5, parse(formula).unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    e.get_cell_value("Sheet1", 1, 5)
        .unwrap_or(LiteralValue::Empty)
}

fn num(formula: &str) -> f64 {
    match eval(formula) {
        LiteralValue::Number(n) => n,
        LiteralValue::Int(i) => i as f64,
        other => panic!("{formula}: expected number, got {other:?}"),
    }
}

fn assert_close(formula: &str, expected: f64, tol: f64) {
    let got = num(formula);
    assert!(
        (got - expected).abs() <= tol,
        "{formula}: expected {expected}, got {got}"
    );
}

fn assert_num_error(formula: &str) {
    match eval(formula) {
        LiteralValue::Error(e) => assert_eq!(e.kind, ExcelErrorKind::Num, "{formula}"),
        other => panic!("{formula}: expected #NUM!, got {other:?}"),
    }
}

// ── CUMIPMT / CUMPRINC (C-R14) ──────────────────────────────────────────────────────────────

#[test]
fn cumipmt_is_negative_for_a_loan() {
    assert_close("=CUMIPMT(0.1,3,100,1,1,0)", -10.0, 1e-9);
    assert_close("=CUMIPMT(0.09/12,30*12,125000,13,24,0)", -11135.23213, 1e-4);
    assert_close("=CUMIPMT(0.09/12,30*12,125000,1,1,0)", -937.5, 1e-9);
    assert_close("=CUMIPMT(0.05/12,360,200000,1,12,0)", -9932.99, 0.01);
}

#[test]
fn cumprinc_accumulates_principal_not_interest() {
    assert_close("=CUMPRINC(0.1,3,100,1,3,0)", -100.0, 1e-9);
    assert_close(
        "=CUMPRINC(0.09/12,30*12,125000,13,24,0)",
        -934.1071234,
        1e-4,
    );
    assert_close("=CUMPRINC(0.09/12,30*12,125000,1,1,0)", -68.27827118, 1e-6);
    assert_close("=CUMPRINC(0.05/12,360,200000,1,12,0)", -2950.73, 0.01);
}

#[test]
fn cumulative_windows_reconcile_with_pmt() {
    // Interest + principal over the whole life equals nper × PMT, for both payment timings.
    let total = num("=CUMIPMT(0.1,3,100,1,3,0)+CUMPRINC(0.1,3,100,1,3,0)");
    assert!((total - 3.0 * num("=PMT(0.1,3,100)")).abs() < 1e-9);
    let total_due = num("=CUMIPMT(0.1,3,100,1,3,1)+CUMPRINC(0.1,3,100,1,3,1)");
    assert!((total_due - 3.0 * num("=PMT(0.1,3,100,0,1)")).abs() < 1e-9);
    // Beginning-of-period payments carry no interest in period 1.
    assert_close("=CUMIPMT(0.1,3,100,1,1,1)", 0.0, 1e-12);
    assert_close("=CUMPRINC(0.1,3,100,1,3,1)", -100.0, 1e-9);
}

#[test]
fn ipmt_with_beginning_of_period_payments_accrues_on_the_post_payment_balance() {
    // PMT(0.1,3,100,0,1) = -36.5559; period 2 interest = -(100 - 36.5559) * 0.1 (Excel -6.34441).
    let pmt = num("=PMT(0.1,3,100,0,1)");
    assert_close("=IPMT(0.1,1,3,100,0,1)", 0.0, 1e-12);
    assert_close("=IPMT(0.1,2,3,100,0,1)", -(100.0 + pmt) * 0.1, 1e-9);
    assert_close("=IPMT(0.1,2,3,100,0,1)", -6.34441088, 1e-7);
    // The three interest charges total 3 × PMT − principal.
    assert_close(
        "=IPMT(0.1,1,3,100,0,1)+IPMT(0.1,2,3,100,0,1)+IPMT(0.1,3,3,100,0,1)",
        3.0 * pmt + 100.0,
        1e-9,
    );
    // End-of-period reference values (Excel docs) are unchanged.
    assert_close("=IPMT(0.1/12,1,3*12,8000)", -66.66666667, 1e-6);
    assert_close("=IPMT(0.1,3,3,8000)", -292.4471299, 1e-6);
    // PPMT = PMT − IPMT: PMT(0.1,2,2000) = −1152.380952, first-period interest −200.
    assert_close("=PPMT(0.1,1,2,2000)", -952.380952, 1e-6);
    assert_close(
        "=PPMT(0.08/12,10,10*12,200000)-(PMT(0.08/12,120,200000)-IPMT(0.08/12,10,120,200000))",
        0.0,
        1e-12,
    );
}

#[test]
fn ipmt_and_ppmt_reconcile_with_the_cumulative_windows_for_both_timings() {
    for pay_type in [0, 1] {
        for per in 1..=3 {
            assert_close(
                &format!(
                    "=IPMT(0.1,{per},3,100,0,{pay_type})-CUMIPMT(0.1,3,100,{per},{per},{pay_type})"
                ),
                0.0,
                1e-12,
            );
            assert_close(
                &format!("=PPMT(0.1,{per},3,100,0,{pay_type})-CUMPRINC(0.1,3,100,{per},{per},{pay_type})"),
                0.0,
                1e-12,
            );
        }
    }
}

#[test]
fn cumulative_windows_truncate_fractional_periods() {
    // Excel: nper, start_period, end_period and type are truncated to integers.
    assert_close("=CUMIPMT(0.1,3,100,1.5,2,0)", -16.97885, 1e-5);
    assert_close(
        "=CUMIPMT(0.1,3,100,1.5,2.9,0)-CUMIPMT(0.1,3,100,1,2,0)",
        0.0,
        1e-12,
    );
    assert_close(
        "=CUMPRINC(0.1,3.7,100,1.5,2.9,0.4)-CUMPRINC(0.1,3,100,1,2,0)",
        0.0,
        1e-12,
    );
}

// ── DB / VDB / FVSCHEDULE (C-J18, A-16) ─────────────────────────────────────────────────────

#[test]
fn db_rounds_the_rate_and_prorates_the_partial_last_period() {
    assert_close("=DB(1000,100,5,1)", 369.0, 1e-9);
    assert_close("=DB(1000,100,5,1,6)", 184.5, 1e-9);
    assert_close("=DB(1000000,100000,6,1,7)", 186083.33, 0.01);
    assert_close("=DB(1000000,100000,6,2,7)", 259639.42, 0.01);
    assert_close("=DB(1000000,100000,6,6,7)", 55841.76, 0.01);
    // Seventh period of a six-year life started in month 7: the remaining five months.
    assert_close("=DB(1000000,100000,6,7,7)", 15845.10, 0.01);
    assert_num_error("=DB(1000000,100000,6,8,7)");
}

#[test]
fn vdb_matches_excel_reference_examples() {
    assert_close("=VDB(2400,300,10,0,1)", 480.0, 1e-9);
    assert_close("=VDB(2400,300,10*12,0,1)", 40.0, 1e-9);
    assert_close("=VDB(2400,300,10*365,0,1)", 1.315068493, 1e-6);
    assert_close("=VDB(2400,300,10*12,6,18)", 396.306, 0.001);
    assert_close("=VDB(2400,300,10*12,6,18,1.5)", 311.809, 0.001);
    assert_close("=VDB(2400,300,10,0,0.875,1.5)", 315.0, 1e-9);
    assert_close("=VDB(1000,100,5,0,1)", 400.0, 1e-9);
    assert_close("=VDB(1000,100,5,0,1,2,TRUE)", 400.0, 1e-9);
}

#[test]
fn vdb_switches_to_straight_line_unless_told_not_to() {
    // Periods 4-5 of a 5-year life: DDB 86.4 + salvage-clamped 29.6.
    assert_close("=VDB(1000,100,5,3,5)", 116.0, 1e-9);
    // Year 9 of a 10-year life: straight-line on the remaining value (338.86) beats DDB (335.54).
    assert_close("=VDB(10000,1000,10,8,9)", 338.8608, 1e-4);
    assert_close("=VDB(10000,1000,10,8,9,2,TRUE)", 335.5443, 1e-4);
    // The whole life depreciates exactly cost − salvage.
    assert_close("=VDB(10000,1000,10,0,10)", 9000.0, 1e-9);
    assert_num_error("=VDB(1000,100,5,3,2)");
    assert_num_error("=VDB(1000,100,5,0,6)");
}

#[test]
fn fvschedule_compounds_a_rate_schedule() {
    assert_close("=FVSCHEDULE(1,{0.09,0.11,0.1})", 1.33089, 1e-9);
    assert_close("=FVSCHEDULE(100,{0.1,0.2})", 132.0, 1e-9);
    assert_close("=FVSCHEDULE(100,{0.1;0.2})", 132.0, 1e-9);
    match eval("=FVSCHEDULE(100,{0.1,\"x\"})") {
        LiteralValue::Error(e) => assert_eq!(e.kind, ExcelErrorKind::Value),
        other => panic!("expected #VALUE!, got {other:?}"),
    }
}

// ── Coupon schedule family (D-F18) ──────────────────────────────────────────────────────────

#[test]
fn coupon_functions_match_excel_reference_examples() {
    let args = "DATE(2011,1,25),DATE(2011,11,15),2,1";
    assert_close(&format!("=COUPDAYBS({args})"), 71.0, 1e-9);
    assert_close(&format!("=COUPDAYS({args})"), 181.0, 1e-9);
    assert_close(&format!("=COUPDAYSNC({args})"), 110.0, 1e-9);
    assert_close(&format!("=COUPNCD({args})-DATE(2011,5,15)"), 0.0, 1e-9);
    assert_close(&format!("=COUPPCD({args})-DATE(2010,11,15)"), 0.0, 1e-9);
    assert_close("=COUPNUM(DATE(2007,1,25),DATE(2008,11,15),2,1)", 4.0, 1e-9);
    // 30/360 bases take the nominal period and its remainder.
    assert_close(
        "=COUPDAYS(DATE(2011,1,25),DATE(2011,11,15),2,0)",
        180.0,
        1e-9,
    );
    assert_close(
        "=COUPDAYSNC(DATE(2011,1,25),DATE(2011,11,15),2,0)",
        110.0,
        1e-9,
    );
    assert_close(
        "=COUPDAYS(DATE(2011,1,25),DATE(2011,11,15),4,3)",
        91.25,
        1e-9,
    );
}

#[test]
fn coupon_schedule_pins_month_end_and_settlement_on_a_coupon_date() {
    // A maturity on the last day of the month pins every coupon date to month-end.
    assert_close(
        "=COUPPCD(DATE(2024,4,10),DATE(2025,8,31),2,1)-DATE(2024,2,29)",
        0.0,
        1e-9,
    );
    // Settlement exactly on a coupon date: no accrued days, one coupon per remaining period.
    assert_close(
        "=COUPDAYBS(DATE(2011,5,15),DATE(2011,11,15),2,1)",
        0.0,
        1e-9,
    );
    assert_close("=COUPNUM(DATE(2011,5,15),DATE(2011,11,15),2,1)", 1.0, 1e-9);
    assert_num_error("=COUPNUM(DATE(2011,5,15),DATE(2011,11,15),3,1)");
    assert_num_error("=COUPNUM(DATE(2012,5,15),DATE(2011,11,15),2,1)");
}

#[test]
fn price_and_yield_match_excel_reference_examples() {
    assert_close(
        "=PRICE(DATE(2008,2,15),DATE(2017,11,15),0.0575,0.065,100,2,0)",
        94.63436162,
        1e-6,
    );
    assert_close(
        "=YIELD(DATE(2008,2,15),DATE(2016,11,15),0.0575,95.04287,100,2,0)",
        0.065,
        1e-6,
    );
    assert_close(
        "=PRICE(DATE(2020,1,1),DATE(2030,1,1),0.05,0.06,100,2)",
        92.5612,
        0.001,
    );
    // One coupon left: the closed-form yield inverts PRICE exactly.
    assert_close(
        "=YIELD(DATE(2020,3,1),DATE(2020,7,1),0.05,PRICE(DATE(2020,3,1),DATE(2020,7,1),0.05,0.04,100,2),100,2)",
        0.04,
        1e-9,
    );
}

#[test]
fn duration_and_mduration_match_excel_reference_examples() {
    assert_close(
        "=DURATION(DATE(2008,1,1),DATE(2016,1,1),0.08,0.09,2,1)",
        5.993775,
        1e-6,
    );
    assert_close(
        "=MDURATION(DATE(2008,1,1),DATE(2016,1,1),0.08,0.09,2,1)",
        5.73567,
        1e-5,
    );
}

#[test]
fn discounted_and_maturity_securities_match_excel_reference_examples() {
    assert_close(
        "=PRICEDISC(DATE(2008,2,16),DATE(2008,3,1),0.0525,100,2)",
        99.79583,
        1e-5,
    );
    assert_close(
        "=YIELDDISC(DATE(2008,2,16),DATE(2008,3,1),99.795,100,2)",
        0.052823,
        1e-6,
    );
    assert_close(
        "=INTRATE(DATE(2008,2,15),DATE(2008,5,15),1000000,1014420,2)",
        0.05768,
        1e-6,
    );
    assert_close(
        "=RECEIVED(DATE(2008,2,15),DATE(2008,5,15),1000000,0.0575,2)",
        1014584.654,
        1e-3,
    );
    assert_close(
        "=PRICEMAT(DATE(2008,2,15),DATE(2008,4,13),DATE(2007,11,11),0.061,0.061,0)",
        99.98449888,
        1e-6,
    );
    assert_close(
        "=YIELDMAT(DATE(2008,3,15),DATE(2008,11,3),DATE(2007,11,8),0.0625,100.0123,0)",
        0.060954,
        1e-6,
    );
    assert_num_error("=PRICEDISC(DATE(2008,3,1),DATE(2008,2,16),0.0525,100,2)");
}

// ── YEARFRAC basis 1 — Excel's actual/actual rule, shared with the securities ─────────────────

#[test]
fn yearfrac_actual_actual_follows_excel() {
    // ≤ 1 year across a year boundary with no Feb 29 inside → 365 (was prorated 61/365 + 31/366).
    assert_close(
        "=YEARFRAC(DATE(2023,11,1),DATE(2024,2,1),1)",
        92.0 / 365.0,
        1e-12,
    );
    // ≤ 1 year inside a single leap year → 366 even though Feb 29 is outside the span.
    assert_close(
        "=YEARFRAC(DATE(2024,3,1),DATE(2024,6,1),1)",
        92.0 / 366.0,
        1e-12,
    );
    // ≤ 1 year straddling a Feb 29 → 366 (Excel docs: 0.989071038 / 0.915300546).
    assert_close(
        "=YEARFRAC(DATE(2020,2,5),DATE(2021,2,1),1)",
        0.989071038,
        1e-9,
    );
    assert_close(
        "=YEARFRAC(DATE(2020,2,1),DATE(2021,1,1),1)",
        0.915300546,
        1e-9,
    );
    // > 1 year → average length of the calendar years touched.
    assert_close(
        "=YEARFRAC(DATE(2022,2,16),DATE(2025,3,1),1)",
        1109.0 / 365.25,
        1e-9,
    );
    assert_close(
        "=YEARFRAC(DATE(2012,1,1),DATE(2019,7,30),1)",
        7.575633128,
        1e-9,
    );
    assert_close(
        "=YEARFRAC(DATE(2020,2,1),DATE(2022,1,1),1)",
        1.916058394,
        1e-9,
    );
    // Excel swaps reversed dates: the fraction is never negative (JS engine agrees).
    assert_close(
        "=YEARFRAC(DATE(2024,2,1),DATE(2023,11,1),1)",
        92.0 / 365.0,
        1e-12,
    );
    assert_close(
        "=YEARFRAC(DATE(2012,7,30),DATE(2012,1,1))",
        0.58055556,
        1e-8,
    );
}

#[test]
fn discount_securities_use_the_yearfrac_year_length() {
    assert_close(
        "=PRICEDISC(DATE(2023,11,1),DATE(2024,2,1),0.05,100,1)",
        100.0 - 5.0 * 92.0 / 365.0,
        1e-9,
    );
    assert_close(
        "=PRICEDISC(DATE(2024,3,1),DATE(2024,6,1),0.05,100,1)",
        100.0 - 5.0 * 92.0 / 366.0,
        1e-9,
    );
    for (s, m) in [
        ("DATE(2023,11,1)", "DATE(2024,2,1)"),
        ("DATE(2024,3,1)", "DATE(2024,6,1)"),
        ("DATE(2022,2,16)", "DATE(2025,3,1)"),
    ] {
        assert_close(
            &format!("=PRICEDISC({s},{m},0.05,100,1)-(100-5*YEARFRAC({s},{m},1))"),
            0.0,
            1e-12,
        );
        assert_close(
            &format!("=INTRATE({s},{m},100,105,1)-0.05/YEARFRAC({s},{m},1)"),
            0.0,
            1e-12,
        );
    }
}

// ── Excel 2007 statistical compatibility names (A-15) ───────────────────────────────────────

#[test]
fn legacy_distribution_aliases_match_their_modern_forms() {
    assert_close("=BINOMDIST(6,10,0.5,FALSE)", 0.205078125, 1e-9);
    assert_close("=EXPONDIST(0.2,10,TRUE)", 0.864664717, 1e-8);
    assert_close("=FDIST(15.2069,6,4)", 0.01, 1e-5);
    assert_close("=FINV(0.01,6,4)", 15.20686, 1e-4);
    assert_close("=BETAINV(0.685470581,8,10,1,3)", 2.0, 1e-6);
    assert_close("=CHIDIST(18.307,10)", 0.050001, 1e-5);
    assert_close("=CHIINV(0.050001,10)", 18.306973, 1e-4);
    assert_close("=GAMMADIST(10.00001131,9,2,FALSE)", 0.032639, 1e-6);
    assert_close("=GAMMADIST(10.00001131,9,2,TRUE)", 0.068094, 1e-6);
    assert_close("=LOGINV(0.039084,3.5,1.2)", 4.0000252, 1e-4);
    assert_close("=POISSON(2,5,TRUE)", 0.124652, 1e-6);
    assert_close("=POISSON(2,5,FALSE)", 0.084224, 1e-6);
    assert_close("=WEIBULL(105,20,100,TRUE)", 0.929581, 1e-6);
    assert_close("=WEIBULL(105,20,100,FALSE)", 0.035589, 1e-6);
}

#[test]
fn legacy_names_with_fixed_cumulative_forms() {
    // HYPGEOMDIST / NEGBINOMDIST are the probability-mass forms; LOGNORMDIST / BETADIST the CDFs.
    assert_close("=HYPGEOMDIST(1,4,8,20)", 0.363261094, 1e-8);
    assert_close("=NEGBINOMDIST(10,5,0.25)", 0.05504866, 1e-8);
    assert_close("=LOGNORMDIST(4,3.5,1.2)", 0.039083556, 1e-7);
    assert_close("=BETADIST(2,8,10,1,3)", 0.685470581, 1e-8);
    assert_close("=BETADIST(0.5,2,3)", 0.6875, 1e-9);
    assert_close("=BETADIST(2.5,8,10,1,3)", 0.996899221, 1e-8);
    assert_num_error("=BETADIST(4,8,10,1,3)");
}

#[test]
fn ztest_uses_the_sample_standard_deviation() {
    assert_close("=Z.TEST({3,6,7,8,6,5,4,2,1,9},4)", 0.090574, 1e-6);
    assert_close("=ZTEST({3,6,7,8,6,5,4,2,1,9},4)", 0.090574, 1e-6);
    assert_close("=Z.TEST({3,6,7,8,6,5,4,2,1,9},6)", 0.863043, 1e-6);
}

#[test]
fn tdist_selects_the_tail_and_rejects_negative_x() {
    assert_close("=TDIST(1.959999998,60,2)", 0.054645, 1e-6);
    assert_close("=TDIST(1.959999998,60,1)", 0.027322, 1e-6);
    assert_num_error("=TDIST(-1,60,1)");
    assert_num_error("=TDIST(1,60,3)");
}
