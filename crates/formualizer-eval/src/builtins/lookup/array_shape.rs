//! Dynamic array shape helpers: TOCOL, TOROW, WRAPROWS, WRAPCOLS and EXPAND.

use super::super::utils::collapse_if_scalar;
use crate::args::{ArgSchema, CoercionPolicy, ShapeKind};
use crate::function::Function;
use crate::traits::{ArgumentHandle, FunctionContext};
use formualizer_common::{ArgKind, ExcelError, ExcelErrorKind, LiteralValue};
use formualizer_macros::func_caps;

/// Converts an array or range into a single column.
///
/// Flattens input values into one column, with options to ignore blanks/errors
/// and to scan by row or by column.
///
/// # Remarks
/// - `ignore` values are `0` keep all, `1` ignore blanks, `2` ignore errors, `3` ignore both.
/// - `scan_by_column` defaults to FALSE, so values are read row by row.
///
/// ```yaml,sandbox
/// title: "Flatten row by row"
/// formula: "=TOCOL({1,2;3,4})"
/// expected: [[1],[2],[3],[4]]
/// ```
///
/// ```yaml,sandbox
/// title: "Scan by column"
/// formula: "=TOCOL({1,2;3,4},0,TRUE)"
/// expected: [[1],[3],[2],[4]]
/// ```
///
/// ```yaml,docs
/// related:
///   - TOROW
///   - HSTACK
///   - VSTACK
/// faq:
///   - q: "Can TOCOL filter blanks and errors?"
///     a: "Yes. Use the ignore argument to drop blanks, errors, or both."
/// ```
#[derive(Debug)]
pub struct ToColFn;

/// Converts an array or range into a single row.
///
/// Flattens input values into one row, with options to ignore blanks/errors and
/// to scan by row or by column.
///
/// # Remarks
/// - `ignore` values are `0` keep all, `1` ignore blanks, `2` ignore errors, `3` ignore both.
/// - `scan_by_column` defaults to FALSE, so values are read row by row.
///
/// ```yaml,sandbox
/// title: "Flatten to row"
/// formula: "=TOROW({1,2;3,4})"
/// expected: [[1,2,3,4]]
/// ```
///
/// ```yaml,sandbox
/// title: "Scan by column"
/// formula: "=TOROW({1,2;3,4},0,TRUE)"
/// expected: [[1,3,2,4]]
/// ```
///
/// ```yaml,docs
/// related:
///   - TOCOL
///   - HSTACK
///   - VSTACK
/// faq:
///   - q: "Does TOROW preserve row order by default?"
///     a: "Yes. The default scan order is row-major."
/// ```
#[derive(Debug)]
pub struct ToRowFn;

fn schema() -> &'static [ArgSchema] {
    use once_cell::sync::Lazy;
    static SCHEMA: Lazy<Vec<ArgSchema>> = Lazy::new(|| {
        vec![
            ArgSchema {
                kinds: smallvec::smallvec![ArgKind::Range, ArgKind::Any],
                required: true,
                by_ref: false,
                shape: ShapeKind::Range,
                coercion: CoercionPolicy::None,
                max: None,
                repeating: None,
                default: None,
            },
            ArgSchema {
                kinds: smallvec::smallvec![ArgKind::Number],
                required: false,
                by_ref: false,
                shape: ShapeKind::Scalar,
                coercion: CoercionPolicy::NumberLenientText,
                max: None,
                repeating: None,
                default: Some(LiteralValue::Int(0)),
            },
            ArgSchema {
                kinds: smallvec::smallvec![ArgKind::Logical, ArgKind::Number],
                required: false,
                by_ref: false,
                shape: ShapeKind::Scalar,
                coercion: CoercionPolicy::None,
                max: None,
                repeating: None,
                default: Some(LiteralValue::Boolean(false)),
            },
        ]
    });
    &SCHEMA
}

fn materialize_arg<'b>(arg: &ArgumentHandle<'_, 'b>) -> Result<Vec<Vec<LiteralValue>>, ExcelError> {
    if let Ok(view) = arg.range_view() {
        let mut rows = Vec::new();
        view.for_each_row(&mut |row| {
            rows.push(row.to_vec());
            Ok(())
        })?;
        return Ok(rows);
    }

    Ok(match arg.value()?.into_literal() {
        LiteralValue::Array(rows) => rows,
        v => vec![vec![v]],
    })
}

