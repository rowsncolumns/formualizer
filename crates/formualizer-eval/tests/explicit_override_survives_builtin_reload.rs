//! An explicit `register_function` override of a stock builtin name must keep
//! winning for the whole process lifetime, even though `load_builtins()` re-runs
//! during `Engine::new`, formula planning, and template canonicalization.
//!
//! Runs as an integration test (own process) because it shadows a real builtin
//! name in the shared global registry.

use std::sync::Arc;

use formualizer_common::{ExcelError, LiteralValue};
use formualizer_eval::args::ArgSchema;
use formualizer_eval::engine::{Engine, EvalConfig};
use formualizer_eval::function::Function;
use formualizer_eval::function_registry;
use formualizer_eval::test_workbook::TestWorkbook;
use formualizer_eval::traits::{ArgumentHandle, CalcValue, FunctionContext};
use formualizer_parse::parser::parse;

/// Host-style DATEDIF replacement that accepts text dates the stock builtin
/// rejects, mirroring how embedding hosts register Excel-parity overrides.
struct HostDatedifFn;

impl Function for HostDatedifFn {
    fn name(&self) -> &'static str {
        "DATEDIF"
    }

    fn min_args(&self) -> usize {
        3
    }

    fn arg_schema(&self) -> &'static [ArgSchema] {
        use std::sync::LazyLock;
        static SCHEMA: LazyLock<Vec<ArgSchema>> =
            LazyLock::new(|| vec![ArgSchema::any(), ArgSchema::any(), ArgSchema::any()]);
        &SCHEMA[..]
    }

    fn eval<'a, 'b, 'c>(
        &self,
        _args: &'c [ArgumentHandle<'a, 'b>],
        _ctx: &dyn FunctionContext<'b>,
    ) -> Result<CalcValue<'b>, ExcelError> {
        Ok(CalcValue::Scalar(LiteralValue::Number(4242.0)))
    }
}

#[test]
fn explicit_override_survives_engine_planning_builtin_reloads() {
    formualizer_eval::builtins::load_builtins();
    function_registry::register_function(Arc::new(HostDatedifFn));

    // Engine construction and formula ingest/planning both re-run
    // `load_builtins()`; the override must survive all of them.
    let mut engine = Engine::new(TestWorkbook::new(), EvalConfig::default());
    engine
        .set_cell_formula(
            "Sheet1",
            1,
            1,
            parse("=DATEDIF(\"1/1/2001\",\"1/1/2003\",\"Y\")").unwrap(),
        )
        .unwrap();
    engine.evaluate_all().unwrap();

    assert_eq!(
        engine.get_cell_value("Sheet1", 1, 1),
        Some(LiteralValue::Number(4242.0)),
        "explicit DATEDIF override was clobbered by a builtin reload",
    );

    let resolved = function_registry::resolve("", "DATEDIF").unwrap();
    assert!(
        !resolved.semantics.trusted_builtin,
        "registry resolution reverted to the stock builtin",
    );
}
