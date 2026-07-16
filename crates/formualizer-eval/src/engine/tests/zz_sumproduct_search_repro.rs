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

fn engine_with_beers() -> Engine<TestWorkbook> {
    let wb = TestWorkbook::new();
    let mut engine = Engine::new(wb, serial_eval_config());
    let vals = [
        "BUDWEISER 12PK",
        "COORS LIGHT",
        "BUD LIGHT BUDWEISER",
        "MODELO",
        "budweiser zero",
    ];
    for (i, v) in vals.iter().enumerate() {
        engine
            .set_cell_value(
                "Sheet1",
                (i + 2) as u32,
                3,
                LiteralValue::Text((*v).to_string()),
            )
            .unwrap();
    }
    engine
}

/// Regression: SEARCH/ISNUMBER must lift element-wise over range args so the
/// classic `SUMPRODUCT(--ISNUMBER(SEARCH("x", range)))` count idiom works.
/// Previously SEARCH collapsed the range to its first cell, so the whole
/// expression degenerated to a scalar 0/1.
#[test]
fn sumproduct_isnumber_search_counts_matches_over_range() {
    let mut engine = engine_with_beers();
    engine
        .set_cell_formula(
            "Sheet1",
            1,
            1,
            parse("=SUMPRODUCT(--ISNUMBER(SEARCH(\"BUDWEISER\",C2:C6)))").unwrap(),
        )
        .unwrap();
    let _ = engine.evaluate_all().unwrap();
    assert_eq!(
        engine.get_cell_value("Sheet1", 1, 1),
        Some(LiteralValue::Number(3.0))
    );
}

#[test]
fn sumproduct_isnumber_search_lifts_over_array_literal() {
    let wb = TestWorkbook::new();
    let mut engine = Engine::new(wb, serial_eval_config());
    engine
        .set_cell_formula(
            "Sheet1",
            1,
            1,
            parse("=SUMPRODUCT(--ISNUMBER(SEARCH(\"BUDWEISER\",{\"BUDWEISER A\";\"X\";\"budweiser b\"})))")
                .unwrap(),
        )
        .unwrap();
    let _ = engine.evaluate_all().unwrap();
    assert_eq!(
        engine.get_cell_value("Sheet1", 1, 1),
        Some(LiteralValue::Number(2.0))
    );
}

#[test]
fn sumproduct_find_is_case_sensitive_over_range() {
    let mut engine = engine_with_beers();
    engine
        .set_cell_formula(
            "Sheet1",
            1,
            1,
            parse("=SUMPRODUCT(--ISNUMBER(FIND(\"BUDWEISER\",C2:C6)))").unwrap(),
        )
        .unwrap();
    let _ = engine.evaluate_all().unwrap();
    // FIND is case-sensitive: "budweiser zero" must not match.
    assert_eq!(
        engine.get_cell_value("Sheet1", 1, 1),
        Some(LiteralValue::Number(2.0))
    );
}

#[test]
fn sumproduct_exact_lifts_over_range() {
    let wb = TestWorkbook::new();
    let mut engine = Engine::new(wb, serial_eval_config());
    for (i, v) in ["a", "A", "a", "b"].iter().enumerate() {
        engine
            .set_cell_value(
                "Sheet1",
                (i + 1) as u32,
                1,
                LiteralValue::Text((*v).to_string()),
            )
            .unwrap();
    }
    engine
        .set_cell_formula(
            "Sheet1",
            1,
            2,
            parse("=SUMPRODUCT(--EXACT(A1:A4,\"a\"))").unwrap(),
        )
        .unwrap();
    let _ = engine.evaluate_all().unwrap();
    assert_eq!(
        engine.get_cell_value("Sheet1", 1, 2),
        Some(LiteralValue::Number(2.0))
    );
}

/// Scalar behavior must not regress: a non-matching scalar SEARCH still yields
/// #VALUE!, and ISNUMBER of that error is FALSE (not an error).
#[test]
fn scalar_search_isnumber_error_semantics_unchanged() {
    let wb = TestWorkbook::new();
    let mut engine = Engine::new(wb, serial_eval_config());
    engine
        .set_cell_formula(
            "Sheet1",
            1,
            1,
            parse("=ISNUMBER(SEARCH(\"zzz\",\"abc\"))").unwrap(),
        )
        .unwrap();
    engine
        .set_cell_formula("Sheet1", 1, 2, parse("=SEARCH(\"zzz\",\"abc\")").unwrap())
        .unwrap();
    engine
        .set_cell_formula(
            "Sheet1",
            1,
            3,
            parse("=SEARCH(\"world\",\"Hello World\")").unwrap(),
        )
        .unwrap();
    let _ = engine.evaluate_all().unwrap();
    assert_eq!(
        engine.get_cell_value("Sheet1", 1, 1),
        Some(LiteralValue::Boolean(false))
    );
    assert!(matches!(
        engine.get_cell_value("Sheet1", 1, 2),
        Some(LiteralValue::Error(_))
    ));
    assert_eq!(
        engine.get_cell_value("Sheet1", 1, 3),
        Some(LiteralValue::Number(7.0))
    );
}

/// start_num broadcasts too, and a per-element invalid start poisons only its
/// own element.
#[test]
fn search_start_num_broadcasts_elementwise() {
    let wb = TestWorkbook::new();
    let mut engine = Engine::new(wb, serial_eval_config());
    engine
        .set_cell_formula(
            "Sheet1",
            1,
            1,
            parse("=SUMPRODUCT(--ISNUMBER(SEARCH(\"a\",{\"abc\";\"xay\";\"zzz\"},{1;2;1})))")
                .unwrap(),
        )
        .unwrap();
    let _ = engine.evaluate_all().unwrap();
    assert_eq!(
        engine.get_cell_value("Sheet1", 1, 1),
        Some(LiteralValue::Number(2.0))
    );
}
