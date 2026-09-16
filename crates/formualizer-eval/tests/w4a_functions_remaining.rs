//! rowsncolumns/spreadsheet#546 W4-A (A-14, E-41): the remaining function-semantics gaps.
//!
//! - `FORECAST.ETS` / `.CONFINT` / `.SEASONALITY` / `.STAT` were `#NAME?`. The port follows the JS
//!   engine's additive Holt-Winters fitter (grid-searched α/β/γ, SSE-compared seasonality
//!   detection, jStat's normal quantile), so the expected values below are the JS engine's results
//!   for the same series — both engines agree to 1e-9. `seasonality` follows Excel: 1 / omitted
//!   auto-detects, 0 is no seasonality, n > 1 is the period.
//! - A call with too few arguments (`SUM()`, `IF(1)`, `COUNT()`, `VLOOKUP(1)`) is `#N/A` in both
//!   engines — Excel refuses to enter such a formula, Google Sheets and the JS engine evaluate a
//!   stored one to `#N/A` — instead of `0` / `#VALUE!`.

use formualizer_common::{ExcelErrorKind, LiteralValue};
use formualizer_eval::engine::{Engine, EvalConfig};
use formualizer_eval::test_workbook::TestWorkbook;
use formualizer_parse::parser::parse;

/// (values, timeline) laid out in A1:A{n} / B1:B{n}.
fn series(name: &str) -> (Vec<f64>, Vec<f64>) {
    match name {
        "linear" => (
            (1..=10).map(|i| i as f64 * 10.0).collect(),
            (1..=10).map(|i| i as f64).collect(),
        ),
        "seasonal" => (
            [10.0, 30.0, 20.0, 40.0]
                .iter()
                .copied()
                .cycle()
                .take(12)
                .collect(),
            (1..=12).map(|i| i as f64).collect(),
        ),
        "noisy" => (
            vec![
                12.5, 14.1, 13.2, 17.8, 16.4, 19.9, 18.7, 23.1, 21.6, 25.4, 24.9, 28.3,
            ],
            vec![
                45292.0, 45323.0, 45352.0, 45383.0, 45413.0, 45444.0, 45474.0, 45505.0, 45536.0,
                45566.0, 45597.0, 45627.0,
            ],
        ),
        "sales" => (
            vec![
                120.0, 135.0, 150.0, 165.0, 130.0, 145.0, 160.0, 175.0, 140.0, 155.0, 170.0, 185.0,
            ],
            (1..=12).map(|i| i as f64).collect(),
        ),
        other => panic!("unknown series {other}"),
    }
}

fn engine_with_series(name: &str) -> Engine<TestWorkbook> {
    let (ys, xs) = series(name);
    let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
    for (i, (y, x)) in ys.iter().zip(&xs).enumerate() {
        let row = i as u32 + 1;
        e.set_cell_value("Sheet1", row, 1, LiteralValue::Number(*y))
            .unwrap();
        e.set_cell_value("Sheet1", row, 2, LiteralValue::Number(*x))
            .unwrap();
    }
    e
}

