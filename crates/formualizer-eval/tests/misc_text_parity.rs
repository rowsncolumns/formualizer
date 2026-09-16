//! rowsncolumns/spreadsheet#546 U17a — misc text-function Excel parity (B-15, B-18, B-29,
//! text rows of B-30): Unicode case mapping in `UPPER`/`LOWER`, `TRIM` touching only ASCII
//! spaces, digits as word boundaries in `PROPER`, number → text coercion rounded to 15
//! significant digits everywhere text is read (`LEN(1/3)` = 17, `&`), and `TEXT(TRUE,"0")`
//! returning the boolean's text. Also pins the `REPT` / `SUBSTITUTE` / `FIXED` edge cases that
//! were already right on this engine so they stay aligned with the JS engine's fixes.

use formualizer_common::{ExcelErrorKind, LiteralValue};
use formualizer_eval::engine::{Engine, EvalConfig};
use formualizer_eval::test_workbook::TestWorkbook;
use formualizer_parse::parser::parse;

fn eval(formula: &str) -> LiteralValue {
    let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
    e.set_cell_value("Sheet1", 1, 1, LiteralValue::Number(1.0 / 3.0))
        .unwrap();
    e.set_cell_value("Sheet1", 2, 1, LiteralValue::Number(0.5))
        .unwrap();
    e.set_cell_formula("Sheet1", 1, 5, parse(formula).unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    e.get_cell_value("Sheet1", 1, 5).unwrap()
}

fn text(s: &str) -> LiteralValue {
    LiteralValue::Text(s.into())
}

fn assert_number(formula: &str, want: f64) {
    match eval(formula) {
        LiteralValue::Number(n) => assert_eq!(n, want, "{formula}"),
        LiteralValue::Int(i) => assert_eq!(i as f64, want, "{formula}"),
        other => panic!("{formula}: expected {want}, got {other:?}"),
    }
}

fn assert_error(formula: &str, kind: ExcelErrorKind) {
    match eval(formula) {
        LiteralValue::Error(e) => assert_eq!(e.kind, kind, "{formula}"),
        other => panic!("{formula}: expected {kind:?}, got {other:?}"),
    }
}

#[test]
fn upper_lower_use_unicode_case_mapping() {
    assert_eq!(eval("=UPPER(\"aé\")"), text("AÉ"));
    assert_eq!(eval("=LOWER(\"AÉ\")"), text("aé"));
    assert_eq!(eval("=UPPER(\"straße ñ ω\")"), text("STRASSE Ñ Ω"));
    assert_eq!(eval("=LOWER(\"ĞÜŞ\")"), text("ğüş"));
    assert_eq!(eval("=UPPER(TRUE)"), text("TRUE"));
    assert_eq!(eval("=LOWER(12.5)"), text("12.5"));
}

#[test]
fn proper_treats_digits_as_word_boundaries() {
    assert_eq!(eval("=PROPER(\"2nd\")"), text("2Nd"));
    assert_eq!(
        eval("=PROPER(\"hello world-o'neil\")"),
        text("Hello World-O'Neil")
    );
    assert_eq!(eval("=PROPER(\"über öl\")"), text("Über Öl"));
    assert_eq!(eval("=PROPER(\"76BudGet\")"), text("76Budget"));
}

#[test]
fn trim_only_touches_ascii_spaces() {
    assert_eq!(eval("=TRIM(\"  a   b  \")"), text("a b"));
    assert_eq!(eval("=TRIM(\"a\")"), text("a"));
    assert_eq!(eval("=TRIM(\"   \")"), text(""));
    // The non-breaking space (160), tabs and line breaks are not spaces to TRIM.
    assert_number("=LEN(TRIM(UNICHAR(160)&\"a\"))", 2.0);
    assert_number("=LEN(TRIM(\"a\"&CHAR(9)&CHAR(9)&\"b\"))", 4.0);
    assert_number("=LEN(TRIM(CHAR(10)&\"a\"))", 2.0);
}

#[test]
fn numbers_read_as_text_use_fifteen_significant_digits() {
    assert_number("=LEN(1/3)", 17.0);
    assert_number("=LEN(A1)", 17.0);
    assert_number("=LEN(0.1+0.2)", 3.0);
    assert_number("=LEN(123.4)", 5.0);
    assert_eq!(eval("=\"x\"&1/3"), text("x0.333333333333333"));
    assert_eq!(eval("=A1&\"\""), text("0.333333333333333"));
    assert_eq!(eval("=2/3&\"\""), text("0.666666666666667"));
    assert_eq!(eval("=CONCATENATE(1/3)"), text("0.333333333333333"));
    assert_eq!(eval("=LEFT(1/3,5)"), text("0.333"));
    assert_eq!(eval("=RIGHT(2/3,1)"), text("7"));
    assert_eq!(eval("=MID(1/3,3,2)"), text("33"));
    assert_number("=FIND(\"7\",2/3)", 17.0);
    assert_eq!(
        eval("=SUBSTITUTE(1/3,\"0.\",\"\")"),
        text("333333333333333")
    );
    assert_eq!(
        eval("=TEXTJOIN(\",\",TRUE,1/3,0.5)"),
        text("0.333333333333333,0.5")
    );
    // Integers, trailing zeros and Excel's General switch to scientific notation.
    assert_eq!(eval("=42&\"\""), text("42"));
    assert_eq!(eval("=1.5&\"\""), text("1.5"));
    assert_eq!(eval("=100000000000000&\"\""), text("100000000000000"));
    assert_eq!(eval("=10^15&\"\""), text("1E+15"));
    // The literal itself is read with 15 significant digits and truncated at parse
    // (Excel stores 1234567890123450000), so the text spells the truncated value.
    assert_eq!(
        eval("=1234567890123456789&\"\""),
        text("1.23456789012345E+18")
    );
    assert_eq!(eval("=0.0001&\"\""), text("0.0001"));
    assert_eq!(eval("=0.00001&\"\""), text("1E-05"));
    assert_eq!(eval("=-1/3&\"\""), text("-0.333333333333333"));
}

#[test]
fn text_returns_booleans_as_their_text() {
    assert_eq!(eval("=TEXT(TRUE,\"0\")"), text("TRUE"));
    assert_eq!(eval("=TEXT(FALSE,\"0.00\")"), text("FALSE"));
    assert_eq!(eval("=TEXT(0.5,\"0%\")"), text("50%"));
}

#[test]
fn rept_substitute_fixed_edge_cases_stay_excel_aligned() {
    assert_eq!(eval("=REPT(\"ab\",2.9)"), text("abab"));
    assert_eq!(eval("=REPT(\"ab\",0)"), text(""));
    assert_error("=REPT(\"a\",-1)", ExcelErrorKind::Value);
    assert_error("=REPT(\"a\",32768)", ExcelErrorKind::Value);
    assert_eq!(eval("=SUBSTITUTE(\"aaa\",\"\",\"b\")"), text("aaa"));
    assert_eq!(eval("=SUBSTITUTE(\"aaa\",\"a\",\"b\",2)"), text("aba"));
    assert_eq!(eval("=FIXED(1234.567,-2)"), text("1,200"));
    assert_eq!(eval("=FIXED(1234.567,1)"), text("1,234.6"));
    assert_eq!(eval("=FIXED(1234.567,1,TRUE)"), text("1234.6"));
    assert_eq!(eval("=FIXED(-1234.567)"), text("-1,234.57"));
}

#[test]
fn value_and_array_to_text_spell_numbers_at_fifteen_digits() {
    // Fixup for spreadsheet#546 U17a review: VALUETOTEXT / ARRAYTOTEXT go through the same
    // 15-significant-digit General conversion as `&` and LEN, in both formats.
    assert_eq!(eval("=VALUETOTEXT(1/3)"), text("0.333333333333333"));
    assert_eq!(eval("=VALUETOTEXT(A1,1)"), text("0.333333333333333"));
    assert_eq!(eval("=ARRAYTOTEXT({0.5,1234.5}&\"\")"), text("0.5, 1234.5"));
    assert_eq!(eval("=ARRAYTOTEXT(A1:A2)"), text("0.333333333333333, 0.5"));
    assert_eq!(
        eval("=ARRAYTOTEXT(A1:A2,1)"),
        text("{0.333333333333333;0.5}")
    );
    assert_eq!(eval("=CONCAT(A1:A2)"), text("0.3333333333333330.5"));
    assert_eq!(
        eval("=CONCAT(\"a\",A1:A2,TRUE)"),
        text("a0.3333333333333330.5TRUE")
    );
    assert_eq!(eval("=CONCAT({1,2;3,4})"), text("1234"));
    assert_number("=LEN(CONCAT(A1:A2))", 20.0);
    assert_eq!(
        eval("=TEXTJOIN(\",\",TRUE,A1:A2)"),
        text("0.333333333333333,0.5")
    );
}

#[test]
fn exact_sixteen_digit_ties_round_away_from_zero_like_excel() {
    // A typed literal is stored with 15 significant digits (Excel truncates `1234567890123445` to
    // `1234567890123440` at entry, and so does the parser), so the exact 16-digit ties have to be
    // COMPUTED: the doubles below are exact and Excel spells them half away from zero.
    assert_eq!(
        eval("=(1234567890123440+5)&\"\""),
        text("1.23456789012345E+15")
    );
    assert_eq!(eval("=(123456789012344+0.5)&\"\""), text("123456789012345"));
    assert_eq!(
        eval("=(-1234567890123440-5)&\"\""),
        text("-1.23456789012345E+15")
    );
    assert_number("=LEN((123456789012344+0.5)&\"\")", 15.0);
    // The literal itself keeps Excel's typed-entry truncation.
    assert_eq!(eval("=1234567890123445&\"\""), text("1.23456789012344E+15"));
}

#[test]
fn fixed_caps_decimals_at_127() {
    assert_error("=FIXED(1,128)", ExcelErrorKind::Value);
    assert_error("=FIXED(1,1000)", ExcelErrorKind::Value);
    assert_number("=LEN(FIXED(1,127))", 129.0);
    assert_eq!(eval("=FIXED(1,127.9)"), eval("=FIXED(1,127)"));
}

#[test]
fn if_propagates_an_error_condition() {
    // B-32: Excel returns the condition's own error, not #VALUE!.
    assert_error("=IF(NA(),1,2)", ExcelErrorKind::Na);
    assert_error("=IF(1/0,1,2)", ExcelErrorKind::Div);
    assert_error("=IF(\"abc\",1,2)", ExcelErrorKind::Value);
    assert_number("=IF(TRUE,1,2)", 1.0);
}
