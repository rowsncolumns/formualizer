//! rowsncolumns/spreadsheet#546 U1 (B-01, B-05, B-17, B-24): Excel coerces numeric-looking text
//! — currency, thousands separators, percent, accounting parentheses, exponents, dates and times —
//! in arithmetic and in `VALUE()`. The engine only parsed plain decimals (and `%`), so `"1,000"+0`,
//! `"$1,000"+0`, `"1/2/2024"+0` and `VALUE("1,234.5")` were `#VALUE!`.

use formualizer_common::{ExcelErrorKind, LiteralValue};
use formualizer_eval::engine::{Engine, EvalConfig};
use formualizer_eval::test_workbook::TestWorkbook;
use formualizer_parse::parser::parse;

fn eval(formula: &str) -> LiteralValue {
    let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
    e.set_cell_value("Sheet1", 1, 1, LiteralValue::Text("1,000".into()))
        .unwrap();
    e.set_cell_value("Sheet1", 2, 1, LiteralValue::Text(String::new()))
        .unwrap();
    e.set_cell_formula("Sheet1", 1, 5, parse(formula).unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    e.get_cell_value("Sheet1", 1, 5).expect("value")
}

fn assert_num(formula: &str, expected: f64) {
    match eval(formula) {
        LiteralValue::Number(n) => assert!(
            (n - expected).abs() < 1e-9,
            "{formula}: expected {expected}, got {n}"
        ),
        LiteralValue::Int(i) => assert_eq!(i as f64, expected, "{formula}"),
        other => panic!("{formula}: expected {expected}, got {other:?}"),
    }
}

fn assert_value_error(formula: &str) {
    match eval(formula) {
        LiteralValue::Error(e) => assert_eq!(e.kind, ExcelErrorKind::Value, "{formula}"),
        other => panic!("{formula}: expected #VALUE!, got {other:?}"),
    }
}

#[test]
fn arithmetic_coerces_formatted_numeric_text() {
    assert_num("=\"1,000\"+0", 1000.0);
    assert_num("=\"$1,000.50\"+0", 1000.5);
    assert_num("=\"-$5\"+0", -5.0);
    assert_num("=\"$-5\"+0", -5.0);
    assert_num("=\"50%\"+0", 0.5);
    assert_num("=\"1e3\"+0", 1000.0);
    assert_num("=\" 5 \"+0", 5.0);
    assert_num("=\"(5)\"+0", -5.0);
    assert_num("=\".5\"+0", 0.5);
    assert_num("=--\"1e3\"", 1000.0);
    assert_num("=A1*2", 2000.0);
}

#[test]
fn arithmetic_coerces_date_and_time_text() {
    assert_num("=\"1/2/2024\"+0", 45293.0);
    assert_num("=\"2024-01-02\"+0", 45293.0);
    assert_num("=\"1/2/24\"+0", 45293.0);
    assert_num("=\"5-Jan-2024\"+0", 45296.0);
    assert_num("=\"Jan 5, 2024\"+0", 45296.0);
    assert_num("=\"6:00 PM\"+0", 0.75);
    assert_num("=\"18:30\"+0", 18.5 / 24.0);
    assert_num("=\"1/2/2024 6:00 PM\"+0", 45293.75);
}

#[test]
fn non_numeric_text_stays_value_error() {
    for f in [
        "=\"TRUE\"+0",
        "=\"abc\"+0",
        "=\"\"+0",
        "=A2+1",
        "=\"1,,000\"+0",
        "=\"1,000,\"+0",
        "=\"0x10\"+0",
        "=\"2/30/2024\"+0",
        "=\"25:00\"+0",
        "=-\"abc\"",
    ] {
        assert_value_error(f);
    }
}

#[test]
fn value_function_follows_the_same_rules() {
    assert_num("=VALUE(\"1,234.5\")", 1234.5);
    assert_num("=VALUE(\"12%\")", 0.12);
    assert_num("=VALUE(\"$5\")", 5.0);
    assert_num("=VALUE(\"1e3\")", 1000.0);
    assert_num("=VALUE(\" 5 \")", 5.0);
    assert_num("=VALUE(\"(12.5)\")", -12.5);
    assert_num("=VALUE(\"1/2/2024\")", 45293.0);
    assert_num("=VALUE(\"6:00 PM\")", 0.75);
    assert_num("=VALUE(5)", 5.0);
    assert_num("=VALUE(A1)", 1000.0);
    assert_num("=VALUE(B1)", 0.0);
    for f in [
        "=VALUE(\"abc\")",
        "=VALUE(\"TRUE\")",
        "=VALUE(TRUE)",
        "=VALUE(\"\")",
    ] {
        assert_value_error(f);
    }
}

/// W5-D (E-24): Excel resolves year-less date text in the current year — `"1/2"+0` is January 2
/// of this year. The year comes from the evaluation clock (the one `TODAY()` reads), pinned here
/// to 2026-06-15 UTC so the expectation is stable.
fn eval_on_2026_06_15(formula: &str) -> LiteralValue {
    use chrono::{TimeZone, Utc};
    use formualizer_eval::engine::DeterministicMode;
    use formualizer_eval::timezone::TimeZoneSpec;
    let mut e = Engine::new(
        TestWorkbook::new(),
        EvalConfig {
            deterministic_mode: DeterministicMode::Enabled {
                timestamp_utc: Utc.with_ymd_and_hms(2026, 6, 15, 12, 0, 0).unwrap(),
                timezone: TimeZoneSpec::Utc,
            },
            ..EvalConfig::default()
        },
    );
    e.set_cell_formula("Sheet1", 1, 5, parse(formula).unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    e.get_cell_value("Sheet1", 1, 5).expect("value")
}

#[test]
fn year_less_date_text_resolves_in_the_clock_year() {
    let jan2_2026 = 46024.0;
    for (f, expected) in [
        ("=\"1/2\"+0", jan2_2026),
        ("=\"01/02\"+0", jan2_2026),
        ("=\"Jan 2\"+0", jan2_2026),
        ("=\"2-Jan\"+0", jan2_2026),
        ("=\"2 January\"+0", jan2_2026),
        ("=\"1/2 6:00 PM\"+0", jan2_2026 + 0.75),
        ("=VALUE(\"1/2\")", jan2_2026),
        ("=--\"12/31\"", 46387.0),
        // month/year is the first of that month and needs no clock
        ("=\"1/2024\"+0", 45292.0),
        // the clock year is the same one TODAY() reports
        ("=\"1/2\"-DATE(YEAR(TODAY()),1,2)", 0.0),
    ] {
        match eval_on_2026_06_15(f) {
            LiteralValue::Number(n) => {
                assert!(
                    (n - expected).abs() < 1e-9,
                    "{f}: expected {expected}, got {n}"
                )
            }
            LiteralValue::Int(i) => assert_eq!(i as f64, expected, "{f}"),
            other => panic!("{f}: expected {expected}, got {other:?}"),
        }
    }
    // month > 12 / day out of range / ambiguous non-date forms stay #VALUE!
    for f in [
        "=\"13/2\"+0",
        "=\"1/32\"+0",
        "=\"2/30\"+0",
        "=\"1/2/3/4\"+0",
        "=\"12 5\"+0",
    ] {
        match eval_on_2026_06_15(f) {
            LiteralValue::Error(e) => assert_eq!(e.kind, ExcelErrorKind::Value, "{f}"),
            other => panic!("{f}: expected #VALUE!, got {other:?}"),
        }
    }
}

fn assert_num_on_2026_06_15(formula: &str, expected: f64) {
    match eval_on_2026_06_15(formula) {
        LiteralValue::Number(n) => assert!(
            (n - expected).abs() < 1e-9,
            "{formula}: expected {expected}, got {n}"
        ),
        LiteralValue::Int(i) => assert_eq!(i as f64, expected, "{formula}"),
        other => panic!("{formula}: expected {expected}, got {other:?}"),
    }
}

/// W5-D review follow-up: the operators resolved `"1/2"` in the clock's year but every
/// function-argument path stayed clock-less — `ABS("1/2")`, `SUM("1/2")`, `ROUND("1/2",0)` were
/// `#VALUE!` while `-"1/2"` and `AVERAGE("1/2","1/2")` were 46024. One coercion now serves the
/// schema-level lenient numeric arguments, the builtins' own `coerce_num`, aggregation, and the
/// interpreter's operators.
#[test]
fn lenient_numeric_arguments_resolve_year_less_date_text_in_the_clock_year() {
    let jan2_2026 = 46024.0;
    for (f, expected) in [
        ("=ABS(\"1/2\")", jan2_2026),
        ("=SQRT(\"1/2\")", f64::sqrt(jan2_2026)),
        ("=ROUND(\"1/2\",0)", jan2_2026),
        ("=ROUNDUP(\"1/2\",-1)", 46030.0),
        ("=MOD(\"1/2\",7)", 46024.0 % 7.0),
        ("=INT(\"1/2\")", jan2_2026),
        ("=TRUNC(\"1/2 6:00 PM\")", jan2_2026),
        ("=POWER(\"1/2\",1)", jan2_2026),
        ("=SIGN(\"1/2\")", 1.0),
        ("=PRODUCT(\"1/2\")", jan2_2026),
        ("=PRODUCT(\"1/2\",2)", 2.0 * jan2_2026),
        ("=SUM(\"1/2\")", jan2_2026),
        ("=SUM(\"1/2\",1)", jan2_2026 + 1.0),
        ("=SUMSQ(\"1/2\")", jan2_2026 * jan2_2026),
        ("=AVERAGE(\"1/2\",\"1/2\")", jan2_2026),
        ("=MIN(\"1/2\",50000)", jan2_2026),
        ("=MAX(\"1/2\",1)", jan2_2026),
        ("=COUNT(\"1/2\")", 1.0),
        ("=MEDIAN(\"1/2\")", jan2_2026),
        // the function paths agree with the operator path and with TODAY()
        ("=ABS(\"1/2\")-(\"1/2\"+0)", 0.0),
        ("=SUM(\"1/2\")-DATE(YEAR(TODAY()),1,2)", 0.0),
        // full dates and numeric text keep working through the same entry
        ("=ABS(\"1/2/2024\")", 45293.0),
        ("=SUM(\"1,000\")", 1000.0),
    ] {
        assert_num_on_2026_06_15(f, expected);
    }
    // ...and text that is not a date stays #VALUE! on the argument path too.
    for f in ["=ABS(\"13/2\")", "=SUM(\"1/32\")", "=ROUND(\"abc\",0)"] {
        match eval_on_2026_06_15(f) {
            LiteralValue::Error(e) => assert_eq!(e.kind, ExcelErrorKind::Value, "{f}"),
            other => panic!("{f}: expected #VALUE!, got {other:?}"),
        }
    }
}

/// `TEXT` coerces its value like `VALUE()` does — including year-less date text in the clock's
/// year (`TEXT("1/2","yyyy")` is this year) and full date text — and keeps returning the text
/// itself only for clearly non-numeric input.
#[test]
fn text_function_coerces_date_text_with_the_clock() {
    for (f, expected) in [
        ("=TEXT(\"1/2\",\"yyyy\")", "2026"),
        ("=TEXT(\"1/2\",\"yyyy-mm-dd\")", "2026-01-02"),
        ("=TEXT(\"1/2 6:00 PM\",\"hh:mm\")", "18:00"),
        ("=TEXT(\"1/2/2024\",\"yyyy\")", "2024"),
        ("=TEXT(\"$1,000\",\"0\")", "1000"),
        ("=TEXT(\"abc\",\"00\")", "abc"),
        // the date codes walk the 1900 serial system (was a 1899-12-31 epoch without the
        // phantom day: 45293 rendered as 01/03/2024)
        ("=TEXT(45293,\"mm/dd/yyyy\")", "01/02/2024"),
        ("=TEXT(60,\"mm/dd/yyyy\")", "02/29/1900"),
        ("=TEXT(61,\"yyyy-mm-dd\")", "1900-03-01"),
        ("=TEXT(1,\"yyyy-mm-dd\")", "1900-01-01"),
        ("=TEXT(0,\"yyyy-mm-dd\")", "1900-01-00"),
    ] {
        match eval_on_2026_06_15(f) {
            LiteralValue::Text(t) => assert_eq!(t, expected, "{f}"),
            other => panic!("{f}: expected {expected:?}, got {other:?}"),
        }
    }
}

/// `DATEVALUE` uses the current year when the year is omitted (Excel's documented behaviour):
/// `DATEVALUE("1/2")` is January 2 of the clock's year, time information is dropped, and the
/// date-part functions accept the same year-less text.
#[test]
fn datevalue_and_date_parts_accept_year_less_text() {
    let jan2_2026 = 46024.0;
    for (f, expected) in [
        ("=DATEVALUE(\"1/2\")", jan2_2026),
        ("=DATEVALUE(\"Jan 2\")", jan2_2026),
        ("=DATEVALUE(\"2-Jan\")", jan2_2026),
        ("=DATEVALUE(\"1/2 6:00 PM\")", jan2_2026),
        ("=DATEVALUE(\"1/2/2024\")", 45293.0),
        ("=DATEVALUE(\"2024-01-02\")", 45293.0),
        ("=DATEVALUE(\"1/2\")-DATE(YEAR(TODAY()),1,2)", 0.0),
        ("=YEAR(\"1/2\")", 2026.0),
        ("=MONTH(\"1/2\")", 1.0),
        ("=DAY(\"1/2\")", 2.0),
        ("=WEEKDAY(\"1/2\")", 6.0),
        ("=EDATE(\"1/2\",1)", 46055.0),
    ] {
        assert_num_on_2026_06_15(f, expected);
    }
    for f in [
        "=DATEVALUE(\"13/2\")",
        "=DATEVALUE(\"abc\")",
        "=YEAR(\"2/30\")",
    ] {
        match eval_on_2026_06_15(f) {
            LiteralValue::Error(e) => assert_eq!(e.kind, ExcelErrorKind::Value, "{f}"),
            other => panic!("{f}: expected #VALUE!, got {other:?}"),
        }
    }
}
