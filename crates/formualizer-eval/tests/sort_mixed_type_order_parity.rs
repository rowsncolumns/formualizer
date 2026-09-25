//! Excel-parity pins for `SORT` / `SORTBY` on mixed-type keys (rowsncolumns/spreadsheet#939 G-05).
//!
//! Excel collates a sort key by TYPE first — numbers (incl. dates) < text (case-insensitive) <
//! logicals (FALSE < TRUE) < errors — and blanks go LAST in both directions; descending reverses
//! the type order. The fork compared keys with the lookup comparator, which coerces `TRUE`→1 and
//! numeric-looking text to numbers and reports every other cross-type pair as "equal", so a mixed
//! column came back in input order (`SORT(G1:G5)` → `{2;"b";TRUE;"a";1}`) or with text before
//! numbers (`SORT({"b";"a";3})` → `{"a";"b";3}`).
//!
//! Seed grid (1-based): A1:A5 = 10,20,30,40,50 · G1:G5 = 2,"b",TRUE,"a",1 · H1:H4 = 3,blank,1,"x" ·
//! U1:U5 = 1,1,2,2,1.

use formualizer_common::{ExcelErrorKind, LiteralValue};
use formualizer_eval::engine::{Engine, EvalConfig};
use formualizer_eval::test_workbook::TestWorkbook;
use formualizer_parse::parser::parse;

fn seeded() -> Engine<TestWorkbook> {
    let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
    for i in 0..5u32 {
        e.set_cell_value(
            "Sheet1",
            i + 1,
            1,
            LiteralValue::Number(((i + 1) * 10) as f64),
        )
        .unwrap();
    }
    let g = [
        LiteralValue::Number(2.0),
        LiteralValue::Text("b".into()),
        LiteralValue::Boolean(true),
        LiteralValue::Text("a".into()),
        LiteralValue::Number(1.0),
    ];
    for (i, v) in g.iter().enumerate() {
        e.set_cell_value("Sheet1", (i + 1) as u32, 7, v.clone())
            .unwrap();
    }
    e.set_cell_value("Sheet1", 1, 8, LiteralValue::Number(3.0))
        .unwrap();
    e.set_cell_value("Sheet1", 3, 8, LiteralValue::Number(1.0))
        .unwrap();
    e.set_cell_value("Sheet1", 4, 8, LiteralValue::Text("x".into()))
        .unwrap();
    for (i, v) in [1.0, 1.0, 2.0, 2.0, 1.0].iter().enumerate() {
        e.set_cell_value("Sheet1", (i + 1) as u32, 21, LiteralValue::Number(*v))
            .unwrap();
    }
    e
}

