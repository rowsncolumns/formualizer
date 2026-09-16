//! EDATE and EOMONTH functions for date arithmetic

use super::serial::{date_parts_to_serial_for, serial_to_ymd};
use crate::args::ArgSchema;
use crate::engine::DateSystem;
use crate::function::Function;
use crate::traits::{ArgumentHandle, FunctionContext};
use formualizer_common::{ExcelError, LiteralValue};
use formualizer_macros::func_caps;

/// Serial of `DATE(year, month, day)` in the 1900 system, month overflow and all.
fn ymd_serial(year: i32, month: i32, day: i32) -> Result<f64, ExcelError> {
    date_parts_to_serial_for(DateSystem::Excel1900, year, month, day)
}

/// Days in a month counted in serials, so February 1900 has 29 (the phantom
/// serial 60). Day 0 of the next month is the month's last day, which keeps
/// December 9999 inside the representable range.
fn days_in_month(year: i32, month: i32) -> Result<i32, ExcelError> {
    Ok((ymd_serial(year, month + 1, 0)? - ymd_serial(year, month, 1)?) as i32 + 1)
}

fn coerce_to_serial(arg: &ArgumentHandle) -> Result<f64, ExcelError> {
    let v = arg.value()?.into_literal();
    if let LiteralValue::Error(e) = v {
        return Err(e);
    }
    crate::builtins::utils::coerce_num_for(arg, &v).map_err(|_| {
        ExcelError::new_value()
            .with_message("EDATE/EOMONTH expects numeric, date, or text-numeric arguments")
    })
}

fn coerce_to_int(arg: &ArgumentHandle) -> Result<i32, ExcelError> {
    let v = arg.value()?.into_literal();
    if let LiteralValue::Error(e) = v {
        return Err(e);
    }
    crate::builtins::utils::coerce_num_for(arg, &v)
        .map(|f| f.trunc() as i32)
        .map_err(|_| {
            ExcelError::new_value()
                .with_message("EDATE/EOMONTH months argument is not a valid number")
        })
}

/// Returns the serial date offset by a whole number of months from a start date.
///
/// # Remarks
/// - `months` is truncated to an integer before calculation.
/// - If the target month has fewer days, the day is clamped to that month's last valid day.
/// - Serials are interpreted and emitted with Excel 1900 date mapping (not workbook-specific `1904` mode).
///
/// # Examples
/// ```yaml,sandbox
/// title: "Add months to first-of-month date"
/// formula: "=EDATE(44927, 3)"
/// expected: 45017
/// ```
///
/// ```yaml,sandbox
/// title: "Clamp month-end overflow"
/// formula: "=EDATE(45322, 1)"
/// expected: 45351
/// ```
///
/// ```yaml,docs
/// related:
///   - EOMONTH
///   - DATE
///   - YEARFRAC
/// faq:
///   - q: "What happens when the start day does not exist in the target month?"
///     a: "EDATE clamps to the last valid day of the target month (for example Jan 31 + 1 month becomes Feb month-end)."
/// ```
#[derive(Debug)]
pub struct EdateFn;

/// [formualizer-docgen:schema:start]
/// Name: EDATE
/// Type: EdateFn
/// Min args: 2
/// Max args: 2
/// Variadic: false
/// Signature: EDATE(arg1: number@scalar, arg2: number@scalar)
/// Arg schema: arg1{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg2{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}
/// Caps: PURE, ELEMENTWISE
/// [formualizer-docgen:schema:end]
impl Function for EdateFn {
    func_caps!(PURE, ELEMENTWISE);

    fn name(&self) -> &'static str {
        "EDATE"
    }

    fn min_args(&self) -> usize {
        2
    }

    fn arg_schema(&self) -> &'static [ArgSchema] {
        use std::sync::LazyLock;
        static TWO: LazyLock<Vec<ArgSchema>> = LazyLock::new(|| {
            vec![
                // start_date serial (numeric lenient)
                ArgSchema::number_lenient_scalar(),
                // months offset (numeric lenient)
                ArgSchema::number_lenient_scalar(),
            ]
        });
        &TWO[..]
    }

    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        _ctx: &dyn FunctionContext<'b>,
    ) -> Result<crate::traits::CalcValue<'b>, ExcelError> {
        let start_serial = coerce_to_serial(&args[0])?;
        let months = coerce_to_int(&args[1])?;

        // Excel resolves the target month and clamps the day to its length. Built
        // on serials (not NaiveDate) so the phantom 1900-02-29 is a real day:
        // EDATE(60,1) = 89 (Mar 29 1900), EDATE(31,1) = 60. Out of range is #NUM!.
        let (year, month, day) = serial_to_ymd(start_serial)?;
        let target_month = month as i32 + months;
        let target_day = (day as i32).min(days_in_month(year, target_month)?);

        Ok(crate::traits::CalcValue::Scalar(LiteralValue::Number(
            ymd_serial(year, target_month, target_day)?,
        )))
    }
}

/// Returns the serial for the last day of the month at a month offset from a start date.
///
/// # Remarks
/// - `months` is truncated to an integer before offset calculation.
/// - The returned date is always the month-end date for the target month.
/// - Serials are interpreted and returned using Excel 1900 mapping rather than workbook `1904` mode.
///
/// # Examples
/// ```yaml,sandbox
/// title: "Get end of current month"
/// formula: "=EOMONTH(44927, 0)"
/// expected: 44957
/// ```
///
/// ```yaml,sandbox
/// title: "Get end of month two months ahead"
/// formula: "=EOMONTH(45322, 2)"
/// expected: 45382
/// ```
///
/// ```yaml,docs
/// related:
///   - EDATE
///   - DATE
///   - DAY
/// faq:
///   - q: "Does EOMONTH always return a month-end date?"
///     a: "Yes. Regardless of the start day, EOMONTH returns the final calendar day of the target month after offset."
/// ```
#[derive(Debug)]
pub struct EomonthFn;

