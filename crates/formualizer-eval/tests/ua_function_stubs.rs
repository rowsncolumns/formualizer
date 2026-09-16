//! rowsncolumns/spreadsheet#546 (U-A) — functions common in business workbooks that evaluated to
//! `#NAME?`: the Excel 2024 regex family, the Excel 2007 statistical compatibility names, and the
//! `CELL` / `INFO` / `AREAS` metadata functions.

use formualizer_common::{ExcelErrorKind, LiteralValue};
use formualizer_eval::engine::{Engine, EvalConfig};
use formualizer_eval::test_workbook::TestWorkbook;
use formualizer_parse::parser::parse;

fn engine() -> Engine<TestWorkbook> {
    Engine::new(TestWorkbook::new(), EvalConfig::default())
}

fn text(s: &str) -> LiteralValue {
    LiteralValue::Text(s.into())
}

/// Evaluate `formula` in Sheet1!E1 with A1:A3 seeded from `seed`.
fn eval(seed: &[LiteralValue], formula: &str) -> LiteralValue {
    let mut e = engine();
    for (i, v) in seed.iter().enumerate() {
        e.set_cell_value("Sheet1", i as u32 + 1, 1, v.clone())
            .unwrap();
    }
    e.set_cell_formula("Sheet1", 1, 5, parse(formula).unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    e.get_cell_value("Sheet1", 1, 5)
        .unwrap_or(LiteralValue::Empty)
}

/// Evaluate `formula` in Sheet1!E1 and return (E1, F1) — a row spill lands its second value in F1.
fn eval_spill(formula: &str) -> (LiteralValue, LiteralValue) {
    let mut e = engine();
    e.set_cell_formula("Sheet1", 1, 5, parse(formula).unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    (
        e.get_cell_value("Sheet1", 1, 5)
            .unwrap_or(LiteralValue::Empty),
        e.get_cell_value("Sheet1", 1, 6)
            .unwrap_or(LiteralValue::Empty),
    )
}

fn num(v: &LiteralValue) -> f64 {
    match v {
        LiteralValue::Number(n) => *n,
        LiteralValue::Int(i) => *i as f64,
        other => panic!("expected number, got {other:?}"),
    }
}

fn err_kind(v: &LiteralValue) -> ExcelErrorKind {
    match v {
        LiteralValue::Error(e) => e.kind,
        other => panic!("expected error, got {other:?}"),
    }
}

#[test]
fn regextest_matches_with_case_flag() {
    assert_eq!(
        eval(&[], "=REGEXTEST(\"Hello World\",\"o W\")"),
        LiteralValue::Boolean(true)
    );
    assert_eq!(
        eval(&[], "=REGEXTEST(\"Hello World\",\"^world$\")"),
        LiteralValue::Boolean(false)
    );
    assert_eq!(
        eval(&[], "=REGEXTEST(\"Hello World\",\"^hello\",1)"),
        LiteralValue::Boolean(true)
    );
    assert_eq!(
        eval(&[], "=REGEXTEST(\"abc123\",\"\\d{3}$\")"),
        LiteralValue::Boolean(true)
    );
    assert_eq!(
        err_kind(&eval(&[], "=REGEXTEST(\"abc\",\"[\")")),
        ExcelErrorKind::Value
    );
    assert_eq!(
        err_kind(&eval(&[], "=REGEXTEST(\"abc\",\"a\",2)")),
        ExcelErrorKind::Value
    );
}

#[test]
fn regexextract_return_modes() {
    assert_eq!(
        eval(&[], "=REGEXEXTRACT(\"Order 66 and 99\",\"\\d+\")"),
        text("66")
    );
    assert_eq!(
        eval_spill("=REGEXEXTRACT(\"Order 66 and 99\",\"\\d+\",1)"),
        (text("66"), text("99"))
    );
    assert_eq!(
        eval_spill("=REGEXEXTRACT(\"John Smith\",\"(\\w+) (\\w+)\",2)"),
        (text("John"), text("Smith"))
    );
    assert_eq!(
        err_kind(&eval(&[], "=REGEXEXTRACT(\"abc\",\"\\d\")")),
        ExcelErrorKind::Na
    );
    assert_eq!(eval(&[], "=REGEXEXTRACT(\"ABC\",\"b\",0,1)"), text("B"));
    assert_eq!(
        err_kind(&eval(&[], "=REGEXEXTRACT(\"ABC\",\"b\")")),
        ExcelErrorKind::Na
    );
    assert_eq!(
        err_kind(&eval(&[], "=REGEXEXTRACT(\"abc\",\"a\",3)")),
        ExcelErrorKind::Value
    );
}

#[test]
fn regexreplace_occurrence_and_groups() {
    assert_eq!(
        eval(&[], "=REGEXREPLACE(\"a1b2c3\",\"\\d\",\"#\")"),
        text("a#b#c#")
    );
    assert_eq!(
        eval(&[], "=REGEXREPLACE(\"a1b2c3\",\"\\d\",\"#\",2)"),
        text("a1b#c3")
    );
    assert_eq!(
        eval(&[], "=REGEXREPLACE(\"a1b2c3\",\"\\d\",\"#\",-1)"),
        text("a1b2c#")
    );
    assert_eq!(
        eval(&[], "=REGEXREPLACE(\"a1b2c3\",\"\\d\",\"#\",5)"),
        text("a1b2c3")
    );
    assert_eq!(
        eval(
            &[],
            "=REGEXREPLACE(\"John Smith\",\"(\\w+) (\\w+)\",\"$2, $1\")"
        ),
        text("Smith, John")
    );
    assert_eq!(
        eval(&[], "=REGEXREPLACE(\"ABC\",\"b\",\"x\",0,1)"),
        text("AxC")
    );
}

#[test]
fn legacy_normal_distribution_names() {
    assert!((num(&eval(&[], "=NORMDIST(42,40,1.5,TRUE)")) - 0.908_788_780).abs() < 1e-7);
    assert!((num(&eval(&[], "=NORMDIST(42,40,1.5,FALSE)")) - 0.109_340_050).abs() < 1e-7);
    assert!((num(&eval(&[], "=NORMSDIST(1.333333)")) - 0.908_788_726).abs() < 1e-7);
    assert!((num(&eval(&[], "=NORMSDIST(0)")) - 0.5).abs() < 1e-7);
    assert!((num(&eval(&[], "=NORMSINV(0.975)")) - 1.959_963_985).abs() < 1e-7);
    assert!((num(&eval(&[], "=NORMINV(0.908789,40,1.5)")) - 42.000_002).abs() < 1e-4);
    assert_eq!(err_kind(&eval(&[], "=NORMSINV(-1)")), ExcelErrorKind::Num);
}

#[test]
fn cell_reports_address_row_col_contents_type() {
    let seed = [text("hello"), LiteralValue::Number(2.5)];
    assert_eq!(eval(&seed, "=CELL(\"address\",A2)"), text("$A$2"));
    assert_eq!(eval(&seed, "=CELL(\"address\",A1:B3)"), text("$A$1"));
    assert_eq!(num(&eval(&seed, "=CELL(\"row\",B7)")), 7.0);
    assert_eq!(num(&eval(&seed, "=CELL(\"col\",B7)")), 2.0);
    assert_eq!(eval(&seed, "=CELL(\"contents\",A1)"), text("hello"));
    assert_eq!(
        eval(&seed, "=CELL(\"contents\",A2)"),
        LiteralValue::Number(2.5)
    );
    assert_eq!(num(&eval(&seed, "=CELL(\"contents\",A3)")), 0.0);
    assert_eq!(eval(&seed, "=CELL(\"type\",A1)"), text("l"));
    assert_eq!(eval(&seed, "=CELL(\"type\",A2)"), text("v"));
    assert_eq!(eval(&seed, "=CELL(\"type\",A3)"), text("b"));
    assert_eq!(eval(&seed, "=CELL(\"prefix\",A1)"), text("'"));
    assert_eq!(eval(&seed, "=CELL(\"prefix\",A2)"), text(""));
    assert_eq!(num(&eval(&seed, "=CELL(\"protect\",A1)")), 1.0);
    assert_eq!(eval(&seed, "=CELL(\"format\",A2)"), text("G"));
    assert_eq!(num(&eval(&seed, "=CELL(\"row\")")), 1.0);
    assert_eq!(num(&eval(&seed, "=CELL(\"col\")")), 5.0);
    assert_eq!(
        err_kind(&eval(&seed, "=CELL(\"nonsense\",A1)")),
        ExcelErrorKind::Value
    );
}

#[test]
fn cell_address_qualifies_other_sheets() {
    let mut e = engine();
    e.add_sheet("My Data").unwrap();
    e.set_cell_value("My Data", 3, 2, LiteralValue::Number(1.0))
        .unwrap();
    e.set_cell_formula(
        "Sheet1",
        1,
        1,
        parse("=CELL(\"address\",'My Data'!B3)").unwrap(),
    )
    .unwrap();
    e.set_cell_formula(
        "Sheet1",
        2,
        1,
        parse("=CELL(\"address\",Sheet1!B3)").unwrap(),
    )
    .unwrap();
    e.evaluate_all().unwrap();
    assert_eq!(
        e.get_cell_value("Sheet1", 1, 1),
        Some(text("'My Data'!$B$3"))
    );
    assert_eq!(e.get_cell_value("Sheet1", 2, 1), Some(text("$B$3")));
}

#[test]
fn info_and_areas() {
    assert_eq!(eval(&[], "=INFO(\"system\")"), text("pcdos"));
    assert_eq!(eval(&[], "=INFO(\"recalc\")"), text("Automatic"));
    assert_eq!(eval(&[], "=INFO(\"release\")"), text("16.0"));
    assert_eq!(
        err_kind(&eval(&[], "=INFO(\"bogus\")")),
        ExcelErrorKind::Value
    );
    assert_eq!(num(&eval(&[], "=AREAS(A1:B2)")), 1.0);
    assert_eq!(num(&eval(&[], "=AREAS(A1)")), 1.0);
    assert_eq!(err_kind(&eval(&[], "=AREAS(\"x\")")), ExcelErrorKind::Value);
}
