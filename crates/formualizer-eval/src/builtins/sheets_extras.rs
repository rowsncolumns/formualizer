//! Google Sheets functions that have no Excel counterpart: `SPLIT`, `JOIN`, `REGEXMATCH`,
//! `SORTN`, `ARRAYFORMULA` and the `QUERY` stub.
//!
//! The legacy JS engine (`@rowsncolumns/fast-formula-parser`) has always carried these as Sheets
//! extras, so workbooks authored on it use them freely. They are registered here with that
//! engine's semantics so the same workbook keeps evaluating when it is opened on this engine:
//!
//! - `SPLIT` returns its pieces as TEXT (no number coercion), splits on every delimiter character
//!   by default and drops empty pieces by default;
//! - `JOIN` flattens ranges / arrays, skips blanks and empty text, and spells values the way
//!   `TEXTJOIN` does (`TRUE`, 15-significant-digit numbers); an error argument propagates;
//! - `REGEXMATCH` is a case-sensitive whole-text search (`REGEXTEST` without the flag); an
//!   invalid pattern is `#VALUE!`;
//! - `SORTN` is `SORT` on one key column followed by `TAKE` of the first `n` rows — the
//!   `display_ties_mode` argument is accepted and ignored, exactly like the JS engine;
//! - `ARRAYFORMULA` is the identity over its argument (Sheets uses it as an array-evaluation
//!   marker; both engines evaluate arrays natively);
//! - `QUERY` needs a query-language interpreter neither engine has and yields `#N/A` (catchable
//!   with `IFNA`), never `#NAME?`.
//!
//! Excel itself has none of these names (it shows `#NAME?`), so exporters write them bare.

use crate::args::{ArgSchema, ShapeKind};
use crate::function::Function;
use crate::traits::{ArgumentHandle, CalcValue, FunctionContext};
use formualizer_common::{ArgKind, CoercionPolicy, ExcelError, ExcelErrorKind, LiteralValue};
use formualizer_macros::func_caps;
use regex::Regex;
use std::sync::{Arc, LazyLock};

use super::lookup::lookup_utils::cmp_for_lookup;
use super::utils::{ARG_ANY_ONE, collapse_if_scalar};

fn scalar_like_value(arg: &ArgumentHandle<'_, '_>) -> Result<LiteralValue, ExcelError> {
    Ok(match arg.value()? {
        CalcValue::Scalar(LiteralValue::Array(rows)) => rows
            .into_iter()
            .flatten()
            .next()
            .unwrap_or(LiteralValue::Empty),
        CalcValue::Scalar(v) => v,
        CalcValue::Range(rv) => rv.get_cell(0, 0),
        CalcValue::Callable(_) => LiteralValue::Error(
            ExcelError::new(ExcelErrorKind::Calc).with_message("LAMBDA value must be invoked"),
        ),
    })
}

/// Excel's text form of one scalar; an error propagates.
fn literal_text(v: LiteralValue) -> Result<String, ExcelError> {
    Ok(match v {
        LiteralValue::Text(s) => s,
        LiteralValue::Empty => String::new(),
        LiteralValue::Boolean(b) => if b { "TRUE" } else { "FALSE" }.to_string(),
        LiteralValue::Int(i) => i.to_string(),
        LiteralValue::Number(n) => formualizer_common::number_to_excel_text(n),
        LiteralValue::Error(e) => return Err(e),
        other => other.to_string(),
    })
}

fn arg_text(arg: &ArgumentHandle<'_, '_>) -> Result<String, ExcelError> {
    literal_text(scalar_like_value(arg)?)
}

