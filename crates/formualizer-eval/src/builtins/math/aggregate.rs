use super::super::utils::{ARG_RANGE_NUM_LENIENT_ONE, coerce_num};
use crate::args::ArgSchema;
use crate::engine::VisibilityMaskMode;
use crate::function::Function;
use crate::function_contract::FunctionDependencyContract;
use crate::reference::{CellRef, Coord};
use crate::traits::{ArgumentHandle, FunctionContext};
use arrow_array::Array;
use formualizer_common::{ExcelError, LiteralValue};
use formualizer_macros::func_caps;

/* ─────────────────────────── SUM() ──────────────────────────── */

#[derive(Debug)]
pub struct SumFn;

/// Adds numeric values across scalars and ranges.
///
/// `SUM` evaluates all arguments, coercing text to numbers where possible,
/// and returns the total. Blank cells and logical values in ranges are ignored.
///
/// # Remarks
/// - If any argument evaluates to an error, `SUM` propagates the first error it encounters.
/// - Unparseable text literals (e.g., `"foo"`) will result in a `#VALUE!` error.
///
/// # Examples
///
/// ```yaml,sandbox
/// title: "Basic scalar addition"
/// formula: "=SUM(10, 20, 5)"
/// expected: 35
/// ```
///
/// ```yaml,sandbox
/// title: "Summing a range"
/// grid:
///   A1: 10
///   A2: 20
///   A3: "N/A"
/// formula: "=SUM(A1:A3)"
/// expected: 30
/// ```
///
/// ```yaml,docs
/// related:
///   - SUMIF
///   - SUMIFS
///   - SUMPRODUCT
///   - AVERAGE
/// faq:
///   - q: "Why does SUM return #VALUE! for some text arguments?"
///     a: "Direct scalar text that cannot be parsed as a number raises #VALUE! during coercion."
///   - q: "Do text and logical values inside ranges get added?"
///     a: "No. In ranged inputs, only numeric cells contribute to the total."
/// ```
///
/// [formualizer-docgen:schema:start]
/// Name: SUM
/// Type: SumFn
/// Min args: 0
/// Max args: variadic
/// Variadic: true
/// Signature: SUM(arg1...: number@range)
/// Arg schema: arg1{kinds=number,required=true,shape=range,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}
/// Caps: PURE, REDUCTION, NUMERIC_ONLY, STREAM_OK, PARALLEL_ARGS
/// [formualizer-docgen:schema:end]
impl Function for SumFn {
    func_caps!(PURE, REDUCTION, NUMERIC_ONLY, STREAM_OK, PARALLEL_ARGS);

    fn name(&self) -> &'static str {
        "SUM"
    }
    fn min_args(&self) -> usize {
        0
    }
    fn variadic(&self) -> bool {
        true
    }
    fn dependency_contract(&self, arity: usize) -> Option<FunctionDependencyContract> {
        FunctionDependencyContract::static_reduction(arity, self.min_args())
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        &ARG_RANGE_NUM_LENIENT_ONE[..]
    }

    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        ctx: &dyn FunctionContext<'b>,
    ) -> Result<crate::traits::CalcValue<'b>, ExcelError> {
        let mut total = 0.0;
        for arg in args {
            if let Ok(view) = arg.range_view() {
                // Propagate errors from range first
                for res in view.errors_slices() {
                    let (_, _, err_cols) = res?;
                    for col in err_cols {
                        if col.null_count() < col.len() {
                            for i in 0..col.len() {
                                if !col.is_null(i) {
                                    return Ok(crate::traits::CalcValue::Scalar(
                                        LiteralValue::Error(ExcelError::new(
                                            crate::arrow_store::unmap_error_code(col.value(i)),
                                        )),
                                    ));
                                }
                            }
                        }
                    }
                }

                for res in view.numbers_slices() {
                    let (_, _, num_cols) = res?;
                    for col in num_cols {
                        total +=
                            arrow::compute::kernels::aggregate::sum(col.as_ref()).unwrap_or(0.0);
                    }
                }
            } else {
                let v = arg.value()?.into_literal();
                match v {
                    LiteralValue::Error(e) => {
                        return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(e)));
                    }
                    v => total += coerce_num(&v)?,
                }
            }
        }
        Ok(crate::traits::CalcValue::Scalar(
            super::super::utils::aggregate_result(total),
        ))
    }
}

/* ─────────────────────────── COUNT() ──────────────────────────── */

#[derive(Debug)]
pub struct CountFn;

/// Counts numeric values across scalars and ranges.
///
/// `COUNT` evaluates all arguments and counts how many are numeric values.
/// Numbers, dates, and text representations of numbers (when supplied directly) are counted.
///
/// # Remarks
/// - Text values inside ranges are ignored and not counted.
/// - Blank cells and logical values in ranges are ignored.
///
/// # Examples
///
/// ```yaml,sandbox
/// title: "Counting mixed scalar inputs"
/// formula: "=COUNT(1, \"x\", 2, 3)"
/// expected: 3
/// ```
///
/// ```yaml,sandbox
/// title: "Counting in a range"
/// grid:
///   A1: 10
///   A2: "foo"
///   A3: 20
/// formula: "=COUNT(A1:A3)"
/// expected: 2
/// ```
///
/// ```yaml,docs
/// related:
///   - COUNTA
///   - COUNTBLANK
///   - COUNTIF
///   - COUNTIFS
/// faq:
///   - q: "Why doesn't COUNT include text in a range?"
///     a: "COUNT only counts numeric values; text cells in ranges are ignored."
///   - q: "Can direct text like \"12\" be counted?"
///     a: "Yes. Direct scalar arguments are coerced and counted when they parse as numbers."
/// ```
///
/// [formualizer-docgen:schema:start]
/// Name: COUNT
/// Type: CountFn
/// Min args: 0
/// Max args: variadic
/// Variadic: true
/// Signature: COUNT(arg1...: number@range)
/// Arg schema: arg1{kinds=number,required=true,shape=range,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}
/// Caps: PURE, REDUCTION, NUMERIC_ONLY, STREAM_OK
/// [formualizer-docgen:schema:end]
impl Function for CountFn {
    func_caps!(PURE, REDUCTION, NUMERIC_ONLY, STREAM_OK);

    fn name(&self) -> &'static str {
        "COUNT"
    }
    fn min_args(&self) -> usize {
        0
    }
    fn variadic(&self) -> bool {
        true
    }
    fn dependency_contract(&self, arity: usize) -> Option<FunctionDependencyContract> {
        FunctionDependencyContract::static_reduction(arity, self.min_args())
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        &ARG_RANGE_NUM_LENIENT_ONE[..]
    }

    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        _: &dyn FunctionContext<'b>,
    ) -> Result<crate::traits::CalcValue<'b>, ExcelError> {
        let mut count: i64 = 0;
        for arg in args {
            if let Ok(view) = arg.range_view() {
                for res in view.numbers_slices() {
                    let (_, _, num_cols) = res?;
                    for col in num_cols {
                        count += (col.len() - col.null_count()) as i64;
                    }
                }
            } else {
                // COUNT never propagates: an error argument is simply not a number.
                let v = arg.value()?.into_literal();
                if !matches!(v, LiteralValue::Empty | LiteralValue::Error(_))
                    && coerce_num(&v).is_ok()
                {
                    count += 1;
                }
            }
        }
        Ok(crate::traits::CalcValue::Scalar(LiteralValue::Number(
            count as f64,
        )))
    }
}

/* ─────────────────────────── AVERAGE() ──────────────────────────── */

#[derive(Debug)]
pub struct AverageFn;

/// Returns the arithmetic mean of numeric values across scalars and ranges.
///
/// `AVERAGE` sums numeric inputs and divides by the count of numeric values that participated.
///
/// # Remarks
/// - Errors in any scalar argument or referenced range propagate immediately.
/// - In ranges, only numeric/date-time serial values are included; text and blanks are ignored.
/// - Scalar arguments use lenient number coercion with locale support.
/// - If no numeric values are found, `AVERAGE` returns `#DIV/0!`.
///
/// # Examples
///
/// ```yaml,sandbox
/// title: "Average of scalar values"
/// formula: "=AVERAGE(10, 20, 5)"
/// expected: 11.666666666666666
/// ```
///
/// ```yaml,sandbox
/// title: "Average over a mixed range"
/// grid:
///   A1: 10
///   A2: "x"
///   A3: 20
/// formula: "=AVERAGE(A1:A3)"
/// expected: 15
/// ```
///
/// ```yaml,sandbox
/// title: "No numeric values returns divide-by-zero"
/// formula: "=AVERAGE(\"x\", \"\")"
/// expected: "#DIV/0!"
/// ```
///
/// ```yaml,docs
/// related:
///   - SUM
///   - COUNT
///   - AVERAGEIF
///   - AVERAGEIFS
/// faq:
///   - q: "When does AVERAGE return #DIV/0!?"
///     a: "It returns #DIV/0! when no numeric values are found after filtering/coercion."
///   - q: "Do text cells in ranges affect the denominator?"
///     a: "No. Only numeric values are counted toward the divisor."
/// ```
///
/// [formualizer-docgen:schema:start]
/// Name: AVERAGE
/// Type: AverageFn
/// Min args: 1
/// Max args: variadic
/// Variadic: true
/// Signature: AVERAGE(arg1...: number@range)
/// Arg schema: arg1{kinds=number,required=true,shape=range,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}
/// Caps: PURE, REDUCTION, NUMERIC_ONLY, STREAM_OK
/// [formualizer-docgen:schema:end]
impl Function for AverageFn {
    func_caps!(PURE, REDUCTION, NUMERIC_ONLY, STREAM_OK);