fn ignore_mode<'b>(args: &[ArgumentHandle<'_, 'b>]) -> Result<i64, ExcelError> {
    if args.len() < 2 {
        return Ok(0);
    }
    let raw = args[1].value()?.into_literal();
    let n = match raw {
        LiteralValue::Int(i) => i,
        LiteralValue::Number(n) => n as i64,
        LiteralValue::Error(e) => return Err(e),
        other => crate::coercion::to_number_lenient(&other)? as i64,
    };
    if !(0..=3).contains(&n) {
        return Err(
            ExcelError::new(ExcelErrorKind::Value).with_message("ignore must be 0, 1, 2, or 3")
        );
    }
    Ok(n)
}

fn scan_by_column<'b>(args: &[ArgumentHandle<'_, 'b>]) -> Result<bool, ExcelError> {
    if args.len() < 3 {
        return Ok(false);
    }
    crate::coercion::to_logical(&args[2].value()?.into_literal())
}

fn include_cell(v: &LiteralValue, ignore: i64) -> bool {
    let is_blank = matches!(v, LiteralValue::Empty);
    let is_error = matches!(v, LiteralValue::Error(_));
    match ignore {
        1 => !is_blank,
        2 => !is_error,
        3 => !is_blank && !is_error,
        _ => true,
    }
}

fn flatten_array<'b>(args: &[ArgumentHandle<'_, 'b>]) -> Result<Vec<LiteralValue>, ExcelError> {
    if args.is_empty() || args.len() > 3 {
        return Err(ExcelError::new(ExcelErrorKind::Value));
    }
    let data = materialize_arg(&args[0])?;
    let ignore = ignore_mode(args)?;
    let scan_by_col = scan_by_column(args)?;

    let rows = data.len();
    let cols = data.iter().map(Vec::len).max().unwrap_or(0);
    let mut flat = Vec::with_capacity(rows.saturating_mul(cols));

    if scan_by_col {
        for c in 0..cols {
            for row in &data {
                let v = row.get(c).cloned().unwrap_or(LiteralValue::Empty);
                if include_cell(&v, ignore) {
                    flat.push(v);
                }
            }
        }
    } else {
        for row in &data {
            for c in 0..cols {
                let v = row.get(c).cloned().unwrap_or(LiteralValue::Empty);
                if include_cell(&v, ignore) {
                    flat.push(v);
                }
            }
        }
    }

    if flat.is_empty() {
        return Err(ExcelError::new(ExcelErrorKind::Calc)
            .with_message("TOCOL/TOROW returned an empty array"));
    }
    Ok(flat)
}

/// [formualizer-docgen:schema:start]
/// Name: TOCOL
/// Type: ToColFn
/// Min args: 1
/// Max args: variadic
/// Variadic: true
/// Signature: TOCOL(arg1: range|any@range, arg2?: number@scalar, arg3?...: logical|number@scalar)
/// Arg schema: arg1{kinds=range|any,required=true,shape=range,by_ref=false,coercion=None,max=None,repeating=None,default=false}; arg2{kinds=number,required=false,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=true}; arg3{kinds=logical|number,required=false,shape=scalar,by_ref=false,coercion=None,max=None,repeating=None,default=true}
/// Caps: PURE
/// [formualizer-docgen:schema:end]
impl Function for ToColFn {
    func_caps!(PURE, MAY_SPILL);
    fn name(&self) -> &'static str {
        "TOCOL"
    }
    fn min_args(&self) -> usize {
        1
    }
    fn variadic(&self) -> bool {
        true
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        schema()
    }
    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        ctx: &dyn FunctionContext<'b>,
    ) -> Result<crate::traits::CalcValue<'b>, ExcelError> {
        match flatten_array(args) {
            Ok(flat) => Ok(collapse_if_scalar(
                flat.into_iter().map(|v| vec![v]).collect(),
                ctx.date_system(),
            )),
            Err(e) => Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(e))),
        }
    }
}

