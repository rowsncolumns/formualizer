//! Excel parity for ZERO-SIZED array results (rowsncolumns/spreadsheet#939 G-01).
//!
//! Excel has no empty array: `TAKE(rng,0)`, `DROP(rng,<every row>)` and any function whose result
//! has no rows or no columns show `#CALC!`. The engine used to hand a `0×n` shape to the spill
//! planner, whose `anchor..=anchor+0-1` rectangle is inverted; `BTreeMap::range` panics on it
//! once another spill exists on the sheet — in wasm that aborts the whole engine. With nothing
//! else spilling, the same formulas silently produced `0`.

use std::sync::Arc;

use formualizer_common::{ExcelError, ExcelErrorKind, LiteralValue};
use formualizer_eval::args::ArgSchema;
use formualizer_eval::engine::{Engine, EvalConfig};
use formualizer_eval::function::Function;
use formualizer_eval::function_registry;
use formualizer_eval::test_workbook::TestWorkbook;
use formualizer_eval::traits::{ArgumentHandle, CalcValue, FunctionContext};
use formualizer_parse::parser::parse;

/// A host function whose result is a `0×0` array — the shape any zero-sized producer hands the
/// engine (the built-in TAKE / DROP now refuse it themselves; this pins the engine-level guard).
struct EmptyArrayFn;

impl Function for EmptyArrayFn {
    fn name(&self) -> &'static str {
        "TEST_EMPTY_ARRAY"
    }
    fn min_args(&self) -> usize {
        0
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        &[]
    }
    fn eval<'a, 'b, 'c>(
        &self,
        _args: &'c [ArgumentHandle<'a, 'b>],
        _ctx: &dyn FunctionContext<'b>,
    ) -> Result<CalcValue<'b>, ExcelError> {
        Ok(CalcValue::Scalar(LiteralValue::Array(vec![])))
    }
}

/// A host function whose result has rows but no columns (`2×0`).
struct ZeroColumnsFn;

impl Function for ZeroColumnsFn {
    fn name(&self) -> &'static str {
        "TEST_ZERO_COLUMNS"
    }
    fn min_args(&self) -> usize {
        0
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        &[]
    }
    fn eval<'a, 'b, 'c>(
        &self,
        _args: &'c [ArgumentHandle<'a, 'b>],
        _ctx: &dyn FunctionContext<'b>,
    ) -> Result<CalcValue<'b>, ExcelError> {
        Ok(CalcValue::Scalar(LiteralValue::Array(vec![vec![], vec![]])))
    }
}

fn n(v: f64) -> LiteralValue {
    LiteralValue::Number(v)
}

/// A1:C5 = the sweep's fruit table plus, when asked, a `SEQUENCE(3)` spill at W1 — the
/// production shape under which the zero-sized result trapped the engine.
fn seeded(with_other_spill: bool) -> Engine<TestWorkbook> {
    formualizer_eval::builtins::load_builtins();
    function_registry::register_function(Arc::new(EmptyArrayFn));
    function_registry::register_function(Arc::new(ZeroColumnsFn));
    let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
    let table: [(f64, &str, f64); 5] = [
        (10.0, "apple", 1.0),
        (20.0, "banana", 2.0),
        (30.0, "cherry", 3.0),
        (40.0, "date", 4.0),
        (50.0, "elderberry", 5.0),
    ];
    for (i, (a, b, c)) in table.iter().enumerate() {
        let r = i as u32 + 1;
        e.set_cell_value("Sheet1", r, 1, n(*a)).unwrap();
        e.set_cell_value("Sheet1", r, 2, LiteralValue::Text((*b).into()))
            .unwrap();
        e.set_cell_value("Sheet1", r, 3, n(*c)).unwrap();
    }
    if with_other_spill {
        e.set_cell_formula("Sheet1", 1, 23, parse("=SEQUENCE(3)").unwrap())
            .unwrap();
        e.evaluate_all().unwrap();
        assert_eq!(e.get_cell_value("Sheet1", 3, 23), Some(n(3.0)));
    }
    e
}

fn is_calc(v: &Option<LiteralValue>) -> bool {
    matches!(v, Some(LiteralValue::Error(err)) if err.kind == ExcelErrorKind::Calc)
}