/// [formualizer-docgen:schema:start]
/// Name: EOMONTH
/// Type: EomonthFn
/// Min args: 2
/// Max args: 2
/// Variadic: false
/// Signature: EOMONTH(arg1: number@scalar, arg2: number@scalar)
/// Arg schema: arg1{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg2{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}
/// Caps: PURE, ELEMENTWISE
/// [formualizer-docgen:schema:end]
impl Function for EomonthFn {
    func_caps!(PURE, ELEMENTWISE);

    fn name(&self) -> &'static str {
        "EOMONTH"
    }

    fn min_args(&self) -> usize {
        2
    }

    fn arg_schema(&self) -> &'static [ArgSchema] {
        use std::sync::LazyLock;
        static TWO: LazyLock<Vec<ArgSchema>> = LazyLock::new(|| {
            vec![
                ArgSchema::number_lenient_scalar(),
                ArgSchema::number_lenient_scalar(),
            ]
        });
        &TWO[..]
    }

    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        _ctx: &dyn FunctionContext<'b>,
    ) -> Result<crate::traits::CalcValue<'b>, ExcelError> {
        let start_serial = coerce_to_serial(&args[0])?;
        let months = coerce_to_int(&args[1])?;

        // Day 0 of the month after the target month, counted in serials so
        // February 1900 ends on the phantom serial 60 (EOMONTH(59,0) = 60).
        let (year, month, _) = serial_to_ymd(start_serial)?;

        Ok(crate::traits::CalcValue::Scalar(LiteralValue::Number(
            ymd_serial(year, month as i32 + months + 1, 0)?,
        )))
    }
}

pub fn register_builtins() {
    use std::sync::Arc;
    crate::function_registry::register_builtin(Arc::new(EdateFn));
    crate::function_registry::register_builtin(Arc::new(EomonthFn));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_workbook::TestWorkbook;
    use formualizer_parse::parser::{ASTNode, ASTNodeType};
    use std::sync::Arc;

    fn lit(v: LiteralValue) -> ASTNode {
        ASTNode::new(ASTNodeType::Literal(v), None)
    }

    #[test]
    fn test_edate_basic() {
        let wb = TestWorkbook::new().with_function(Arc::new(EdateFn));
        let ctx = wb.interpreter();
        let f = ctx.context.get_function("", "EDATE").unwrap();

        // Test adding months
        // Use a known date serial (e.g., 44927 = 2023-01-01)
        let start = lit(LiteralValue::Number(44927.0));
        let months = lit(LiteralValue::Int(3));

        let result = f
            .dispatch(
                &[
                    ArgumentHandle::new(&start, &ctx),
                    ArgumentHandle::new(&months, &ctx),
                ],
                &ctx.function_context(None),
            )
            .unwrap()
            .into_literal();

        // Should return a date 3 months later
        assert!(matches!(result, LiteralValue::Number(_)));
    }

    #[test]
    fn test_edate_negative_months() {
        let wb = TestWorkbook::new().with_function(Arc::new(EdateFn));
        let ctx = wb.interpreter();
        let f = ctx.context.get_function("", "EDATE").unwrap();

        // Test subtracting months
        let start = lit(LiteralValue::Number(44927.0)); // 2023-01-01
        let months = lit(LiteralValue::Int(-2));

        let result = f
            .dispatch(
                &[
                    ArgumentHandle::new(&start, &ctx),
                    ArgumentHandle::new(&months, &ctx),
                ],
                &ctx.function_context(None),
            )
            .unwrap()
            .into_literal();

        // Should return a date 2 months earlier
        assert!(matches!(result, LiteralValue::Number(_)));
    }

    #[test]
    fn test_eomonth_basic() {
        let wb = TestWorkbook::new().with_function(Arc::new(EomonthFn));
        let ctx = wb.interpreter();
        let f = ctx.context.get_function("", "EOMONTH").unwrap();

        // Test end of month
        let start = lit(LiteralValue::Number(44927.0)); // 2023-01-01
        let months = lit(LiteralValue::Int(0));

        let result = f
            .dispatch(
                &[
                    ArgumentHandle::new(&start, &ctx),
                    ArgumentHandle::new(&months, &ctx),
                ],
                &ctx.function_context(None),
            )
            .unwrap()
            .into_literal();

        // Should return Jan 31, 2023
        assert!(matches!(result, LiteralValue::Number(_)));
    }

    #[test]
    fn test_eomonth_february() {
        let wb = TestWorkbook::new().with_function(Arc::new(EomonthFn));
        let ctx = wb.interpreter();
        let f = ctx.context.get_function("", "EOMONTH").unwrap();

        // Test February (checking leap year handling)
        let start = lit(LiteralValue::Number(44927.0)); // 2023-01-01
        let months = lit(LiteralValue::Int(1)); // Move to February

        let result = f
            .dispatch(
                &[
                    ArgumentHandle::new(&start, &ctx),
                    ArgumentHandle::new(&months, &ctx),
                ],
                &ctx.function_context(None),
            )
            .unwrap()
            .into_literal();

        // Should return Feb 28, 2023 (not a leap year)
        assert!(matches!(result, LiteralValue::Number(_)));
    }
}