    fn name(&self) -> &'static str {
        "AVERAGE"
    }
    fn min_args(&self) -> usize {
        1
    }
    fn variadic(&self) -> bool {
        true
    }
    fn dependency_contract(&self, arity: usize) -> Option<FunctionDependencyContract> {
        FunctionDependencyContract::static_reduction(arity, self.min_args())
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        &ARG_RANGE_NUM_LENIENT_ONE[..]
    }

    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        ctx: &dyn FunctionContext<'b>,
    ) -> Result<crate::traits::CalcValue<'b>, ExcelError> {
        let mut sum = 0.0f64;
        let mut cnt: i64 = 0;
        for arg in args {
            if let Ok(view) = arg.range_view() {
                // Propagate errors from range first
                for res in view.errors_slices() {
                    let (_, _, err_cols) = res?;
                    for col in err_cols {
                        if col.null_count() < col.len() {
                            for i in 0..col.len() {
                                if !col.is_null(i) {
                                    return Ok(crate::traits::CalcValue::Scalar(
                                        LiteralValue::Error(ExcelError::new(
                                            crate::arrow_store::unmap_error_code(col.value(i)),
                                        )),
                                    ));
                                }
                            }
                        }
                    }
                }

                for res in view.numbers_slices() {
                    let (_, _, num_cols) = res?;
                    for col in num_cols {
                        sum += arrow::compute::kernels::aggregate::sum(col.as_ref()).unwrap_or(0.0);
                        cnt += (col.len() - col.null_count()) as i64;
                    }
                }
            } else {
                let v = arg.value()?.into_literal();
                if let LiteralValue::Error(e) = v {
                    return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(e)));
                }
                if let Ok(n) =
                    crate::coercion::to_number_lenient_with_clock(&v, &ctx.locale(), ctx.clock())
                {
                    sum += n;
                    cnt += 1;
                }
            }
        }
        if cnt == 0 {
            return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                ExcelError::new_div(),
            )));
        }
        Ok(crate::traits::CalcValue::Scalar(
            super::super::utils::aggregate_result(sum / (cnt as f64)),
        ))
    }
}

/* ──────────────────────── SUMPRODUCT() ───────────────────────── */

#[derive(Debug)]
pub struct SumProductFn;

/// Multiplies aligned values across arrays and returns the sum of those products.
///
/// `SUMPRODUCT` supports scalar or range inputs, applies broadcast semantics, and accumulates
/// the product for each aligned cell position.
///
/// # Remarks
/// - Input shapes must be broadcast-compatible; otherwise `SUMPRODUCT` returns `#VALUE!`.
/// - Non-numeric values are treated as `0` during multiplication.
/// - Any explicit error value in the inputs propagates immediately.
///
/// # Examples
///
/// ```yaml,sandbox
/// title: "Pairwise sum of products"
/// formula: "=SUMPRODUCT({1,2,3}, {4,5,6})"
/// expected: 32
/// ```
///
/// ```yaml,sandbox
/// title: "Range-based sumproduct"
/// grid:
///   A1: 2
///   A2: 3
///   A3: 4
///   B1: 10
///   B2: 20
///   B3: 30
/// formula: "=SUMPRODUCT(A1:A3, B1:B3)"
/// expected: 200
/// ```
///
/// ```yaml,sandbox
/// title: "Text entries contribute zero"
/// formula: "=SUMPRODUCT({1,\"x\",3}, {1,1,1})"
/// expected: 4
/// ```
///
/// ```yaml,docs
/// related:
///   - SUM
///   - PRODUCT
///   - MMULT
///   - SUMIFS
/// faq:
///   - q: "Why does SUMPRODUCT return #VALUE! with some array shapes?"
///     a: "The argument arrays must be broadcast-compatible; incompatible shapes raise #VALUE!."
///   - q: "How are text values handled in multiplication?"
///     a: "Non-numeric values are treated as 0, unless an explicit error is present."
/// ```
///
/// [formualizer-docgen:schema:start]
/// Name: SUMPRODUCT
/// Type: SumProductFn
/// Min args: 1
/// Max args: variadic
/// Variadic: true
/// Signature: SUMPRODUCT(arg1...: number@range)
/// Arg schema: arg1{kinds=number,required=true,shape=range,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}
/// Caps: PURE, REDUCTION
/// [formualizer-docgen:schema:end]
impl Function for SumProductFn {
    // Pure reduction over arrays; uses broadcasting and lenient coercion
    func_caps!(PURE, REDUCTION);

    fn name(&self) -> &'static str {
        "SUMPRODUCT"
    }
    fn min_args(&self) -> usize {
        1
    }
    fn variadic(&self) -> bool {
        true
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        // Accept ranges or scalars; numeric lenient coercion
        &ARG_RANGE_NUM_LENIENT_ONE[..]
    }

    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        _: &dyn FunctionContext<'b>,
    ) -> Result<crate::traits::CalcValue<'b>, ExcelError> {
        use crate::broadcast::{broadcast_shape, project_index};

        if args.is_empty() {
            return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Number(0.0)));
        }

        // Helper: materialize an argument to a 2D array of LiteralValue
        let to_array = |ah: &ArgumentHandle| -> Result<Vec<Vec<LiteralValue>>, ExcelError> {
            if let Ok(rv) = ah.range_view() {
                let mut rows: Vec<Vec<LiteralValue>> = Vec::new();
                rv.for_each_row(&mut |row| {
                    rows.push(row.to_vec());
                    Ok(())
                })?;
                Ok(rows)
            } else {
                let v = ah.value()?.into_literal();
                Ok(match v {
                    LiteralValue::Array(arr) => arr,
                    other => vec![vec![other]],
                })
            }
        };

        // Collect arrays and shapes
        let mut arrays: Vec<Vec<Vec<LiteralValue>>> = Vec::with_capacity(args.len());
        let mut shapes: Vec<(usize, usize)> = Vec::with_capacity(args.len());
        for a in args.iter() {
            let arr = to_array(a)?;
            let shape = (arr.len(), arr.first().map(|r| r.len()).unwrap_or(0));
            arrays.push(arr);
            shapes.push(shape);
        }

        // Compute broadcast target shape across all args
        let target = match broadcast_shape(&shapes) {
            Ok(s) => s,
            Err(_) => {
                return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                    ExcelError::new_value(),
                )));
            }
        };

        // Iterate target shape, multiply coerced values across args, sum total
        let mut total = 0.0f64;
        for r in 0..target.0 {
            for c in 0..target.1 {
                let mut prod = 1.0f64;
                for (arr, &shape) in arrays.iter().zip(shapes.iter()) {
                    let (rr, cc) = project_index((r, c), shape);
                    let lv = arr
                        .get(rr)
                        .and_then(|row| row.get(cc))
                        .cloned()
                        .unwrap_or(LiteralValue::Empty);
                    match lv {
                        LiteralValue::Error(e) => {
                            return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(e)));
                        }
                        _ => match super::super::utils::coerce_num(&lv) {
                            Ok(n) => {
                                prod *= n;
                            }
                            Err(_) => {
                                // Non-numeric -> treated as 0 in SUMPRODUCT
                                prod *= 0.0;
                            }
                        },
                    }
                }
                total += prod;
            }
        }
        Ok(crate::traits::CalcValue::Scalar(
            super::super::utils::aggregate_result(total),
        ))
    }
}

#[cfg(test)]
mod tests_sumproduct {
    use super::*;
    use crate::test_workbook::TestWorkbook;
    use crate::traits::ArgumentHandle;
    use formualizer_parse::LiteralValue;
    use formualizer_parse::parser::{ASTNode, ASTNodeType};

