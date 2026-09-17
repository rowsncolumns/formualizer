//! rowsncolumns/spreadsheet#642 — `FORECAST.ETS` behaves like Excel on short histories and far
//! targets.
//!
//! B-06: `ForecastEtsFn::eval` used to size a `Vec` by the forecast horizon
//! (`(target - last_x) / step`), so a target like `1E15` over a unit-step timeline asked for a
//! `usize::MAX`-length allocation on wasm32 (`capacity overflow` panic → the wasm traps → every
//! later engine call throws "recursive use of an object detected"). Excel returns a finite
//! extrapolation for any target after the last timeline point: the forecast is
//! `level + h·trend + seasonal[h mod m]`, O(1) in `h`.
//!
//! B-01: Excel fits from as few as two numeric pairs (there was a hard-coded four-pair `#N/A`), and
//! a perfectly linear history forecasts its exact continuation (the Holt level started *at* the
//! first value, so `y₀` was predicted as `y₀ + trend` and the grid search absorbed the spurious
//! residual: `FORECAST.ETS(5,{1,2,3,4},{1,2,3,4})` was 4.916, `…(11, 10..100)` was 109.56).

use formualizer_common::{ExcelErrorKind, LiteralValue};
use formualizer_eval::engine::{Engine, EvalConfig};
use formualizer_eval::test_workbook::TestWorkbook;
use formualizer_parse::parser::parse;
use std::time::{Duration, Instant};