/// [formualizer-docgen:schema:start]
/// Name: TOROW
/// Type: ToRowFn
/// Min args: 1
/// Max args: variadic
/// Variadic: true
/// Signature: TOROW(arg1: range|any@range, arg2?: number@scalar, arg3?...: logical|number@scalar)
/// Arg schema: arg1{kinds=range|any,required=true,shape=range,by_ref=false,coercion=None,max=None,repeating=None,default=false}; arg2{kinds=number,required=false,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=true}; arg3{kinds=logical|number,required=false,shape=scalar,by_ref=false,coercion=None,max=None,repeating=None,default=true}
/// Caps: PURE
/// [formualizer-docgen:schema:end]
impl Function for ToRowFn {
    func_caps!(PURE, MAY_SPILL);
    fn name(&self) -> &'static str {
        "TOROW"
    }
    fn min_args(&self) -> usize {
        1
    }
    fn variadic(&self) -> bool {
        true
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        schema()
    }
    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        ctx: &dyn FunctionContext<'b>,
    ) -> Result<crate::traits::CalcValue<'b>, ExcelError> {
        match flatten_array(args) {
            Ok(flat) => Ok(collapse_if_scalar(vec![flat], ctx.date_system())),
            Err(e) => Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(e))),
        }
    }
}

/* ───────────────────────── WRAPROWS / WRAPCOLS / EXPAND ───────────────────────── */

#[derive(Debug)]
pub struct WrapRowsFn;
#[derive(Debug)]
pub struct WrapColsFn;
#[derive(Debug)]
pub struct ExpandFn;

fn wrap_schema() -> &'static [ArgSchema] {
    use once_cell::sync::Lazy;
    static SCHEMA: Lazy<Vec<ArgSchema>> = Lazy::new(|| {
        vec![
            // vector
            ArgSchema {
                kinds: smallvec::smallvec![ArgKind::Range, ArgKind::Any],
                required: true,
                by_ref: false,
                shape: ShapeKind::Range,
                coercion: CoercionPolicy::None,
                max: None,
                repeating: None,
                default: None,
            },
            // wrap_count
            ArgSchema {
                kinds: smallvec::smallvec![ArgKind::Number],
                required: true,
                by_ref: false,
                shape: ShapeKind::Scalar,
                coercion: CoercionPolicy::NumberLenientText,
                max: None,
                repeating: None,
                default: None,
            },
            // pad_with
            ArgSchema {
                kinds: smallvec::smallvec![ArgKind::Any],
                required: false,
                by_ref: false,
                shape: ShapeKind::Scalar,
                coercion: CoercionPolicy::None,
                max: None,
                repeating: None,
                default: None,
            },
        ]
    });
    &SCHEMA
}

fn expand_schema() -> &'static [ArgSchema] {
    use once_cell::sync::Lazy;
    static SCHEMA: Lazy<Vec<ArgSchema>> = Lazy::new(|| {
        vec![
            // array
            ArgSchema {
                kinds: smallvec::smallvec![ArgKind::Range, ArgKind::Any],
                required: true,
                by_ref: false,
                shape: ShapeKind::Range,
                coercion: CoercionPolicy::None,
                max: None,
                repeating: None,
                default: None,
            },
            // rows
            ArgSchema {
                kinds: smallvec::smallvec![ArgKind::Number, ArgKind::Any],
                required: true,
                by_ref: false,
                shape: ShapeKind::Scalar,
                coercion: CoercionPolicy::None,
                max: None,
                repeating: None,
                default: None,
            },
            // columns
            ArgSchema {
                kinds: smallvec::smallvec![ArgKind::Number, ArgKind::Any],
                required: false,
                by_ref: false,
                shape: ShapeKind::Scalar,
                coercion: CoercionPolicy::None,
                max: None,
                repeating: None,
                default: None,
            },
            // pad_with
            ArgSchema {
                kinds: smallvec::smallvec![ArgKind::Any],
                required: false,
                by_ref: false,
                shape: ShapeKind::Scalar,
                coercion: CoercionPolicy::None,
                max: None,
                repeating: None,
                default: None,
            },
        ]
    });
    &SCHEMA
}

/// Excel pads WRAPROWS/WRAPCOLS/EXPAND/VSTACK/HSTACK with `#N/A` unless `pad_with` is given.
pub(crate) fn na_pad() -> LiteralValue {
    LiteralValue::Error(ExcelError::new(ExcelErrorKind::Na))
}

