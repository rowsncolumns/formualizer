use crate::traits::ArgumentHandle;
// Note: Validator no longer depends on EvaluationContext; keep it engine-agnostic.
use formualizer_common::{ArgKind, ExcelError, ExcelErrorKind, LiteralValue};
use smallvec::{SmallVec, smallvec};
use std::borrow::Cow;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ShapeKind {
    Scalar,
    Range,
    Array,
}

pub use formualizer_common::CoercionPolicy;

#[derive(Clone, Debug)]
pub struct ArgSchema {
    pub kinds: SmallVec<[ArgKind; 2]>,
    pub required: bool,
    pub by_ref: bool,
    pub shape: ShapeKind,
    pub coercion: CoercionPolicy,
    pub max: Option<usize>,
    pub repeating: Option<usize>,
    pub default: Option<LiteralValue>,
}

impl ArgSchema {
    pub fn any() -> Self {
        Self {
            kinds: smallvec![ArgKind::Any],
            required: true,
            by_ref: false,
            shape: ShapeKind::Scalar,
            coercion: CoercionPolicy::None,
            max: None,
            repeating: None,
            default: None,
        }
    }

    pub fn number_lenient_scalar() -> Self {
        Self {
            kinds: smallvec![ArgKind::Number],
            required: true,
            by_ref: false,
            shape: ShapeKind::Scalar,
            coercion: CoercionPolicy::NumberLenientText,
            max: None,
            repeating: None,
            default: None,
        }
    }
}

#[derive(Clone, Debug)]
pub enum CriteriaPredicate {
    Eq(LiteralValue),
    Ne(LiteralValue),
    Gt(f64),
    Ge(f64),
    Lt(f64),
    Le(f64),
    TextLike {
        pattern: String,
        case_insensitive: bool,
    },
    /// `"<>a*"`: negation of a wildcard pattern (matches non-text cells too).
    NotLike(Box<CriteriaPredicate>),
    /// Case-insensitive text ordering (`">m"`): matches text cells only.
    TextGt(String),
    TextGe(String),
    TextLt(String),
    TextLe(String),
    /// `"="`: truly blank cells only.
    IsBlank,
    /// `"<>"`: every cell that is not blank (an empty-string result counts as
    /// non-blank, as in Excel).
    IsNotBlank,
    /// `""`: blank cells and cells holding an empty-string value.
    IsBlankOrEmptyText,
    IsNumber,
    IsText,
    IsLogical,
}

#[derive(Debug)]
pub enum PreparedArg<'a> {
    Value(Cow<'a, LiteralValue>),
    Range(crate::engine::range_view::RangeView<'a>),
    Reference(formualizer_parse::parser::ReferenceType),
    Predicate(CriteriaPredicate),
}

pub struct PreparedArgs<'a> {
    pub items: Vec<PreparedArg<'a>>,
}

#[derive(Default)]
pub struct ValidationOptions {
    pub warn_only: bool,
    /// Minimum number of arguments the function requires.  When non-zero,
    /// `validate_and_prepare` rejects calls with fewer arguments before any
    /// per-argument validation runs, preventing out-of-bounds panics in
    /// `eval` implementations.
    pub min_args: usize,
}

// Legacy adapter removed in clean break.

/// A call with fewer arguments than the function requires (`SUM()`, `IF(1)`, `VLOOKUP(1)`).
///
/// Excel refuses to *enter* such a formula, so it has no Excel result; a stored one (typed through
/// an API, imported, or entered on the JS engine) is `#N/A` — Google Sheets' "Wrong number of
/// arguments" value and the JS engine's result — rather than `#VALUE!`, so both engines agree
/// (rowsncolumns/spreadsheet#546 E-41).
pub fn too_few_arguments(min_args: usize, got: usize) -> ExcelError {
    ExcelError::new(ExcelErrorKind::Na).with_message(format!(
        "Too few arguments: expected at least {min_args}, got {got}"
    ))
}

