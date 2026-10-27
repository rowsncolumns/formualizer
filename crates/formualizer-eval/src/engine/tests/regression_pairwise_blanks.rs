//! The paired-data statistics (`SLOPE`, `INTERCEPT`, `FORECAST.LINEAR` /
//! `FORECAST`, `RSQ`, `PEARSON`, `CORREL`, `STEYX`, `COVARIANCE.P` / `.S`)
//! drop a *pair* when either its x or its y is blank, text or logical, as Excel
//! does, instead of counting each side separately and reporting `#N/A` for the
//! length mismatch (Athena-Intel/demo-app-monorepo#43560). `#N/A` is reserved
//! for arguments of different sizes; too few pairs or a zero x variance is
//! `#DIV/0!`; an error cell propagates.

use crate::engine::{Engine, EvalConfig};
use crate::test_workbook::TestWorkbook;
use formualizer_common::{ExcelErrorKind, LiteralValue};
use formualizer_parse::parser::parse;

const EPS: f64 = 1e-9;

fn engine() -> Engine<TestWorkbook> {
    Engine::new(TestWorkbook::new(), EvalConfig::default())
}

fn set_num(engine: &mut Engine<TestWorkbook>, row: u32, col: u32, v: f64) {
    engine
        .set_cell_value("Sheet1", row, col, LiteralValue::Number(v))
        .unwrap();
}

fn set(engine: &mut Engine<TestWorkbook>, row: u32, col: u32, f: &str) {
    engine
        .set_cell_formula("Sheet1", row, col, parse(f).expect("parse"))
        .expect("set formula");
}

fn value(engine: &Engine<TestWorkbook>, row: u32, col: u32) -> LiteralValue {
    engine
        .get_cell_value("Sheet1", row, col)
        .unwrap_or(LiteralValue::Empty)
}

fn assert_num(engine: &Engine<TestWorkbook>, row: u32, col: u32, expected: f64, what: &str) {
    match value(engine, row, col) {
        LiteralValue::Number(n) => assert!(
            (n - expected).abs() < EPS,
            "{what}: expected {expected}, got {n}"
        ),
        LiteralValue::Int(i) => assert!(
            (i as f64 - expected).abs() < EPS,
            "{what}: expected {expected}, got {i}"
        ),
        other => panic!("{what}: expected {expected}, got {other:?}"),
    }
}

fn assert_err(engine: &Engine<TestWorkbook>, row: u32, col: u32, kind: ExcelErrorKind, what: &str) {
    match value(engine, row, col) {
        LiteralValue::Error(e) => assert_eq!(e.kind, kind, "{what}"),
        other => panic!("{what}: expected {kind:?}, got {other:?}"),
    }
}

/// x = A1:A5 = 1..5, y = B1:B5 = 2, (blank), 5, 4, 5 → the four pairs
/// (1,2) (3,5) (4,4) (5,5). Expected values computed by least squares over those pairs.
fn fill_noisy(engine: &mut Engine<TestWorkbook>) {
    for r in 1..=5 {
        set_num(engine, r, 1, r as f64);
    }
    set_num(engine, 1, 2, 2.0);
    set_num(engine, 3, 2, 5.0);
    set_num(engine, 4, 2, 4.0);
    set_num(engine, 5, 2, 5.0);
}

const SLOPE: f64 = 0.685_714_285_714_285_7;
const INTERCEPT: f64 = 1.771_428_571_428_571_4;
const FORECAST_6: f64 = 5.885_714_285_714_285;
const RSQ: f64 = 0.685_714_285_714_285_7;
const PEARSON: f64 = 0.828_078_671_210_825_1;
const STEYX: f64 = 0.971_008_312_455_224_6;
const COV_P: f64 = 1.5;
const COV_S: f64 = 2.0;