/// A1:A{n} = B1:B{n} = 1..=n, the identity series.
fn engine_with_identity_series(n: u32) -> Engine<TestWorkbook> {
    let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
    for i in 1..=n {
        e.set_cell_value("Sheet1", i, 1, LiteralValue::Number(i as f64))
            .unwrap();
        e.set_cell_value("Sheet1", i, 2, LiteralValue::Number(i as f64))
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

fn assert_close(formula: &str, actual: &LiteralValue, expected: f64) {
    let actual = as_num(actual, formula);
    assert!(
        (actual - expected).abs() <= 1e-9 * expected.abs().max(1.0),
        "{formula}: expected {expected}, got {actual}"
    );
}

/* ─────────────────────────── B-06 ─────────────────────────── */

#[test]
fn forecast_ets_far_target_returns_a_finite_extrapolation_in_constant_time() {
    let started = Instant::now();
    let mut e = engine_with_identity_series(4);
    let v = eval_in(&mut e, "=FORECAST.ETS(1E15,A1:A4,B1:B4)");
    let elapsed = started.elapsed();
    // A perfect line continues exactly: 1E15 steps of slope 1 from 4 → 1E15 (h = 1E15 - 4).
    assert_close("=FORECAST.ETS(1E15,A1:A4,B1:B4)", &v, 1e15);
    assert!(
        elapsed < Duration::from_secs(2),
        "FORECAST.ETS over a huge horizon must be O(1) in the horizon, took {elapsed:?}"
    );
    // Array constants take the same path, and so does the confidence interval.
    let v = eval("=FORECAST.ETS(1E15,{1,2,3,4},{1,2,3,4})");
    assert_close("=FORECAST.ETS(1E15,{1,2,3,4},{1,2,3,4})", &v, 1e15);
    let confint = eval("=FORECAST.ETS.CONFINT(1E15,{1,2,3,4},{1,2,3,4})");
    assert!(
        as_num(&confint, "confint").is_finite(),
        "CONFINT over a huge horizon is finite: {confint:?}"
    );
}

#[test]
fn forecast_ets_horizon_beyond_f64_is_num_not_a_panic() {
    // A step so small the horizon overflows f64: Excel's answer is an error, never a hang.
    let v = eval("=FORECAST.ETS(1E308,{1,2,3,4},{1E-300,2E-300,3E-300,4E-300})");
    match v {
        LiteralValue::Error(e) => assert_eq!(e.kind, ExcelErrorKind::Num, "{e:?}"),
        other => panic!("expected #NUM!, got {other:?}"),
    }
}

#[test]
fn forecast_ets_small_horizons_match_the_step_by_step_forecast() {
    // The terminal-state forecast must be the value the old per-step vector produced: the seasonal
    // slot for h steps out is (n + h - 1) mod m, so on a period-4 cycle three steps past the end
    // (h = 3 over 12 observations) is the cycle's third value.
    let seasonal = "{10;30;20;40;10;30;20;40;10;30;20;40},{1;2;3;4;5;6;7;8;9;10;11;12}";
    for (h, expected) in [(1, 10.0), (2, 30.0), (3, 20.0), (4, 40.0), (5, 10.0)] {
        let formula = format!("=FORECAST.ETS({},{seasonal},4)", 12 + h);
        assert_close(&formula, &eval(&formula), expected);
    }
    // Holt's linear method: a perfect line continues exactly at every horizon.
    for h in [1, 2, 7, 1_000, 1_000_000] {
        let formula = format!("=FORECAST.ETS({},{{1,2,3,4}},{{1,2,3,4}},0)", 4 + h);
        assert_close(&formula, &eval(&formula), (4 + h) as f64);
    }
}

/* ─────────────────────────── B-01 ─────────────────────────── */

#[test]
fn forecast_ets_fits_a_three_point_history_like_excel() {
    // Excel: =FORECAST.ETS(4,{1,2,3},{1,2,3},1) → 4, array constants and ranges alike.
    assert_close(
        "=FORECAST.ETS(4,{1,2,3},{1,2,3},1)",
        &eval("=FORECAST.ETS(4,{1,2,3},{1,2,3},1)"),
        4.0,
    );
    let mut e = engine_with_identity_series(3);
    assert_close(
        "=FORECAST.ETS(4,A1:A3,B1:B3,1)",
        &eval_in(&mut e, "=FORECAST.ETS(4,A1:A3,B1:B3,1)"),
        4.0,
    );
    // Two pairs are enough; fewer than two numeric pairs is #N/A; two pairs on the same timeline
    // point have no step (#NUM!).
    assert_close(
        "=FORECAST.ETS(3,{1,2},{1,2})",
        &eval("=FORECAST.ETS(3,{1,2},{1,2})"),
        3.0,
    );
    for formula in [
        "=FORECAST.ETS(2,{1},{1})",
        "=FORECAST.ETS(2,{\"a\",2},{1,2})",
    ] {
        match eval(formula) {
            LiteralValue::Error(e) => assert_eq!(e.kind, ExcelErrorKind::Na, "{formula}: {e:?}"),
            other => panic!("{formula}: expected #N/A, got {other:?}"),
        }
    }
    match eval("=FORECAST.ETS(2,{1,2},{1,1})") {
        LiteralValue::Error(e) => assert_eq!(e.kind, ExcelErrorKind::Num, "{e:?}"),
        other => panic!("no positive gap: expected #NUM!, got {other:?}"),
    }
}

#[test]
fn forecast_ets_perfectly_linear_history_forecasts_its_exact_continuation() {
    assert_close(
        "=FORECAST.ETS(5,{1,2,3,4},{1,2,3,4},1)",
        &eval("=FORECAST.ETS(5,{1,2,3,4},{1,2,3,4},1)"),
        5.0,
    );
    let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
    for i in 1..=10u32 {
        e.set_cell_value("Sheet1", i, 1, LiteralValue::Number(i as f64 * 10.0))
            .unwrap();
        e.set_cell_value("Sheet1", i, 2, LiteralValue::Number(i as f64))
            .unwrap();
    }
    assert_close(
        "=FORECAST.ETS(11,A1:A10,B1:B10,0)",
        &eval_in(&mut e, "=FORECAST.ETS(11,A1:A10,B1:B10,0)"),
        110.0,
    );
    // …so the in-sample error, and with it the confidence band, is zero on a perfect line.
    assert_close(
        "=FORECAST.ETS.CONFINT(11,A1:A10,B1:B10)",
        &eval_in(&mut e, "=FORECAST.ETS.CONFINT(11,A1:A10,B1:B10)"),
        0.0,
    );
}