fn eval_in(e: &mut Engine<TestWorkbook>, formula: &str) -> LiteralValue {
    e.set_cell_formula("Sheet1", 1, 5, parse(formula).unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    e.get_cell_value("Sheet1", 1, 5).expect("value")
}

fn eval(formula: &str) -> LiteralValue {
    let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
    eval_in(&mut e, formula)
}

fn as_num(v: &LiteralValue, formula: &str) -> f64 {
    match v {
        LiteralValue::Number(n) => *n,
        LiteralValue::Int(i) => *i as f64,
        other => panic!("{formula}: expected a number, got {other:?}"),
    }
}

fn assert_error(v: LiteralValue, kind: ExcelErrorKind, formula: &str) {
    match v {
        LiteralValue::Error(e) => assert_eq!(e.kind, kind, "{formula}: {e:?}"),
        other => panic!("{formula}: expected {kind:?}, got {other:?}"),
    }
}

/// The JS engine's results (`fast-formula-parser`, same series, same arguments after the Excel
/// `seasonality` spelling), agreed to 1e-9.
const JS_FIXTURES: &[(&str, &str, f64)] = &[
    // linear
    (
        "linear",
        "=FORECAST.ETS(11,A1:A10,B1:B10,0)",
        109.55787660644461,
    ),
    (
        "linear",
        "=FORECAST.ETS(13,A1:A10,B1:B10,0)",
        128.77201805970907,
    ),
    (
        "linear",
        "=FORECAST.ETS(11,A1:A10,B1:B10,0)",
        109.55787660644461,
    ),
    (
        "linear",
        "=FORECAST.ETS(11,A1:A10,B1:B10,4)",
        108.94703533828222,
    ),
    (
        "linear",
        "=FORECAST.ETS(13,A1:A10,B1:B10,4)",
        129.83612682113574,
    ),
    (
        "linear",
        "=FORECAST.ETS(11,A1:A10,B1:B10,1)",
        109.55787660644461,
    ),
    ("linear", "=FORECAST.ETS(10,A1:A10,B1:B10,0)", 100.0),
    (
        "linear",
        "=FORECAST.ETS.CONFINT(11,A1:A10,B1:B10,0.95,0)",
        6.329863983955223,
    ),
    (
        "linear",
        "=FORECAST.ETS.CONFINT(13,A1:A10,B1:B10,0.9,4)",
        40.870168149219936,
    ),
    ("linear", "=FORECAST.ETS.SEASONALITY(A1:A10,B1:B10)", 1.0),
    ("linear", "=FORECAST.ETS.STAT(A1:A10,B1:B10,1,0)", 0.9),
    ("linear", "=FORECAST.ETS.STAT(A1:A10,B1:B10,2,0)", 0.1),
    ("linear", "=FORECAST.ETS.STAT(A1:A10,B1:B10,3,0)", 0.0),
    (
        "linear",
        "=FORECAST.ETS.STAT(A1:A10,B1:B10,4,0)",
        0.15834119184802453,
    ),
    (
        "linear",
        "=FORECAST.ETS.STAT(A1:A10,B1:B10,5,0)",
        0.07832251807104482,
    ),
    (
        "linear",
        "=FORECAST.ETS.STAT(A1:A10,B1:B10,6,0)",
        1.5834119184802453,
    ),
    (
        "linear",
        "=FORECAST.ETS.STAT(A1:A10,B1:B10,7,0)",
        3.2295817851166557,
    ),
    ("linear", "=FORECAST.ETS.STAT(A1:A10,B1:B10,8,0)", 1.0),
    ("linear", "=FORECAST.ETS.STAT(A1:A10,B1:B10,1,4)", 0.5),
    ("linear", "=FORECAST.ETS.STAT(A1:A10,B1:B10,2,4)", 0.1),
    ("linear", "=FORECAST.ETS.STAT(A1:A10,B1:B10,3,4)", 0.9),
    (
        "linear",
        "=FORECAST.ETS.STAT(A1:A10,B1:B10,4,4)",
        1.2753330365060542,
    ),
    (
        "linear",
        "=FORECAST.ETS.STAT(A1:A10,B1:B10,5,4)",
        0.3320944194498802,
    ),
    (
        "linear",
        "=FORECAST.ETS.STAT(A1:A10,B1:B10,6,4)",
        12.753330365060542,
    ),
    (
        "linear",
        "=FORECAST.ETS.STAT(A1:A10,B1:B10,7,4)",
        14.345594158740033,
    ),
    ("linear", "=FORECAST.ETS.STAT(A1:A10,B1:B10,8,4)", 1.0),
    // seasonal
    (
        "seasonal",
        "=FORECAST.ETS(13,A1:A12,B1:B12,0)",
        36.15392131048526,
    ),
    (
        "seasonal",
        "=FORECAST.ETS(15,A1:A12,B1:B12,0)",
        47.312805601695324,
    ),
    (
        "seasonal",
        "=FORECAST.ETS(13,A1:A12,B1:B12,0)",
        36.15392131048526,
    ),
    ("seasonal", "=FORECAST.ETS(13,A1:A12,B1:B12,4)", 10.0),
    ("seasonal", "=FORECAST.ETS(15,A1:A12,B1:B12,4)", 20.0),
    ("seasonal", "=FORECAST.ETS(13,A1:A12,B1:B12,1)", 10.0),
    ("seasonal", "=FORECAST.ETS(12,A1:A12,B1:B12,0)", 40.0),
    (
        "seasonal",
        "=FORECAST.ETS.CONFINT(13,A1:A12,B1:B12,0.95,0)",
        34.56631949886052,
    ),
    (
        "seasonal",
        "=FORECAST.ETS.CONFINT(15,A1:A12,B1:B12,0.9,4)",
        0.0,
    ),
    ("seasonal", "=FORECAST.ETS.SEASONALITY(A1:A12,B1:B12)", 4.0),
    ("seasonal", "=FORECAST.ETS.STAT(A1:A12,B1:B12,1,0)", 0.5),
    ("seasonal", "=FORECAST.ETS.STAT(A1:A12,B1:B12,2,0)", 0.7),
    ("seasonal", "=FORECAST.ETS.STAT(A1:A12,B1:B12,3,0)", 0.0),
    (
        "seasonal",
        "=FORECAST.ETS.STAT(A1:A12,B1:B12,4,0)",
        0.7728193463268915,
    ),
    (
        "seasonal",
        "=FORECAST.ETS.STAT(A1:A12,B1:B12,5,0)",
        0.5730672631495511,
    ),
    (
        "seasonal",
        "=FORECAST.ETS.STAT(A1:A12,B1:B12,6,0)",
        14.753823884422474,
    ),
    (
        "seasonal",
        "=FORECAST.ETS.STAT(A1:A12,B1:B12,7,0)",
        17.636201364675696,
    ),
    ("seasonal", "=FORECAST.ETS.STAT(A1:A12,B1:B12,8,0)", 1.0),
    ("seasonal", "=FORECAST.ETS.STAT(A1:A12,B1:B12,1,4)", 0.1),
    ("seasonal", "=FORECAST.ETS.STAT(A1:A12,B1:B12,2,4)", 0.1),
    ("seasonal", "=FORECAST.ETS.STAT(A1:A12,B1:B12,3,4)", 0.1),
    ("seasonal", "=FORECAST.ETS.STAT(A1:A12,B1:B12,4,4)", 0.0),
    ("seasonal", "=FORECAST.ETS.STAT(A1:A12,B1:B12,5,4)", 0.0),
    ("seasonal", "=FORECAST.ETS.STAT(A1:A12,B1:B12,6,4)", 0.0),
    ("seasonal", "=FORECAST.ETS.STAT(A1:A12,B1:B12,7,4)", 0.0),
    ("seasonal", "=FORECAST.ETS.STAT(A1:A12,B1:B12,8,4)", 1.0),
    // noisy
    (
        "noisy",
        "=FORECAST.ETS(45658,A1:A12,B1:B12,0)",
        28.519378528399958,
    ),
    (
        "noisy",
        "=FORECAST.ETS(45720,A1:A12,B1:B12,0)",
        31.38546005527558,
    ),
    (
        "noisy",
        "=FORECAST.ETS(45658,A1:A12,B1:B12,0)",
        28.519378528399958,
    ),
    (
        "noisy",
        "=FORECAST.ETS(45658,A1:A12,B1:B12,4)",
        26.86627360534232,
    ),
    (
        "noisy",
        "=FORECAST.ETS(45720,A1:A12,B1:B12,4)",
        28.773382106779877,
    ),
    (
        "noisy",
        "=FORECAST.ETS(45658,A1:A12,B1:B12,1)",
        27.186999096632675,
    ),
    ("noisy", "=FORECAST.ETS(45627,A1:A12,B1:B12,0)", 28.3),
    (
        "noisy",
        "=FORECAST.ETS.CONFINT(45658,A1:A12,B1:B12,0.95,0)",
        3.2997780981206923,
    ),
    (
        "noisy",
        "=FORECAST.ETS.CONFINT(45720,A1:A12,B1:B12,0.9,4)",
        4.793288134696768,
    ),
    ("noisy", "=FORECAST.ETS.SEASONALITY(A1:A12,B1:B12)", 2.0),
    ("noisy", "=FORECAST.ETS.STAT(A1:A12,B1:B12,1,0)", 0.3),
    ("noisy", "=FORECAST.ETS.STAT(A1:A12,B1:B12,2,0)", 0.3),
    ("noisy", "=FORECAST.ETS.STAT(A1:A12,B1:B12,3,0)", 0.0),
    (
        "noisy",
        "=FORECAST.ETS.STAT(A1:A12,B1:B12,4,0)",
        0.6474514143529115,
    ),
    (
        "noisy",
        "=FORECAST.ETS.STAT(A1:A12,B1:B12,5,0)",
        0.08554511986983275,
    ),
    (
        "noisy",
        "=FORECAST.ETS.STAT(A1:A12,B1:B12,6,0)",
        1.5774270822416394,
    ),
    (
        "noisy",
        "=FORECAST.ETS.STAT(A1:A12,B1:B12,7,0)",
        1.683591190526418,
    ),
    ("noisy", "=FORECAST.ETS.STAT(A1:A12,B1:B12,8,0)", 31.0),
    ("noisy", "=FORECAST.ETS.STAT(A1:A12,B1:B12,1,4)", 0.5),
    ("noisy", "=FORECAST.ETS.STAT(A1:A12,B1:B12,2,4)", 0.1),
    ("noisy", "=FORECAST.ETS.STAT(A1:A12,B1:B12,3,4)", 0.7),
    (
        "noisy",
        "=FORECAST.ETS.STAT(A1:A12,B1:B12,4,4)",
        0.644836181553036,
    ),
    (
        "noisy",
        "=FORECAST.ETS.STAT(A1:A12,B1:B12,5,4)",
        0.0864657836058637,
    ),
    (
        "noisy",
        "=FORECAST.ETS.STAT(A1:A12,B1:B12,6,4)",
        1.5710554241473973,
    ),
    (
        "noisy",
        "=FORECAST.ETS.STAT(A1:A12,B1:B12,7,4)",
        1.6824635028465447,
    ),
    ("noisy", "=FORECAST.ETS.STAT(A1:A12,B1:B12,8,4)", 31.0),
    // sales
    (
        "sales",
        "=FORECAST.ETS(13,A1:A12,B1:B12,0)",
        177.5126111688807,
    ),
    (
        "sales",
        "=FORECAST.ETS(15,A1:A12,B1:B12,0)",
        196.0754828673537,
    ),
    (
        "sales",
        "=FORECAST.ETS(13,A1:A12,B1:B12,0)",
        177.5126111688807,
    ),
    (
        "sales",
        "=FORECAST.ETS(13,A1:A12,B1:B12,4)",
        149.61303104651,
    ),
    (
        "sales",
        "=FORECAST.ETS(15,A1:A12,B1:B12,4)",
        179.0536370673752,
    ),
    (
        "sales",
        "=FORECAST.ETS(13,A1:A12,B1:B12,1)",
        149.61303104651,
    ),
    ("sales", "=FORECAST.ETS(12,A1:A12,B1:B12,0)", 185.0),
    (
        "sales",
        "=FORECAST.ETS.CONFINT(13,A1:A12,B1:B12,0.95,0)",
        37.00198257379272,
    ),
    (
        "sales",
        "=FORECAST.ETS.CONFINT(15,A1:A12,B1:B12,0.9,4)",
        9.368391165086983,
    ),
    ("sales", "=FORECAST.ETS.SEASONALITY(A1:A12,B1:B12)", 4.0),
    ("sales", "=FORECAST.ETS.STAT(A1:A12,B1:B12,1,0)", 0.3),
    ("sales", "=FORECAST.ETS.STAT(A1:A12,B1:B12,2,0)", 0.9),
    ("sales", "=FORECAST.ETS.STAT(A1:A12,B1:B12,3,0)", 0.0),
    (
        "sales",
        "=FORECAST.ETS.STAT(A1:A12,B1:B12,4,0)",
        0.811159893097448,
    ),
    (
        "sales",
        "=FORECAST.ETS.STAT(A1:A12,B1:B12,5,0)",
        0.098215460875628,
    ),
    (
        "sales",
        "=FORECAST.ETS.STAT(A1:A12,B1:B12,6,0)",
        15.117070734997894,
    ),
    (
        "sales",
        "=FORECAST.ETS.STAT(A1:A12,B1:B12,7,0)",
        18.878909442040587,
    ),
    ("sales", "=FORECAST.ETS.STAT(A1:A12,B1:B12,8,0)", 1.0),
    ("sales", "=FORECAST.ETS.STAT(A1:A12,B1:B12,1,4)", 0.3),
    ("sales", "=FORECAST.ETS.STAT(A1:A12,B1:B12,2,4)", 0.1),
    ("sales", "=FORECAST.ETS.STAT(A1:A12,B1:B12,3,4)", 0.9),
    (
        "sales",
        "=FORECAST.ETS.STAT(A1:A12,B1:B12,4,4)",
        0.1406441103283995,
    ),
    (
        "sales",
        "=FORECAST.ETS.STAT(A1:A12,B1:B12,5,4)",
        0.018071071863405522,
    ),
    (
        "sales",
        "=FORECAST.ETS.STAT(A1:A12,B1:B12,6,4)",
        2.6210947833928997,
    ),
    (
        "sales",
        "=FORECAST.ETS.STAT(A1:A12,B1:B12,7,4)",
        3.2883431524914997,
    ),
    ("sales", "=FORECAST.ETS.STAT(A1:A12,B1:B12,8,4)", 1.0),
];

#[test]
fn forecast_ets_family_matches_the_js_engine_to_1e_9() {
    for (name, formula, expected) in JS_FIXTURES {
        let mut e = engine_with_series(name);
        let actual = as_num(&eval_in(&mut e, formula), formula);
        let tolerance = 1e-9 * expected.abs().max(1.0);
        assert!(
            (actual - expected).abs() <= tolerance,
            "{name}: {formula} expected {expected}, got {actual}"
        );
    }
}

#[test]
fn forecast_ets_seasonality_defaults_to_auto_detection() {
    let mut e = engine_with_series("seasonal");
    // Omitted and 1 both auto-detect (period 4 here); 0 is Holt's linear method.
    let auto = as_num(&eval_in(&mut e, "=FORECAST.ETS(13,A1:A12,B1:B12)"), "auto");
    let one = as_num(&eval_in(&mut e, "=FORECAST.ETS(13,A1:A12,B1:B12,1)"), "one");
    let four = as_num(
        &eval_in(&mut e, "=FORECAST.ETS(13,A1:A12,B1:B12,4)"),
        "four",
    );
    let none = as_num(
        &eval_in(&mut e, "=FORECAST.ETS(13,A1:A12,B1:B12,0)"),
        "none",
    );
    assert_eq!(auto, four);
    assert_eq!(one, four);
    assert!(
        (four - 10.0).abs() < 1e-9,
        "next cycle restarts at 10: {four}"
    );
    assert!((none - 36.15392131048526).abs() < 1e-9, "{none}");
    let period = eval_in(&mut e, "=FORECAST.ETS.SEASONALITY(A1:A12,B1:B12)");
    assert_eq!(as_num(&period, "seasonality"), 4.0);
    // Skipped slots take the defaults.
    let skipped = as_num(
        &eval_in(&mut e, "=FORECAST.ETS(13,A1:A12,B1:B12,,1,1)"),
        "skipped",
    );
    assert_eq!(skipped, four);
}

#[test]
fn forecast_ets_accepts_inline_arrays_and_unsorted_timelines() {
    let mut e = engine_with_series("linear");
    let from_cells = as_num(
        &eval_in(&mut e, "=FORECAST.ETS(11,A1:A10,B1:B10,0)"),
        "cells",
    );
    let from_arrays = as_num(
        &eval("=FORECAST.ETS(11,{10;20;30;40;50;60;70;80;90;100},{1;2;3;4;5;6;7;8;9;10},0)"),
        "arrays",
    );
    assert_eq!(from_cells, from_arrays);
    // The timeline is sorted before fitting.
    let shuffled = as_num(
        &eval("=FORECAST.ETS(11,{100;10;20;30;40;50;60;70;80;90},{10;1;2;3;4;5;6;7;8;9},0)"),
        "shuffled",
    );
    assert_eq!(shuffled, from_arrays);
    // Non-numeric pairs are skipped; blanks in the range don't break the length check.
    let mut e = engine_with_series("linear");
    e.set_cell_value("Sheet1", 4, 1, LiteralValue::Text("n/a".into()))
        .unwrap();
    let with_gap = eval_in(&mut e, "=FORECAST.ETS(11,A1:A10,B1:B10,0)");
    as_num(&with_gap, "gap");
    let with_blank_rows = eval_in(&mut e, "=FORECAST.ETS(11,A1:A12,B1:B12,0)");
    assert_eq!(with_blank_rows, with_gap);
}

#[test]
fn forecast_ets_error_kinds() {
    let mut e = engine_with_series("linear");
    // Mismatched series sizes and fewer than four numeric pairs are #N/A.
    assert_error(
        eval_in(&mut e, "=FORECAST.ETS(11,A1:A10,B1:B9)"),
        ExcelErrorKind::Na,
        "sizes",
    );
    assert_error(
        eval_in(&mut e, "=FORECAST.ETS(11,A1:A3,B1:B3)"),
        ExcelErrorKind::Na,
        "three points",
    );
    // A target before the end of the history is #NUM!.
    assert_error(
        eval_in(&mut e, "=FORECAST.ETS(3,A1:A10,B1:B10)"),
        ExcelErrorKind::Num,
        "in-history target",
    );
    // seasonality outside 0..=8760, data_completion outside {0,1}, aggregation outside 1..=7.
    assert_error(
        eval_in(&mut e, "=FORECAST.ETS(11,A1:A10,B1:B10,-1)"),
        ExcelErrorKind::Num,
        "negative seasonality",
    );
    assert_error(
        eval_in(&mut e, "=FORECAST.ETS(11,A1:A10,B1:B10,8761)"),
        ExcelErrorKind::Num,
        "seasonality cap",
    );
    assert_error(
        eval_in(&mut e, "=FORECAST.ETS(11,A1:A10,B1:B10,1,2)"),
        ExcelErrorKind::Num,
        "data_completion",
    );
    assert_error(
        eval_in(&mut e, "=FORECAST.ETS(11,A1:A10,B1:B10,1,1,8)"),
        ExcelErrorKind::Num,
        "aggregation",
    );
    // CONFINT's confidence level is strictly inside (0, 1).
    assert_error(
        eval_in(&mut e, "=FORECAST.ETS.CONFINT(11,A1:A10,B1:B10,1)"),
        ExcelErrorKind::Num,
        "confidence 1",
    );
    assert_error(
        eval_in(&mut e, "=FORECAST.ETS.CONFINT(11,A1:A10,B1:B10,0)"),
        ExcelErrorKind::Num,
        "confidence 0",
    );
    // STAT's statistic_type is 1..=8.
    assert_error(
        eval_in(&mut e, "=FORECAST.ETS.STAT(A1:A10,B1:B10,9)"),
        ExcelErrorKind::Num,
        "stat 9",
    );
    assert_error(
        eval_in(&mut e, "=FORECAST.ETS.STAT(A1:A10,B1:B10,0)"),
        ExcelErrorKind::Num,
        "stat 0",
    );
    // A text target is #VALUE!; an error in the series propagates.
    assert_error(
        eval_in(&mut e, "=FORECAST.ETS(\"soon\",A1:A10,B1:B10)"),
        ExcelErrorKind::Value,
        "text target",
    );
    e.set_cell_formula("Sheet1", 5, 1, parse("=1/0").unwrap())
        .unwrap();
    assert_error(
        eval_in(&mut e, "=FORECAST.ETS(11,A1:A10,B1:B10)"),
        ExcelErrorKind::Div,
        "error in series",
    );
}

#[test]
fn too_few_arguments_is_na_like_the_js_engine_and_sheets() {
    for formula in [
        "=SUM()",
        "=COUNT()",
        "=IF(1)",
        "=VLOOKUP(1)",
        "=AVERAGE()",
        "=FORECAST.ETS(1,A1:A4)",
        "=LET(x)",
        "=LAMBDA()",
    ] {
        assert_error(eval(formula), ExcelErrorKind::Na, formula);
    }
    // One argument is enough for SUM / COUNT; IS-functions catch the value.
    assert_eq!(as_num(&eval("=SUM(2)"), "SUM(2)"), 2.0);
    assert_eq!(as_num(&eval("=COUNT(2)"), "COUNT(2)"), 1.0);
    assert_eq!(eval("=ISNA(SUM())"), LiteralValue::Boolean(true));
    assert_eq!(
        eval("=IFERROR(IF(1),\"bad\")"),
        LiteralValue::Text("bad".into())
    );
}

// ───────────── W4-A follow-up (review of rowsncolumns/spreadsheet#609) ─────────────

/// An error anywhere in `values` or `timeline` is the result (`series_number` propagates it; the
/// JS engine skipped the pair and forecast from the remaining points). Text and blanks are skipped.
#[test]
fn forecast_ets_family_propagates_an_error_in_the_series() {
    let mut e = engine_with_series("linear");
    e.set_cell_formula("Sheet1", 5, 1, parse("=1/0").unwrap())
        .unwrap();
    for formula in [
        "=FORECAST.ETS(11,A1:A10,B1:B10)",
        "=FORECAST.ETS.CONFINT(11,A1:A10,B1:B10)",
        "=FORECAST.ETS.SEASONALITY(A1:A10,B1:B10)",
        "=FORECAST.ETS.STAT(A1:A10,B1:B10,1)",
        // …and in the timeline.
        "=FORECAST.ETS(11,B1:B10,A1:A10)",
    ] {
        assert_error(eval_in(&mut e, formula), ExcelErrorKind::Div, formula);
    }
    // A text entry in the series is skipped, not an error.
    e.set_cell_value("Sheet1", 5, 1, LiteralValue::Text("n/a".into()))
        .unwrap();
    let skipped = eval_in(&mut e, "=FORECAST.ETS(11,A1:A10,B1:B10,0)");
    assert!(
        matches!(skipped, LiteralValue::Number(_) | LiteralValue::Int(_)),
        "text is skipped: {skipped:?}"
    );
}

/// C-J17's real defect was in the JS engine's `ROWS` / `COLUMNS`, which rejected every array
/// argument (`ROWS(RANDARRAY(3,2))*10+COLUMNS(RANDARRAY(3,2))` was `#VALUE!`). Pin the Rust
/// engine's shape reporting for arrays, dynamic-array results and scalars — and, new here, that
/// an error argument is the result (`ROWS(NA())` was 1).
#[test]
fn rows_and_columns_report_array_shapes_and_propagate_errors() {
    let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
    for (formula, expected) in [
        ("=ROWS({1,2;3,4})", 2.0),
        ("=COLUMNS({1,2;3,4})", 2.0),
        ("=ROWS(SEQUENCE(3,2))", 3.0),
        ("=COLUMNS(SEQUENCE(3,2))", 2.0),
        ("=ROWS(RANDARRAY(3,2))", 3.0),
        ("=COLUMNS(RANDARRAY(3,2))", 2.0),
        ("=ROWS(SORT(RANDARRAY(3,2)))", 3.0),
        ("=ROWS(RANDARRAY(3,2))*10+COLUMNS(RANDARRAY(3,2))", 32.0),
        ("=ROWS(5)", 1.0),
        ("=COLUMNS(\"x\")", 1.0),
        ("=ROWS(TRUE)", 1.0),
        ("=ROWS(A1)", 1.0),
        ("=ROWS(A1:B3)", 3.0),
        ("=COLUMNS(A1:B3)", 2.0),
    ] {
        assert_eq!(as_num(&eval_in(&mut e, formula), formula), expected, "{formula}");
    }
    assert_error(eval_in(&mut e, "=ROWS(NA())"), ExcelErrorKind::Na, "ROWS(NA())");
    assert_error(eval_in(&mut e, "=ROWS(1/0)"), ExcelErrorKind::Div, "ROWS(1/0)");
    assert_error(
        eval_in(&mut e, "=COLUMNS(#REF!)"),
        ExcelErrorKind::Ref,
        "COLUMNS(#REF!)",
    );
}

/// The too-few-arguments contract beyond the five cases #609 pinned — the JS engine now agrees on
/// each of these (it surfaced `#NAME?`, a parse-level `#ERROR!` or `#VALUE!` before).
#[test]
fn too_few_arguments_contract_covers_lambda_helpers_index_and_criteria_functions() {
    let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
    for formula in [
        "=LET(x)",
        "=LET(x,1)",
        "=LAMBDA()",
        "=INDEX()",
        "=SUMIF()",
        "=SUMIF(A1:A5)",
        "=COUNTIF(A1:A3)",
        "=COUNTIFS(A1:A3)",
        "=MAP(LAMBDA(v,v))",
        "=MAP(A1:A3)",
        "=BYROW(A1:A3)",
        "=BYCOL(A1:A3)",
        "=MAKEARRAY(2,2)",
    ] {
        assert_error(eval_in(&mut e, formula), ExcelErrorKind::Na, formula);
    }
    // Enough arguments of the wrong kind is a different error: an even LET count / a non-lambda.
    assert_error(eval_in(&mut e, "=LET(x,1,2,y)"), ExcelErrorKind::Value, "LET(x,1,2,y)");
    assert_error(eval_in(&mut e, "=REDUCE(0,A1:A3)"), ExcelErrorKind::Value, "REDUCE(0,A1:A3)");
    // A body-less LAMBDA is a lambda value as the cell result.
    assert_error(eval_in(&mut e, "=LAMBDA(x)"), ExcelErrorKind::Calc, "LAMBDA(x)");
}
