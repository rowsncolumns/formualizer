//! Excel 2024 regular-expression functions: REGEXTEST, REGEXEXTRACT, REGEXREPLACE.
//!
//! Excel's flavour is PCRE2; the `regex` crate covers the everyday subset (classes, quantifiers,
//! anchors, alternation, capture groups, `(?i)`). Constructs it does not support (look-around,
//! back-references) surface as `#VALUE!`, the same error Excel gives an invalid pattern.

use crate::args::{ArgSchema, ShapeKind};
use crate::function::Function;
use crate::traits::{ArgumentHandle, CalcValue, FunctionContext};
use formualizer_common::{ArgKind, CoercionPolicy, ExcelError, ExcelErrorKind, LiteralValue};
use formualizer_macros::func_caps;
use regex::Regex;
use std::sync::LazyLock;

fn scalar_like_value(arg: &ArgumentHandle<'_, '_>) -> Result<LiteralValue, ExcelError> {
    Ok(match arg.value()? {
        CalcValue::Scalar(v) => v,
        CalcValue::Range(rv) => rv.get_cell(0, 0),
        CalcValue::Callable(_) => LiteralValue::Error(
            ExcelError::new(ExcelErrorKind::Calc).with_message("LAMBDA value must be invoked"),
        ),
    })
}

fn arg_text(arg: &ArgumentHandle<'_, '_>) -> Result<String, ExcelError> {
    Ok(match scalar_like_value(arg)? {
        LiteralValue::Text(s) => s,
        LiteralValue::Empty => String::new(),
        LiteralValue::Boolean(b) => if b { "TRUE" } else { "FALSE" }.to_string(),
        LiteralValue::Int(i) => i.to_string(),
        LiteralValue::Number(n) => {
            let s = n.to_string();
            s.strip_suffix(".0").map(str::to_string).unwrap_or(s)
        }
        LiteralValue::Error(e) => return Err(e),
        other => other.to_string(),
    })
}

/// Optional integer argument; a skipped/omitted slot yields `default`.
fn arg_int(args: &[ArgumentHandle<'_, '_>], idx: usize, default: i64) -> Result<i64, ExcelError> {
    match args.get(idx) {
        None => Ok(default),
        Some(a) if a.is_skipped() => Ok(default),
        Some(a) => match scalar_like_value(a)? {
            LiteralValue::Empty => Ok(default),
            LiteralValue::Int(i) => Ok(i),
            LiteralValue::Number(n) => Ok(n.trunc() as i64),
            LiteralValue::Boolean(b) => Ok(b as i64),
            LiteralValue::Error(e) => Err(e),
            _ => Err(ExcelError::new(ExcelErrorKind::Value)),
        },
    }
}

/// Compile `pattern` with Excel's `case_sensitivity` flag (0 = sensitive, 1 = insensitive).
fn compile(pattern: &str, case_sensitivity: i64) -> Result<Regex, ExcelError> {
    let source = match case_sensitivity {
        0 => pattern.to_string(),
        1 => format!("(?i){pattern}"),
        _ => return Err(ExcelError::new(ExcelErrorKind::Value)),
    };
    Regex::new(&source).map_err(|_| ExcelError::new(ExcelErrorKind::Value))
}

fn text_arg() -> ArgSchema {
    ArgSchema {
        kinds: smallvec::smallvec![ArgKind::Text],
        required: true,
        by_ref: false,
        shape: ShapeKind::Scalar,
        coercion: CoercionPolicy::None,
        max: None,
        repeating: None,
        default: None,
    }
}

fn optional_number_arg(default: f64) -> ArgSchema {
    ArgSchema {
        kinds: smallvec::smallvec![ArgKind::Number],
        required: false,
        by_ref: false,
        shape: ShapeKind::Scalar,
        coercion: CoercionPolicy::NumberLenientText,
        max: None,
        repeating: None,
        default: Some(LiteralValue::Number(default)),
    }
}

fn scalar<'c>(v: LiteralValue) -> CalcValue<'c> {
    CalcValue::Scalar(v)
}

fn row_array<'c>(values: Vec<String>) -> CalcValue<'c> {
    CalcValue::Scalar(LiteralValue::Array(vec![
        values.into_iter().map(LiteralValue::Text).collect(),
    ]))
}

/// `REGEXTEST(text, pattern, [case_sensitivity])` — TRUE when any part of `text` matches.
#[derive(Debug)]
pub struct RegexTestFn;

impl Function for RegexTestFn {
    func_caps!(PURE);
    fn name(&self) -> &'static str {
        "REGEXTEST"
    }
    fn min_args(&self) -> usize {
        2
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        static SCHEMA: LazyLock<Vec<ArgSchema>> =
            LazyLock::new(|| vec![text_arg(), text_arg(), optional_number_arg(0.0)]);
        &SCHEMA[..]
    }
    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        _ctx: &dyn FunctionContext<'b>,
    ) -> Result<CalcValue<'b>, ExcelError> {
        let text = arg_text(&args[0])?;
        let pattern = arg_text(&args[1])?;
        let re = compile(&pattern, arg_int(args, 2, 0)?)?;
        Ok(scalar(LiteralValue::Boolean(re.is_match(&text))))
    }
}