#[test]
fn a_blank_y_drops_its_pair_everywhere() {
    let mut engine = engine();
    fill_noisy(&mut engine);
    set(&mut engine, 10, 1, "=SLOPE(B1:B5,A1:A5)");
    set(&mut engine, 11, 1, "=INTERCEPT(B1:B5,A1:A5)");
    set(&mut engine, 12, 1, "=FORECAST(6,B1:B5,A1:A5)");
    set(&mut engine, 13, 1, "=FORECAST.LINEAR(6,B1:B5,A1:A5)");
    set(&mut engine, 14, 1, "=RSQ(B1:B5,A1:A5)");
    set(&mut engine, 15, 1, "=PEARSON(B1:B5,A1:A5)");
    set(&mut engine, 16, 1, "=CORREL(B1:B5,A1:A5)");
    set(&mut engine, 17, 1, "=STEYX(B1:B5,A1:A5)");
    set(&mut engine, 18, 1, "=COVARIANCE.P(B1:B5,A1:A5)");
    set(&mut engine, 19, 1, "=COVARIANCE.S(B1:B5,A1:A5)");
    engine.evaluate_all().unwrap();

    assert_num(&engine, 10, 1, SLOPE, "SLOPE");
    assert_num(&engine, 11, 1, INTERCEPT, "INTERCEPT");
    assert_num(&engine, 12, 1, FORECAST_6, "FORECAST");
    assert_num(&engine, 13, 1, FORECAST_6, "FORECAST.LINEAR");
    assert_num(&engine, 14, 1, RSQ, "RSQ");
    assert_num(&engine, 15, 1, PEARSON, "PEARSON");
    assert_num(&engine, 16, 1, PEARSON, "CORREL");
    assert_num(&engine, 17, 1, STEYX, "STEYX");
    assert_num(&engine, 18, 1, COV_P, "COVARIANCE.P");
    assert_num(&engine, 19, 1, COV_S, "COVARIANCE.S");
}

#[test]
fn text_and_logical_cells_on_either_side_drop_the_pair() {
    let mut engine = engine();
    fill_noisy(&mut engine);
    // Same four surviving pairs: the blank is now text, and an extra pair whose x is TRUE goes too.
    engine
        .set_cell_value("Sheet1", 2, 2, LiteralValue::Text("n/a".to_string()))
        .unwrap();
    engine
        .set_cell_value("Sheet1", 6, 1, LiteralValue::Boolean(true))
        .unwrap();
    set_num(&mut engine, 6, 2, 100.0);
    set(&mut engine, 10, 1, "=SLOPE(B1:B6,A1:A6)");
    set(&mut engine, 11, 1, "=INTERCEPT(B1:B6,A1:A6)");
    set(&mut engine, 12, 1, "=RSQ(B1:B6,A1:A6)");
    // Row vectors against column vectors pair by position too.
    set(&mut engine, 13, 1, "=SLOPE(B1:B5,{1,2,3,4,5})");
    engine.evaluate_all().unwrap();

    assert_num(&engine, 10, 1, SLOPE, "SLOPE with text y and logical x");
    assert_num(
        &engine,
        11,
        1,
        INTERCEPT,
        "INTERCEPT with text y and logical x",
    );
    assert_num(&engine, 12, 1, RSQ, "RSQ with text y and logical x");
    assert_num(&engine, 13, 1, SLOPE, "SLOPE range vs array constant");
}

#[test]
fn filter_wrapped_inputs_also_work() {
    let mut engine = engine();
    fill_noisy(&mut engine);
    set(
        &mut engine,
        10,
        1,
        "=LET(y,B1:B5,x,A1:A5,SLOPE(FILTER(y,y<>\"\"),FILTER(x,y<>\"\")))",
    );
    set(
        &mut engine,
        11,
        1,
        "=LET(y,B1:B5,x,A1:A5,INTERCEPT(FILTER(y,y<>\"\"),FILTER(x,y<>\"\")))",
    );
    engine.evaluate_all().unwrap();
    assert_num(&engine, 10, 1, SLOPE, "SLOPE over FILTERed inputs");
    assert_num(&engine, 11, 1, INTERCEPT, "INTERCEPT over FILTERed inputs");
}

