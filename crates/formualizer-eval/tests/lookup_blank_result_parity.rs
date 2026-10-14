//! Excel-parity pins for a lookup whose matched cell is blank (rowsncolumns/spreadsheet#1146):
//! VLOOKUP / HLOOKUP / LOOKUP return the blank cell's value — the same `Empty` a bare `=B2`
//! on a blank cell yields — and the consumer coerces it: 0 in arithmetic and comparison, "" in
//! text, 0 on display. So `+0` is 0, `=0` and `=""` are TRUE, `&"x"` is "x" and `LEN` is 0, in
//! exact and approximate mode alike. A cell holding `=""` is text, not blank, and stays "".
//! INDEX / XLOOKUP / INDEX-MATCH already behave this way and are pinned alongside.
//!
//! Seed grid (1-based): A1:C2 = [[1,2,3],[10,blank,30]] (the issue's HLOOKUP table) ·
//! E1:F3 = [[1,10],[2,blank],[3,30]] (the issue's VLOOKUP table) · H1:I3 = [[1,10],[2,`=""`],[3,30]].

use formualizer_common::{ExcelErrorKind, LiteralValue};
use formualizer_eval::engine::{Engine, EvalConfig};
use formualizer_eval::test_workbook::TestWorkbook;
use formualizer_parse::parser::parse;

fn seeded() -> Engine<TestWorkbook> {
    let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
    let num = |e: &mut Engine<TestWorkbook>, row: u32, col: u32, v: f64| {
        e.set_cell_value("Sheet1", row, col, LiteralValue::Number(v))
            .unwrap();
    };
    for (c, v) in [1.0, 2.0, 3.0].into_iter().enumerate() {
        num(&mut e, 1, c as u32 + 1, v);
    }
    num(&mut e, 2, 1, 10.0);
    num(&mut e, 2, 3, 30.0);
    for (r, (key, val)) in [(1.0, Some(10.0)), (2.0, None), (3.0, Some(30.0))]
        .into_iter()
        .enumerate()
    {
        let row = r as u32 + 1;
        num(&mut e, row, 5, key);
        num(&mut e, row, 8, key);
        if let Some(v) = val {
            num(&mut e, row, 6, v);
            num(&mut e, row, 9, v);
        }
    }
    e.set_cell_formula("Sheet1", 2, 9, parse("=\"\"").unwrap())
        .unwrap();
    e
}