/// `REGEXEXTRACT(text, pattern, [return_mode], [case_sensitivity])`
/// return_mode 0 = first match, 1 = every match (a row), 2 = the first match's capture groups (a row).
#[derive(Debug)]
pub struct RegexExtractFn;

impl Function for RegexExtractFn {
    func_caps!(PURE);
    fn name(&self) -> &'static str {
        "REGEXEXTRACT"
    }
    fn min_args(&self) -> usize {
        2
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        static SCHEMA: LazyLock<Vec<ArgSchema>> = LazyLock::new(|| {
            vec![
                text_arg(),
                text_arg(),
                optional_number_arg(0.0),
                optional_number_arg(0.0),
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
        let return_mode = arg_int(args, 2, 0)?;
        let re = compile(&pattern, arg_int(args, 3, 0)?)?;
        match return_mode {
            0 => match re.find(&text) {
                Some(m) => Ok(scalar(LiteralValue::Text(m.as_str().to_string()))),
                None => Ok(scalar(LiteralValue::Error(ExcelError::new(
                    ExcelErrorKind::Na,
                )))),
            },
            1 => {
                let all: Vec<String> = re
                    .find_iter(&text)
                    .map(|m| m.as_str().to_string())
                    .collect();
                if all.is_empty() {
                    return Ok(scalar(LiteralValue::Error(ExcelError::new(
                        ExcelErrorKind::Na,
                    ))));
                }
                Ok(row_array(all))
            }
            2 => {
                let Some(caps) = re.captures(&text) else {
                    return Ok(scalar(LiteralValue::Error(ExcelError::new(
                        ExcelErrorKind::Na,
                    ))));
                };
                if caps.len() < 2 {
                    return Ok(scalar(LiteralValue::Error(ExcelError::new(
                        ExcelErrorKind::Value,
                    ))));
                }
                let groups: Vec<String> = (1..caps.len())
                    .map(|i| {
                        caps.get(i)
                            .map(|g| g.as_str().to_string())
                            .unwrap_or_default()
                    })
                    .collect();
                Ok(row_array(groups))
            }
            _ => Ok(scalar(LiteralValue::Error(ExcelError::new(
                ExcelErrorKind::Value,
            )))),
        }
    }
}

/// `REGEXREPLACE(text, pattern, replacement, [occurrence], [case_sensitivity])`
/// occurrence 0 = every match, n = the n-th match only, -n = the n-th match from the end.
/// `$1`…`$9` / `${name}` in `replacement` insert capture groups.
#[derive(Debug)]
pub struct RegexReplaceFn;

impl Function for RegexReplaceFn {
    func_caps!(PURE);
    fn name(&self) -> &'static str {
        "REGEXREPLACE"
    }
    fn min_args(&self) -> usize {
        3
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        static SCHEMA: LazyLock<Vec<ArgSchema>> = LazyLock::new(|| {
            vec![
                text_arg(),
                text_arg(),
                text_arg(),
                optional_number_arg(0.0),
                optional_number_arg(0.0),
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
        let replacement = arg_text(&args[2])?;
        let occurrence = arg_int(args, 3, 0)?;
        let re = compile(&pattern, arg_int(args, 4, 0)?)?;

        if occurrence == 0 {
            return Ok(scalar(LiteralValue::Text(
                re.replace_all(&text, replacement.as_str()).into_owned(),
            )));
        }
        let matches: Vec<regex::Captures> = re.captures_iter(&text).collect();
        if matches.is_empty() {
            return Ok(scalar(LiteralValue::Text(text)));
        }
        let index = if occurrence > 0 {
            occurrence - 1
        } else {
            matches.len() as i64 + occurrence
        };
        if index < 0 || index as usize >= matches.len() {
            return Ok(scalar(LiteralValue::Text(text)));
        }
        let caps = &matches[index as usize];
        let whole = caps.get(0).expect("group 0 always present");
        let mut expanded = String::new();
        caps.expand(&replacement, &mut expanded);
        let mut out = String::with_capacity(text.len() + expanded.len());
        out.push_str(&text[..whole.start()]);
        out.push_str(&expanded);
        out.push_str(&text[whole.end()..]);
        Ok(scalar(LiteralValue::Text(out)))
    }
}

pub fn register_builtins() {
    use crate::function_registry::register_builtin;
    use std::sync::Arc;
    register_builtin(Arc::new(RegexTestFn));
    register_builtin(Arc::new(RegexExtractFn));
    register_builtin(Arc::new(RegexReplaceFn));
}
