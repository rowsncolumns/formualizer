//! Functions as arguments (Excel's eta-reduced lambdas) and the GROUPBY / PIVOTBY result shape
//! (demo-app-monorepo#49969 FX-05 / LV-10).
//!
//! Excel passes a bare function name where a LAMBDA is expected — `=BYROW(r,SUM)`,
//! `=MAP(r,ABS)`, `=REDUCE(0,r,SUM)`, `=GROUPBY(k,v,SUM)` — and a file stores it as
//! `_xleta.SUM`. GROUPBY / PIVOTBY sort ascending and close with a `Total` row / column.
//! <https://support.microsoft.com/en-us/office/groupby-function-5e08ae8c-6800-4b72-b623-c41773611505>
//! <https://support.microsoft.com/en-us/office/pivotby-function-de86516a-90ad-4ced-8522-3a25fac389cf>
//!
//! Seed grid (1-based): A1:A3 = "a","b","a"; B1:B3 = 2,3,4; C1:C3 = "Q1","Q1","Q2".

use formualizer_common::{ExcelErrorKind, LiteralValue};
use formualizer_eval::engine::named_range::{NameScope, NamedDefinition};
use formualizer_eval::engine::{Engine, EvalConfig};
use formualizer_eval::test_workbook::TestWorkbook;
use formualizer_parse::parser::parse;

fn seeded() -> Engine<TestWorkbook> {
    let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
    for (i, (k, v, q)) in [("a", 2.0, "Q1"), ("b", 3.0, "Q1"), ("a", 4.0, "Q2")]
        .into_iter()
        .enumerate()
    {
        let r = i as u32 + 1;
        e.set_cell_value("Sheet1", r, 1, LiteralValue::Text(k.into()))
            .unwrap();
        e.set_cell_value("Sheet1", r, 2, LiteralValue::Number(v))
            .unwrap();
        e.set_cell_value("Sheet1", r, 3, LiteralValue::Text(q.into()))
            .unwrap();
    }
    e
}

