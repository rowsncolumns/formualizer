use crate::engine::{Engine, EvalConfig};
use crate::test_workbook::TestWorkbook;
use formualizer_common::LiteralValue;
use formualizer_parse::parser::parse;

/// Manual perf probe (run explicitly): `cargo test --release -p formualizer-eval
/// zz_perf_sumproduct_search -- --ignored --nocapture`
#[test]
#[ignore]
fn zz_perf_sumproduct_search_400k_rows() {
    let wb = TestWorkbook::new();
    let mut engine = Engine::new(
        wb,
        EvalConfig {
            enable_parallel: false,
            ..Default::default()
        },
    );

    let n: u32 = 393_739;
    for i in 0..n {
        let v = if i % 6 == 0 {
            format!("BUDWEISER ITEM {i}")
        } else {
            format!("OTHER BRAND {i}")
        };
        engine
            .set_cell_value("Sheet1", i + 2, 3, LiteralValue::Text(v))
            .unwrap();
    }
    engine
        .set_cell_formula(
            "Sheet1",
            1,
            30,
            parse("=SUMPRODUCT(--ISNUMBER(SEARCH(\"BUDWEISER\",C2:C393740)))").unwrap(),
        )
        .unwrap();
    let t0 = std::time::Instant::now();
    let _ = engine.evaluate_all().unwrap();
    let elapsed = t0.elapsed();
    let v = engine.get_cell_value("Sheet1", 1, 30);
    eprintln!("evaluate_all took {elapsed:?}, result={v:?}");
    assert_eq!(v, Some(LiteralValue::Number(65624.0)));
}
