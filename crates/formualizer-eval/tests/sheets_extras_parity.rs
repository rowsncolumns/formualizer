//! rowsncolumns/spreadsheet#546 A-17 — the Google Sheets extras the legacy JS engine carries
//! (`SPLIT`, `JOIN`, `REGEXMATCH`, `SORTN`, `ARRAYFORMULA`, and the `QUERY` stub) evaluate on this
//! engine with the JS engine's semantics, so a workbook authored on the legacy engine keeps its
//! values when it is opened on the Rust engine instead of turning into `#NAME?`.

use formualizer_common::{ExcelErrorKind, LiteralValue};
use formualizer_eval::engine::{Engine, EvalConfig};
use formualizer_eval::test_workbook::TestWorkbook;
use formualizer_parse::parser::parse;

fn n(v: f64) -> LiteralValue {
    LiteralValue::Number(v)
}
fn t(s: &str) -> LiteralValue {
    LiteralValue::Text(s.into())
}
fn b(v: bool) -> LiteralValue {
    LiteralValue::Boolean(v)
}
fn err_kind(v: &LiteralValue) -> ExcelErrorKind {
    match v {
        LiteralValue::Error(e) => e.kind,
        other => panic!("expected error, got {other:?}"),
    }
}

/// A1:B3 = (3,"c"),(1,"a"),(2,"b"); C1 = "x,y;;z"; D1 = TRUE; E1 blank.
fn seeded() -> Engine<TestWorkbook> {
    let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
    for (i, (a, s)) in [(3.0, "c"), (1.0, "a"), (2.0, "b")].iter().enumerate() {
        let r = i as u32 + 1;
        e.set_cell_value("Sheet1", r, 1, n(*a)).unwrap();
        e.set_cell_value("Sheet1", r, 2, t(s)).unwrap();
    }
    e.set_cell_value("Sheet1", 1, 3, t("x,y;;z")).unwrap();
    e.set_cell_value("Sheet1", 1, 4, b(true)).unwrap();
    e
}

