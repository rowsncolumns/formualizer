//! Excel parity for dynamic-array signatures (rowsncolumns/spreadsheet#546, unit U7):
//! `SORT` takes `1`/`-1` orders and array keys, `SORTBY` chains key/order pairs, `VSTACK`/`HSTACK`
//! pad ragged inputs with `#N/A`, `ROW`/`COLUMN` spill over ranges, `WRAPROWS`/`WRAPCOLS`/`EXPAND`
//! exist, and `LET` can bind ranges and array constants.

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
fn na() -> LiteralValue {
    LiteralValue::Error(formualizer_common::ExcelError::new(ExcelErrorKind::Na))
}

/// A1:C5 = the sweep's fruit table; F1:F3 = 1,2,3; F1:G2 also seeds G1:G2 = 4,5; H1 = 3.
fn seeded() -> Engine<TestWorkbook> {
    let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
    let table: [(f64, &str, f64); 5] = [
        (10.0, "apple", 5.0),
        (20.0, "banana", 3.0),
        (30.0, "cherry", 5.0),
        (40.0, "date", 1.0),
        (50.0, "elderberry", 2.0),
    ];
    for (i, (a, b, c)) in table.iter().enumerate() {
        let r = i as u32 + 1;
        e.set_cell_value("Sheet1", r, 1, n(*a)).unwrap();
        e.set_cell_value("Sheet1", r, 2, t(b)).unwrap();
        e.set_cell_value("Sheet1", r, 3, n(*c)).unwrap();
    }
    for (i, v) in [1.0, 2.0, 3.0].iter().enumerate() {
        e.set_cell_value("Sheet1", i as u32 + 1, 6, n(*v)).unwrap();
    }
    e.set_cell_value("Sheet1", 1, 7, n(4.0)).unwrap();
    e.set_cell_value("Sheet1", 2, 7, n(5.0)).unwrap();
    e.set_cell_value("Sheet1", 1, 8, n(3.0)).unwrap();
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

fn col(vals: &[f64]) -> Vec<Vec<LiteralValue>> {
    vals.iter().map(|v| vec![n(*v)]).collect()
}

#[test]
fn sort_descending_with_minus_one() {
    assert_eq!(
        spill("=SORT(C1:C5,1,-1)", 5, 1),
        col(&[5.0, 5.0, 3.0, 2.0, 1.0])
    );
}

#[test]
fn sort_rejects_orders_other_than_plus_minus_one() {
    match scalar("=SORT(C1:C5,1,2)") {
        LiteralValue::Error(e) => assert_eq!(e.kind, ExcelErrorKind::Value),
        other => panic!("expected #VALUE!, got {other:?}"),
    }
}

#[test]
fn sort_multi_key_with_array_index_and_orders() {
    // by C ascending, then by A descending
    assert_eq!(
        spill("=SORT(A1:C5,{3,1},{1,-1})", 5, 3),
        vec![
            vec![n(40.0), t("date"), n(1.0)],
            vec![n(50.0), t("elderberry"), n(2.0)],
            vec![n(20.0), t("banana"), n(3.0)],
            vec![n(30.0), t("cherry"), n(5.0)],
            vec![n(10.0), t("apple"), n(5.0)],
        ]
    );
}

#[test]
fn sort_by_col_true_sorts_columns() {
    assert_eq!(
        spill("=SORT({3,1,2;9,7,8},1,1,TRUE)", 2, 3),
        vec![vec![n(1.0), n(2.0), n(3.0)], vec![n(7.0), n(8.0), n(9.0)]]
    );
}

#[test]
fn sortby_two_keys_with_orders() {
    // by C ascending, ties broken by A descending
    assert_eq!(
        spill("=SORTBY(B1:B5,C1:C5,1,A1:A5,-1)", 5, 1),
        vec![
            vec![t("date")],
            vec![t("elderberry")],
            vec![t("banana")],
            vec![t("cherry")],
            vec![t("apple")]
        ]
    );
}

#[test]
fn sortby_rejects_bad_order() {
    match scalar("=SORTBY(B1:B5,C1:C5,2)") {
        LiteralValue::Error(e) => assert_eq!(e.kind, ExcelErrorKind::Value),
        other => panic!("expected #VALUE!, got {other:?}"),
    }
}

#[test]
fn unique_by_col_returns_unique_columns() {
    assert_eq!(
        spill("=UNIQUE({1,2,1;3,4,3},TRUE)", 2, 2),
        vec![vec![n(1.0), n(2.0)], vec![n(3.0), n(4.0)]]
    );
}

#[test]
fn vstack_and_hstack_pad_ragged_inputs_with_na() {
    assert_eq!(
        spill("=VSTACK(F1:G2,H1)", 3, 2),
        vec![
            vec![n(1.0), n(4.0)],
            vec![n(2.0), n(5.0)],
            vec![n(3.0), na()]
        ]
    );
    assert_eq!(
        spill("=HSTACK(F1:F3,H1)", 3, 2),
        vec![vec![n(1.0), n(3.0)], vec![n(2.0), na()], vec![n(3.0), na()]]
    );
}

#[test]
fn row_and_column_spill_over_ranges() {
    assert_eq!(spill("=ROW(A3:A5)", 3, 1), col(&[3.0, 4.0, 5.0]));
    assert_eq!(
        spill("=COLUMN(A1:C1)", 1, 3),
        vec![vec![n(1.0), n(2.0), n(3.0)]]
    );
    assert_eq!(scalar("=SUM(ROW(A1:A5))"), n(15.0));
    assert_eq!(scalar("=ROW(A7)"), n(7.0));
}

#[test]
fn wraprows_wrapcols_expand() {
    assert_eq!(
        spill("=WRAPROWS({1,2,3,4,5},2)", 3, 2),
        vec![
            vec![n(1.0), n(2.0)],
            vec![n(3.0), n(4.0)],
            vec![n(5.0), na()]
        ]
    );
    assert_eq!(
        spill("=WRAPROWS({1,2,3,4,5},2,0)", 3, 2),
        vec![
            vec![n(1.0), n(2.0)],
            vec![n(3.0), n(4.0)],
            vec![n(5.0), n(0.0)]
        ]
    );
    assert_eq!(
        spill("=WRAPCOLS({1,2,3,4,5},2)", 2, 3),
        vec![vec![n(1.0), n(3.0), n(5.0)], vec![n(2.0), n(4.0), na()]]
    );
    assert_eq!(
        spill("=EXPAND(F1:G2,3,3,0)", 3, 3),
        vec![
            vec![n(1.0), n(4.0), n(0.0)],
            vec![n(2.0), n(5.0), n(0.0)],
            vec![n(0.0), n(0.0), n(0.0)]
        ]
    );
    assert_eq!(
        spill("=EXPAND(F1:G2,3)", 3, 2),
        vec![vec![n(1.0), n(4.0)], vec![n(2.0), n(5.0)], vec![na(), na()]]
    );
    match scalar("=EXPAND(F1:G2,1)") {
        LiteralValue::Error(e) => assert_eq!(e.kind, ExcelErrorKind::Value),
        other => panic!("expected #VALUE!, got {other:?}"),
    }
    match scalar("=WRAPROWS({1,2,3},0)") {
        LiteralValue::Error(e) => assert_eq!(e.kind, ExcelErrorKind::Num),
        other => panic!("expected #NUM!, got {other:?}"),
    }
}

#[test]
fn let_binds_ranges_and_array_constants() {
    assert_eq!(scalar("=LET(a,F1:F3,SUM(a))"), n(6.0));
    assert_eq!(scalar("=LET(a,{1,2,3},SUM(a))"), n(6.0));
    assert_eq!(scalar("=LET(a,A1:A5,SUM(a)/COUNT(a))"), n(30.0));
    assert_eq!(spill("=LET(a,F1:F3,a*2)", 3, 1), col(&[2.0, 4.0, 6.0]));
}
