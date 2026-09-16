//! rowsncolumns/spreadsheet#546 (W5-A) — a host that overrides an aliased builtin by re-registering
//! its canonical name (rnc-engine does this for `COVARIANCE.P`, `POISSON.DIST`, `F.DIST.RT`,
//! `F.INV.RT`, `GAMMA.INV`, `T.INV.2T`) must not lose the builtin's legacy spellings: `COVAR`,
//! `POISSON`, `FDIST`, `FINV`, `GAMMAINV`, `TINV` were `#NAME?` at runtime because the registry
//! dropped every alias of a replaced registration.
//!
//! Integration tests run in their own process, so overriding the default namespace here cannot
//! leak into the crate's other tests.

use std::sync::Arc;

use formualizer_common::{ExcelError, LiteralValue};
use formualizer_eval::engine::{Engine, EvalConfig};
use formualizer_eval::function::Function;
use formualizer_eval::function_registry;
use formualizer_eval::test_workbook::TestWorkbook;
use formualizer_eval::traits::{ArgumentHandle, CalcValue, FunctionContext};
use formualizer_parse::parser::parse;

/// A host shim: same canonical name as the builtin, no aliases, recognisable constant result.
struct Shim {
    name: &'static str,
    marker: f64,
}

impl Function for Shim {
    fn name(&self) -> &'static str {
        self.name
    }
    fn min_args(&self) -> usize {
        0
    }
    fn variadic(&self) -> bool {
        true
    }
    fn eval<'a, 'b, 'c>(
        &self,
        _args: &'c [ArgumentHandle<'a, 'b>],
        _ctx: &dyn FunctionContext<'b>,
    ) -> Result<CalcValue<'b>, ExcelError> {
        Ok(CalcValue::Scalar(LiteralValue::Number(self.marker)))
    }
}

fn eval(formula: &str) -> LiteralValue {
    let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
    e.set_cell_formula("Sheet1", 1, 1, parse(formula).unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    e.get_cell_value("Sheet1", 1, 1)
        .unwrap_or(LiteralValue::Empty)
}

const OVERRIDES: &[(&str, &str, &str)] = &[
    // (builtin canonical name, legacy alias, alias call)
    ("COVARIANCE.P", "COVAR", "=COVAR({1,2,3},{4,5,7})"),
    ("POISSON.DIST", "POISSON", "=POISSON(2,5,TRUE)"),
    ("F.DIST.RT", "FDIST", "=FDIST(15.2068649,6,4)"),
    ("F.INV.RT", "FINV", "=FINV(0.01,6,4)"),
    ("GAMMA.INV", "GAMMAINV", "=GAMMAINV(0.068094,9,2)"),
    ("T.INV.2T", "TINV", "=TINV(0.05,10)"),
];

#[test]
fn legacy_stat_aliases_follow_a_host_override_of_their_builtin() {
    formualizer_eval::builtins::load_builtins();
    for (canonical, alias, _) in OVERRIDES {
        let resolved = function_registry::resolve("", alias)
            .unwrap_or_else(|| panic!("{alias} is not registered by the builtin library"));
        assert_eq!(resolved.canonical_name, *canonical, "{alias}");
        assert!(resolved.semantics.trusted_builtin, "{alias}");
    }

    for (index, (canonical, _, _)) in OVERRIDES.iter().enumerate() {
        function_registry::register_function(Arc::new(Shim {
            name: canonical,
            marker: 1000.0 + index as f64,
        }));
    }
    // `Engine::new` re-runs `load_builtins()`; the overrides and their inherited aliases survive it.
    for (index, (canonical, alias, call)) in OVERRIDES.iter().enumerate() {
        let marker = 1000.0 + index as f64;
        assert_eq!(
            eval(&format!("={canonical}(1,2,3)")),
            LiteralValue::Number(marker),
            "{canonical} should resolve to the host shim"
        );
        assert_eq!(
            eval(call),
            LiteralValue::Number(marker),
            "{alias} should resolve to the {canonical} host shim, not #NAME?"
        );
        let resolved = function_registry::resolve("", alias).unwrap();
        assert_eq!(resolved.canonical_name, *canonical, "{alias}");
        assert!(!resolved.semantics.trusted_builtin, "{alias}");
    }

    let aliases = function_registry::snapshot_aliases();
    for (canonical, alias, _) in OVERRIDES {
        assert!(
            aliases.iter().any(|a| a.namespace.is_empty()
                && a.alias == *alias
                && a.target_name == *canonical),
            "{alias} → {canonical} missing from snapshot_aliases()"
        );
    }
}