/// Evaluate `formula` anchored at K20 and read back a `rows`×`cols` block from the anchor.
fn spill(formula: &str, rows: u32, cols: u32) -> Vec<Vec<LiteralValue>> {
    let mut e = seeded();
    e.set_cell_formula("Sheet1", 20, 11, parse(formula).unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    (0..rows)
        .map(|r| {
            (0..cols)
                .map(|c| {
                    e.get_cell_value("Sheet1", 20 + r, 11 + c)
                        .unwrap_or(LiteralValue::Empty)
                })
                .collect()
        })
        .collect()
}

fn scalar(formula: &str) -> LiteralValue {
    spill(formula, 1, 1).remove(0).remove(0)
}

fn row(vals: &[&str]) -> Vec<Vec<LiteralValue>> {
    vec![vals.iter().map(|s| t(s)).collect()]
}

#[test]
fn sheets_extras_are_registered() {
    formualizer_eval::builtins::load_builtins();
    for name in [
        "SPLIT",
        "JOIN",
        "REGEXMATCH",
        "SORTN",
        "ARRAYFORMULA",
        "QUERY",
    ] {
        assert!(
            formualizer_eval::function_registry::get("", name).is_some(),
            "{name} should be registered"
        );
    }
}

// ───────────────────────── SPLIT ─────────────────────────

#[test]
fn split_on_each_delimiter_character_by_default() {
    assert_eq!(
        spill("=SPLIT(\"a,b;c\", \",;\")", 1, 3),
        row(&["a", "b", "c"])
    );
    assert_eq!(spill("=SPLIT(C1, \",;\")", 1, 3), row(&["x", "y", "z"]));
}

#[test]
fn split_on_the_whole_delimiter_when_split_by_each_is_false() {
    assert_eq!(scalar("=SPLIT(\"a,b;c\", \",;\", FALSE)"), t("a,b;c"));
    assert_eq!(
        spill("=SPLIT(\"a::b::c\", \"::\", FALSE)", 1, 3),
        row(&["a", "b", "c"])
    );
}

#[test]
fn split_keeps_empty_pieces_only_when_asked() {
    assert_eq!(spill("=SPLIT(\"a,,b\", \",\")", 1, 2), row(&["a", "b"]));
    assert_eq!(
        spill("=SPLIT(\"a,,b\", \",\", TRUE, FALSE)", 1, 3),
        row(&["a", "", "b"])
    );
}

#[test]
fn split_pieces_stay_text_like_the_js_engine() {
    // "1,2" splits into the TEXT pieces "1" and "2" — no number coercion.
    assert_eq!(spill("=SPLIT(\"1,2\", \",\")", 1, 2), row(&["1", "2"]));
    assert_eq!(
        scalar("=ISTEXT(INDEX(SPLIT(\"1,2\", \",\"), 1, 1))"),
        b(true)
    );
}

#[test]
fn split_with_an_empty_delimiter_is_value_error() {
    assert_eq!(
        err_kind(&scalar("=SPLIT(\"abc\", \"\")")),
        ExcelErrorKind::Value
    );
}

#[test]
fn split_coerces_a_numeric_text_argument() {
    assert_eq!(spill("=SPLIT(1234.5, \".\")", 1, 2), row(&["1234", "5"]));
}

// ───────────────────────── JOIN ─────────────────────────

#[test]
fn join_flattens_a_range_in_row_major_order() {
    assert_eq!(scalar("=JOIN(\",\", A1:A3)"), t("3,1,2"));
    assert_eq!(scalar("=JOIN(\"-\", A1:B2)"), t("3-c-1-a"));
    assert_eq!(scalar("=JOIN(\",\", {1,2;3,4})"), t("1,2,3,4"));
}

#[test]
fn join_skips_blanks_and_empty_text_and_spells_values_like_textjoin() {
    assert_eq!(scalar("=JOIN(\"-\", \"a\", \"\", \"b\", E1)"), t("a-b"));
    assert_eq!(scalar("=JOIN(\",\", TRUE, 1.5, D1)"), t("TRUE,1.5,TRUE"));
    assert_eq!(scalar("=JOIN(\",\", 1/3)"), t("0.333333333333333"));
}

#[test]
fn join_propagates_an_error_argument() {
    assert_eq!(
        err_kind(&scalar("=JOIN(\",\", 1, NA())")),
        ExcelErrorKind::Na
    );
}

// ───────────────────────── REGEXMATCH ─────────────────────────

#[test]
fn regexmatch_is_a_case_sensitive_search() {
    assert_eq!(scalar("=REGEXMATCH(\"hello\", \"^h.*o$\")"), b(true));
    assert_eq!(scalar("=REGEXMATCH(\"hello\", \"ell\")"), b(true));
    assert_eq!(scalar("=REGEXMATCH(\"hello\", \"^H\")"), b(false));
    assert_eq!(scalar("=REGEXMATCH(B2, \"^[a-c]$\")"), b(true));
}

#[test]
fn regexmatch_coerces_numbers_and_rejects_an_invalid_pattern() {
    assert_eq!(scalar("=REGEXMATCH(123, \"^\\d+$\")"), b(true));
    assert_eq!(
        err_kind(&scalar("=REGEXMATCH(\"a\", \"(\")")),
        ExcelErrorKind::Value
    );
}

// ───────────────────────── SORTN ─────────────────────────

#[test]
fn sortn_returns_the_first_n_rows_sorted_on_the_first_column() {
    assert_eq!(
        spill("=SORTN(A1:B3, 2)", 2, 2),
        vec![vec![n(1.0), t("a")], vec![n(2.0), t("b")]]
    );
}

#[test]
fn sortn_defaults_to_every_row() {
    assert_eq!(
        spill("=SORTN(A1:B3)", 3, 2),
        vec![
            vec![n(1.0), t("a")],
            vec![n(2.0), t("b")],
            vec![n(3.0), t("c")]
        ]
    );
}

#[test]
fn sortn_takes_a_sort_column_and_direction_and_ignores_display_ties_mode() {
    assert_eq!(
        spill("=SORTN(A1:B3, 1, 0, 2, FALSE)", 1, 2),
        vec![vec![n(3.0), t("c")]]
    );
    assert_eq!(
        spill("=SORTN(A1:B3, 1, 1, 1, TRUE)", 1, 2),
        vec![vec![n(1.0), t("a")]]
    );
    assert_eq!(spill("=SORTN(A1:B3, 5)", 3, 2).len(), 3);
}

#[test]
fn sortn_rejects_a_sort_column_outside_the_range_and_a_negative_n() {
    assert_eq!(
        err_kind(&scalar("=SORTN(A1:B3, 2, 0, 5)")),
        ExcelErrorKind::Value
    );
    assert_eq!(
        err_kind(&scalar("=SORTN(A1:B3, -1)")),
        ExcelErrorKind::Value
    );
}

// ───────────────────────── ARRAYFORMULA ─────────────────────────

#[test]
fn arrayformula_is_the_identity_over_its_argument() {
    assert_eq!(
        spill("=ARRAYFORMULA(A1:A3*2)", 3, 1),
        vec![vec![n(6.0)], vec![n(2.0)], vec![n(4.0)]]
    );
    assert_eq!(
        spill("=ARRAYFORMULA({1,2,3})", 1, 3),
        vec![vec![n(1.0), n(2.0), n(3.0)]]
    );
    assert_eq!(scalar("=ARRAYFORMULA(1+1)"), n(2.0));
    assert_eq!(scalar("=ARRAYFORMULA(B2)"), t("a"));
}

// ───────────────────────── QUERY ─────────────────────────

#[test]
fn query_is_a_recognised_call_that_yields_na() {
    assert_eq!(
        err_kind(&scalar("=QUERY(A1:B3, \"select A\")")),
        ExcelErrorKind::Na
    );
    assert_eq!(scalar("=ISNA(QUERY(A1:B3, \"select A\", 1))"), b(true));
    assert_eq!(
        scalar("=IFNA(QUERY(A1:B3, \"select A\"), \"no query engine\")"),
        t("no query engine")
    );
}