/// `pad_with` argument: omitted or blank → `#N/A`; an error is an error value, not a failure.
fn pad_value<'b>(args: &[ArgumentHandle<'_, 'b>], idx: usize) -> Result<LiteralValue, ExcelError> {
    if args.len() <= idx {
        return Ok(na_pad());
    }
    Ok(match args[idx].value()?.into_literal() {
        LiteralValue::Empty => na_pad(),
        v => v,
    })
}

/// Optional whole-number argument; `None` when omitted or blank.
fn opt_count<'b>(args: &[ArgumentHandle<'_, 'b>], idx: usize) -> Result<Option<i64>, ExcelError> {
    if args.len() <= idx {
        return Ok(None);
    }
    Ok(match args[idx].value()?.into_literal() {
        LiteralValue::Empty => None,
        LiteralValue::Int(i) => Some(i),
        LiteralValue::Number(n) => Some(n.trunc() as i64),
        LiteralValue::Error(e) => return Err(e),
        other => Some(crate::coercion::to_number_lenient(&other)?.trunc() as i64),
    })
}

/// The one-dimensional input WRAPROWS/WRAPCOLS accept; a 2-D array is `#VALUE!` as in Excel.
fn wrap_vector<'b>(arg: &ArgumentHandle<'_, 'b>) -> Result<Vec<LiteralValue>, ExcelError> {
    let data = materialize_arg(arg)?;
    let rows = data.len();
    let cols = data.iter().map(Vec::len).max().unwrap_or(0);
    if rows > 1 && cols > 1 {
        return Err(ExcelError::new(ExcelErrorKind::Value)
            .with_message("WRAPROWS/WRAPCOLS require a one-dimensional vector"));
    }
    Ok(data.into_iter().flatten().collect())
}

fn wrap_count<'b>(arg: &ArgumentHandle<'_, 'b>) -> Result<usize, ExcelError> {
    match opt_count(std::slice::from_ref(arg), 0)? {
        Some(n) if n >= 1 => Ok(n as usize),
        _ => {
            Err(ExcelError::new(ExcelErrorKind::Num).with_message("wrap_count must be at least 1"))
        }
    }
}

/// Lays a vector out row by row, `wrap_count` values per row.
///
/// # Remarks
/// - The last row is padded with `pad_with` (default `#N/A`) so the result stays rectangular.
/// - `wrap_count < 1` returns `#NUM!`; a two-dimensional `vector` returns `#VALUE!`.
///
/// ```yaml,sandbox
/// title: "Wrap five values into rows of two"
/// formula: "=WRAPROWS({1,2,3,4,5},2)"
/// expected: [[1,2],[3,4],[5,"#N/A"]]
/// ```
///
/// ```yaml,sandbox
/// title: "Custom pad value"
/// formula: "=WRAPROWS({1,2,3,4,5},2,0)"
/// expected: [[1,2],[3,4],[5,0]]
/// ```
/// [formualizer-docgen:schema:start]
/// Name: WRAPROWS
/// Type: WrapRowsFn
/// Min args: 2
/// Max args: 3
/// Variadic: false
/// Signature: WRAPROWS(arg1: range|any@range, arg2: number@scalar, arg3?: any@scalar)
/// Arg schema: arg1{kinds=range|any,required=true,shape=range,by_ref=false,coercion=None,max=None,repeating=None,default=false}; arg2{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg3{kinds=any,required=false,shape=scalar,by_ref=false,coercion=None,max=None,repeating=None,default=false}
/// Caps: PURE
/// [formualizer-docgen:schema:end]
impl Function for WrapRowsFn {
    func_caps!(PURE, MAY_SPILL);
    fn name(&self) -> &'static str {
        "WRAPROWS"
    }
    fn min_args(&self) -> usize {
        2
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        wrap_schema()
    }
    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        ctx: &dyn FunctionContext<'b>,
    ) -> Result<crate::traits::CalcValue<'b>, ExcelError> {
        let run = || -> Result<Vec<Vec<LiteralValue>>, ExcelError> {
            let flat = wrap_vector(&args[0])?;
            let width = wrap_count(&args[1])?;
            let pad = pad_value(args, 2)?;
            let mut out: Vec<Vec<LiteralValue>> = Vec::new();
            for chunk in flat.chunks(width) {
                let mut row = chunk.to_vec();
                row.resize(width, pad.clone());
                out.push(row);
            }
            if out.is_empty() {
                out.push(vec![pad]);
            }
            Ok(out)
        };
        match run() {
            Ok(rows) => Ok(collapse_if_scalar(rows, ctx.date_system())),
            Err(e) => Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(e))),
        }
    }
}

