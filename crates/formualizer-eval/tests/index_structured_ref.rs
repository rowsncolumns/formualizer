//! INDEX over a structured (table) reference returns a reference into the table, as in Excel
//! (demo-app-monorepo#49969 FX-01): `INDEX(Sales[Qty],MATCH(..))`, `INDEX(Sales[#Data],r,c)` and
//! `INDEX(Sales[[Qty]:[Price]],r,c)` were `#REF!` because INDEX's bounds resolver only knew plain
//! cell/range references, while `SUM(Sales[Qty])` (a value read) worked.
//! https://support.microsoft.com/en-us/office/index-function-a5dcf0dd-996d-40a4-a822-b56b061328bd
//! https://support.microsoft.com/en-us/office/using-structured-references-with-excel-tables-f5ed2452-2337-4f71-bed3-c8ae6d2b276e

use formualizer_common::{ExcelErrorKind, LiteralValue};
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

/// `sheet`!A1:C4 = Item/Qty/Price headers + (a,2,10) (b,3,20) (c,4,30), defined as table `Sales`.
fn seeded(table_sheet: &str) -> Engine<TestWorkbook> {
    let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
    if table_sheet != "Sheet1" {
        e.add_sheet(table_sheet).unwrap();
    }
    let rows: [[LiteralValue; 3]; 4] = [
        [t("Item"), t("Qty"), t("Price")],
        [t("a"), n(2.0), n(10.0)],
        [t("b"), n(3.0), n(20.0)],
        [t("c"), n(4.0), n(30.0)],
    ];
    for (r, row) in rows.into_iter().enumerate() {
        for (c, v) in row.into_iter().enumerate() {
            e.set_cell_value(table_sheet, r as u32 + 1, c as u32 + 1, v)
                .unwrap();
        }
    }
    let sid = e.sheet_id(table_sheet).unwrap();
    e.define_table(
        "Sales",
        RangeRef::new(
            CellRef::new(sid, Coord::from_excel(1, 1, true, true)),
            CellRef::new(sid, Coord::from_excel(4, 3, true, true)),
        ),
        true,
        vec!["Item".into(), "Qty".into(), "Price".into()],
        false,
    )
    .unwrap();
    e
}

/// Evaluate `formula` at Sheet1!H10 and read the result back.
fn eval(e: &mut Engine<TestWorkbook>, formula: &str) -> LiteralValue {
    e.set_cell_formula("Sheet1", 10, 8, parse(formula).unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    e.get_cell_value("Sheet1", 10, 8)
        .unwrap_or(LiteralValue::Empty)
}

fn num(v: &LiteralValue) -> f64 {
    match v {
        LiteralValue::Number(x) => *x,
        LiteralValue::Int(i) => *i as f64,
        other => panic!("expected a number, got {other:?}"),
    }
}

fn assert_cases(e: &mut Engine<TestWorkbook>) {
    let cases: &[(&str, LiteralValue)] = &[
        ("=INDEX(Sales[Qty],2)", n(3.0)),
        ("=INDEX(Sales[Qty],MATCH(\"c\",Sales[Item],0))", n(4.0)),
        ("=INDEX(Sales[#Data],2,2)", n(3.0)),
        ("=INDEX(Sales[[Qty]:[Price]],2,2)", n(20.0)),
        ("=INDEX(Sales[[#Data],[Price]],3)", n(30.0)),
        ("=INDEX(Sales[#All],1,2)", t("Qty")),
        ("=INDEX(Sales[#Headers],1,3)", t("Price")),
        ("=INDEX(Sales,3,1)", t("c")),
        // INDEX returns a reference: it can feed reference-taking contexts.
        ("=SUM(INDEX(Sales[Qty],1):INDEX(Sales[Qty],3))", n(9.0)),
        ("=SUM(INDEX(Sales[[Qty]:[Price]],0,2))", n(60.0)),
        ("=SUM(INDEX(Sales[#Data],2,0))", n(23.0)),
        ("=ROW(INDEX(Sales[Qty],3))", n(4.0)),
        ("=COLUMN(INDEX(Sales[[Qty]:[Price]],1,2))", n(3.0)),
        // OFFSET shares INDEX's bounds resolution, so a table column is a valid base too.
        ("=SUM(OFFSET(Sales[Price],1,0,2,1))", n(50.0)),
    ];
    for (f, expected) in cases {
        let got = eval(e, f);
        match expected {
            LiteralValue::Text(_) => assert_eq!(&got, expected, "{f}"),
            _ => assert_eq!(num(&got), num(expected), "{f} => {got:?}"),
        }
    }
    // Out of the table's bounds is #REF!, as for a plain range.
    for f in ["=INDEX(Sales[Qty],4)", "=INDEX(Sales[[Qty]:[Price]],1,3)"] {
        match eval(e, f) {
            LiteralValue::Error(err) => assert_eq!(err.kind, ExcelErrorKind::Ref, "{f}"),
            other => panic!("{f}: expected #REF!, got {other:?}"),
        }
    }
}

#[test]
fn index_over_structured_references_on_the_same_sheet() {
    let mut e = seeded("Sheet1");
    assert_cases(&mut e);
}

#[test]
fn index_over_a_table_on_another_sheet_reads_that_sheet() {
    // The table lives on Data while the formula is on Sheet1: the returned reference must keep
    // the table's sheet, not fall back to the formula's.
    let mut e = seeded("Data");
    assert_cases(&mut e);
}

#[test]
fn index_over_a_structured_reference_tracks_edits_to_the_table() {
    let mut e = seeded("Sheet1");
    assert_eq!(
        num(&eval(
            &mut e,
            "=INDEX(Sales[Qty],MATCH(\"c\",Sales[Item],0))"
        )),
        4.0
    );
    e.set_cell_value("Sheet1", 4, 2, n(40.0)).unwrap();
    e.evaluate_all().unwrap();
    assert_eq!(
        e.get_cell_value("Sheet1", 10, 8).map(|v| num(&v)),
        Some(40.0)
    );
}