/// Optional logical argument; a skipped / omitted / blank slot yields `default`.
fn arg_bool(
    args: &[ArgumentHandle<'_, '_>],
    idx: usize,
    default: bool,
) -> Result<bool, ExcelError> {
    match args.get(idx) {
        None => Ok(default),
        Some(a) if a.is_skipped() => Ok(default),
        Some(a) => match scalar_like_value(a)? {
            LiteralValue::Empty => Ok(default),
            LiteralValue::Boolean(b) => Ok(b),
            LiteralValue::Int(i) => Ok(i != 0),
            LiteralValue::Number(n) => Ok(n != 0.0),
            LiteralValue::Text(t) => match t.to_ascii_uppercase().as_str() {
                "TRUE" => Ok(true),
                "FALSE" => Ok(false),
                _ => Err(ExcelError::new(ExcelErrorKind::Value)),
            },
            LiteralValue::Error(e) => Err(e),
            _ => Err(ExcelError::new(ExcelErrorKind::Value)),
        },
    }
}

/// Optional integer argument; a skipped / omitted / blank slot yields `default`.
fn arg_int(args: &[ArgumentHandle<'_, '_>], idx: usize, default: i64) -> Result<i64, ExcelError> {
    match args.get(idx) {
        None => Ok(default),
        Some(a) if a.is_skipped() => Ok(default),
        Some(a) => match scalar_like_value(a)? {
            LiteralValue::Empty => Ok(default),
            LiteralValue::Int(i) => Ok(i),
            LiteralValue::Number(n) => Ok(n.trunc() as i64),
            LiteralValue::Boolean(b) => Ok(b as i64),
            LiteralValue::Text(t) => t
                .trim()
                .parse::<f64>()
                .map(|n| n.trunc() as i64)
                .map_err(|_| ExcelError::new(ExcelErrorKind::Value)),
            LiteralValue::Error(e) => Err(e),
            _ => Err(ExcelError::new(ExcelErrorKind::Value)),
        },
    }
}

/// Every value an argument contributes, flattening ranges and arrays in row-major order.
fn flattened_values(arg: &ArgumentHandle<'_, '_>) -> Result<Vec<LiteralValue>, ExcelError> {
    Ok(match arg.value()? {
        CalcValue::Scalar(LiteralValue::Array(rows)) => rows.into_iter().flatten().collect(),
        CalcValue::Scalar(v) => vec![v],
        CalcValue::Range(rv) => {
            let mut out = Vec::new();
            rv.for_each_cell(&mut |v| {
                out.push(v.clone());
                Ok(())
            })?;
            out
        }
        CalcValue::Callable(_) => vec![LiteralValue::Error(
            ExcelError::new(ExcelErrorKind::Calc).with_message("LAMBDA value must be invoked"),
        )],
    })
}

fn scalar_arg(kinds: &[ArgKind], required: bool, default: Option<LiteralValue>) -> ArgSchema {
    ArgSchema {
        kinds: kinds.iter().copied().collect(),
        required,
        by_ref: false,
        shape: ShapeKind::Scalar,
        coercion: CoercionPolicy::None,
        max: None,
        repeating: None,
        default,
    }
}

fn range_arg() -> ArgSchema {
    ArgSchema {
        kinds: smallvec::smallvec![ArgKind::Range, ArgKind::Any],
        required: true,
        by_ref: true,
        shape: ShapeKind::Range,
        coercion: CoercionPolicy::None,
        max: None,
        repeating: None,
        default: None,
    }
}

fn value_err<'c>() -> CalcValue<'c> {
    CalcValue::Scalar(LiteralValue::Error(ExcelError::new(ExcelErrorKind::Value)))
}

/* ───────────────────────── SPLIT() ───────────────────────── */

/// Splits text around a delimiter into a row of text pieces (Google Sheets).
///
/// `SPLIT(text, delimiter, [split_by_each], [remove_empty_text])`
///
/// # Remarks
/// - `split_by_each` (default TRUE) treats every character of `delimiter` as its own separator;
///   FALSE splits on the whole `delimiter` string.
/// - `remove_empty_text` (default TRUE) drops the empty pieces adjacent delimiters produce.
/// - Pieces stay TEXT (no number coercion), matching the legacy JS engine.
/// - An empty `delimiter` is `#VALUE!`.
///
/// ```yaml,sandbox
/// title: "Split on each delimiter character"
/// formula: '=SPLIT("a,b;c", ",;")'
/// expected: [["a","b","c"]]
/// ```
#[derive(Debug)]
pub struct SplitFn;