/// Lays a vector out column by column, `wrap_count` values per column.
///
/// # Remarks
/// - The last column is padded with `pad_with` (default `#N/A`).
/// - `wrap_count < 1` returns `#NUM!`; a two-dimensional `vector` returns `#VALUE!`.
///
/// ```yaml,sandbox
/// title: "Wrap five values into columns of two"
/// formula: "=WRAPCOLS({1,2,3,4,5},2)"
/// expected: [[1,3,5],[2,4,"#N/A"]]
/// ```
/// [formualizer-docgen:schema:start]
/// Name: WRAPCOLS
/// Type: WrapColsFn
/// Min args: 2
/// Max args: 3
/// Variadic: false
/// Signature: WRAPCOLS(arg1: range|any@range, arg2: number@scalar, arg3?: any@scalar)
/// Arg schema: arg1{kinds=range|any,required=true,shape=range,by_ref=false,coercion=None,max=None,repeating=None,default=false}; arg2{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg3{kinds=any,required=false,shape=scalar,by_ref=false,coercion=None,max=None,repeating=None,default=false}
/// Caps: PURE
/// [formualizer-docgen:schema:end]
impl Function for WrapColsFn {
    func_caps!(PURE, MAY_SPILL);
    fn name(&self) -> &'static str {
        "WRAPCOLS"
    }
    fn min_args(&self) -> usize {
        2
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        wrap_schema()
    }
    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        ctx: &dyn FunctionContext<'b>,
    ) -> Result<crate::traits::CalcValue<'b>, ExcelError> {
        let run = || -> Result<Vec<Vec<LiteralValue>>, ExcelError> {
            let flat = wrap_vector(&args[0])?;
            let height = wrap_count(&args[1])?;
            let pad = pad_value(args, 2)?;
            if flat.is_empty() {
                return Ok(vec![vec![pad]]);
            }
            let cols = flat.len().div_ceil(height);
            let mut out = vec![vec![pad; cols]; height];
            for (i, v) in flat.into_iter().enumerate() {
                out[i % height][i / height] = v;
            }
            Ok(out)
        };
        match run() {
            Ok(rows) => Ok(collapse_if_scalar(rows, ctx.date_system())),
            Err(e) => Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(e))),
        }
    }
}

/// Expands an array to the given row and column counts, padding new cells.
///
/// # Remarks
/// - Omitted or blank `rows` / `columns` keep the current size.
/// - Sizes smaller than the source return `#VALUE!`.
/// - New cells hold `pad_with` (default `#N/A`).
///
/// ```yaml,sandbox
/// title: "Expand a 2x2 array to 3x3 with zeros"
/// formula: "=EXPAND({1,4;2,5},3,3,0)"
/// expected: [[1,4,0],[2,5,0],[0,0,0]]
/// ```
/// [formualizer-docgen:schema:start]
/// Name: EXPAND
/// Type: ExpandFn
/// Min args: 2
/// Max args: 4
/// Variadic: false
/// Signature: EXPAND(arg1: range|any@range, arg2: number|any@scalar, arg3?: number|any@scalar, arg4?: any@scalar)
/// Arg schema: arg1{kinds=range|any,required=true,shape=range,by_ref=false,coercion=None,max=None,repeating=None,default=false}; arg2{kinds=number|any,required=true,shape=scalar,by_ref=false,coercion=None,max=None,repeating=None,default=false}; arg3{kinds=number|any,required=false,shape=scalar,by_ref=false,coercion=None,max=None,repeating=None,default=false}; arg4{kinds=any,required=false,shape=scalar,by_ref=false,coercion=None,max=None,repeating=None,default=false}
/// Caps: PURE
/// [formualizer-docgen:schema:end]
impl Function for ExpandFn {
    func_caps!(PURE, MAY_SPILL);
    fn name(&self) -> &'static str {
        "EXPAND"
    }
    fn min_args(&self) -> usize {
        2
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        expand_schema()
    }
    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        ctx: &dyn FunctionContext<'b>,
    ) -> Result<crate::traits::CalcValue<'b>, ExcelError> {
        let run = || -> Result<Vec<Vec<LiteralValue>>, ExcelError> {
            let data = materialize_arg(&args[0])?;
            let src_rows = data.len().max(1);
            let src_cols = data.iter().map(Vec::len).max().unwrap_or(0).max(1);
            let rows = opt_count(args, 1)?.map_or(src_rows, |n| n.max(0) as usize);
            let cols = opt_count(args, 2)?.map_or(src_cols, |n| n.max(0) as usize);
            if rows < src_rows || cols < src_cols {
                return Err(ExcelError::new(ExcelErrorKind::Value)
                    .with_message("EXPAND cannot shrink an array"));
            }
            let pad = pad_value(args, 3)?;
            let mut out = Vec::with_capacity(rows);
            for r in 0..rows {
                let mut row: Vec<LiteralValue> = data.get(r).cloned().unwrap_or_default();
                row.resize(cols, pad.clone());
                out.push(row);
            }
            Ok(out)
        };
        match run() {
            Ok(rows) => Ok(collapse_if_scalar(rows, ctx.date_system())),
            Err(e) => Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(e))),
        }
    }
}

