//! rowsncolumns/spreadsheet#546 (U3) — Excel criteria semantics for COUNTIF / SUMIF /
//! AVERAGEIF and the *IFS family: blank vs `""` vs `"<>"`, numeric criteria matching numeric
//! text, booleans never matching numbers, wildcards on text only with the `~` escape, `<>` as
//! the exact complement of `=`, and same-shape validation for multi-criteria functions.

use formualizer_common::{ExcelErrorKind, LiteralValue};
use formualizer_eval::engine::{Engine, EvalConfig};
use formualizer_eval::test_workbook::TestWorkbook;
use formualizer_parse::parser::parse;

fn text(s: &str) -> LiteralValue {
    LiteralValue::Text(s.into())
}
fn num(n: f64) -> LiteralValue {
    LiteralValue::Number(n)
}

/// Probe grid from the parity sweep: A1=1, A2="x", A3=TRUE, A4=blank, A5=2, A6="1" (text),
/// A7=blank, A8=3; B1=10, B2=20, B3="abc", B4="a*c", B5=5; C1:C5 = 1..5.
fn engine() -> Engine<TestWorkbook> {
    let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
    let col_a = [
        Some(num(1.0)),
        Some(text("x")),
        Some(LiteralValue::Boolean(true)),
        None,
        Some(num(2.0)),
        Some(text("1")),
        None,
        Some(num(3.0)),
    ];
    for (i, v) in col_a.iter().enumerate() {
        if let Some(v) = v {
            e.set_cell_value("Sheet1", i as u32 + 1, 1, v.clone())
                .unwrap();
        }
    }
    let col_b = [num(10.0), num(20.0), text("abc"), text("a*c"), num(5.0)];
    for (i, v) in col_b.iter().enumerate() {
        e.set_cell_value("Sheet1", i as u32 + 1, 2, v.clone())
            .unwrap();
    }
    for r in 1..=5u32 {
        e.set_cell_value("Sheet1", r, 3, num(r as f64)).unwrap();
    }
    e
}

fn eval(formula: &str) -> LiteralValue {
    let mut e = engine();
    e.set_cell_formula("Sheet1", 1, 10, parse(formula).unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    e.get_cell_value("Sheet1", 1, 10)
        .unwrap_or(LiteralValue::Empty)
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

#[test]
fn blank_criteria_family() {
    // "<>" = every non-blank cell; "" and "=" = the two blank cells.
    assert_num("=COUNTIF(A1:A8,\"<>\")", 6.0);
    assert_num("=COUNTIF(A1:A8,\"=\")", 2.0);
    assert_num("=COUNTIF(A1:A8,\"\")", 2.0);
    assert_num("=SUMIF(A1:A8,\"\",C1:C8)", 4.0); // blanks A4, A7 → C4 (C7 is blank)
    assert_num("=SUMIF(A1:A8,\"<>\",C1:C8)", 11.0); // C1+C2+C3+C5 (C6, C8 blank)
}

#[test]
fn numeric_criteria_match_numbers_and_numeric_text() {
    assert_num("=COUNTIF(A1:A8,1)", 2.0);
    assert_num("=COUNTIF(A1:A8,\"1\")", 2.0);
    assert_num("=COUNTIF(A1:A8,\"=1\")", 2.0);
    // Ordering criteria only see real numbers: 2 and 3 (TRUE and "1" excluded).
    assert_num("=COUNTIF(A1:A8,\">1\")", 2.0);
    assert_num("=SUMIF(A1:A8,\">1\")", 5.0);
    assert_num("=SUMIF(A1:A8,\">=1\")", 6.0);
    assert_num("=AVERAGEIF(A1:A8,\">1\")", 2.5);
    assert_num("=COUNTIFS(A1:A8,\">0\",A1:A8,\"<3\")", 2.0);
    assert_num("=SUMIFS(A1:A8,A1:A8,\">1\")", 5.0);
    // "<>10" is the complement of "=10": text, blanks and the other numbers all count.
    assert_num("=COUNTIF(B1:B5,\"<>10\")", 4.0);
    assert_num("=SUMIF(B1:B5,\"<>10\")", 25.0);
    // Thousands separators and percentages parse as numbers.
    assert_num("=COUNTIF(C1:C5,\"<1,000\")", 5.0);
}

#[test]
fn boolean_criteria_only_match_booleans() {
    assert_num("=COUNTIF(A1:A8,TRUE)", 1.0);
    assert_num("=COUNTIF(A1:A8,\"TRUE\")", 1.0);
    assert_num("=COUNTIF(A1:A8,\"<>TRUE\")", 7.0);
}

#[test]
fn text_criteria_are_case_insensitive_and_text_only() {
    assert_num("=COUNTIF(A1:A8,\"X\")", 1.0);
    assert_num("=COUNTIF(A1:A8,\"<>x\")", 7.0);
    // Lexical ordering on text cells only.
    assert_num("=COUNTIF(B1:B5,\">a\")", 2.0);
    assert_num("=COUNTIF(B1:B5,\"<b\")", 2.0);
}

#[test]
fn wildcards_match_text_only_and_honour_tilde_escape() {
    assert_num("=COUNTIF(A1:A8,\"*\")", 2.0);
    assert_num("=COUNTIF(A1:A8,\"?\")", 2.0);
    assert_num("=COUNTIF(B1:B5,\"a*\")", 2.0);
    assert_num("=COUNTIF(B1:B5,\"a?c\")", 2.0);
    assert_num("=COUNTIF(B1:B5,\"a~*c\")", 1.0);
    assert_num("=COUNTIF(B1:B5,\"<>a*\")", 3.0);
    assert_num("=SUMIF(B1:B5,\"*c\",C1:C5)", 7.0);
}

#[test]
fn ifs_family_requires_same_shape_ranges() {
    let v = eval("=SUMIFS(C1:C5,B1:B4,\"x\")");
    assert!(
        matches!(v, LiteralValue::Error(ref e) if e.kind == ExcelErrorKind::Value),
        "SUMIFS with a smaller criteria range must be #VALUE!, got {v:?}"
    );
    let v = eval("=COUNTIFS(A1:A8,\">0\",B1:B5,\">0\")");
    assert!(
        matches!(v, LiteralValue::Error(ref e) if e.kind == ExcelErrorKind::Value),
        "COUNTIFS with mismatched criteria ranges must be #VALUE!, got {v:?}"
    );
    // SUMIF (single) keeps Excel's top-left expansion of the sum range.
    assert_num("=SUMIF(A1:A8,\">1\",C1)", 5.0); // C5 (A5=2) + C8 (blank)
}

#[test]
fn parse_criteria_shapes() {
    use formualizer_eval::args::{CriteriaPredicate as P, parse_criteria};
    assert!(matches!(
        parse_criteria(&text("<>")).unwrap(),
        P::IsNotBlank
    ));
    assert!(matches!(
        parse_criteria(&text("")).unwrap(),
        P::IsBlankOrEmptyText
    ));
    assert!(
        matches!(parse_criteria(&text("1")).unwrap(), P::Eq(LiteralValue::Number(n)) if n == 1.0)
    );
    assert!(matches!(parse_criteria(&text(">a")).unwrap(), P::TextGt(ref s) if s == "a"));
}
