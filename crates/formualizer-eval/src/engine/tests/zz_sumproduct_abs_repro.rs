use crate::engine::{Engine, EvalConfig};
use crate::test_workbook::TestWorkbook;
use formualizer_common::LiteralValue;
use formualizer_parse::parser::parse;

fn serial_eval_config() -> EvalConfig {
    EvalConfig {
        enable_parallel: false,
        ..Default::default()
    }
}

fn engine_with_signed_row() -> Engine<TestWorkbook> {
    let wb = TestWorkbook::new();
    let mut engine = Engine::new(wb, serial_eval_config());
    for (i, v) in [-2.0, 3.0, -5.0, 10.0].iter().enumerate() {
        engine
            .set_cell_value("Sheet1", 3, (i + 1) as u32, LiteralValue::Number(*v))
            .unwrap();
    }
    engine
}

/// Regression: ABS must lift element-wise over range args so the share-of-total idiom
/// `=ABS(A3)/SUMPRODUCT(ABS($A3:$D3))` works. Previously ABS collapsed the range arg to a
/// scalar coercion failure, so every cell of the idiom computed `#VALUE!` (observed on an
/// imported customer workbook: 13,486 cells of `#VALUE!` where Excel showed percentages).
#[test]
fn sumproduct_abs_sums_magnitudes_over_range() {
    let mut engine = engine_with_signed_row();
    engine
        .set_cell_formula("Sheet1", 1, 1, parse("=SUMPRODUCT(ABS($A3:$D3))").unwrap())
        .unwrap();
    engine
        .set_cell_formula(
            "Sheet1",
            1,
            2,
            parse("=ABS(A3)/SUMPRODUCT(ABS($A3:$D3))").unwrap(),
        )
        .unwrap();
    let _ = engine.evaluate_all().unwrap();
    assert_eq!(
        engine.get_cell_value("Sheet1", 1, 1),
        Some(LiteralValue::Number(20.0))
    );
    assert_eq!(
        engine.get_cell_value("Sheet1", 1, 2),
        Some(LiteralValue::Number(0.1))
    );
}

/// Regression: INT must lift element-wise over range args inside array context.
#[test]
fn sumproduct_int_floors_each_cell_over_range() {
    let wb = TestWorkbook::new();
    let mut engine = Engine::new(wb, serial_eval_config());
    for (i, v) in [1.4, 2.6, -3.7, 4.25].iter().enumerate() {
        engine
            .set_cell_value("Sheet1", 5, (i + 1) as u32, LiteralValue::Number(*v))
            .unwrap();
    }
    engine
        .set_cell_formula("Sheet1", 1, 1, parse("=SUMPRODUCT(INT(A5:D5))").unwrap())
        .unwrap();
    let _ = engine.evaluate_all().unwrap();
    // floor: 1 + 2 + (-4) + 4
    assert_eq!(
        engine.get_cell_value("Sheet1", 1, 1),
        Some(LiteralValue::Number(3.0))
    );
}

/// Regression: ROUND must broadcast a scalar second arg against a range first arg.
#[test]
fn sumproduct_round_broadcasts_scalar_digits_over_range() {
    let wb = TestWorkbook::new();
    let mut engine = Engine::new(wb, serial_eval_config());
    for (i, v) in [1.234, 2.567, -3.891, 4.006].iter().enumerate() {
        engine
            .set_cell_value("Sheet1", 5, (i + 1) as u32, LiteralValue::Number(*v))
            .unwrap();
    }
    engine
        .set_cell_formula(
            "Sheet1",
            1,
            1,
            parse("=SUMPRODUCT(ROUND(A5:D5, 2))").unwrap(),
        )
        .unwrap();
    let _ = engine.evaluate_all().unwrap();
    // 1.23 + 2.57 + (-3.89) + 4.01
    match engine.get_cell_value("Sheet1", 1, 1) {
        Some(LiteralValue::Number(v)) => assert!((v - 3.92).abs() < 1e-9, "got {v}"),
        other => panic!("expected number, got {other:?}"),
    }
}