/// Evaluate `formula` in K1 and return the K column spill as a list (blank → `Empty`).
fn eval_col(formula: &str, rows: u32) -> Vec<LiteralValue> {
    let mut e = seeded();
    e.set_cell_formula("Sheet1", 1, 11, parse(formula).unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    (0..rows)
        .map(|r| {
            e.get_cell_value("Sheet1", 1 + r, 11)
                .unwrap_or(LiteralValue::Empty)
        })
        .collect()
}

fn num(n: f64) -> LiteralValue {
    LiteralValue::Number(n)
}
fn text(s: &str) -> LiteralValue {
    LiteralValue::Text(s.into())
}
fn boolean(b: bool) -> LiteralValue {
    LiteralValue::Boolean(b)
}

fn assert_col(formula: &str, expected: &[LiteralValue]) {
    let got = eval_col(formula, expected.len() as u32);
    // Ints and Numbers are the same Excel value.
    let norm = |v: &LiteralValue| match v {
        LiteralValue::Int(i) => LiteralValue::Number(*i as f64),
        other => other.clone(),
    };
    let got: Vec<LiteralValue> = got.iter().map(norm).collect();
    let expected: Vec<LiteralValue> = expected.iter().map(norm).collect();
    assert_eq!(got, expected, "{formula}");
}

#[test]
fn sort_mixed_column_numbers_then_text_then_logicals() {
    assert_col(
        "=SORT(G1:G5)",
        &[num(1.0), num(2.0), text("a"), text("b"), boolean(true)],
    );
    assert_col("=SORT({\"b\";\"a\";3})", &[num(3.0), text("a"), text("b")]);
    assert_col("=SORT({3;1;TRUE})", &[num(1.0), num(3.0), boolean(true)]);
    assert_col("=SORT({3;\"a\";1})", &[num(1.0), num(3.0), text("a")]);
    assert_col(
        "=SORT({TRUE;FALSE;\"z\";0})",
        &[num(0.0), text("z"), boolean(false), boolean(true)],
    );
}

#[test]
fn sort_descending_reverses_the_type_order() {
    assert_col(
        "=SORT(G1:G5,1,-1)",
        &[boolean(true), text("b"), text("a"), num(2.0), num(1.0)],
    );
    assert_col("=SORT({3;\"a\";1},1,-1)", &[text("a"), num(3.0), num(1.0)]);
}

#[test]
fn sort_numeric_looking_text_stays_text_and_text_is_case_insensitive() {
    // "1" and "10" are text: after the number 2, in lexicographic (not numeric) order.
    assert_col(
        "=SORT({\"10\";2;\"1\"})",
        &[num(2.0), text("1"), text("10")],
    );
    // Case-insensitive, stable for equal keys ("A" and "a" keep their input order).
    assert_col(
        "=SORT({\"b\";\"A\";\"a\";\"B\"})",
        &[text("A"), text("a"), text("b"), text("B")],
    );
}

#[test]
fn sort_blanks_last_in_both_directions() {
    assert_col(
        "=SORT(H1:H4)",
        &[num(1.0), num(3.0), text("x"), LiteralValue::Empty],
    );
    assert_col(
        "=SORT(H1:H4,1,-1)",
        &[text("x"), num(3.0), num(1.0), LiteralValue::Empty],
    );
}

#[test]
fn sort_errors_after_logicals_ascending_and_first_descending() {
    let asc = eval_col("=SORT({TRUE;1;\"a\";#N/A})", 4);
    assert_eq!(asc[0], num(1.0));
    assert_eq!(asc[1], text("a"));
    assert_eq!(asc[2], boolean(true));
    match &asc[3] {
        LiteralValue::Error(e) => assert_eq!(e.kind, ExcelErrorKind::Na),
        other => panic!("expected #N/A last, got {other:?}"),
    }
    let desc = eval_col("=SORT({TRUE;1;\"a\";#N/A},1,-1)", 4);
    assert!(
        matches!(&desc[0], LiteralValue::Error(e) if e.kind == ExcelErrorKind::Na),
        "{desc:?}"
    );
    assert_eq!(desc[1], boolean(true));
    assert_eq!(desc[2], text("a"));
    assert_eq!(desc[3], num(1.0));
}

#[test]
fn sortby_uses_the_same_collation_for_every_key_order_pair() {
    // Keys G1:G5 = 2,"b",TRUE,"a",1 → 1 (A5), 2 (A1), "a" (A4), "b" (A2), TRUE (A3).
    assert_col(
        "=SORTBY(A1:A5,G1:G5)",
        &[num(50.0), num(10.0), num(40.0), num(20.0), num(30.0)],
    );
    assert_col(
        "=SORTBY(A1:A5,G1:G5,-1)",
        &[num(30.0), num(20.0), num(40.0), num(10.0), num(50.0)],
    );
    // Two key/order pairs: U ascending (1,1,2,2,1), then G descending inside each U group.
    // U=1 rows 1,2,5 → G = 2,"b",1 → desc: "b"(20), 2(10), 1(50); U=2 rows 3,4 → TRUE(30), "a"(40).
    assert_col(
        "=SORTBY(A1:A5,U1:U5,1,G1:G5,-1)",
        &[num(20.0), num(10.0), num(50.0), num(30.0), num(40.0)],
    );
}
