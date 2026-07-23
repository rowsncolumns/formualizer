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
        .set_cell_formula(
            "Sheet1",
            1,
            1,
            parse("=SUMPRODUCT(ABS($A3:$D3))").unwrap(),
        )
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