    fn interp(wb: &TestWorkbook) -> crate::interpreter::Interpreter<'_> {
        wb.interpreter()
    }

    fn arr(vals: Vec<Vec<LiteralValue>>) -> ASTNode {
        ASTNode::new(ASTNodeType::Literal(LiteralValue::Array(vals)), None)
    }

    fn num(n: f64) -> ASTNode {
        ASTNode::new(ASTNodeType::Literal(LiteralValue::Number(n)), None)
    }

    #[test]
    fn sumproduct_basic_pairwise() {
        let wb = TestWorkbook::new().with_function(std::sync::Arc::new(SumProductFn));
        let ctx = interp(&wb);
        // {1,2,3} * {4,5,6} = 1*4 + 2*5 + 3*6 = 32
        let a = arr(vec![vec![
            LiteralValue::Int(1),
            LiteralValue::Int(2),
            LiteralValue::Int(3),
        ]]);
        let b = arr(vec![vec![
            LiteralValue::Int(4),
            LiteralValue::Int(5),
            LiteralValue::Int(6),
        ]]);
        let args = vec![ArgumentHandle::new(&a, &ctx), ArgumentHandle::new(&b, &ctx)];
        let f = ctx.context.get_function("", "SUMPRODUCT").unwrap();
        assert_eq!(
            f.dispatch(&args, &ctx.function_context(None))
                .unwrap()
                .into_literal(),
            LiteralValue::Number(32.0)
        );
    }

    #[test]
    fn sumproduct_variadic_three_arrays() {
        let wb = TestWorkbook::new().with_function(std::sync::Arc::new(SumProductFn));
        let ctx = interp(&wb);
        // {1,2} * {3,4} * {2,2} = (1*3*2) + (2*4*2) = 6 + 16 = 22
        let a = arr(vec![vec![LiteralValue::Int(1), LiteralValue::Int(2)]]);
        let b = arr(vec![vec![LiteralValue::Int(3), LiteralValue::Int(4)]]);
        let c = arr(vec![vec![LiteralValue::Int(2), LiteralValue::Int(2)]]);
        let args = vec![
            ArgumentHandle::new(&a, &ctx),
            ArgumentHandle::new(&b, &ctx),
            ArgumentHandle::new(&c, &ctx),
        ];
        let f = ctx.context.get_function("", "SUMPRODUCT").unwrap();
        assert_eq!(
            f.dispatch(&args, &ctx.function_context(None))
                .unwrap()
                .into_literal(),
            LiteralValue::Number(22.0)
        );
    }

    #[test]
    fn sumproduct_broadcast_scalar_over_array() {
        let wb = TestWorkbook::new().with_function(std::sync::Arc::new(SumProductFn));
        let ctx = interp(&wb);
        // {1,2,3} * 10 => (1*10 + 2*10 + 3*10) = 60
        let a = arr(vec![vec![
            LiteralValue::Int(1),
            LiteralValue::Int(2),
            LiteralValue::Int(3),
        ]]);
        let s = num(10.0);
        let args = vec![ArgumentHandle::new(&a, &ctx), ArgumentHandle::new(&s, &ctx)];
        let f = ctx.context.get_function("", "SUMPRODUCT").unwrap();
        assert_eq!(
            f.dispatch(&args, &ctx.function_context(None))
                .unwrap()
                .into_literal(),
            LiteralValue::Number(60.0)
        );
    }

    #[test]
    fn sumproduct_2d_arrays_broadcast_rows_cols() {
        let wb = TestWorkbook::new().with_function(std::sync::Arc::new(SumProductFn));
        let ctx = interp(&wb);
        // A is 2x2, B is 1x2 -> broadcast B across rows
        // A = [[1,2],[3,4]], B = [[10,20]]
        // sum = 1*10 + 2*20 + 3*10 + 4*20 = 10 + 40 + 30 + 80 = 160
        let a = arr(vec![
            vec![LiteralValue::Int(1), LiteralValue::Int(2)],
            vec![LiteralValue::Int(3), LiteralValue::Int(4)],
        ]);
        let b = arr(vec![vec![LiteralValue::Int(10), LiteralValue::Int(20)]]);
        let args = vec![ArgumentHandle::new(&a, &ctx), ArgumentHandle::new(&b, &ctx)];
        let f = ctx.context.get_function("", "SUMPRODUCT").unwrap();
        assert_eq!(
            f.dispatch(&args, &ctx.function_context(None))
                .unwrap()
                .into_literal(),
            LiteralValue::Number(160.0)
        );
    }

    #[test]
    fn sumproduct_non_numeric_treated_as_zero() {
        let wb = TestWorkbook::new().with_function(std::sync::Arc::new(SumProductFn));
        let ctx = interp(&wb);
        // {1,"x",3} * {1,1,1} => 1*1 + 0*1 + 3*1 = 4
        let a = arr(vec![vec![
            LiteralValue::Int(1),
            LiteralValue::Text("x".into()),
            LiteralValue::Int(3),
        ]]);
        let b = arr(vec![vec![
            LiteralValue::Int(1),
            LiteralValue::Int(1),
            LiteralValue::Int(1),
        ]]);
        let args = vec![ArgumentHandle::new(&a, &ctx), ArgumentHandle::new(&b, &ctx)];
        let f = ctx.context.get_function("", "SUMPRODUCT").unwrap();
        assert_eq!(
            f.dispatch(&args, &ctx.function_context(None))
                .unwrap()
                .into_literal(),
            LiteralValue::Number(4.0)
        );
    }

    #[test]
    fn sumproduct_error_in_input_propagates() {
        let wb = TestWorkbook::new().with_function(std::sync::Arc::new(SumProductFn));
        let ctx = interp(&wb);
        let a = arr(vec![vec![LiteralValue::Int(1), LiteralValue::Int(2)]]);
        let e = ASTNode::new(
            ASTNodeType::Literal(LiteralValue::Error(ExcelError::new_na())),
            None,
        );
        let args = vec![ArgumentHandle::new(&a, &ctx), ArgumentHandle::new(&e, &ctx)];
        let f = ctx.context.get_function("", "SUMPRODUCT").unwrap();
        match f
            .dispatch(&args, &ctx.function_context(None))
            .unwrap()
            .into_literal()
        {
            LiteralValue::Error(err) => assert_eq!(err, "#N/A"),
            v => panic!("expected error, got {v:?}"),
        }
    }

    #[test]
    fn sumproduct_incompatible_shapes_value_error() {
        let wb = TestWorkbook::new().with_function(std::sync::Arc::new(SumProductFn));
        let ctx = interp(&wb);
        // 1x3 and 1x2 -> #VALUE!
        let a = arr(vec![vec![
            LiteralValue::Int(1),
            LiteralValue::Int(2),
            LiteralValue::Int(3),
        ]]);
        let b = arr(vec![vec![LiteralValue::Int(4), LiteralValue::Int(5)]]);
        let args = vec![ArgumentHandle::new(&a, &ctx), ArgumentHandle::new(&b, &ctx)];
        let f = ctx.context.get_function("", "SUMPRODUCT").unwrap();
        match f
            .dispatch(&args, &ctx.function_context(None))
            .unwrap()
            .into_literal()
        {
            LiteralValue::Error(e) => assert_eq!(e, "#VALUE!"),
            v => panic!("expected value error, got {v:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_workbook::TestWorkbook;
    use formualizer_parse::LiteralValue;

    fn interp(wb: &TestWorkbook) -> crate::interpreter::Interpreter<'_> {
        wb.interpreter()
    }

    #[test]
    fn test_sum_caps() {
        let sum_fn = SumFn;
        let caps = sum_fn.caps();

        // Check that the expected capabilities are set
        assert!(caps.contains(crate::function::FnCaps::PURE));
        assert!(caps.contains(crate::function::FnCaps::REDUCTION));
        assert!(caps.contains(crate::function::FnCaps::NUMERIC_ONLY));
        assert!(caps.contains(crate::function::FnCaps::STREAM_OK));

        // Check that other caps are not set
        assert!(!caps.contains(crate::function::FnCaps::VOLATILE));
        assert!(!caps.contains(crate::function::FnCaps::ELEMENTWISE));
    }

    #[test]
    fn test_sum_basic() {
        let wb = TestWorkbook::new().with_function(std::sync::Arc::new(SumFn));
        let ctx = interp(&wb);
        let fctx = ctx.function_context(None);

        // Test basic SUM functionality by creating ArgumentHandles manually
        let dummy_ast_1 = formualizer_parse::parser::ASTNode::new(
            formualizer_parse::parser::ASTNodeType::Literal(LiteralValue::Number(1.0)),
            None,
        );
        let dummy_ast_2 = formualizer_parse::parser::ASTNode::new(
            formualizer_parse::parser::ASTNodeType::Literal(LiteralValue::Number(2.0)),
            None,
        );
        let dummy_ast_3 = formualizer_parse::parser::ASTNode::new(
            formualizer_parse::parser::ASTNodeType::Literal(LiteralValue::Number(3.0)),
            None,
        );

        let args = vec![
            ArgumentHandle::new(&dummy_ast_1, &ctx),
            ArgumentHandle::new(&dummy_ast_2, &ctx),
            ArgumentHandle::new(&dummy_ast_3, &ctx),
        ];

        let sum_fn = ctx.context.get_function("", "SUM").unwrap();
        let result = sum_fn.dispatch(&args, &fctx).unwrap().into_literal();
        assert_eq!(result, LiteralValue::Number(6.0));
    }
}

#[cfg(test)]
mod tests_count {
    use super::*;
    use crate::test_workbook::TestWorkbook;
    use crate::traits::ArgumentHandle;
    use formualizer_parse::LiteralValue;
    use formualizer_parse::parser::ASTNode;
    use formualizer_parse::parser::ASTNodeType;

    fn interp(wb: &TestWorkbook) -> crate::interpreter::Interpreter<'_> {
        wb.interpreter()
    }

    #[test]
    fn count_numbers_ignores_text() {
        let wb = TestWorkbook::new().with_function(std::sync::Arc::new(CountFn));
        let ctx = interp(&wb);
        // COUNT({1,2,"x",3}) => 3
        let arr = LiteralValue::Array(vec![vec![
            LiteralValue::Int(1),
            LiteralValue::Int(2),
            LiteralValue::Text("x".into()),
            LiteralValue::Int(3),
        ]]);
        let node = ASTNode::new(ASTNodeType::Literal(arr), None);
        let args = vec![ArgumentHandle::new(&node, &ctx)];
        let f = ctx.context.get_function("", "COUNT").unwrap();
        let fctx = ctx.function_context(None);
        assert_eq!(
            f.dispatch(&args, &fctx).unwrap().into_literal(),
            LiteralValue::Number(3.0)
        );
    }

    #[test]
    fn count_multiple_args_and_scalars() {
        let wb = TestWorkbook::new().with_function(std::sync::Arc::new(CountFn));
        let ctx = interp(&wb);
        let n1 = ASTNode::new(ASTNodeType::Literal(LiteralValue::Int(10)), None);
        let n2 = ASTNode::new(ASTNodeType::Literal(LiteralValue::Text("n".into())), None);
        let arr = LiteralValue::Array(vec![vec![LiteralValue::Int(1), LiteralValue::Int(2)]]);
        let a = ASTNode::new(ASTNodeType::Literal(arr), None);
        let args = vec![
            ArgumentHandle::new(&a, &ctx),
            ArgumentHandle::new(&n1, &ctx),
            ArgumentHandle::new(&n2, &ctx),
        ];
        let f = ctx.context.get_function("", "COUNT").unwrap();
        // Two from array + scalar 10 = 3
        let fctx = ctx.function_context(None);
        assert_eq!(
            f.dispatch(&args, &fctx).unwrap().into_literal(),
            LiteralValue::Number(3.0)
        );
    }

    #[test]
    fn count_ignores_direct_error_argument() {
        // Excel: COUNT never propagates; `COUNT(A1,NA())` counts the number only.
        let wb = TestWorkbook::new().with_function(std::sync::Arc::new(CountFn));
        let ctx = interp(&wb);
        let err = ASTNode::new(
            ASTNodeType::Literal(LiteralValue::Error(ExcelError::from_error_string(
                "#DIV/0!",
            ))),
            None,
        );
        let one = ASTNode::new(ASTNodeType::Literal(LiteralValue::Int(1)), None);
        let args = vec![
            ArgumentHandle::new(&one, &ctx),
            ArgumentHandle::new(&err, &ctx),
        ];
        let f = ctx.context.get_function("", "COUNT").unwrap();
        let fctx = ctx.function_context(None);
        assert_eq!(
            f.dispatch(&args, &fctx).unwrap().into_literal(),
            LiteralValue::Number(1.0)
        );
    }
}

