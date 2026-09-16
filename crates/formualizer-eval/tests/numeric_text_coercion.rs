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