/// Regression: MOD must broadcast a scalar divisor against a range dividend.
#[test]
fn sumproduct_mod_broadcasts_scalar_divisor_over_range() {
    let mut engine = engine_with_signed_row();
    engine
        .set_cell_formula(
            "Sheet1",
            1,
            1,
            parse("=SUMPRODUCT(MOD($A3:$D3, 3))").unwrap(),
        )
        .unwrap();
    let _ = engine.evaluate_all().unwrap();
    // MOD(-2,3)=1, MOD(3,3)=0, MOD(-5,3)=1, MOD(10,3)=1
    assert_eq!(
        engine.get_cell_value("Sheet1", 1, 1),
        Some(LiteralValue::Number(3.0))
    );
}

/// Scalar semantics of the swept functions are unchanged.
#[test]
fn swept_numeric_scalar_semantics_unchanged() {
    let mut engine = engine_with_signed_row();
    engine
        .set_cell_formula("Sheet1", 1, 1, parse("=INT(A3)").unwrap())
        .unwrap();
    engine
        .set_cell_formula("Sheet1", 1, 2, parse("=MOD(10,3)").unwrap())
        .unwrap();
    engine
        .set_cell_formula("Sheet1", 1, 3, parse("=SQRT(D3*D3)").unwrap())
        .unwrap();
    let _ = engine.evaluate_all().unwrap();
    assert_eq!(
        engine.get_cell_value("Sheet1", 1, 1),
        Some(LiteralValue::Number(-2.0))
    );
    assert_eq!(
        engine.get_cell_value("Sheet1", 1, 2),
        Some(LiteralValue::Number(1.0))
    );
    assert_eq!(
        engine.get_cell_value("Sheet1", 1, 3),
        Some(LiteralValue::Number(10.0))
    );
}

/// An error cell inside the lifted range still poisons the aggregate result.
#[test]
fn sumproduct_int_propagates_error_cells() {
    let wb = TestWorkbook::new();
    let mut engine = Engine::new(wb, serial_eval_config());
    for (i, v) in [1.4, 2.6, -3.7].iter().enumerate() {
        engine
            .set_cell_value("Sheet1", 6, (i + 1) as u32, LiteralValue::Number(*v))
            .unwrap();
    }
    engine
        .set_cell_formula("Sheet1", 6, 4, parse("=1/0").unwrap())
        .unwrap();
    engine
        .set_cell_formula("Sheet1", 1, 1, parse("=SUMPRODUCT(INT(A6:D6))").unwrap())
        .unwrap();
    let _ = engine.evaluate_all().unwrap();
    match engine.get_cell_value("Sheet1", 1, 1) {
        Some(LiteralValue::Error(e)) => assert_eq!(e.to_string(), "#DIV/0!"),
        other => panic!("expected #DIV/0!, got {other:?}"),
    }
}

/// Scalar ABS semantics are unchanged: plain refs and literals still return a scalar,
/// and input errors still propagate.
#[test]
fn abs_scalar_semantics_unchanged() {
    let mut engine = engine_with_signed_row();
    engine
        .set_cell_formula("Sheet1", 1, 1, parse("=ABS(A3)").unwrap())
        .unwrap();
    engine
        .set_cell_formula("Sheet1", 1, 2, parse("=ABS(1/0)").unwrap())
        .unwrap();
    let _ = engine.evaluate_all().unwrap();
    assert_eq!(
        engine.get_cell_value("Sheet1", 1, 1),
        Some(LiteralValue::Number(2.0))
    );
    match engine.get_cell_value("Sheet1", 1, 2) {
        Some(LiteralValue::Error(e)) => assert_eq!(e.to_string(), "#DIV/0!"),
        other => panic!("expected #DIV/0!, got {other:?}"),
    }
}