#[cfg(test)]
mod tests_average {
    use super::*;
    use crate::test_workbook::TestWorkbook;
    use crate::traits::ArgumentHandle;
    use formualizer_parse::LiteralValue;
    use formualizer_parse::parser::ASTNode;
    use formualizer_parse::parser::ASTNodeType;

    fn interp(wb: &TestWorkbook) -> crate::interpreter::Interpreter<'_> {
        wb.interpreter()
    }

    #[test]
    fn average_basic_numbers() {
        let wb = TestWorkbook::new().with_function(std::sync::Arc::new(AverageFn));
        let ctx = interp(&wb);
        let arr = LiteralValue::Array(vec![vec![
            LiteralValue::Int(2),
            LiteralValue::Int(4),
            LiteralValue::Int(6),
        ]]);
        let node = ASTNode::new(ASTNodeType::Literal(arr), None);
        let args = vec![ArgumentHandle::new(&node, &ctx)];
        let f = ctx.context.get_function("", "AVERAGE").unwrap();
        assert_eq!(
            f.dispatch(&args, &ctx.function_context(None))
                .unwrap()
                .into_literal(),
            LiteralValue::Number(4.0)
        );
    }

    #[test]
    fn average_mixed_with_text() {
        let wb = TestWorkbook::new().with_function(std::sync::Arc::new(AverageFn));
        let ctx = interp(&wb);
        let arr = LiteralValue::Array(vec![vec![
            LiteralValue::Int(2),
            LiteralValue::Text("x".into()),
            LiteralValue::Int(6),
        ]]);
        let node = ASTNode::new(ASTNodeType::Literal(arr), None);
        let args = vec![ArgumentHandle::new(&node, &ctx)];
        let f = ctx.context.get_function("", "AVERAGE").unwrap();
        // average of 2 and 6 = 4
        assert_eq!(
            f.dispatch(&args, &ctx.function_context(None))
                .unwrap()
                .into_literal(),
            LiteralValue::Number(4.0)
        );
    }

    #[test]
    fn average_no_numeric_div0() {
        let wb = TestWorkbook::new().with_function(std::sync::Arc::new(AverageFn));
        let ctx = interp(&wb);
        let arr = LiteralValue::Array(vec![vec![
            LiteralValue::Text("a".into()),
            LiteralValue::Text("b".into()),
        ]]);
        let node = ASTNode::new(ASTNodeType::Literal(arr), None);
        let args = vec![ArgumentHandle::new(&node, &ctx)];
        let f = ctx.context.get_function("", "AVERAGE").unwrap();
        let fctx = ctx.function_context(None);
        match f.dispatch(&args, &fctx).unwrap().into_literal() {
            LiteralValue::Error(e) => assert_eq!(e, "#DIV/0!"),
            v => panic!("expected #DIV/0!, got {v:?}"),
        }
    }

    #[test]
    fn average_direct_error_argument_propagates() {
        let wb = TestWorkbook::new().with_function(std::sync::Arc::new(AverageFn));
        let ctx = interp(&wb);
        let err = ASTNode::new(
            ASTNodeType::Literal(LiteralValue::Error(ExcelError::from_error_string(
                "#DIV/0!",
            ))),
            None,
        );
        let args = vec![ArgumentHandle::new(&err, &ctx)];
        let f = ctx.context.get_function("", "AVERAGE").unwrap();
        let fctx = ctx.function_context(None);
        match f.dispatch(&args, &fctx).unwrap().into_literal() {
            LiteralValue::Error(e) => assert_eq!(e, "#DIV/0!"),
            v => panic!("unexpected {v:?}"),
        }
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum VisibilityPolicy {
    IncludeAll,
    /// SUBTOTAL 1–11: rows hidden by an AutoFilter are always excluded, manually
    /// hidden rows still count.
    ExcludeFilterHidden,
    ExcludeManualOrFilterHidden,
}

impl VisibilityPolicy {
    fn mask_mode(self) -> Option<VisibilityMaskMode> {
        match self {
            VisibilityPolicy::IncludeAll => None,
            VisibilityPolicy::ExcludeFilterHidden => Some(VisibilityMaskMode::ExcludeFilterHidden),
            VisibilityPolicy::ExcludeManualOrFilterHidden => {
                Some(VisibilityMaskMode::ExcludeManualOrFilterHidden)
            }
        }
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum ErrorPolicy {
    Propagate,
    Ignore,
}

/// Whether cells that are themselves `SUBTOTAL(...)` / `AGGREGATE(...)` formulas are
/// skipped. Excel always skips them for SUBTOTAL and for AGGREGATE options 0–3.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum NestedPolicy {
    Exclude,
    Include,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum AggregateOp {
    Average,
    Count,
    CountA,
    Max,
    Min,
    Product,
    StdevSample,
    StdevPopulation,
    Sum,
    VarSample,
    VarPopulation,
    Median,
    ModeSngl,
    Large,
    Small,
    PercentileInc,
    QuartileInc,
    PercentileExc,
    QuartileExc,
}

impl AggregateOp {
    /// Excel `function_num` 1–19 (SUBTOTAL only accepts 1–11 / 101–111).
    fn from_function_num(function_num: i32) -> Option<AggregateOp> {
        match function_num {
            1 => Some(AggregateOp::Average),
            2 => Some(AggregateOp::Count),
            3 => Some(AggregateOp::CountA),
            4 => Some(AggregateOp::Max),
            5 => Some(AggregateOp::Min),
            6 => Some(AggregateOp::Product),
            7 => Some(AggregateOp::StdevSample),
            8 => Some(AggregateOp::StdevPopulation),
            9 => Some(AggregateOp::Sum),
            10 => Some(AggregateOp::VarSample),
            11 => Some(AggregateOp::VarPopulation),
            12 => Some(AggregateOp::Median),
            13 => Some(AggregateOp::ModeSngl),
            14 => Some(AggregateOp::Large),
            15 => Some(AggregateOp::Small),
            16 => Some(AggregateOp::PercentileInc),
            17 => Some(AggregateOp::QuartileInc),
            18 => Some(AggregateOp::PercentileExc),
            19 => Some(AggregateOp::QuartileExc),
            _ => None,
        }
    }

    /// Functions 14–19 use the array form `AGGREGATE(fn, options, array, k)`.
    fn takes_k(self) -> bool {
        matches!(
            self,
            AggregateOp::Large
                | AggregateOp::Small
                | AggregateOp::PercentileInc
                | AggregateOp::QuartileInc
                | AggregateOp::PercentileExc
                | AggregateOp::QuartileExc
        )
    }
}

fn aggregate_op_from_function_num(function_num: i32) -> Option<AggregateOp> {
    AggregateOp::from_function_num(function_num)
}

fn parse_strict_int_arg(arg: &ArgumentHandle<'_, '_>) -> Result<i32, ExcelError> {
    let raw = arg.value()?.into_literal();
    if let LiteralValue::Error(e) = raw {
        return Err(e);
    }

    let n = coerce_num(&raw)?;
    if !n.is_finite() {
        return Err(ExcelError::new_value());
    }

    let rounded = n.round();
    if (n - rounded).abs() > 1e-9 {
        return Err(ExcelError::new_value());
    }

    if rounded < i32::MIN as f64 || rounded > i32::MAX as f64 {
        return Err(ExcelError::new_value());
    }

    Ok(rounded as i32)
}

fn row_is_visible(mask: Option<&arrow_array::BooleanArray>, relative_row: usize) -> bool {
    let Some(mask) = mask else {
        return true;
    };

    if relative_row >= mask.len() || mask.is_null(relative_row) {
        return true;
    }

    mask.value(relative_row)
}

fn numeric_from_range_value(value: &LiteralValue) -> Option<f64> {
    match value {
        LiteralValue::Number(n) => Some(*n),
        LiteralValue::Int(i) => Some(*i as f64),
        LiteralValue::Date(_)
        | LiteralValue::DateTime(_)
        | LiteralValue::Time(_)
        | LiteralValue::Duration(_) => coerce_num(value).ok(),
        _ => None,
    }
}

/// `=SUBTOTAL(…)` / `=AGGREGATE(…)` (any case, any leading whitespace) — the cells
/// Excel skips when a SUBTOTAL/AGGREGATE range contains other subtotals.
fn formula_is_nested_aggregate(formula: &str) -> bool {
    let body = formula.trim_start().trim_start_matches('=').trim_start();
    let head: String = body
        .chars()
        .take_while(|c| c.is_ascii_alphabetic() || *c == '.')
        .flat_map(|c| c.to_uppercase())
        .collect();
    (head == "SUBTOTAL" || head == "AGGREGATE") && body[head.len()..].trim_start().starts_with('(')
}

#[derive(Debug, Default)]
struct AggregateCollector {
    numeric_values: Vec<f64>,
    counta: usize,
}

#[derive(Copy, Clone, Debug)]
struct CollectPolicy {
    op: AggregateOp,
    visibility: VisibilityPolicy,
    errors: ErrorPolicy,
    nested: NestedPolicy,
}

impl AggregateCollector {
    fn collect_args<'a, 'b>(
        args: &[ArgumentHandle<'a, 'b>],
        ctx: &dyn FunctionContext<'b>,
        policy: CollectPolicy,
    ) -> Result<Self, ExcelError> {
        let mut out = Self::default();

        for arg in args {
            if let Ok(view) = arg.range_view() {
                out.collect_range_arg(&view, ctx, policy)?;
            } else {
                out.consume_scalar_value(arg.value()?.into_literal(), policy.op, policy.errors)?;
            }
        }

        Ok(out)
    }

    fn collect_range_arg<'b>(
        &mut self,
        view: &crate::engine::range_view::RangeView<'_>,
        ctx: &dyn FunctionContext<'b>,
        policy: CollectPolicy,
    ) -> Result<(), ExcelError> {
        let visibility_mask = policy
            .visibility
            .mask_mode()
            .and_then(|mode| ctx.get_row_visibility_mask(view, mode));

        // Nested SUBTOTAL/AGGREGATE exclusion needs the cells' formulas; hosts that
        // cannot map the view back to sheet cells (owned/array views) skip it.
        let nested_sheet = match policy.nested {
            NestedPolicy::Exclude => ctx.sheet_id_by_name(view.sheet_name()),
            NestedPolicy::Include => None,
        };

        let (_, cols) = view.dims();
        if cols == 0 {
            return Ok(());
        }

        for chunk in view.iter_row_chunks() {
            let chunk = chunk?;
            for row_offset in 0..chunk.row_len {
                let rel_row = chunk.row_start + row_offset;
                if !row_is_visible(visibility_mask.as_deref(), rel_row) {
                    continue;
                }

                for col in 0..cols {
                    let value = view.get_cell(rel_row, col);
                    if let Some(sheet_id) = nested_sheet
                        && matches!(
                            value,
                            LiteralValue::Number(_) | LiteralValue::Int(_) | LiteralValue::Error(_)
                        )
                        && self.is_nested_aggregate_cell(
                            ctx,
                            sheet_id,
                            (view.start_row() + rel_row) as u32,
                            (view.start_col() + col) as u32,
                        )
                    {
                        continue;
                    }
                    self.consume_range_value(value, policy.op, policy.errors)?;
                }
            }
        }

        Ok(())
    }

    fn is_nested_aggregate_cell<'b>(
        &self,
        ctx: &dyn FunctionContext<'b>,
        sheet_id: crate::reference::SheetId,
        row0: u32,
        col0: u32,
    ) -> bool {
        let cell = CellRef::new(sheet_id, Coord::new(row0, col0, false, false));
        match ctx.formula_text_at_cell(cell) {
            Ok(Some(text)) => formula_is_nested_aggregate(&text),
            _ => false,
        }
    }

    fn consume_range_value(
        &mut self,
        value: LiteralValue,
        op: AggregateOp,
        error_policy: ErrorPolicy,
    ) -> Result<(), ExcelError> {
        match value {
            LiteralValue::Error(e) => {
                if op == AggregateOp::CountA {
                    if error_policy == ErrorPolicy::Ignore {
                        return Ok(());
                    }
                    self.counta += 1;
                    return Ok(());
                }
                match error_policy {
                    ErrorPolicy::Propagate => Err(e),
                    ErrorPolicy::Ignore => Ok(()),
                }
            }
            LiteralValue::Empty => Ok(()),
            other => {
                self.counta += 1;
                if let Some(n) = numeric_from_range_value(&other) {
                    self.numeric_values.push(n);
                }
                Ok(())
            }
        }
    }

    fn consume_scalar_value(
        &mut self,
        value: LiteralValue,
        op: AggregateOp,
        error_policy: ErrorPolicy,
    ) -> Result<(), ExcelError> {
        match value {
            LiteralValue::Error(e) => {
                if op == AggregateOp::CountA {
                    if error_policy == ErrorPolicy::Ignore {
                        return Ok(());
                    }
                    self.counta += 1;
                    return Ok(());
                }
                match error_policy {
                    ErrorPolicy::Propagate => Err(e),
                    ErrorPolicy::Ignore => Ok(()),
                }
            }
            LiteralValue::Array(rows) => {
                for row in rows {
                    for cell in row {
                        self.consume_range_value(cell, op, error_policy)?;
                    }
                }
                Ok(())
            }
            other => {
                match op {
                    AggregateOp::CountA => {
                        if !matches!(other, LiteralValue::Empty) {
                            self.counta += 1;
                        }
                    }
                    AggregateOp::Count => {
                        if !matches!(other, LiteralValue::Empty) && coerce_num(&other).is_ok() {
                            self.numeric_values.push(0.0);
                        }
                    }
                    _ => {
                        if let Ok(n) = coerce_num(&other) {
                            self.numeric_values.push(n);
                        }
                    }
                }
                Ok(())
            }
        }
    }

    fn variance(values: &[f64], sample: bool) -> Result<f64, ExcelError> {
        let n = values.len();
        if sample {
            if n < 2 {
                return Err(ExcelError::new_div());
            }
        } else if n == 0 {
            return Err(ExcelError::new_div());
        }

        let mean = values.iter().copied().sum::<f64>() / (n as f64);
        let mut ss = 0.0;
        for value in values {
            let d = *value - mean;
            ss += d * d;
        }

        if sample {
            Ok(ss / ((n - 1) as f64))
        } else {
            Ok(ss / (n as f64))
        }
    }

    /// Order-statistic functions (AGGREGATE 12–19). `k` is the fourth argument for
    /// 14–19 (LARGE/SMALL rank, PERCENTILE p, QUARTILE quart) and unused otherwise.
    fn finalize_order_statistic(mut self, op: AggregateOp, k: Option<f64>) -> LiteralValue {
        use super::super::stats::{nth_smallest, percentile_exc, percentile_inc};
        let nums = &mut self.numeric_values;
        let num_or_err = |r: Result<f64, ExcelError>| match r {
            Ok(v) => LiteralValue::Number(v),
            Err(e) => LiteralValue::Error(e),
        };
        match op {
            AggregateOp::Median => {
                if nums.is_empty() {
                    return LiteralValue::Error(ExcelError::new_num());
                }
                let n = nums.len();
                let mid = n / 2;
                let med = if n % 2 == 1 {
                    nth_smallest(nums, mid)
                } else {
                    let (lo, hi) = super::super::stats::adjacent_smallest(nums, mid - 1);
                    (lo + hi) / 2.0
                };
                LiteralValue::Number(med)
            }
            AggregateOp::ModeSngl => {
                if nums.is_empty() {
                    return LiteralValue::Error(ExcelError::new_na());
                }
                nums.sort_by(|a, b| a.partial_cmp(b).unwrap());
                let (mut best_val, mut best_cnt) = (nums[0], 1usize);
                let (mut cur_val, mut cur_cnt) = (nums[0], 1usize);
                for &v in &nums[1..] {
                    if (v - cur_val).abs() < 1e-12 {
                        cur_cnt += 1;
                    } else {
                        if cur_cnt > best_cnt {
                            best_cnt = cur_cnt;
                            best_val = cur_val;
                        }
                        cur_val = v;
                        cur_cnt = 1;
                    }
                }
                if cur_cnt > best_cnt {
                    best_cnt = cur_cnt;
                    best_val = cur_val;
                }
                if best_cnt <= 1 {
                    LiteralValue::Error(ExcelError::new_na())
                } else {
                    LiteralValue::Number(best_val)
                }
            }
            AggregateOp::Large | AggregateOp::Small => {
                let Some(k) = k else {
                    return LiteralValue::Error(ExcelError::new_value());
                };
                let k = k as i64;
                if nums.is_empty() || k < 1 || k as usize > nums.len() {
                    return LiteralValue::Error(ExcelError::new_num());
                }
                let idx = if op == AggregateOp::Large {
                    nums.len() - k as usize
                } else {
                    k as usize - 1
                };
                LiteralValue::Number(nth_smallest(nums, idx))
            }
            AggregateOp::PercentileInc | AggregateOp::PercentileExc => {
                let Some(p) = k else {
                    return LiteralValue::Error(ExcelError::new_value());
                };
                if op == AggregateOp::PercentileInc {
                    num_or_err(percentile_inc(nums, p))
                } else {
                    num_or_err(percentile_exc(nums, p))
                }
            }
            AggregateOp::QuartileInc | AggregateOp::QuartileExc => {
                let Some(q) = k else {
                    return LiteralValue::Error(ExcelError::new_value());
                };
                let q = q as i64;
                if nums.is_empty() || !(0..=4).contains(&q) {
                    return LiteralValue::Error(ExcelError::new_num());
                }
                if op == AggregateOp::QuartileInc {
                    match q {
                        0 => LiteralValue::Number(nth_smallest(nums, 0)),
                        4 => {
                            let idx = nums.len() - 1;
                            LiteralValue::Number(nth_smallest(nums, idx))
                        }
                        _ => num_or_err(percentile_inc(nums, q as f64 / 4.0)),
                    }
                } else {
                    // QUARTILE.EXC is undefined at the extremes.
                    if q == 0 || q == 4 {
                        return LiteralValue::Error(ExcelError::new_num());
                    }
                    num_or_err(percentile_exc(nums, q as f64 / 4.0))
                }
            }
            _ => LiteralValue::Error(ExcelError::new_value()),
        }
    }

    fn finalize(self, op: AggregateOp) -> LiteralValue {
        use super::super::utils::aggregate_result;
        match op {
            AggregateOp::Average => {
                if self.numeric_values.is_empty() {
                    LiteralValue::Error(ExcelError::new_div())
                } else {
                    let sum = self.numeric_values.iter().copied().sum::<f64>();
                    aggregate_result(sum / (self.numeric_values.len() as f64))
                }
            }
            // Counts cannot overflow to non-finite; keep them branch-free.
            AggregateOp::Count => LiteralValue::Number(self.numeric_values.len() as f64),
            AggregateOp::CountA => LiteralValue::Number(self.counta as f64),
            AggregateOp::Max => aggregate_result(
                self.numeric_values
                    .iter()
                    .copied()
                    .reduce(f64::max)
                    .unwrap_or(0.0),
            ),
            AggregateOp::Min => aggregate_result(
                self.numeric_values
                    .iter()
                    .copied()
                    .reduce(f64::min)
                    .unwrap_or(0.0),
            ),
            AggregateOp::Product => {
                if self.numeric_values.is_empty() {
                    LiteralValue::Number(0.0)
                } else {
                    aggregate_result(self.numeric_values.iter().copied().product::<f64>())
                }
            }
            AggregateOp::StdevSample => match Self::variance(&self.numeric_values, true) {
                Ok(v) => aggregate_result(v.sqrt()),
                Err(e) => LiteralValue::Error(e),
            },
            AggregateOp::StdevPopulation => match Self::variance(&self.numeric_values, false) {
                Ok(v) => aggregate_result(v.sqrt()),
                Err(e) => LiteralValue::Error(e),
            },
            AggregateOp::Sum => aggregate_result(self.numeric_values.iter().copied().sum()),
            AggregateOp::VarSample => match Self::variance(&self.numeric_values, true) {
                Ok(v) => aggregate_result(v),
                Err(e) => LiteralValue::Error(e),
            },
            AggregateOp::VarPopulation => match Self::variance(&self.numeric_values, false) {
                Ok(v) => aggregate_result(v),
                Err(e) => LiteralValue::Error(e),
            },
            order_statistic => self.finalize_order_statistic(order_statistic, None),
        }
    }
}

#[derive(Debug)]
pub struct SubtotalFn;

/// [formualizer-docgen:schema:start]
/// Name: SUBTOTAL
/// Type: SubtotalFn
/// Min args: 2
/// Max args: variadic
/// Variadic: true
/// Signature: SUBTOTAL(arg1...: number@range)
/// Arg schema: arg1{kinds=number,required=true,shape=range,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}
/// Caps: VOLATILE, REDUCTION, NUMERIC_ONLY, STREAM_OK
/// [formualizer-docgen:schema:end]
impl Function for SubtotalFn {
    func_caps!(VOLATILE, REDUCTION, NUMERIC_ONLY, STREAM_OK);

    fn name(&self) -> &'static str {
        "SUBTOTAL"
    }

    fn min_args(&self) -> usize {
        2
    }

    fn variadic(&self) -> bool {
        true
    }

    fn arg_schema(&self) -> &'static [ArgSchema] {
        &ARG_RANGE_NUM_LENIENT_ONE[..]
    }

    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        ctx: &dyn FunctionContext<'b>,
    ) -> Result<crate::traits::CalcValue<'b>, ExcelError> {
        if args.len() < 2 {
            return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                ExcelError::new_value(),
            )));
        }

        let function_num = match parse_strict_int_arg(&args[0]) {
            Ok(v) => v,
            Err(e) => return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(e))),
        };

        // 1–11 skip only AutoFilter-hidden rows; 101–111 skip manually hidden rows too.
        let (mapped_code, visibility) = if (1..=11).contains(&function_num) {
            (function_num, VisibilityPolicy::ExcludeFilterHidden)
        } else if (101..=111).contains(&function_num) {
            (
                function_num - 100,
                VisibilityPolicy::ExcludeManualOrFilterHidden,
            )
        } else {
            return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                ExcelError::new_value(),
            )));
        };

        let Some(op) = aggregate_op_from_function_num(mapped_code) else {
            return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                ExcelError::new_value(),
            )));
        };

        let policy = CollectPolicy {
            op,
            visibility,
            errors: ErrorPolicy::Propagate,
            nested: NestedPolicy::Exclude,
        };
        let collected = match AggregateCollector::collect_args(&args[1..], ctx, policy) {
            Ok(c) => c,
            Err(e) => return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(e))),
        };

        Ok(crate::traits::CalcValue::Scalar(collected.finalize(op)))
    }
}