/// Excel criteria-string parsing shared by COUNTIF/SUMIF/AVERAGEIF, the *IFS
/// family and the database functions.
///
/// Rules (Excel): an optional comparison operator (`= <> > >= < <=`) followed
/// by a value. The value is a number when it parses as one (so `"1"` and `1`
/// are the same criterion and both match numeric 1 and the text "1"), a
/// boolean for `TRUE`/`FALSE`, otherwise text — with `*`/`?` wildcards and the
/// `~` escape, matched case-insensitively against text cells only. `""`
/// matches blank cells and empty-string values, `"="` matches truly blank
/// cells, `"<>"` matches every non-blank cell.
pub fn parse_criteria(v: &LiteralValue) -> Result<CriteriaPredicate, ExcelError> {
    match v {
        LiteralValue::Text(s) => {
            let s_trim = s.trim();
            if s_trim.is_empty() {
                return Ok(CriteriaPredicate::IsBlankOrEmptyText);
            }

            let unquote = |t: &str| -> String {
                let t = t.trim();
                if let Some(inner) = t.strip_prefix('"').and_then(|x| x.strip_suffix('"')) {
                    inner.replace("\"\"", "\"")
                } else {
                    t.to_string()
                }
            };

            // Operators: >=, <=, <>, >, <, =
            let ops = [">=", "<=", "<>", ">", "<", "="];
            for op in ops.iter() {
                if let Some(rhs) = s_trim.strip_prefix(op) {
                    let rhs_trim = rhs.trim();
                    if rhs_trim.is_empty() {
                        return Ok(match *op {
                            "=" => CriteriaPredicate::IsBlank,
                            "<>" => CriteriaPredicate::IsNotBlank,
                            ">" => CriteriaPredicate::TextGt(String::new()),
                            ">=" => CriteriaPredicate::TextGe(String::new()),
                            "<" => CriteriaPredicate::TextLt(String::new()),
                            "<=" => CriteriaPredicate::TextLe(String::new()),
                            _ => unreachable!(),
                        });
                    }
                    if let Some(n) = parse_criteria_number(rhs_trim) {
                        return Ok(match *op {
                            ">=" => CriteriaPredicate::Ge(n),
                            "<=" => CriteriaPredicate::Le(n),
                            ">" => CriteriaPredicate::Gt(n),
                            "<" => CriteriaPredicate::Lt(n),
                            "=" => CriteriaPredicate::Eq(LiteralValue::Number(n)),
                            "<>" => CriteriaPredicate::Ne(LiteralValue::Number(n)),
                            _ => unreachable!(),
                        });
                    }
                    let lower = rhs_trim.to_ascii_lowercase();
                    if matches!(*op, "=" | "<>") && (lower == "true" || lower == "false") {
                        let b = LiteralValue::Boolean(lower == "true");
                        return Ok(if *op == "=" {
                            CriteriaPredicate::Eq(b)
                        } else {
                            CriteriaPredicate::Ne(b)
                        });
                    }
                    let lit = unquote(rhs_trim);
                    if matches!(*op, "=" | "<>") && has_wildcard(&lit) {
                        // `"<>a*"` = every cell that does NOT match the pattern.
                        let like = CriteriaPredicate::TextLike {
                            pattern: lit,
                            case_insensitive: true,
                        };
                        return Ok(if *op == "=" {
                            like
                        } else {
                            CriteriaPredicate::NotLike(Box::new(like))
                        });
                    }
                    let lit = unescape_wildcards(&lit);
                    return Ok(match *op {
                        "=" => CriteriaPredicate::Eq(LiteralValue::Text(lit)),
                        "<>" => CriteriaPredicate::Ne(LiteralValue::Text(lit)),
                        ">" => CriteriaPredicate::TextGt(lit),
                        ">=" => CriteriaPredicate::TextGe(lit),
                        "<" => CriteriaPredicate::TextLt(lit),
                        "<=" => CriteriaPredicate::TextLe(lit),
                        _ => unreachable!(),
                    });
                }
            }

            let plain = unquote(s_trim);

            if has_wildcard(&plain) {
                return Ok(CriteriaPredicate::TextLike {
                    pattern: plain,
                    case_insensitive: true,
                });
            }
            let lower = plain.to_ascii_lowercase();
            if lower == "true" {
                return Ok(CriteriaPredicate::Eq(LiteralValue::Boolean(true)));
            } else if lower == "false" {
                return Ok(CriteriaPredicate::Eq(LiteralValue::Boolean(false)));
            }
            if let Some(n) = parse_criteria_number(&plain) {
                return Ok(CriteriaPredicate::Eq(LiteralValue::Number(n)));
            }
            Ok(CriteriaPredicate::Eq(LiteralValue::Text(
                unescape_wildcards(&plain),
            )))
        }
        LiteralValue::Empty => Ok(CriteriaPredicate::IsBlank),
        LiteralValue::Number(n) => Ok(CriteriaPredicate::Eq(LiteralValue::Number(*n))),
        // Normalize integer criteria to Number so numeric text cells ("1") match a
        // numeric criterion the way they do in Excel.
        LiteralValue::Int(i) => Ok(CriteriaPredicate::Eq(LiteralValue::Number(*i as f64))),
        LiteralValue::Boolean(b) => Ok(CriteriaPredicate::Eq(LiteralValue::Boolean(*b))),
        LiteralValue::Error(e) => Err(e.clone()),
        LiteralValue::Array(arr) => {
            // Treat 1x1 array literals as scalars for criteria parsing
            if arr.len() == 1 && arr.first().map(|r| r.len()).unwrap_or(0) == 1 {
                parse_criteria(&arr[0][0])
            } else {
                Ok(CriteriaPredicate::Eq(LiteralValue::Array(arr.clone())))
            }
        }
        other => Ok(CriteriaPredicate::Eq(other.clone())),
    }
}