impl Function for SplitFn {
    func_caps!(PURE, MAY_SPILL);
    fn name(&self) -> &'static str {
        "SPLIT"
    }
    fn min_args(&self) -> usize {
        2
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        static SCHEMA: LazyLock<Vec<ArgSchema>> = LazyLock::new(|| {
            vec![
                scalar_arg(&[ArgKind::Any], true, None),
                scalar_arg(&[ArgKind::Any], true, None),
                scalar_arg(
                    &[ArgKind::Logical],
                    false,
                    Some(LiteralValue::Boolean(true)),
                ),
                scalar_arg(
                    &[ArgKind::Logical],
                    false,
                    Some(LiteralValue::Boolean(true)),
                ),
            ]
        });
        &SCHEMA[..]
    }
    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        ctx: &dyn FunctionContext<'b>,
    ) -> Result<CalcValue<'b>, ExcelError> {
        let text = arg_text(&args[0])?;
        let delimiter = arg_text(&args[1])?;
        let split_by_each = arg_bool(args, 2, true)?;
        let remove_empty = arg_bool(args, 3, true)?;
        if delimiter.is_empty() {
            return Ok(value_err());
        }
        let pieces: Vec<&str> = if split_by_each {
            text.split(|c: char| delimiter.contains(c)).collect()
        } else {
            text.split(delimiter.as_str()).collect()
        };
        let row: Vec<LiteralValue> = pieces
            .into_iter()
            .filter(|p| !(remove_empty && p.is_empty()))
            .map(|p| LiteralValue::Text(p.to_string()))
            .collect();
        if row.is_empty() {
            return Ok(CalcValue::Scalar(LiteralValue::Text(String::new())));
        }
        Ok(collapse_if_scalar(vec![row], ctx.date_system()))
    }
}

/* ───────────────────────── JOIN() ───────────────────────── */

/// Concatenates the elements of one or more values / arrays with a delimiter (Google Sheets).
///
/// `JOIN(delimiter, value_or_array1, [value_or_array2, …])`
///
/// # Remarks
/// - Ranges and array constants contribute every element in row-major order.
/// - Blank cells and empty text are skipped; other values spell as `TEXTJOIN` would (`TRUE`,
///   15-significant-digit numbers).
/// - An error among the values propagates.
///
/// ```yaml,sandbox
/// title: "Join a column"
/// grid:
///   A1: 3
///   A2: 1
///   A3: 2
/// formula: '=JOIN(",", A1:A3)'
/// expected: "3,1,2"
/// ```
#[derive(Debug)]
pub struct JoinFn;

impl Function for JoinFn {
    func_caps!(PURE);
    fn name(&self) -> &'static str {
        "JOIN"
    }
    fn min_args(&self) -> usize {
        2
    }
    fn variadic(&self) -> bool {
        true
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        &ARG_ANY_ONE[..]
    }
    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        _ctx: &dyn FunctionContext<'b>,
    ) -> Result<CalcValue<'b>, ExcelError> {
        let delimiter = arg_text(&args[0])?;
        let mut parts: Vec<String> = Vec::new();
        for arg in &args[1..] {
            if arg.is_skipped() {
                continue;
            }
            for v in flattened_values(arg)? {
                match v {
                    LiteralValue::Empty => {}
                    LiteralValue::Text(t) if t.is_empty() => {}
                    other => parts.push(literal_text(other)?),
                }
            }
        }
        Ok(CalcValue::Scalar(LiteralValue::Text(
            parts.join(&delimiter),
        )))
    }
}

/* ───────────────────────── REGEXMATCH() ───────────────────────── */

