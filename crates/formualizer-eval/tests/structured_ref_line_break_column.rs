//! A table column whose name holds a line break (Alt+Enter in the header cell) resolves from a
//! structured reference, as in Excel. The name is written literally inside the brackets; the
//! header cell's text, the table part's `<tableColumn name>` and the formula text can carry the
//! break as CR LF or LF, and all of them name the same column (matching stays case-insensitive).
//! Before the fix any spelling that differed from the registered header (CR LF vs LF) was an
//! unknown column, so the reference — and everything downstream of it — was `#REF!`.
//! https://support.microsoft.com/en-us/office/using-structured-references-with-excel-tables-f5ed2452-2337-4f71-bed3-c8ae6d2b276e

use formualizer_common::LiteralValue;
use formualizer_eval::engine::{Engine, EvalConfig};
use formualizer_eval::reference::{CellRef, Coord, RangeRef};
use formualizer_eval::test_workbook::TestWorkbook;
use formualizer_parse::parser::parse;

fn n(v: f64) -> LiteralValue {
    LiteralValue::Number(v)
}

fn t(s: &str) -> LiteralValue {
    LiteralValue::Text(s.into())
}

/// Sheet1!A1:C3 = table `T`: headers `Id`, `Code\r\n(new)` (the header cell holds CR LF, as Excel's
/// shared string does) and `Qty`; rows (a, X1, 2) and (b, X2, 3). The column names are registered
/// as the table part spells them — LF only.
fn seeded() -> Engine<TestWorkbook> {
    let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
    let rows: [[LiteralValue; 3]; 3] = [
        [t("Id"), t("Code\r\n(new)"), t("Qty")],
        [t("a"), t("X1"), n(2.0)],
        [t("b"), t("X2"), n(3.0)],
    ];
    for (r, row) in rows.into_iter().enumerate() {
        for (c, v) in row.into_iter().enumerate() {
            e.set_cell_value("Sheet1", r as u32 + 1, c as u32 + 1, v)
                .unwrap();
        }
    }
    let sid = e.sheet_id("Sheet1").unwrap();
    e.define_table(
        "T",
        RangeRef::new(
            CellRef::new(sid, Coord::from_excel(1, 1, true, true)),
            CellRef::new(sid, Coord::from_excel(3, 3, true, true)),
        ),
        true,
        vec!["Id".into(), "Code\n(new)".into(), "Qty".into()],
        false,
    )
    .unwrap();
    e
}

fn eval_at(e: &mut Engine<TestWorkbook>, row: u32, col: u32, formula: &str) -> LiteralValue {
    e.set_cell_formula("Sheet1", row, col, parse(formula).unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    e.get_cell_value("Sheet1", row, col)
        .unwrap_or(LiteralValue::Empty)
}

#[test]
fn this_row_reference_to_a_line_break_column_resolves_lf_and_crlf() {
    let mut e = seeded();
    // A calculated column beside the table, on data row 2 (the first body row).
    assert_eq!(
        eval_at(&mut e, 2, 5, "=T[[#This Row],[Code\n(new)]]"),
        t("X1")
    );
    assert_eq!(
        eval_at(&mut e, 3, 5, "=T[[#This Row],[Code\r\n(new)]]"),
        t("X2")
    );
    // Case-insensitive like every column name.
    assert_eq!(
        eval_at(&mut e, 2, 6, "=T[[#This Row],[CODE\r\n(NEW)]]"),
        t("X1")
    );
    // The shorthand `[@[…]]` form.
    assert_eq!(eval_at(&mut e, 3, 6, "=T[@[Code\n(new)]]"), t("X2"));
}

#[test]
fn headers_and_whole_column_references_to_a_line_break_column() {
    let mut e = seeded();
    assert_eq!(
        eval_at(&mut e, 10, 8, "=T[[#Headers],[Code\r\n(new)]]"),
        t("Code\r\n(new)")
    );
    assert_eq!(
        eval_at(&mut e, 11, 8, "=COUNTA(T[[Code\r\n(new)]])"),
        n(2.0)
    );
    assert_eq!(eval_at(&mut e, 12, 8, "=COUNTA(T[Code\n(new)])"), n(2.0));
    assert_eq!(
        eval_at(&mut e, 13, 8, "=COUNTA(T[[Code\r\n(new)]:[Qty]])"),
        n(4.0)
    );
}

#[test]
fn a_dependent_of_a_line_break_column_reference_computes() {
    let mut e = seeded();
    // E2 = Id & "-" & Code (the customer-workbook shape), F2 looks it up by that key.
    assert_eq!(
        eval_at(
            &mut e,
            2,
            5,
            "=T[[#This Row],[Id]]&\"-\"&T[[#This Row],[Code\r\n(new)]]"
        ),
        t("a-X1")
    );
    assert_eq!(
        eval_at(&mut e, 2, 6, "=IF(E2=\"a-X1\",T[[#This Row],[Qty]]*10,-1)"),
        n(20.0)
    );
    // An edit to the line-break column flows through to the dependent chain.
    e.set_cell_value("Sheet1", 2, 2, t("Z9")).unwrap();
    e.evaluate_all().unwrap();
    assert_eq!(e.get_cell_value("Sheet1", 2, 5), Some(t("a-Z9")));
    assert_eq!(e.get_cell_value("Sheet1", 2, 6), Some(n(-1.0)));
}

#[test]
fn escaped_specials_still_resolve_beside_a_line_break() {
    // `'` escapes `[`, `]`, `#` and `'` inside a column name; a line break next to them is literal.
    let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
    e.set_cell_value("Sheet1", 1, 1, t("#Qty [x]\nit's"))
        .unwrap();
    e.set_cell_value("Sheet1", 2, 1, n(7.0)).unwrap();
    let sid = e.sheet_id("Sheet1").unwrap();
    e.define_table(
        "U",
        RangeRef::new(
            CellRef::new(sid, Coord::from_excel(1, 1, true, true)),
            CellRef::new(sid, Coord::from_excel(2, 1, true, true)),
        ),
        true,
        vec!["#Qty [x]\nit's".into()],
        false,
    )
    .unwrap();
    assert_eq!(
        eval_at(&mut e, 5, 5, "=SUM(U['#Qty '[x']\r\nit''s])"),
        n(7.0)
    );
}