#[test]
fn too_few_pairs_is_div0_and_size_mismatch_is_na() {
    let mut engine = engine();
    for r in 1..=5 {
        set_num(&mut engine, r, 1, r as f64); // x = 1..5, y column D blank
    }
    set_num(&mut engine, 1, 4, 10.0); // one valid pair (1, 10)
    set_num(&mut engine, 1, 5, 1.0); // E1:E2 = 1,2  F1:F2 = 3,4 → exactly two pairs
    set_num(&mut engine, 2, 5, 2.0);
    set_num(&mut engine, 1, 6, 3.0);
    set_num(&mut engine, 2, 6, 4.0);

    set(&mut engine, 10, 1, "=SLOPE(C1:C5,A1:A5)"); // all y blank
    set(&mut engine, 11, 1, "=INTERCEPT(C1:C5,A1:A5)");
    set(&mut engine, 12, 1, "=FORECAST(6,C1:C5,A1:A5)");
    set(&mut engine, 13, 1, "=RSQ(C1:C5,A1:A5)");
    set(&mut engine, 14, 1, "=CORREL(C1:C5,A1:A5)");
    set(&mut engine, 15, 1, "=SLOPE(D1:D5,A1:A5)"); // one pair → x variance 0
    set(&mut engine, 16, 1, "=FORECAST(6,D1:D5,A1:A5)");
    set(&mut engine, 17, 1, "=STEYX(F1:F2,E1:E2)"); // two pairs → n < 3
    set(&mut engine, 18, 1, "=SLOPE(F1:F2,E1:E2)"); // two pairs is enough for a slope
    set(&mut engine, 19, 1, "=SLOPE(B1:B4,A1:A5)"); // 4 cells vs 5 cells
    set(&mut engine, 20, 1, "=FORECAST(6,B1:B4,A1:A5)");
    set(&mut engine, 21, 1, "=COVARIANCE.S(D1:D5,A1:A5)"); // one pair → n-1 = 0
    engine.evaluate_all().unwrap();

    for (row, what) in [
        (10, "SLOPE all-blank y"),
        (11, "INTERCEPT all-blank y"),
        (12, "FORECAST all-blank y"),
        (13, "RSQ all-blank y"),
        (14, "CORREL all-blank y"),
        (15, "SLOPE one pair"),
        (16, "FORECAST one pair"),
        (17, "STEYX two pairs"),
        (21, "COVARIANCE.S one pair"),
    ] {
        assert_err(&engine, row, 1, ExcelErrorKind::Div, what);
    }
    assert_num(&engine, 18, 1, 1.0, "SLOPE over two pairs");
    assert_err(&engine, 19, 1, ExcelErrorKind::Na, "SLOPE size mismatch");
    assert_err(&engine, 20, 1, ExcelErrorKind::Na, "FORECAST size mismatch");
}

#[test]
fn an_error_cell_propagates() {
    let mut engine = engine();
    fill_noisy(&mut engine);
    set(&mut engine, 2, 2, "=\"x\"+1"); // B2 = #VALUE!
    set(&mut engine, 10, 1, "=SLOPE(B1:B5,A1:A5)");
    set(&mut engine, 11, 1, "=FORECAST(6,B1:B5,A1:A5)");
    set(&mut engine, 12, 1, "=CORREL(A1:A5,B1:B5)");
    engine.evaluate_all().unwrap();
    assert_err(&engine, 10, 1, ExcelErrorKind::Value, "SLOPE error in y");
    assert_err(&engine, 11, 1, ExcelErrorKind::Value, "FORECAST error in y");
    assert_err(&engine, 12, 1, ExcelErrorKind::Value, "CORREL error in x");
}

#[test]
fn clean_inputs_are_unchanged() {
    let mut engine = engine();
    set(&mut engine, 1, 1, "=SLOPE({2,4,6},{1,2,3})");
    set(&mut engine, 2, 1, "=INTERCEPT({3,5,7},{1,2,3})");
    set(&mut engine, 3, 1, "=FORECAST(4,{2,4,6},{1,2,3})");
    set(&mut engine, 4, 1, "=STEYX({2,5,7},{1,2,3})");
    set(&mut engine, 5, 1, "=COVARIANCE.P({1,3,5},{2,4,6})");
    engine.evaluate_all().unwrap();
    assert_num(&engine, 1, 1, 2.0, "SLOPE");
    assert_num(&engine, 2, 1, 1.0, "INTERCEPT");
    assert_num(&engine, 3, 1, 8.0, "FORECAST");
    assert_num(&engine, 4, 1, 0.408_248_290_463_863, "STEYX");
    assert_num(&engine, 5, 1, 2.666_666_666_666_666_5, "COVARIANCE.P");
}