/// Evaluate in Sheet1!F1 and read back `rows`×`cols` of the spill.
fn spill(
    e: &mut Engine<TestWorkbook>,
    formula: &str,
    rows: u32,
    cols: u32,
) -> Vec<Vec<LiteralValue>> {
    e.set_cell_formula("Sheet1", 1, 6, parse(formula).unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    (1..=rows)
        .map(|r| {
            (6..6 + cols)
                .map(|c| {
                    e.get_cell_value("Sheet1", r, c)
                        .unwrap_or(LiteralValue::Empty)
                })
                .collect()
        })
        .collect()
}

fn eval(formula: &str) -> LiteralValue {
    spill(&mut seeded(), formula, 1, 1).remove(0).remove(0)
}

fn num(v: &LiteralValue) -> f64 {
    match v {
        LiteralValue::Number(n) => *n,
        LiteralValue::Int(i) => *i as f64,
        other => panic!("expected a number, got {other:?}"),
    }
}

fn t(s: &str) -> LiteralValue {
    LiteralValue::Text(s.into())
}

fn n(v: f64) -> LiteralValue {
    LiteralValue::Number(v)
}

fn normalise(rows: Vec<Vec<LiteralValue>>) -> Vec<Vec<LiteralValue>> {
    rows.into_iter()
        .map(|r| {
            r.into_iter()
                .map(|v| match v {
                    LiteralValue::Int(i) => LiteralValue::Number(i as f64),
                    v => v,
                })
                .collect()
        })
        .collect()
}

#[test]
fn bare_function_name_is_a_lambda_argument() {
    assert_eq!(num(&eval("=SUM(BYROW(A1:B3,SUM))")), 9.0);
    assert_eq!(num(&eval("=SUM(BYCOL(B1:B3,SUM))")), 9.0);
    assert_eq!(num(&eval("=SUM(MAP({-1,2,-3},ABS))")), 6.0);
    assert_eq!(num(&eval("=REDUCE(0,B1:B3,SUM)")), 9.0);
    assert_eq!(num(&eval("=INDEX(SCAN(0,B1:B3,SUM),3)")), 9.0);
    assert_eq!(num(&eval("=SUM(MAKEARRAY(2,2,PRODUCT))")), 9.0);
    assert_eq!(num(&eval("=LET(f,SUM,f(1,2))")), 3.0);
    assert_eq!(num(&eval("=sum(byrow(B1:B3,max))")), 9.0);
}

#[test]
fn xleta_storage_spelling_is_the_function() {
    assert_eq!(num(&eval("=SUM(BYROW(A1:B3,_xleta.SUM))")), 9.0);
    assert_eq!(num(&eval("=ROWS(GROUPBY(A1:A3,B1:B3,_xleta.SUM))")), 3.0);
}

#[test]
fn unknown_bare_name_is_still_name_error() {
    match eval("=SUM(BYROW(B1:B3,NOSUCHFN))") {
        LiteralValue::Error(e) => assert_eq!(e.kind, ExcelErrorKind::Name),
        other => panic!("expected #NAME?, got {other:?}"),
    }
}

#[test]
fn a_defined_name_wins_over_the_function_of_the_same_spelling() {
    let mut e = seeded();
    e.define_name(
        "MAX",
        NamedDefinition::Formula {
            ast: parse("=LAMBDA(x,100)").unwrap(),
            dependencies: Vec::new(),
            range_deps: Vec::new(),
        },
        NameScope::Workbook,
    )
    .unwrap();
    let out = spill(&mut e, "=SUM(BYROW(B1:B3,MAX))", 1, 1);
    assert_eq!(num(&out[0][0]), 300.0);
}

#[test]
fn groupby_with_function_argument_has_excel_shape() {
    let want = vec![
        vec![t("a"), n(6.0)],
        vec![t("b"), n(3.0)],
        vec![t("Total"), n(9.0)],
    ];
    for f in [
        "=GROUPBY(A1:A3,B1:B3,SUM)",
        "=GROUPBY(A1:A3,B1:B3,LAMBDA(v,SUM(v)))",
        "=GROUPBY(A1:A3,B1:B3,\"SUM\")",
    ] {
        assert_eq!(normalise(spill(&mut seeded(), f, 3, 2)), want, "{f}");
    }
    // Excel's literal-array example from the audit.
    assert_eq!(
        normalise(spill(
            &mut seeded(),
            "=GROUPBY({\"x\";\"y\";\"x\"},{1;2;3},SUM)",
            3,
            2
        )),
        vec![
            vec![t("x"), n(4.0)],
            vec![t("y"), n(2.0)],
            vec![t("Total"), n(6.0)]
        ]
    );
}

#[test]
fn groupby_other_functions_and_percentof() {
    assert_eq!(num(&eval("=INDEX(GROUPBY(A1:A3,B1:B3,AVERAGE),1,2)")), 3.0);
    assert_eq!(num(&eval("=INDEX(GROUPBY(A1:A3,B1:B3,COUNTA),3,2)")), 3.0);
    assert_eq!(num(&eval("=INDEX(GROUPBY(A1:A3,B1:B3,MAX),1,2)")), 4.0);
    // PERCENTOF takes (subset, all): the share of the grand total.
    let share = num(&eval("=INDEX(GROUPBY(A1:A3,B1:B3,PERCENTOF),1,2)"));
    assert!((share - 6.0 / 9.0).abs() < 1e-12, "{share}");
    assert_eq!(
        num(&eval("=INDEX(GROUPBY(A1:A3,B1:B3,PERCENTOF),3,2)")),
        1.0
    );
}

#[test]
fn groupby_optional_arguments() {
    // total_depth 0: no Total row.
    assert_eq!(num(&eval("=ROWS(GROUPBY(A1:A3,B1:B3,SUM,0,0))")), 2.0);
    // total_depth -1: Total at the top.
    assert_eq!(
        eval("=INDEX(GROUPBY(A1:A3,B1:B3,SUM,0,-1),1,1)"),
        t("Total")
    );
    // sort_order -2: descending by the value column.
    assert_eq!(
        normalise(spill(
            &mut seeded(),
            "=GROUPBY(A1:A3,B1:B3,SUM,0,0,-2)",
            2,
            2
        )),
        vec![vec![t("a"), n(6.0)], vec![t("b"), n(3.0)]]
    );
    // sort_order -1: descending by key.
    assert_eq!(eval("=INDEX(GROUPBY(A1:A3,B1:B3,SUM,0,0,-1),1,1)"), t("b"));
    // filter_array drops rows.
    assert_eq!(
        num(&eval("=INDEX(GROUPBY(A1:A3,B1:B3,SUM,0,1,1,B1:B3>2),1,2)")),
        4.0
    );
    // field_headers 3 with a header row in the data shows it.
    let mut e = seeded();
    e.set_cell_value("Sheet1", 5, 1, t("Key")).unwrap();
    e.set_cell_value("Sheet1", 5, 2, t("Qty")).unwrap();
    e.set_cell_value("Sheet1", 6, 1, t("z")).unwrap();
    e.set_cell_value("Sheet1", 6, 2, n(7.0)).unwrap();
    assert_eq!(
        normalise(spill(&mut e, "=GROUPBY(A5:A6,B5:B6,SUM,3)", 3, 2)),
        vec![
            vec![t("Key"), t("Qty")],
            vec![t("z"), n(7.0)],
            vec![t("Total"), n(7.0)]
        ]
    );
    // Automatic headers (text above a number) are skipped, not shown.
    assert_eq!(num(&eval("=ROWS(GROUPBY(A5:A6,B5:B6,SUM))")), 2.0);
    // Subtotals need two row-field columns.
    match eval("=GROUPBY(A1:A3,B1:B3,SUM,0,2)") {
        LiteralValue::Error(e) => assert_eq!(e.kind, ExcelErrorKind::Value),
        other => panic!("expected #VALUE!, got {other:?}"),
    }
}

#[test]
fn groupby_two_row_fields_get_subtotals() {
    // Keys (A,C): (a,Q1)=2, (b,Q1)=3, (a,Q2)=4.
    assert_eq!(
        normalise(spill(
            &mut seeded(),
            "=GROUPBY(HSTACK(A1:A3,C1:C3),B1:B3,SUM)",
            6,
            3
        )),
        vec![
            vec![t("a"), t("Q1"), n(2.0)],
            vec![t("a"), t("Q2"), n(4.0)],
            vec![t("a"), LiteralValue::Empty, n(6.0)],
            vec![t("b"), t("Q1"), n(3.0)],
            vec![t("b"), LiteralValue::Empty, n(3.0)],
            vec![t("Total"), LiteralValue::Empty, n(9.0)],
        ]
    );
}

#[test]
fn pivotby_with_function_argument_has_totals() {
    let e = LiteralValue::Empty;
    assert_eq!(
        normalise(spill(
            &mut seeded(),
            "=PIVOTBY(A1:A3,C1:C3,B1:B3,SUM)",
            4,
            4
        )),
        vec![
            vec![e.clone(), t("Q1"), t("Q2"), t("Total")],
            vec![t("a"), n(2.0), n(4.0), n(6.0)],
            vec![t("b"), n(3.0), e.clone(), n(3.0)],
            vec![t("Total"), n(5.0), n(4.0), n(9.0)],
        ]
    );
    assert_eq!(num(&eval("=ROWS(PIVOTBY(A1:A3,A1:A3,B1:B3,SUM))")), 4.0);
    assert_eq!(
        num(&eval(
            "=INDEX(PIVOTBY(A1:A3,C1:C3,B1:B3,LAMBDA(v,SUM(v))),4,4)"
        )),
        9.0
    );
    // relative_to 0 (column totals): a's Q1 share of the Q1 column.
    let share = num(&eval("=INDEX(PIVOTBY(A1:A3,C1:C3,B1:B3,PERCENTOF),2,2)"));
    assert!((share - 2.0 / 5.0).abs() < 1e-12, "{share}");
    // relative_to 1 (row totals): a's Q1 share of row a.
    let share = num(&eval(
        "=INDEX(PIVOTBY(A1:A3,C1:C3,B1:B3,PERCENTOF,,,,,,,1),2,2)",
    ));
    assert!((share - 2.0 / 6.0).abs() < 1e-12, "{share}");
    // No totals at all.
    assert_eq!(
        num(&eval("=COLUMNS(PIVOTBY(A1:A3,C1:C3,B1:B3,SUM,0,0,1,0))")),
        3.0
    );
}