const EMPTY_RESULTS: &[&str] = &[
    "=TAKE(A1:C5,0)",
    "=TAKE(A1:C5,,0)",
    "=TAKE(A1:C5,0,0)",
    "=DROP(A1:C5,5)",
    "=DROP(A1:C5,-5)",
    "=DROP(A1:C5,7)",
    "=DROP(A1:C5,,3)",
    "=TAKE(SEQUENCE(3),0)",
    "=DROP(SEQUENCE(3),3)",
    "=TEST_EMPTY_ARRAY()",
    "=TEST_ZERO_COLUMNS()",
];

/// The trap: a zero-sized result installed while another dynamic array already spills.
#[test]
fn zero_sized_result_next_to_another_spill_is_calc_error_and_engine_survives() {
    for formula in EMPTY_RESULTS {
        let mut e = seeded(true);
        e.set_cell_formula("Sheet1", 20, 40, parse(formula).unwrap())
            .unwrap();
        e.evaluate_all().unwrap();
        let v = e.get_cell_value("Sheet1", 20, 40);
        assert!(is_calc(&v), "{formula} next to W1 SEQUENCE(3) → {v:?}");
        // The neighbouring spill is intact and the engine keeps evaluating.
        assert_eq!(e.get_cell_value("Sheet1", 3, 23), Some(n(3.0)), "{formula}");
        e.set_cell_formula("Sheet1", 30, 40, parse("=1+1").unwrap())
            .unwrap();
        e.evaluate_all().unwrap();
        assert_eq!(
            e.get_cell_value("Sheet1", 30, 40),
            Some(n(2.0)),
            "{formula}"
        );
    }
}

/// Alone on the sheet the same formulas evaluated to `0`; Excel shows `#CALC!` there too.
#[test]
fn zero_sized_result_alone_is_calc_error() {
    for formula in EMPTY_RESULTS {
        let mut e = seeded(false);
        e.set_cell_formula("Sheet1", 20, 40, parse(formula).unwrap())
            .unwrap();
        e.evaluate_all().unwrap();
        let v = e.get_cell_value("Sheet1", 20, 40);
        assert!(is_calc(&v), "{formula} alone → {v:?}");
    }
}

/// A zero-sized result REPLACING a committed spill at the same anchor vacates the old projection.
#[test]
fn zero_sized_result_replacing_a_spill_vacates_it() {
    let mut e = seeded(true);
    e.set_cell_formula("Sheet1", 20, 40, parse("=SEQUENCE(3)").unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    assert_eq!(e.get_cell_value("Sheet1", 22, 40), Some(n(3.0)));
    e.set_cell_formula("Sheet1", 20, 40, parse("=TAKE(A1:C5,0)").unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    assert!(is_calc(&e.get_cell_value("Sheet1", 20, 40)));
    let leftover = e.get_cell_value("Sheet1", 22, 40);
    assert!(
        matches!(leftover, None | Some(LiteralValue::Empty)),
        "old projection must be vacated, got {leftover:?}"
    );
}

/// `#CALC!` is an error VALUE downstream formulas see (Excel: `=ROWS(TAKE(A1:C5,0))` is `#CALC!`).
#[test]
fn zero_sized_result_propagates_as_calc_error() {
    let mut e = seeded(false);
    e.set_cell_formula("Sheet1", 20, 40, parse("=ROWS(TAKE(A1:C5,0))").unwrap())
        .unwrap();
    e.set_cell_formula(
        "Sheet1",
        21,
        40,
        parse("=IFERROR(TAKE(A1:C5,0),\"empty\")").unwrap(),
    )
    .unwrap();
    e.evaluate_all().unwrap();
    assert!(is_calc(&e.get_cell_value("Sheet1", 20, 40)));
    assert_eq!(
        e.get_cell_value("Sheet1", 21, 40),
        Some(LiteralValue::Text("empty".into()))
    );
}

/// Non-empty TAKE / DROP results still spill (guard against over-correcting).
#[test]
fn non_empty_take_still_spills() {
    let mut e = seeded(true);
    e.set_cell_formula("Sheet1", 20, 40, parse("=TAKE(A1:C5,2)").unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    assert_eq!(e.get_cell_value("Sheet1", 20, 40), Some(n(10.0)));
    assert_eq!(e.get_cell_value("Sheet1", 21, 42), Some(n(2.0)));
}
