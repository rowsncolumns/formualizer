//! Excel-parity pins for the SHAPE of `TREND` / `GROWTH` results (rowsncolumns/spreadsheet#939 G-09).
//!
//! Excel returns the fitted values in the orientation of `new_x` — a column of new x's spills DOWN
//! (`TREND(B1:B5,A1:A5,A6:A8)` is 3×1), a row spills across — and, with `new_x` omitted, in the
//! orientation of `known_y`. The fork always returned a single row, so a column model's forecast
//! landed beside the data instead of under it. `LINEST` / `LOGEST` (1×2 or 5×2 blocks) and
//! `FORECAST.LINEAR` (scalar) keep their Excel shapes and are pinned alongside.
//!
//! Seed grid (1-based): A1:A5 = 1..5, B1:B5 = 2,4,6,8,10 (y = 2x), A6:A8 = 6,7,8 ·
//! D1:H1 = 1..5, D2:H2 = 2,4,6,8,10 · K1:M1 = 6,7,8.

use formualizer_common::LiteralValue;
use formualizer_eval::engine::{Engine, EvalConfig};
use formualizer_eval::test_workbook::TestWorkbook;
use formualizer_parse::parser::parse;

fn seeded() -> Engine<TestWorkbook> {
    let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
    for i in 1..=5u32 {
        e.set_cell_value("Sheet1", i, 1, LiteralValue::Number(i as f64))
            .unwrap();
        e.set_cell_value("Sheet1", i, 2, LiteralValue::Number(2.0 * i as f64))
            .unwrap();
        e.set_cell_value("Sheet1", 1, 3 + i, LiteralValue::Number(i as f64))
            .unwrap();
        e.set_cell_value("Sheet1", 2, 3 + i, LiteralValue::Number(2.0 * i as f64))
            .unwrap();
    }
    for (i, v) in [6.0, 7.0, 8.0].into_iter().enumerate() {
        e.set_cell_value("Sheet1", 6 + i as u32, 1, LiteralValue::Number(v))
            .unwrap();
        e.set_cell_value("Sheet1", 1, 11 + i as u32, LiteralValue::Number(v))
            .unwrap();
    }
    e
}

/// Evaluate `formula` anchored at Z1 and read back the `rows`×`cols` block it spilled, asserting
/// the cell just past it in each direction is untouched (so the SHAPE, not only the values, is
/// checked).
fn spill(formula: &str, rows: u32, cols: u32) -> Vec<Vec<f64>> {
    let mut e = seeded();
    e.set_cell_formula("Sheet1", 1, 26, parse(formula).unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    let num = |r: u32, c: u32| match e.get_cell_value("Sheet1", r, c) {
        Some(LiteralValue::Number(n)) => n,
        Some(LiteralValue::Int(i)) => i as f64,
        other => panic!("{formula}: expected a number at r{r} c{c}, got {other:?}"),
    };
    let block: Vec<Vec<f64>> = (0..rows)
        .map(|r| (0..cols).map(|c| num(1 + r, 26 + c)).collect())
        .collect();
    let beyond_row = e.get_cell_value("Sheet1", 1 + rows, 26);
    let beyond_col = e.get_cell_value("Sheet1", 1, 26 + cols);
    assert!(
        matches!(beyond_row, None | Some(LiteralValue::Empty)),
        "{formula}: spilled more than {rows} row(s): {beyond_row:?}"
    );
    assert!(
        matches!(beyond_col, None | Some(LiteralValue::Empty)),
        "{formula}: spilled more than {cols} column(s): {beyond_col:?}"
    );
    block
}

fn close(a: &[Vec<f64>], b: &[Vec<f64>]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(ra, rb)| {
            ra.len() == rb.len() && ra.iter().zip(rb).all(|(x, y)| (x - y).abs() < 1e-9)
        })
}

fn assert_block(formula: &str, expected: &[Vec<f64>]) {
    let got = spill(formula, expected.len() as u32, expected[0].len() as u32);
    assert!(
        close(&got, expected),
        "{formula}: got {got:?}, expected {expected:?}"
    );
}

#[test]
fn trend_column_new_x_spills_down() {
    assert_block(
        "=TREND(B1:B5,A1:A5,A6:A8)",
        &[vec![12.0], vec![14.0], vec![16.0]],
    );
    assert_block("=TREND(B1:B5,A1:A5,{6;7})", &[vec![12.0], vec![14.0]]);
}

#[test]
fn trend_row_new_x_spills_across() {
    assert_block("=TREND(D2:H2,D1:H1,K1:M1)", &[vec![12.0, 14.0, 16.0]]);
    assert_block("=TREND(B1:B5,A1:A5,{6,7})", &[vec![12.0, 14.0]]);
}

#[test]
fn trend_without_new_x_follows_known_y() {
    let col: Vec<Vec<f64>> = (1..=5).map(|i| vec![2.0 * i as f64]).collect();
    assert_block("=TREND(B1:B5,A1:A5)", &col);
    assert_block("=TREND(B1:B5)", &col);
    assert_block("=TREND(D2:H2,D1:H1)", &[vec![2.0, 4.0, 6.0, 8.0, 10.0]]);
}

#[test]
fn growth_follows_new_x_then_known_y() {
    assert_block("=GROWTH({2;4;8},{1;2;3},{4;5})", &[vec![16.0], vec![32.0]]);
    assert_block("=GROWTH({2,4,8},{1,2,3},{4,5})", &[vec![16.0, 32.0]]);
    assert_block(
        "=GROWTH({2;4;8},{1;2;3})",
        &[vec![2.0], vec![4.0], vec![8.0]],
    );
    assert_block("=GROWTH({3,6,12})", &[vec![3.0, 6.0, 12.0]]);
}

#[test]
fn trend_single_new_x_is_a_scalar() {
    assert_block("=TREND(B1:B5,A1:A5,6)", &[vec![12.0]]);
    assert_block("=TREND(B1:B5,A1:A5,A6)", &[vec![12.0]]);
}

#[test]
fn linest_logest_forecast_keep_their_shapes() {
    assert_block("=LINEST(B1:B5,A1:A5)", &[vec![2.0, 0.0]]);
    let stats = spill("=LINEST({1;3;2;5;4},{1;2;3;4;5},TRUE,TRUE)", 5, 2);
    assert!(
        (stats[0][0] - 0.8).abs() < 1e-9 && (stats[0][1] - 0.6).abs() < 1e-9,
        "{stats:?}"
    );
    assert_block("=LOGEST({2;4;8},{1;2;3})", &[vec![2.0, 1.0]]);
    assert_block("=FORECAST.LINEAR(6,B1:B5,A1:A5)", &[vec![12.0]]);
    assert_block("=FORECAST(6,B1:B5,A1:A5)", &[vec![12.0]]);
}