/// Evaluate `formula` in K1 and read K1 back; a blank result reads back as `Empty`.
fn eval(formula: &str) -> LiteralValue {
    let mut e = seeded();
    e.set_cell_formula("Sheet1", 1, 11, parse(formula).unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    e.get_cell_value("Sheet1", 1, 11)
        .unwrap_or(LiteralValue::Empty)
}

fn num(n: f64) -> LiteralValue {
    LiteralValue::Number(n)
}

fn text(s: &str) -> LiteralValue {
    LiteralValue::Text(s.into())
}

fn is_error(v: &LiteralValue, kind: ExcelErrorKind) -> bool {
    matches!(v, LiteralValue::Error(e) if e.kind == kind)
}

/// Every value context Excel coerces a blank lookup result in, for one lookup expression.
fn assert_blank_result(lookup: &str) {
    assert_eq!(eval(&format!("={lookup}")), LiteralValue::Empty, "{lookup}");
    assert_eq!(eval(&format!("={lookup}+0")), num(0.0), "{lookup}+0");
    assert_eq!(eval(&format!("={lookup}*2")), num(0.0), "{lookup}*2");
    assert_eq!(
        eval(&format!("={lookup}=0")),
        LiteralValue::Boolean(true),
        "{lookup}=0"
    );
    assert_eq!(
        eval(&format!("={lookup}=\"\"")),
        LiteralValue::Boolean(true),
        "{lookup}=\"\""
    );
    assert_eq!(
        eval(&format!("={lookup}&\"x\"")),
        text("x"),
        "{lookup}&\"x\""
    );
    assert_eq!(eval(&format!("=LEN({lookup})")), num(0.0), "LEN({lookup})");
    assert_eq!(
        eval(&format!("=IF({lookup},1,2)")),
        num(2.0),
        "IF({lookup},1,2)"
    );
    assert_eq!(
        eval(&format!("=TEXT({lookup},\"0\")")),
        text("0"),
        "TEXT({lookup})"
    );
    assert_eq!(
        eval(&format!("=SUM({lookup},5)")),
        num(5.0),
        "SUM({lookup},5)"
    );
}

#[test]
fn a_bare_reference_to_a_blank_cell_is_the_baseline() {
    assert_blank_result("B2");
    assert_blank_result("F2");
}

#[test]
fn hlookup_of_a_blank_cell_is_the_blank_cell_in_exact_and_approximate_mode() {
    // Issue #1146: `=HLOOKUP(2,A1:C2,2,FALSE)` is 0 in Excel.
    assert_blank_result("HLOOKUP(2,A1:C2,2,FALSE)");
    assert_blank_result("HLOOKUP(2,A1:C2,2,TRUE)");
    assert_blank_result("HLOOKUP(2,A1:C2,2)");
    assert_blank_result("HLOOKUP(2.5,A1:C2,2,TRUE)");
}

#[test]
fn vlookup_of_a_blank_cell_is_the_blank_cell_in_exact_and_approximate_mode() {
    // Issue #1146: `=VLOOKUP(2,A1:B3,2,FALSE)` is 0 and `=VLOOKUP(2,A1:B3,2,FALSE)+0` is 0.
    assert_blank_result("VLOOKUP(2,E1:F3,2,FALSE)");
    assert_blank_result("VLOOKUP(2,E1:F3,2,TRUE)");
    assert_blank_result("VLOOKUP(2,E1:F3,2)");
    assert_blank_result("VLOOKUP(2.5,E1:F3,2,TRUE)");
}

#[test]
fn lookup_of_a_blank_cell_is_the_blank_cell_in_vector_and_array_form() {
    assert_blank_result("LOOKUP(2,E1:E3,F1:F3)");
    assert_blank_result("LOOKUP(2.5,E1:E3,F1:F3)");
    assert_blank_result("LOOKUP(2,E1:F3)");
    assert_blank_result("LOOKUP(2,A1:C2)");
}

#[test]
fn index_xlookup_and_index_match_of_a_blank_cell_are_the_blank_cell() {
    assert_blank_result("INDEX(E1:F3,2,2)");
    assert_blank_result("INDEX(F1:F3,MATCH(2,E1:E3,0))");
    assert_blank_result("XLOOKUP(2,E1:E3,F1:F3)");
    assert_blank_result("XLOOKUP(2,A1:C1,A2:C2)");
}

#[test]
fn a_matched_cell_holding_empty_text_is_text_not_blank() {
    // `=""` in I2: Excel returns "" and `+0` is #VALUE!.
    for lookup in [
        "VLOOKUP(2,H1:I3,2,FALSE)",
        "VLOOKUP(2,H1:I3,2,TRUE)",
        "LOOKUP(2,H1:H3,I1:I3)",
        "INDEX(H1:I3,2,2)",
        "XLOOKUP(2,H1:H3,I1:I3)",
        "VLOOKUP(2,{1,10;2,\"\";3,30},2,FALSE)",
        "HLOOKUP(2,{1,2,3;10,\"\",30},2,FALSE)",
    ] {
        assert_eq!(eval(&format!("={lookup}")), text(""), "{lookup}");
        assert_eq!(
            eval(&format!("={lookup}=\"\"")),
            LiteralValue::Boolean(true),
            "{lookup}=\"\""
        );
        assert_eq!(
            eval(&format!("={lookup}=0")),
            LiteralValue::Boolean(false),
            "{lookup}=0"
        );
        assert_eq!(
            eval(&format!("={lookup}&\"x\"")),
            text("x"),
            "{lookup}&\"x\""
        );
        assert!(
            is_error(&eval(&format!("={lookup}+0")), ExcelErrorKind::Value),
            "{lookup}+0"
        );
    }
}

#[test]
fn non_blank_matches_still_return_the_matched_value() {
    assert_eq!(eval("=VLOOKUP(3,E1:F3,2,FALSE)"), num(30.0));
    assert_eq!(eval("=VLOOKUP(1,E1:F3,2,FALSE)"), num(10.0));
    assert_eq!(eval("=VLOOKUP(3.5,E1:F3,2,TRUE)"), num(30.0));
    assert_eq!(eval("=HLOOKUP(3,A1:C2,2,FALSE)"), num(30.0));
    assert_eq!(eval("=HLOOKUP(1,A1:C2,2,FALSE)"), num(10.0));
    assert_eq!(eval("=HLOOKUP(3.5,A1:C2,2,TRUE)"), num(30.0));
    assert_eq!(eval("=LOOKUP(3,E1:E3,F1:F3)"), num(30.0));
    assert!(is_error(
        &eval("=VLOOKUP(4,E1:F3,2,FALSE)"),
        ExcelErrorKind::Na
    ));
    assert!(is_error(
        &eval("=HLOOKUP(4,A1:C2,2,FALSE)"),
        ExcelErrorKind::Na
    ));
}