#[derive(Debug)]
pub struct AggregateFn;

/// [formualizer-docgen:schema:start]
/// Name: AGGREGATE
/// Type: AggregateFn
/// Min args: 3
/// Max args: variadic
/// Variadic: true
/// Signature: AGGREGATE(arg1...: number@range)
/// Arg schema: arg1{kinds=number,required=true,shape=range,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}
/// Caps: VOLATILE, REDUCTION, NUMERIC_ONLY, STREAM_OK
/// [formualizer-docgen:schema:end]
impl Function for AggregateFn {
    func_caps!(VOLATILE, REDUCTION, NUMERIC_ONLY, STREAM_OK);

    fn name(&self) -> &'static str {
        "AGGREGATE"
    }

    fn min_args(&self) -> usize {
        3
    }

    fn variadic(&self) -> bool {
        true
    }

    fn arg_schema(&self) -> &'static [ArgSchema] {
        &ARG_RANGE_NUM_LENIENT_ONE[..]
    }

    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        ctx: &dyn FunctionContext<'b>,
    ) -> Result<crate::traits::CalcValue<'b>, ExcelError> {
        if args.len() < 3 {
            return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                ExcelError::new_value(),
            )));
        }

        let function_num = match parse_strict_int_arg(&args[0]) {
            Ok(v) => v,
            Err(e) => return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(e))),
        };

        let Some(op) = aggregate_op_from_function_num(function_num) else {
            return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                ExcelError::new_value(),
            )));
        };

        let options = match parse_strict_int_arg(&args[1]) {
            Ok(v) => v,
            Err(e) => return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(e))),
        };

        // options: bit 0 = skip hidden rows, bit 1 = skip errors, bit 2 = include
        // nested SUBTOTAL/AGGREGATE results (0–3 exclude them, 4–7 keep them).
        if !(0..=7).contains(&options) {
            return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                ExcelError::new_value(),
            )));
        }
        let visibility = if options & 1 == 1 {
            VisibilityPolicy::ExcludeManualOrFilterHidden
        } else {
            VisibilityPolicy::IncludeAll
        };
        let errors = if options & 2 == 2 {
            ErrorPolicy::Ignore
        } else {
            ErrorPolicy::Propagate
        };
        let nested = if options & 4 == 4 {
            NestedPolicy::Include
        } else {
            NestedPolicy::Exclude
        };
        let policy = CollectPolicy {
            op,
            visibility,
            errors,
            nested,
        };

        if op.takes_k() {
            // Array form: AGGREGATE(function_num, options, array, k).
            if args.len() != 4 {
                return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                    ExcelError::new_value(),
                )));
            }
            let k_raw = args[3].value()?.into_literal();
            if let LiteralValue::Error(e) = k_raw {
                return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(e)));
            }
            let k = match coerce_num(&k_raw) {
                Ok(k) if k.is_finite() => k,
                _ => {
                    return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                        ExcelError::new_value(),
                    )));
                }
            };
            let collected = match AggregateCollector::collect_args(&args[2..3], ctx, policy) {
                Ok(c) => c,
                Err(e) => return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(e))),
            };
            return Ok(crate::traits::CalcValue::Scalar(
                collected.finalize_order_statistic(op, Some(k)),
            ));
        }

        let collected = match AggregateCollector::collect_args(&args[2..], ctx, policy) {
            Ok(c) => c,
            Err(e) => return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(e))),
        };

        Ok(crate::traits::CalcValue::Scalar(collected.finalize(op)))
    }
}

