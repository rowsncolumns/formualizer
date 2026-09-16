//! rowsncolumns/spreadsheet#546 U15 (B-14, B-22, B-27/E-43, B-28): Excel date-function parity.
//! `DATE` counts across Excel's phantom 1900-02-29 (`DATE(1900,2,29)` = 60), `YEAR`/`MONTH`/`DAY`
//! report serial 60 as Feb 29 and serial 0 as "January 0, 1900", `DAYS360` accepts text dates,
//! and the already-correct `DATEDIF`, `WEEKNUM`, `ISOWEEKNUM`, `WORKDAY` and `TIME` semantics
//! are pinned.

use formualizer_common::{ExcelErrorKind, LiteralValue};
use formualizer_eval::engine::{DateSystem, Engine, EvalConfig};
use formualizer_eval::test_workbook::TestWorkbook;
use formualizer_parse::parser::parse;

fn eval_with(config: EvalConfig, formula: &str) -> LiteralValue {
    let mut e = Engine::new(TestWorkbook::new(), config);
    e.set_cell_formula("Sheet1", 1, 5, parse(formula).unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    e.get_cell_value("Sheet1", 1, 5).expect("value")
}

fn eval(formula: &str) -> LiteralValue {
    eval_with(EvalConfig::default(), formula)
}

fn assert_num_in(config: EvalConfig, formula: &str, expected: f64) {
    match eval_with(config, formula) {
        LiteralValue::Number(n) => assert!(
            (n - expected).abs() < 1e-9,
            "{formula}: expected {expected}, got {n}"
        ),
        LiteralValue::Int(i) => assert_eq!(i as f64, expected, "{formula}"),
        other => panic!("{formula}: expected {expected}, got {other:?}"),
    }
}

fn assert_num(formula: &str, expected: f64) {
    assert_num_in(EvalConfig::default(), formula, expected);
}

fn assert_error(formula: &str, kind: ExcelErrorKind) {
    match eval(formula) {
        LiteralValue::Error(e) => assert_eq!(e.kind, kind, "{formula}"),
        other => panic!("{formula}: expected {kind:?}, got {other:?}"),
    }
}

#[test]
fn date_counts_across_the_phantom_1900_leap_day() {
    assert_num("=DATE(1900,1,1)", 1.0);
    assert_num("=DATE(1900,2,28)", 59.0);
    assert_num("=DATE(1900,2,29)", 60.0);
    assert_num("=DATE(1900,3,1)", 61.0);
    assert_num("=DATE(1900,1,60)", 60.0);
    assert_num("=DATE(1900,3,0)", 60.0);
    assert_num("=DATE(1900,1,0)", 0.0);
    assert_error("=DATE(1900,1,-1)", ExcelErrorKind::Num);
}

#[test]
fn date_normalizes_overflow_and_bounds_like_excel() {
    assert_num("=DATE(2024,2,30)", 45352.0);
    assert_num("=DATE(2024,13,1)", 45658.0);
    assert_num("=DATE(2024,0,1)", 45261.0);
    assert_num("=DATE(24,1,1)", 8767.0);
    assert_num("=DATE(2023,2,29)", 44986.0);
    assert_num("=DATE(2024,1,1.9)", 45292.0);
    assert_num("=DATE(9999,12,31)", 2958465.0);
    assert_error("=DATE(9999,12,32)", ExcelErrorKind::Num);
    assert_error("=DATE(-1,1,1)", ExcelErrorKind::Num);
    assert_error("=DATE(10000,1,1)", ExcelErrorKind::Num);
}

#[test]
fn date_honours_the_1904_date_system() {
    let cfg = || EvalConfig {
        date_system: DateSystem::Excel1904,
        ..Default::default()
    };
    assert_num_in(cfg(), "=DATE(1904,1,1)", 0.0);
    // 1904 is a real leap year: no phantom day, Mar 1 is serial 60.
    assert_num_in(cfg(), "=DATE(1904,3,1)", 60.0);
    assert_num_in(cfg(), "=DATE(2024,1,1)", 45292.0 - 1462.0);
    match eval_with(cfg(), "=DATE(1903,12,31)") {
        LiteralValue::Error(e) => assert_eq!(e.kind, ExcelErrorKind::Num),
        other => panic!("pre-1904 date in the 1904 system should be #NUM!, got {other:?}"),
    }
}

#[test]
fn year_month_day_report_serial_0_and_60_like_excel() {
    assert_num("=YEAR(0)", 1900.0);
    assert_num("=MONTH(0)", 1.0);
    assert_num("=DAY(0)", 0.0);
    assert_num("=DAY(59)", 28.0);
    assert_num("=YEAR(60)", 1900.0);
    assert_num("=MONTH(60)", 2.0);
    assert_num("=DAY(60)", 29.0);
    assert_num("=DAY(60.5)", 29.0);
    assert_num("=DAY(61)", 1.0);
    assert_num("=MONTH(61)", 3.0);
    assert_num("=DAY(DATE(1900,2,29))", 29.0);
    assert_num("=DAY(\"2024-02-29\")", 29.0);
    assert_num("=YEAR(\"2024-06-15\")", 2024.0);
    assert_error("=DAY(\"\")", ExcelErrorKind::Value);
    assert_error("=DAY(-1)", ExcelErrorKind::Num);
}

#[test]
fn days360_accepts_text_dates() {
    assert_num("=DAYS360(\"2024-01-31\",\"2024-03-01\")", 31.0);
    assert_num("=DAYS360(\"2024-01-31\",\"2024-03-01\",TRUE)", 31.0);
    assert_num("=DAYS360(DATE(2024,1,31),DATE(2024,3,1))", 31.0);
    assert_num("=DAYS360(\"2/1/2019\",\"3/31/2019\")", 60.0);
    assert_num("=DAYS360(\"2/1/2019\",\"3/31/2019\",TRUE)", 59.0);
}

#[test]
fn datedif_md_follows_excel_roll_over() {
    // Excel's "md" moves the start day into the month before the end date and lets a
    // non-existent day roll over (2024-02-31 -> 2024-03-02), so this is -1 in Excel too
    // (a documented DATEDIF "md" quirk).
    assert_num("=DATEDIF(\"2024-01-31\",\"2024-03-01\",\"md\")", -1.0);
    assert_num("=DATEDIF(\"2024-01-31\",\"2024-03-01\",\"m\")", 1.0);
    assert_num("=DATEDIF(\"2024-01-31\",\"2024-03-01\",\"d\")", 30.0);
    assert_num("=DATEDIF(\"2023-05-10\",\"2024-03-01\",\"y\")", 0.0);
    assert_num("=DATEDIF(\"2023-05-10\",\"2024-03-01\",\"ym\")", 9.0);
    assert_num("=DATEDIF(\"6/1/2001\",\"8/15/2002\",\"MD\")", 14.0);
    assert_num("=DATEDIF(\"5/16/2001\",\"7/15/2002\",\"MD\")", 29.0);
    assert_error(
        "=DATEDIF(\"2024-03-01\",\"2024-01-01\",\"d\")",
        ExcelErrorKind::Num,
    );
}

#[test]
fn weeknum_and_isoweeknum_match_excel() {
    // 2024-01-01 is a Monday: Sunday-start weeks (type 1) roll to week 2 on Jan 7.
    assert_num("=WEEKNUM(\"2024-01-01\")", 1.0);
    assert_num("=WEEKNUM(\"2024-01-06\")", 1.0);
    assert_num("=WEEKNUM(\"2024-01-07\")", 2.0);
    assert_num("=WEEKNUM(\"2024-01-07\",2)", 1.0);
    assert_num("=WEEKNUM(\"2024-01-08\",2)", 2.0);
    assert_num("=WEEKNUM(\"2024-01-01\",21)", 1.0);
    assert_num("=WEEKNUM(\"2021-01-03\",21)", 53.0);
    assert_num("=WEEKNUM(\"2024-12-31\")", 53.0);
    assert_num("=WEEKNUM(\"3/9/2012\")", 10.0);
    assert_num("=WEEKNUM(\"3/9/2012\",2)", 11.0);
    assert_error("=WEEKNUM(\"3/9/2012\",5)", ExcelErrorKind::Num);
    assert_num("=ISOWEEKNUM(\"2021-01-03\")", 53.0);
    assert_num("=ISOWEEKNUM(\"2024-12-30\")", 1.0);
    assert_num("=ISOWEEKNUM(\"2024-01-07\")", 1.0);
}

#[test]
fn workday_walks_backwards_for_negative_days() {
    assert_num("=WORKDAY(\"2024-01-05\",1)", 45299.0);
    assert_num("=WORKDAY(\"2024-01-05\",-1)", 45295.0);
    assert_num("=WORKDAY(\"2024-01-08\",-1)", 45296.0);
    assert_num("=WORKDAY(\"2024-01-08\",-5)", 45292.0);
    assert_num("=WORKDAY(\"2024-01-08\",-1,{45296})", 45295.0);
    assert_num("=WORKDAY(\"2024-01-06\",0)", 45297.0);
    assert_num("=WORKDAY.INTL(\"2024-01-05\",1,\"0000011\")", 45299.0);
    assert_num("=WORKDAY.INTL(\"2024-01-08\",-1,\"0000011\")", 45296.0);
    assert_num("=WORKDAY.INTL(\"2024-01-08\",-1,11)", 45297.0);
}

#[test]
fn time_wraps_at_24_hours() {
    assert_num("=TIME(25,0,0)", 1.0 / 24.0);
    assert_num("=TIME(0,90,0)", 0.0625);
    assert_num("=TIME(24,0,0)", 0.0);
    assert_num("=TIME(12,0,0)", 0.5);
    assert_num("=TIME(12.9,0,0)", 0.5);
}