/// Whether a piece of text matches a regular expression (Google Sheets).
///
/// `REGEXMATCH(text, regular_expression)`
///
/// # Remarks
/// - Case-sensitive; the pattern may match anywhere in `text` (anchor with `^` / `$`).
/// - An invalid pattern is `#VALUE!`.
///
/// ```yaml,sandbox
/// title: "Anchored match"
/// formula: '=REGEXMATCH("hello", "^h.*o$")'
/// expected: true
/// ```
#[derive(Debug)]
pub struct RegexMatchFn;

impl Function for RegexMatchFn {
    func_caps!(PURE);
    fn name(&self) -> &'static str {
        "REGEXMATCH"
    }
    fn min_args(&self) -> usize {
        2
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        static SCHEMA: LazyLock<Vec<ArgSchema>> = LazyLock::new(|| {
            vec![
                scalar_arg(&[ArgKind::Any], true, None),
                scalar_arg(&[ArgKind::Any], true, None),
            ]
        });
        &SCHEMA[..]
    }
    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        _ctx: &dyn FunctionContext<'b>,
    ) -> Result<CalcValue<'b>, ExcelError> {
        let text = arg_text(&args[0])?;
        let pattern = arg_text(&args[1])?;
        let Ok(re) = Regex::new(&pattern) else {
            return Ok(value_err());
        };
        Ok(CalcValue::Scalar(LiteralValue::Boolean(re.is_match(&text))))
    }
}

/* ───────────────────────── SORTN() ───────────────────────── */

/// The first `n` rows of a range after sorting it on one column (Google Sheets).
///
/// `SORTN(range, [n], [display_ties_mode], [sort_column], [is_ascending])`
///
/// # Remarks
/// - `n` defaults to every row; a negative `n` is `#VALUE!`.
/// - `display_ties_mode` is accepted and ignored (the legacy JS engine does the same): the
///   result is always exactly the first `n` sorted rows.
/// - `sort_column` (default 1) is 1-based within `range`; out of range is `#VALUE!`.
/// - `is_ascending` defaults to TRUE. The sort is stable, like `SORT`.
///
/// ```yaml,sandbox
/// title: "Two smallest rows"
/// grid:
///   A1: 3
///   B1: "c"
///   A2: 1
///   B2: "a"
///   A3: 2
///   B3: "b"
/// formula: '=SORTN(A1:B3, 2)'
/// expected: [[1,"a"],[2,"b"]]
/// ```
#[derive(Debug)]
pub struct SortNFn;

impl Function for SortNFn {
    func_caps!(PURE, MAY_SPILL);
    fn name(&self) -> &'static str {
        "SORTN"
    }
    fn min_args(&self) -> usize {
        1
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        static SCHEMA: LazyLock<Vec<ArgSchema>> = LazyLock::new(|| {
            vec![
                range_arg(),
                scalar_arg(&[ArgKind::Number, ArgKind::Any], false, None),
                scalar_arg(
                    &[ArgKind::Number, ArgKind::Any],
                    false,
                    Some(LiteralValue::Int(0)),
                ),
                scalar_arg(
                    &[ArgKind::Number, ArgKind::Any],
                    false,
                    Some(LiteralValue::Int(1)),
                ),
                scalar_arg(
                    &[ArgKind::Logical],
                    false,
                    Some(LiteralValue::Boolean(true)),
                ),
            ]
        });
        &SCHEMA[..]
    }
    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        ctx: &dyn FunctionContext<'b>,
    ) -> Result<CalcValue<'b>, ExcelError> {
        let view = match args[0].range_view() {
            Ok(v) => v,
            Err(e) => return Ok(CalcValue::Scalar(LiteralValue::Error(e))),
        };
        let (rows, cols) = view.dims();
        let n = arg_int(args, 1, rows as i64)?;
        // display_ties_mode: validated as a number, otherwise ignored (JS-engine parity).
        let _ties = arg_int(args, 2, 0)?;
        let sort_column = arg_int(args, 3, 1)?;
        let ascending = arg_bool(args, 4, true)?;
        if n < 0 {
            return Ok(value_err());
        }
        if rows == 0 || cols == 0 {
            return Ok(CalcValue::Range(
                crate::engine::range_view::RangeView::from_owned_rows(vec![], ctx.date_system()),
            ));
        }
        if sort_column < 1 || sort_column as usize > cols {
            return Ok(value_err());
        }
        let key = (sort_column - 1) as usize;
        let mut data: Vec<Vec<LiteralValue>> = (0..rows)
            .map(|r| (0..cols).map(|c| view.get_cell(r, c)).collect())
            .collect();
        data.sort_by(|a, b| {
            let cmp = cmp_for_lookup(&a[key], &b[key]).unwrap_or(0);
            if ascending { cmp.cmp(&0) } else { 0.cmp(&cmp) }
        });
        data.truncate(n as usize);
        Ok(collapse_if_scalar(data, ctx.date_system()))
    }
}