#[cfg(test)]
mod tests_subtotal_aggregate {
    use super::*;
    use crate::test_workbook::TestWorkbook;
    use crate::traits::ArgumentHandle;
    use formualizer_common::{ExcelErrorKind, LiteralValue};
    use formualizer_parse::parser::{ASTNode, ASTNodeType};

    fn interp(wb: &TestWorkbook) -> crate::interpreter::Interpreter<'_> {
        wb.interpreter()
    }

    fn lit(value: LiteralValue) -> ASTNode {
        ASTNode::new(ASTNodeType::Literal(value), None)
    }

    fn dispatch(
        ctx: &crate::interpreter::Interpreter<'_>,
        fn_name: &str,
        nodes: &[ASTNode],
    ) -> LiteralValue {
        let args: Vec<_> = nodes.iter().map(|n| ArgumentHandle::new(n, ctx)).collect();
        let f = ctx.context.get_function("", fn_name).expect("function");
        f.dispatch(&args, &ctx.function_context(None))
            .expect("dispatch")
            .into_literal()
    }

    fn assert_num_close(value: LiteralValue, expected: f64) {
        match value {
            LiteralValue::Number(n) => assert!((n - expected).abs() < 1e-9, "{n} != {expected}"),
            LiteralValue::Int(i) => assert!(((i as f64) - expected).abs() < 1e-9),
            other => panic!("expected numeric {expected}, got {other:?}"),
        }
    }

    fn assert_error_kind(value: LiteralValue, expected: ExcelErrorKind) {
        match value {
            LiteralValue::Error(e) => assert_eq!(e.kind, expected),
            other => panic!("expected error {:?}, got {other:?}", expected),
        }
    }

    #[test]
    fn subtotal_function_num_mapping_basics() {
        let wb = TestWorkbook::new().with_function(std::sync::Arc::new(SubtotalFn));
        let ctx = interp(&wb);
        let values = LiteralValue::Array(vec![vec![
            LiteralValue::Int(1),
            LiteralValue::Int(2),
            LiteralValue::Int(3),
        ]]);

        let cases: &[(i64, f64)] = &[
            (1, 2.0),
            (2, 3.0),
            (3, 3.0),
            (4, 3.0),
            (5, 1.0),
            (6, 6.0),
            (7, 1.0),
            (8, (2.0f64 / 3.0).sqrt()),
            (9, 6.0),
            (10, 1.0),
            (11, 2.0 / 3.0),
        ];

        for (code, expected) in cases {
            let args = vec![lit(LiteralValue::Int(*code)), lit(values.clone())];
            let out = dispatch(&ctx, "SUBTOTAL", &args);
            assert_num_close(out, *expected);
        }
    }

    #[test]
    fn subtotal_counta_counts_errors_as_non_empty() {
        let wb = TestWorkbook::new().with_function(std::sync::Arc::new(SubtotalFn));
        let ctx = interp(&wb);

        let args = vec![
            lit(LiteralValue::Int(3)),
            lit(LiteralValue::Array(vec![vec![
                LiteralValue::Int(1),
                LiteralValue::Error(ExcelError::new_div()),
                LiteralValue::Text("x".into()),
                LiteralValue::Text("".into()),
            ]])),
        ];
        let out = dispatch(&ctx, "SUBTOTAL", &args);
        assert_num_close(out, 4.0);
    }

    #[test]
    fn subtotal_invalid_function_num_returns_value_error() {
        let wb = TestWorkbook::new().with_function(std::sync::Arc::new(SubtotalFn));
        let ctx = interp(&wb);

        let args = vec![
            lit(LiteralValue::Number(9.5)),
            lit(LiteralValue::Array(vec![vec![LiteralValue::Int(1)]])),
        ];
        let out = dispatch(&ctx, "SUBTOTAL", &args);
        assert_error_kind(out, ExcelErrorKind::Value);
    }

    #[test]
    fn subtotal_requires_ref_argument() {
        let wb = TestWorkbook::new().with_function(std::sync::Arc::new(SubtotalFn));
        let ctx = interp(&wb);

        let out = dispatch(&ctx, "SUBTOTAL", &[lit(LiteralValue::Int(9))]);
        assert_error_kind(out, ExcelErrorKind::Value);
    }

    #[test]
    fn aggregate_requires_options_and_ref_argument() {
        let wb = TestWorkbook::new().with_function(std::sync::Arc::new(AggregateFn));
        let ctx = interp(&wb);

        let out = dispatch(
            &ctx,
            "AGGREGATE",
            &[lit(LiteralValue::Int(9)), lit(LiteralValue::Int(0))],
        );
        assert_error_kind(out, ExcelErrorKind::Value);
    }

    #[test]
    fn aggregate_options_zero_to_three_control_error_behavior() {
        let wb = TestWorkbook::new().with_function(std::sync::Arc::new(AggregateFn));
        let ctx = interp(&wb);
        let values = LiteralValue::Array(vec![vec![
            LiteralValue::Int(10),
            LiteralValue::Error(ExcelError::new_div()),
            LiteralValue::Int(30),
        ]]);

        let opt0 = dispatch(
            &ctx,
            "AGGREGATE",
            &[
                lit(LiteralValue::Int(9)),
                lit(LiteralValue::Int(0)),
                lit(values.clone()),
            ],
        );
        assert_error_kind(opt0, ExcelErrorKind::Div);

        let opt1 = dispatch(
            &ctx,
            "AGGREGATE",
            &[
                lit(LiteralValue::Int(9)),
                lit(LiteralValue::Int(1)),
                lit(values.clone()),
            ],
        );
        assert_error_kind(opt1, ExcelErrorKind::Div);

        let opt2 = dispatch(
            &ctx,
            "AGGREGATE",
            &[
                lit(LiteralValue::Int(9)),
                lit(LiteralValue::Int(2)),
                lit(values.clone()),
            ],
        );
        assert_num_close(opt2, 40.0);

        let opt3 = dispatch(
            &ctx,
            "AGGREGATE",
            &[
                lit(LiteralValue::Int(9)),
                lit(LiteralValue::Int(3)),
                lit(values),
            ],
        );
        assert_num_close(opt3, 40.0);
    }

    #[test]
    fn aggregate_counta_option_ignore_errors_skips_error_values() {
        let wb = TestWorkbook::new().with_function(std::sync::Arc::new(AggregateFn));
        let ctx = interp(&wb);

        let out = dispatch(
            &ctx,
            "AGGREGATE",
            &[
                lit(LiteralValue::Int(3)),
                lit(LiteralValue::Int(2)),
                lit(LiteralValue::Array(vec![vec![
                    LiteralValue::Int(1),
                    LiteralValue::Error(ExcelError::new_div()),
                    LiteralValue::Text("x".into()),
                ]])),
            ],
        );
        assert_num_close(out, 2.0);
    }

    #[test]
    fn aggregate_option_four_ignores_nothing() {
        let wb = TestWorkbook::new().with_function(std::sync::Arc::new(AggregateFn));
        let ctx = interp(&wb);

        let out = dispatch(
            &ctx,
            "AGGREGATE",
            &[
                lit(LiteralValue::Int(9)),
                lit(LiteralValue::Int(4)),
                lit(LiteralValue::Array(vec![vec![LiteralValue::Int(1)]])),
            ],
        );
        assert_num_close(out, 1.0);
    }

    fn sample_array() -> ASTNode {
        lit(LiteralValue::Array(vec![vec![
            LiteralValue::Int(3),
            LiteralValue::Int(1),
            LiteralValue::Int(4),
            LiteralValue::Int(1),
            LiteralValue::Int(5),
            LiteralValue::Int(9),
            LiteralValue::Int(2),
            LiteralValue::Int(6),
        ]]))
    }

    fn aggregate(
        ctx: &crate::interpreter::Interpreter<'_>,
        function_num: i64,
        options: i64,
        k: Option<f64>,
    ) -> LiteralValue {
        let mut nodes = vec![
            lit(LiteralValue::Int(function_num)),
            lit(LiteralValue::Int(options)),
            sample_array(),
        ];
        if let Some(k) = k {
            nodes.push(lit(LiteralValue::Number(k)));
        }
        dispatch(ctx, "AGGREGATE", &nodes)
    }

    #[test]
    fn aggregate_order_statistic_functions_12_to_19() {
        let wb = TestWorkbook::new().with_function(std::sync::Arc::new(AggregateFn));
        let ctx = interp(&wb);

        assert_num_close(aggregate(&ctx, 12, 0, None), 3.5); // MEDIAN
        assert_num_close(aggregate(&ctx, 13, 0, None), 1.0); // MODE.SNGL
        assert_num_close(aggregate(&ctx, 14, 0, Some(2.0)), 6.0); // LARGE
        assert_num_close(aggregate(&ctx, 15, 0, Some(2.0)), 1.0); // SMALL
        assert_num_close(aggregate(&ctx, 16, 0, Some(0.5)), 3.5); // PERCENTILE.INC
        assert_num_close(aggregate(&ctx, 17, 0, Some(1.0)), 1.75); // QUARTILE.INC
        assert_num_close(aggregate(&ctx, 18, 0, Some(0.5)), 3.5); // PERCENTILE.EXC
        assert_num_close(aggregate(&ctx, 19, 0, Some(1.0)), 1.25); // QUARTILE.EXC
    }

    #[test]
    fn aggregate_k_argument_errors_follow_excel() {
        let wb = TestWorkbook::new().with_function(std::sync::Arc::new(AggregateFn));
        let ctx = interp(&wb);

        // 14–19 require the array form with k.
        assert_error_kind(aggregate(&ctx, 14, 0, None), ExcelErrorKind::Value);
        assert_error_kind(aggregate(&ctx, 14, 0, Some(0.0)), ExcelErrorKind::Num);
        assert_error_kind(aggregate(&ctx, 15, 0, Some(9.0)), ExcelErrorKind::Num);
        assert_error_kind(aggregate(&ctx, 18, 0, Some(0.0)), ExcelErrorKind::Num);
        assert_error_kind(aggregate(&ctx, 19, 0, Some(4.0)), ExcelErrorKind::Num);
        assert_error_kind(aggregate(&ctx, 20, 0, None), ExcelErrorKind::Value);
        assert_error_kind(aggregate(&ctx, 9, 8, None), ExcelErrorKind::Value);
    }

    #[test]
    fn aggregate_options_four_to_seven_control_errors_and_hidden_rows() {
        let wb = TestWorkbook::new().with_function(std::sync::Arc::new(AggregateFn));
        let ctx = interp(&wb);
        let with_error = || {
            lit(LiteralValue::Array(vec![vec![
                LiteralValue::Int(1),
                LiteralValue::Error(ExcelError::new_div()),
                LiteralValue::Int(2),
            ]]))
        };
        for options in [4, 5] {
            let out = dispatch(
                &ctx,
                "AGGREGATE",
                &[
                    lit(LiteralValue::Int(9)),
                    lit(LiteralValue::Int(options)),
                    with_error(),
                ],
            );
            assert_error_kind(out, ExcelErrorKind::Div);
        }
        for options in [6, 7] {
            let out = dispatch(
                &ctx,
                "AGGREGATE",
                &[
                    lit(LiteralValue::Int(9)),
                    lit(LiteralValue::Int(options)),
                    with_error(),
                ],
            );
            assert_num_close(out, 3.0);
        }
        // Array form with errors ignored: AGGREGATE(14,6,{…},1) = 9.
        let out = dispatch(
            &ctx,
            "AGGREGATE",
            &[
                lit(LiteralValue::Int(14)),
                lit(LiteralValue::Int(6)),
                lit(LiteralValue::Array(vec![vec![
                    LiteralValue::Int(9),
                    LiteralValue::Error(ExcelError::new_na()),
                    LiteralValue::Int(2),
                ]])),
                lit(LiteralValue::Int(1)),
            ],
        );
        assert_num_close(out, 9.0);
    }

    #[test]
    fn nested_aggregate_formula_detection() {
        assert!(formula_is_nested_aggregate("=SUBTOTAL(9,A1:A3)"));
        assert!(formula_is_nested_aggregate("= subtotal ( 109 , A1 )"));
        assert!(formula_is_nested_aggregate("AGGREGATE(9,6,A1:A3)"));
        assert!(!formula_is_nested_aggregate("=SUM(A1:A3)"));
        assert!(!formula_is_nested_aggregate("=SUBTOTALS"));
        assert!(!formula_is_nested_aggregate("=1+SUBTOTAL(9,A1)"));
    }
}

pub fn register_builtins() {
    crate::function_registry::register_builtin(std::sync::Arc::new(SumProductFn));
    crate::function_registry::register_builtin(std::sync::Arc::new(SumFn));
    crate::function_registry::register_builtin(std::sync::Arc::new(CountFn));
    crate::function_registry::register_builtin(std::sync::Arc::new(AverageFn));
    crate::function_registry::register_builtin(std::sync::Arc::new(SubtotalFn));
    crate::function_registry::register_builtin(std::sync::Arc::new(AggregateFn));
}
