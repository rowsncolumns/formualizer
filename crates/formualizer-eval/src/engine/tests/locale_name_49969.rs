//! The workbook locale the host hands the engine reaches host-registered functions through
//! `FunctionContext::locale_name` (demo-app-monorepo#49969 FX-06): Excel's `TEXT`, `VALUE`,
//! `DATEVALUE`, `NUMBERVALUE`, `DOLLAR` and `FIXED` read the regional settings.

use std::sync::Arc;

use crate::engine::{Engine, EvalConfig};
use crate::function::Function;
use crate::test_workbook::TestWorkbook;
use crate::traits::{ArgumentHandle, CalcValue, FunctionContext};
use formualizer_common::{ExcelError, LiteralValue};
use formualizer_parse::parser::parse;

#[derive(Debug)]
struct LocaleNameFn;
impl Function for LocaleNameFn {
    fn name(&self) -> &'static str {
        "LOCALENAME"
    }
    fn eval<'a, 'b, 'c>(
        &self,
        _args: &'c [ArgumentHandle<'a, 'b>],
        ctx: &dyn FunctionContext<'b>,
    ) -> Result<CalcValue<'b>, ExcelError> {
        Ok(CalcValue::Scalar(LiteralValue::Text(
            ctx.locale_name()
                .map(|l| l.to_string())
                .unwrap_or_else(|| "invariant".to_string()),
        )))
    }
}

fn locale_of(engine: &mut Engine<TestWorkbook>) -> Option<LiteralValue> {
    engine
        .set_cell_formula("Sheet1", 1, 1, parse("=LOCALENAME()").expect("parse"))
        .expect("set formula");
    engine.evaluate_all().unwrap();
    engine.get_cell_value("Sheet1", 1, 1)
}

#[test]
fn functions_read_the_workbook_locale_the_host_sets() {
    let wb = TestWorkbook::new().with_function(Arc::new(LocaleNameFn));
    let mut engine = Engine::new(wb, EvalConfig::default());
    assert_eq!(
        locale_of(&mut engine),
        Some(LiteralValue::Text("invariant".into()))
    );
    engine.set_locale_name(Some(Arc::from("de-DE")));
    assert_eq!(
        locale_of(&mut engine),
        Some(LiteralValue::Text("de-DE".into()))
    );
}