/// Number parsing for criteria text: plain numbers, `%` suffix and
/// thousands-grouped digits (`"1,000"`) — the forms Excel accepts in a
/// criterion.
fn parse_criteria_number(s: &str) -> Option<f64> {
    let loc = crate::locale::Locale::invariant();
    if let Some(n) = loc.parse_number_invariant(s) {
        return Some(n);
    }
    let t = s.trim();
    if t.contains(',') {
        let body = t.strip_prefix('-').unwrap_or(t);
        let (int_part, frac_part) = match body.split_once('.') {
            Some((i, f)) => (i, Some(f)),
            None => (body, None),
        };
        let groups: Vec<&str> = int_part.split(',').collect();
        let grouped_ok = groups.len() > 1
            && !groups[0].is_empty()
            && groups[0].len() <= 3
            && groups[0].chars().all(|c| c.is_ascii_digit())
            && groups[1..]
                .iter()
                .all(|g| g.len() == 3 && g.chars().all(|c| c.is_ascii_digit()))
            && frac_part.is_none_or(|f| !f.is_empty() && f.chars().all(|c| c.is_ascii_digit()));
        if grouped_ok {
            return t.replace(',', "").parse::<f64>().ok();
        }
    }
    None
}

/// `true` when `s` holds an unescaped `*` or `?` (a `~` escapes the next char).
pub fn has_wildcard(s: &str) -> bool {
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '~' => {
                chars.next();
            }
            '*' | '?' => return true,
            _ => {}
        }
    }
    false
}