pub fn register_builtins() {
    use crate::function_registry::register_builtin;
    use std::sync::Arc;

    register_builtin(Arc::new(ToColFn));
    register_builtin(Arc::new(ToRowFn));
    register_builtin(Arc::new(WrapRowsFn));
    register_builtin(Arc::new(WrapColsFn));
    register_builtin(Arc::new(ExpandFn));
}

#[cfg(test)]
mod tests {
    use crate::builtins::logical::{FalseFn, TrueFn};
    use crate::test_workbook::TestWorkbook;
    use formualizer_common::{ExcelError, ExcelErrorKind, LiteralValue};
    use formualizer_parse::parser::parse;
    use std::sync::Arc;

    fn eval(formula: &str) -> LiteralValue {
        let wb = TestWorkbook::new()
            .with_function(Arc::new(super::ToColFn))
            .with_function(Arc::new(super::ToRowFn))
            .with_function(Arc::new(TrueFn))
            .with_function(Arc::new(FalseFn));
        let interp = wb.interpreter();
        let ast = parse(formula).expect("parse");
        interp.evaluate_ast(&ast).expect("eval").into_literal()
    }

    #[test]
    fn tocol_flattens_rows_by_default() {
        assert_eq!(
            eval("=TOCOL({1,2;3,4})"),
            LiteralValue::Array(vec![
                vec![LiteralValue::Number(1.0)],
                vec![LiteralValue::Number(2.0)],
                vec![LiteralValue::Number(3.0)],
                vec![LiteralValue::Number(4.0)],
            ])
        );
    }

    #[test]
    fn torow_can_scan_by_column() {
        assert_eq!(
            eval("=TOROW({1,2;3,4},0,TRUE)"),
            LiteralValue::Array(vec![vec![
                LiteralValue::Number(1.0),
                LiteralValue::Number(3.0),
                LiteralValue::Number(2.0),
                LiteralValue::Number(4.0),
            ]])
        );
    }

    #[test]
    fn ignores_blanks_and_errors() {
        let value = eval("=TOROW({1,#N/A;\"\",2},2,FALSE)");
        assert_eq!(
            value,
            LiteralValue::Array(vec![vec![
                LiteralValue::Number(1.0),
                LiteralValue::Text(String::new()),
                LiteralValue::Number(2.0),
            ]])
        );

        let value = eval("=TOROW({#N/A},2)");
        assert!(matches!(value, LiteralValue::Error(e) if e.kind == ExcelErrorKind::Calc));
    }

    #[test]
    fn rejects_invalid_ignore_mode() {
        let value = eval("=TOCOL({1,2},4)");
        assert!(matches!(
            value,
            LiteralValue::Error(ExcelError {
                kind: ExcelErrorKind::Value,
                ..
            })
        ));
    }
}
