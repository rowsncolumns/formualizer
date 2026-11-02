//! Athena-Intel/demo-app-monorepo#49969 FX-04 / FX-19.
//!
//! FX-04 — the 1904 date system (support.microsoft.com, "Date systems in Excel"): date text
//! coerced by arithmetic or `VALUE` lands on the workbook's system (`"1/1/2024"+0` is 43830), and
//! `DAYS360` plus the coupon / bond / depreciation functions read their serials in it.
//!
//! FX-19 — edge semantics: every Excel error literal parses (`#FIELD!`, `#BLOCKED!`,
//! `#CONNECT!`, `#BUSY!`, `#UNKNOWN!`, `#PYTHON!`) and keeps its own kind and `ERROR.TYPE` code;
//! an omitted trailing argument of a LAMBDA call (`LAMBDA(x,y,…)(1,)`) parses as omitted; and a
//! subnormal number (`=1E-320`) is 0.

use formualizer_common::{ExcelErrorKind, LiteralValue};
use formualizer_eval::engine::{DateSystem, Engine, EvalConfig};
use formualizer_eval::test_workbook::TestWorkbook;
use formualizer_parse::parser::parse;

fn eval_with(config: EvalConfig, formula: &str) -> LiteralValue {
    let mut e = Engine::new(TestWorkbook::new(), config);
    e.set_cell_formula("Sheet1", 1, 5, parse(formula).expect("parses"))
        .unwrap();
    e.evaluate_all().unwrap();
    e.get_cell_value("Sheet1", 1, 5).expect("value")
}

fn cfg_1904() -> EvalConfig {
    EvalConfig {
        date_system: DateSystem::Excel1904,
        ..Default::default()
    }
}

fn num(v: LiteralValue, formula: &str) -> f64 {
    match v {
        LiteralValue::Number(n) => n,
        LiteralValue::Int(i) => i as f64,
        other => panic!("{formula}: expected a number, got {other:?}"),
    }
}

fn assert_num_in(config: EvalConfig, formula: &str, expected: f64) {
    let n = num(eval_with(config, formula), formula);
    assert!(
        (n - expected).abs() < 1e-9,
        "{formula}: expected {expected}, got {n}"
    );
}

fn assert_error_in(config: EvalConfig, formula: &str, kind: ExcelErrorKind) {
    match eval_with(config, formula) {
        LiteralValue::Error(e) => assert_eq!(e.kind, kind, "{formula}"),
        other => panic!("{formula}: expected {kind:?}, got {other:?}"),
    }
}

#[test]
fn date_text_coerces_onto_the_1904_system() {
    assert_num_in(cfg_1904(), "=\"1/1/2024\"+0", 43830.0);
    assert_num_in(cfg_1904(), "=VALUE(\"1/1/2024\")", 43830.0);
    assert_num_in(cfg_1904(), "=\"1/1/2024 12:00\"+0", 43830.5);
    // Numbers and times are the same in both systems.
    assert_num_in(cfg_1904(), "=\"43830\"+0", 43830.0);
    assert_num_in(cfg_1904(), "=\"6:00 PM\"+0", 0.75);
    // A date before 1904-01-01 has no 1904 serial.
    assert_error_in(cfg_1904(), "=\"1/1/1900\"+0", ExcelErrorKind::Value);
    // The 1900 system is unchanged.
    assert_num_in(EvalConfig::default(), "=\"1/1/2024\"+0", 45292.0);
}

#[test]
fn days360_and_coupon_dates_read_1904_serials() {
    // 1904 serials: 43860 = 2024-01-31, 43890 = 2024-03-01 (US method: Jan 31 counts as the 30th).
    assert_num_in(cfg_1904(), "=DAYS360(43860,43890)", 31.0);
    // Settlement 2024-01-16 (43845), maturity 2024-02-29 (43889, a month end), semiannual: the
    // coupons stay on month ends, so the previous one is 2023-08-31 (43707). Read on the 1900
    // calendar the same serials are 2020-01-15 and 2020-02-28 (not a month end) → 43705.
    assert_num_in(cfg_1904(), "=COUPPCD(43845,43889,2)", 43707.0);
    assert_num_in(cfg_1904(), "=COUPNCD(43845,43889,2)", 43889.0);
    assert_num_in(EvalConfig::default(), "=COUPPCD(43845,43889,2)", 43705.0);
}

#[test]
fn every_excel_error_literal_parses_with_its_own_kind() {
    for (literal, kind, error_type) in [
        ("#FIELD!", ExcelErrorKind::Field, Some(13.0)),
        ("#BLOCKED!", ExcelErrorKind::Blocked, Some(11.0)),
        ("#CONNECT!", ExcelErrorKind::Connect, Some(10.0)),
        ("#UNKNOWN!", ExcelErrorKind::Unknown, Some(12.0)),
        ("#BUSY!", ExcelErrorKind::Busy, None),
        ("#PYTHON!", ExcelErrorKind::Python, None),
    ] {
        assert_error_in(EvalConfig::default(), &format!("={literal}"), kind);
        assert_eq!(kind.to_string(), literal);
        assert_eq!(
            ExcelErrorKind::try_parse(&literal.to_lowercase()),
            Some(kind)
        );
        match eval_with(EvalConfig::default(), &format!("=ISERROR({literal})")) {
            LiteralValue::Boolean(b) => assert!(b, "ISERROR({literal})"),
            other => panic!("ISERROR({literal}): {other:?}"),
        }
        let et = eval_with(EvalConfig::default(), &format!("=ERROR.TYPE({literal})"));
        match error_type {
            Some(code) => assert_eq!(num(et, literal), code, "ERROR.TYPE({literal})"),
            None => assert!(
                matches!(et, LiteralValue::Error(ref e) if e.kind == ExcelErrorKind::Na),
                "ERROR.TYPE({literal}) is #N/A outside Excel's table: {et:?}"
            ),
        }
    }
}

#[test]
fn a_lambda_call_with_an_omitted_trailing_argument_parses() {
    assert_num_in(EvalConfig::default(), "=LAMBDA(x,y,x+1)(1,)", 2.0);
    match eval_with(
        EvalConfig::default(),
        "=LAMBDA(x,y,IF(ISOMITTED(y),\"omitted\",y))(1,)",
    ) {
        LiteralValue::Text(t) => assert_eq!(t, "omitted"),
        other => panic!("ISOMITTED of an omitted call argument: {other:?}"),
    }
    assert_num_in(EvalConfig::default(), "=LAMBDA(x,y,x+y)(1,2)", 3.0);
}

#[test]
fn subnormal_numbers_flush_to_zero() {
    assert_num_in(EvalConfig::default(), "=1E-320", 0.0);
    assert_num_in(EvalConfig::default(), "=1E-300*1E-15", 0.0);
    assert_num_in(EvalConfig::default(), "=1E-300", 1e-300);
}