/// Resolve `~` escapes in a criterion that has no live wildcards (`"a~*c"`
/// compared as the literal text `a*c`).
pub fn unescape_wildcards(s: &str) -> String {
    if !s.contains('~') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '~' {
            match chars.peek() {
                Some('*') | Some('?') | Some('~') => {
                    out.push(chars.next().unwrap());
                }
                _ => out.push('~'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

pub fn validate_and_prepare<'a, 'b>(
    args: &'a [ArgumentHandle<'a, 'b>],
    schema: &[ArgSchema],
    options: ValidationOptions,
) -> Result<PreparedArgs<'a>, ExcelError> {
    // Minimum arity — reject too-few arguments before per-arg validation so
    // that individual `eval` implementations cannot panic on indexing.
    if options.min_args > 0 && args.len() < options.min_args {
        if options.warn_only {
            return Ok(PreparedArgs { items: Vec::new() });
        }
        return Err(too_few_arguments(options.min_args, args.len()));
    }

    // Arity: simple rule – if schema.len() == 1, allow variadic repetition; else match up to schema.len()
    if schema.is_empty() {
        return Ok(PreparedArgs { items: Vec::new() });
    }

    let mut items: Vec<PreparedArg<'a>> = Vec::with_capacity(args.len());
    for (idx, arg) in args.iter().enumerate() {
        let spec = if schema.len() == 1 {
            &schema[0]
        } else if idx < schema.len() {
            &schema[idx]
        } else {
            // Attempt to find a repeating spec (e.g., variadic tail like CHOOSE, SUM, etc.)
            if let Some(rep_spec) = schema.iter().find(|s| s.repeating.is_some()) {
                rep_spec
            } else if options.warn_only {
                continue;
            } else {
                return Err(
                    ExcelError::new(ExcelErrorKind::Value).with_message("Too many arguments")
                );
            }
        };

        // A skipped slot (`INDEX(rng,,2)`) is an omitted argument: leave it to the function's
        // per-slot default instead of coercing the parser's empty-text marker to `#VALUE!`.
        if arg.is_skipped() {
            items.push(PreparedArg::Value(Cow::Owned(LiteralValue::Empty)));
            continue;
        }

        // By-ref argument: prefer a reference (AST literal or function-returned). Range/array
        // shaped by-ref slots (`FILTER`'s include, `SORT`/`UNIQUE`'s array, …) also accept a
        // computed array such as `B1:B4>0` or a nested `FILTER(...)` — those fall through to the
        // shape handling below instead of failing the whole call with `#REF!`.
        if spec.by_ref {
            match arg.as_reference_or_eval() {
                Ok(r) => {
                    items.push(PreparedArg::Reference(r));
                    continue;
                }
                Err(e) => {
                    if options.warn_only {
                        continue;
                    } else if !matches!(spec.shape, ShapeKind::Range | ShapeKind::Array) {
                        return Err(e);
                    }
                }
            }
        }

        // Criteria policy: parse into predicate
        if matches!(spec.coercion, CoercionPolicy::Criteria) {
            let v = arg.value()?.into_literal();
            match parse_criteria(&v) {
                Ok(pred) => {
                    items.push(PreparedArg::Predicate(pred));
                    continue;
                }
                Err(e) => {
                    if options.warn_only {
                        continue;
                    } else {
                        return Err(e);
                    }
                }
            }
        }

        // Shape handling
        match spec.shape {
            ShapeKind::Scalar => {
                // Collapse to scalar if needed (top-left for arrays)
                match arg.value() {
                    Ok(cv) => {
                        let v: Cow<'_, LiteralValue> = match cv {
                            crate::traits::CalcValue::Scalar(LiteralValue::Array(arr)) => {
                                let tl = arr
                                    .first()
                                    .and_then(|row| row.first())
                                    .cloned()
                                    .unwrap_or(LiteralValue::Empty);
                                Cow::Owned(tl)
                            }
                            crate::traits::CalcValue::Range(rv) => Cow::Owned(rv.get_cell(0, 0)),
                            crate::traits::CalcValue::Scalar(s) => Cow::Owned(s),
                            crate::traits::CalcValue::Callable(_) => {
                                Cow::Owned(LiteralValue::Error(
                                    ExcelError::new(ExcelErrorKind::Calc)
                                        .with_message("LAMBDA value must be invoked"),
                                ))
                            }
                        };
                        // Apply coercion policy to Value shapes when applicable
                        let coerced = match spec.coercion {
                            CoercionPolicy::None => v,
                            CoercionPolicy::NumberStrict => {
                                match crate::coercion::to_number_strict(v.as_ref()) {
                                    Ok(n) => Cow::Owned(LiteralValue::Number(n)),
                                    Err(e) => {
                                        if options.warn_only {
                                            v
                                        } else {
                                            return Err(e);
                                        }
                                    }
                                }
                            }
                            CoercionPolicy::NumberLenientText => {
                                match arg.lenient_number(v.as_ref()) {
                                    Ok(n) => Cow::Owned(LiteralValue::Number(n)),
                                    Err(e) => {
                                        if options.warn_only {
                                            v
                                        } else {
                                            return Err(e);
                                        }
                                    }
                                }
                            }
                            CoercionPolicy::Logical => {
                                match crate::coercion::to_logical(v.as_ref()) {
                                    Ok(b) => Cow::Owned(LiteralValue::Boolean(b)),
                                    Err(e) => {
                                        if options.warn_only {
                                            v
                                        } else {
                                            return Err(e);
                                        }
                                    }
                                }
                            }
                            CoercionPolicy::Criteria => v, // handled per-function currently
                            CoercionPolicy::DateTimeSerial => {
                                match crate::coercion::to_datetime_serial(v.as_ref()) {
                                    Ok(n) => Cow::Owned(LiteralValue::Number(n)),
                                    Err(e) => {
                                        if options.warn_only {
                                            v
                                        } else {
                                            return Err(e);
                                        }
                                    }
                                }
                            }
                        };
                        items.push(PreparedArg::Value(coerced))
                    }
                    Err(e) => items.push(PreparedArg::Value(Cow::Owned(LiteralValue::Error(e)))),
                }
            }
            ShapeKind::Range | ShapeKind::Array => {
                match arg.range_view() {
                    Ok(r) => items.push(PreparedArg::Range(r)),
                    Err(_e) => {
                        // Excel-compatible: functions that accept ranges typically also accept scalars.
                        // Fall back to treating the argument as a scalar value, even in strict mode.
                        match arg.value() {
                            Ok(v) => items.push(PreparedArg::Value(Cow::Owned(v.into_literal()))),
                            Err(e2) => {
                                items.push(PreparedArg::Value(Cow::Owned(LiteralValue::Error(e2))))
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(PreparedArgs { items })
}
