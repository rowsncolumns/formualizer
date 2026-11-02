//! XLOOKUP / XMATCH `match_mode` 3 — regex match (demo-app-monorepo#49969 FX-11). The lookup value
//! is a regular expression in REGEXTEST's flavour (case-sensitive, a match anywhere in the cell);
//! the first matching cell wins, the last one with `search_mode` -1. An invalid pattern is `#VALUE!`.
//! <https://support.microsoft.com/en-us/office/xlookup-function-b7fd680e-6d10-43e6-84f9-88eae8bf5929>
//! <https://support.microsoft.com/en-us/office/xmatch-function-d966da31-7a6b-4a13-a1c6-5a33ed6a0312>
//!
//! Seed grid (1-based): A1:A4 = "apple","Banana","cherry",42; B1:B4 = 1,2,3,4.

use formualizer_common::{ExcelErrorKind, LiteralValue};
use formualizer_eval::engine::{Engine, EvalConfig};
use formualizer_eval::test_workbook::TestWorkbook;
use formualizer_parse::parser::parse;

fn eval(formula: &str) -> LiteralValue {
    let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
    let keys = [
        LiteralValue::Text("apple".into()),
        LiteralValue::Text("Banana".into()),
        LiteralValue::Text("cherry".into()),
        LiteralValue::Number(42.0),
    ];
    for (i, k) in keys.into_iter().enumerate() {
        let r = i as u32 + 1;
        e.set_cell_value("Sheet1", r, 1, k).unwrap();
        e.set_cell_value("Sheet1", r, 2, LiteralValue::Number(r as f64))
            .unwrap();
    }
    e.set_cell_formula("Sheet1", 1, 5, parse(formula).unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    e.get_cell_value("Sheet1", 1, 5)
        .unwrap_or(LiteralValue::Empty)
}

fn num(v: LiteralValue) -> f64 {
    match v {
        LiteralValue::Number(n) => n,
        LiteralValue::Int(i) => i as f64,
        other => panic!("expected a number, got {other:?}"),
    }
}

fn kind(v: LiteralValue) -> ExcelErrorKind {
    match v {
        LiteralValue::Error(e) => e.kind,
        other => panic!("expected an error, got {other:?}"),
    }
}

#[test]
fn xmatch_regex_mode() {
    assert_eq!(num(eval("=XMATCH(\"^c\",A1:A4,3)")), 3.0);
    assert_eq!(num(eval("=XMATCH(\"an+a\",A1:A4,3)")), 2.0);
    // Case-sensitive, like REGEXTEST without its flag; (?i) opts in.
    assert_eq!(kind(eval("=XMATCH(\"^b\",A1:A4,3)")), ExcelErrorKind::Na);
    assert_eq!(num(eval("=XMATCH(\"(?i)^b\",A1:A4,3)")), 2.0);
    // Numbers are matched by their text.
    assert_eq!(num(eval("=XMATCH(\"^\\d+$\",A1:A4,3)")), 4.0);
    // Last match when searching last-to-first.
    assert_eq!(num(eval("=XMATCH(\"[a-z]\",A1:A4,3,-1)")), 3.0);
    assert_eq!(kind(eval("=XMATCH(\"(\",A1:A4,3)")), ExcelErrorKind::Value);
}

#[test]
fn xlookup_regex_mode() {
    assert_eq!(num(eval("=XLOOKUP(\"rr\",A1:A4,B1:B4,,3)")), 3.0);
    assert_eq!(num(eval("=XLOOKUP(\"^a.*e$\",A1:A4,B1:B4,,3)")), 1.0);
    assert_eq!(
        eval("=XLOOKUP(\"^z\",A1:A4,B1:B4,\"none\",3)"),
        LiteralValue::Text("none".into())
    );
    assert_eq!(num(eval("=XLOOKUP(\"a\",A1:A4,B1:B4,,3,-1)")), 2.0);
    assert_eq!(
        kind(eval("=XLOOKUP(\"[\",A1:A4,B1:B4,,3)")),
        ExcelErrorKind::Value
    );
}