/* ───────────────────────── ARRAYFORMULA() ───────────────────────── */

/// Returns its argument unchanged — Sheets' array-evaluation marker (Google Sheets).
///
/// `ARRAYFORMULA(array_formula)`
///
/// # Remarks
/// - Both engines evaluate array expressions natively, so the wrapper is the identity: a range
///   or array argument spills, a scalar stays a scalar.
///
/// ```yaml,sandbox
/// title: "Identity over an array expression"
/// grid:
///   A1: 3
///   A2: 1
///   A3: 2
/// formula: '=ARRAYFORMULA(A1:A3*2)'
/// expected: [[6],[2],[4]]
/// ```
#[derive(Debug)]
pub struct ArrayFormulaFn;

impl Function for ArrayFormulaFn {
    func_caps!(PURE, MAY_SPILL);
    fn name(&self) -> &'static str {
        "ARRAYFORMULA"
    }
    fn min_args(&self) -> usize {
        1
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        static SCHEMA: LazyLock<Vec<ArgSchema>> = LazyLock::new(|| vec![range_arg()]);
        &SCHEMA[..]
    }
    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        _ctx: &dyn FunctionContext<'b>,
    ) -> Result<CalcValue<'b>, ExcelError> {
        args[0].value()
    }
}

/* ───────────────────────── QUERY() ───────────────────────── */

/// Runs a Google Visualization API query over a range (Google Sheets) — not available here.
///
/// `QUERY(data, query, [headers])`
///
/// # Remarks
/// - Needs a query-language interpreter neither engine has; the call is a recognised function
///   that yields `#N/A` (catchable with `IFNA`) rather than `#NAME?`.
///
/// ```yaml,sandbox
/// title: "No query engine"
/// formula: '=QUERY(A1:B3, "select A")'
/// expected: "#N/A"
/// ```
#[derive(Debug)]
pub struct QueryFn;

impl Function for QueryFn {
    func_caps!(PURE);
    fn name(&self) -> &'static str {
        "QUERY"
    }
    fn min_args(&self) -> usize {
        1
    }
    fn variadic(&self) -> bool {
        true
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        &ARG_ANY_ONE[..]
    }
    fn eval<'a, 'b, 'c>(
        &self,
        _args: &'c [ArgumentHandle<'a, 'b>],
        _ctx: &dyn FunctionContext<'b>,
    ) -> Result<CalcValue<'b>, ExcelError> {
        Ok(CalcValue::Scalar(LiteralValue::Error(ExcelError::new_na())))
    }
}

pub fn register_builtins() {
    use crate::function_registry::register_builtin;
    register_builtin(Arc::new(SplitFn));
    register_builtin(Arc::new(JoinFn));
    register_builtin(Arc::new(RegexMatchFn));
    register_builtin(Arc::new(SortNFn));
    register_builtin(Arc::new(ArrayFormulaFn));
    register_builtin(Arc::new(QueryFn));
}
