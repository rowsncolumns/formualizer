//! `GROUPBY` and `PIVOTBY`: group rows by key fields and aggregate value columns with a function.
//!
//! The `function` argument is a LAMBDA — usually Excel's eta-reduced form, a bare function name
//! (`=GROUPBY(A2:A9,C2:C9,SUM)`), or an explicit one (`LAMBDA(v,SUM(v))`). A two-parameter
//! function (`PERCENTOF`, `LAMBDA(subset,total,…)`) also receives the comparison set. The
//! function name as text (`"SUM"`) or an AGGREGATE-style number is still accepted for formulas
//! written before LAMBDA arguments worked.
//!
//! Output (Excel): rows sorted ascending by the row fields unless `sort_order` says otherwise,
//! a `Total` row (and, with two or more row-field columns, subtotal rows) unless `total_depth`
//! turns them off, and headers shown only when `field_headers` is 2 or 3.
//! <https://support.microsoft.com/en-us/office/groupby-function-5e08ae8c-6800-4b72-b623-c41773611505>
//! <https://support.microsoft.com/en-us/office/pivotby-function-de86516a-90ad-4ced-8522-3a25fac389cf>

use super::super::utils::collapse_if_scalar;
use super::lookup_utils::sort_ordering;
use crate::args::{ArgSchema, CoercionPolicy, ShapeKind};
use crate::builtins::lambda::EtaCallable;
use crate::function::Function;
use crate::traits::{ArgumentHandle, CalcValue, CustomCallable, FunctionContext};
use formualizer_common::{ArgKind, ExcelError, ExcelErrorKind, LiteralValue};
use formualizer_macros::func_caps;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::Arc;

const TOTAL_LABEL: &str = "Total";

fn error_value(kind: ExcelErrorKind, msg: &str) -> LiteralValue {
    LiteralValue::Error(ExcelError::new(kind).with_message(msg.to_string()))
}

fn scalar(v: LiteralValue) -> CalcValue<'static> {
    CalcValue::Scalar(v)
}

/// The cells of a range / array argument, row-major.
fn rows_of(arg: &ArgumentHandle<'_, '_>) -> Result<Vec<Vec<LiteralValue>>, LiteralValue> {
    let view = arg.range_view().map_err(LiteralValue::Error)?;
    let (rows, cols) = view.dims();
    Ok((0..rows)
        .map(|r| (0..cols).map(|c| view.get_cell(r, c)).collect())
        .collect())
}

/// An optional integer argument: `None` when missing, skipped or blank.
fn opt_int(args: &[ArgumentHandle<'_, '_>], idx: usize) -> Result<Option<i64>, LiteralValue> {
    let Some(arg) = args.get(idx) else {
        return Ok(None);
    };
    if arg.is_skipped() {
        return Ok(None);
    }
    match arg.value().map(CalcValue::into_literal) {
        Ok(LiteralValue::Empty) => Ok(None),
        Ok(LiteralValue::Int(i)) => Ok(Some(i)),
        Ok(LiteralValue::Number(n)) if n.is_finite() => Ok(Some(n.trunc() as i64)),
        Ok(LiteralValue::Boolean(b)) => Ok(Some(b as i64)),
        Ok(LiteralValue::Error(e)) | Err(e) => Err(LiteralValue::Error(e)),
        Ok(_) => Err(error_value(ExcelErrorKind::Value, "Expected a number")),
    }
}

/// A sort-order argument: one number or a vector of numbers.
fn opt_int_list(
    args: &[ArgumentHandle<'_, '_>],
    idx: usize,
) -> Result<Option<Vec<i64>>, LiteralValue> {
    let Some(arg) = args.get(idx) else {
        return Ok(None);
    };
    if arg.is_skipped() {
        return Ok(None);
    }
    let value = match arg.value() {
        Ok(CalcValue::Range(_)) => LiteralValue::Array(rows_of(arg)?),
        Ok(v) => v.into_literal(),
        Err(e) => return Err(LiteralValue::Error(e)),
    };
    let one = |v: &LiteralValue| match v {
        LiteralValue::Int(i) => Ok(*i),
        LiteralValue::Number(n) if n.is_finite() => Ok(n.trunc() as i64),
        LiteralValue::Error(e) => Err(LiteralValue::Error(e.clone())),
        _ => Err(error_value(ExcelErrorKind::Value, "Expected a number")),
    };
    match &value {
        LiteralValue::Empty => Ok(None),
        LiteralValue::Array(rows) => Ok(Some(
            rows.iter().flatten().map(one).collect::<Result<_, _>>()?,
        )),
        v => Ok(Some(vec![one(v)?])),
    }
}

/// Function names the text / number form of `function` used to accept (AGGREGATE numbering).
fn legacy_function_name(v: &LiteralValue) -> Option<&'static str> {
    let by_num = |n: i64| match n {
        1 => Some("AVERAGE"),
        2 => Some("COUNT"),
        3 => Some("COUNTA"),
        4 => Some("MAX"),
        5 => Some("MIN"),
        6 => Some("PRODUCT"),
        7 => Some("STDEV.S"),
        8 => Some("STDEV.P"),
        9 => Some("SUM"),
        10 => Some("VAR.S"),
        11 => Some("VAR.P"),
        12 => Some("MEDIAN"),
        _ => None,
    };
    match v {
        LiteralValue::Int(n) => by_num(*n),
        LiteralValue::Number(n) => by_num(*n as i64),
        LiteralValue::Text(s) => match s.trim().to_ascii_uppercase().as_str() {
            "SUM" => Some("SUM"),
            "AVERAGE" | "AVG" => Some("AVERAGE"),
            "COUNT" => Some("COUNT"),
            "COUNTA" => Some("COUNTA"),
            "MAX" => Some("MAX"),
            "MIN" => Some("MIN"),
            "PRODUCT" => Some("PRODUCT"),
            "STDEV" | "STDEV.S" => Some("STDEV.S"),
            "STDEVP" | "STDEV.P" => Some("STDEV.P"),
            "VAR" | "VAR.S" => Some("VAR.S"),
            "VARP" | "VAR.P" => Some("VAR.P"),
            "MEDIAN" => Some("MEDIAN"),
            _ => None,
        },
        _ => None,
    }
}

/// The aggregation function and how many arguments it takes (one: the subset; two: the subset
/// and the comparison set, as `PERCENTOF` expects).
struct Aggregator<'h, 'a, 'b> {
    handle: &'h ArgumentHandle<'a, 'b>,
    callable: Arc<dyn CustomCallable>,
    two_args: bool,
}

impl<'h, 'a, 'b> Aggregator<'h, 'a, 'b> {
    fn new(handle: &'h ArgumentHandle<'a, 'b>) -> Result<Self, LiteralValue> {
        let callable: Arc<dyn CustomCallable> = match handle.value() {
            Ok(CalcValue::Callable(c)) => c,
            Ok(other) => match other.into_literal() {
                LiteralValue::Error(e) => return Err(LiteralValue::Error(e)),
                lit => {
                    let f = legacy_function_name(&lit)
                        .and_then(|name| crate::function_registry::get("", name))
                        .ok_or_else(|| {
                            error_value(
                                ExcelErrorKind::Value,
                                "function must be a LAMBDA, e.g. SUM or LAMBDA(v,SUM(v))",
                            )
                        })?;
                    Arc::new(EtaCallable::new(f))
                }
            },
            Err(e) => return Err(LiteralValue::Error(e)),
        };
        let min = callable.min_arity();
        if min > 2 || callable.arity() == 0 {
            return Err(error_value(
                ExcelErrorKind::Value,
                "The aggregation LAMBDA must take one or two arguments",
            ));
        }
        Ok(Self {
            handle,
            callable,
            two_args: min == 2,
        })
    }

    fn apply(&self, subset: Vec<LiteralValue>, total: &[LiteralValue]) -> LiteralValue {
        let column =
            |v: Vec<LiteralValue>| LiteralValue::Array(v.into_iter().map(|x| vec![x]).collect());
        let mut args = vec![column(subset)];
        if self.two_args {
            args.push(column(total.to_vec()));
        }
        match self.handle.invoke_callable(&self.callable, &args) {
            Ok(cv) => match cv.into_literal() {
                LiteralValue::Array(rows) => {
                    if rows.len() == 1 && rows[0].len() == 1 {
                        rows[0][0].clone()
                    } else {
                        error_value(ExcelErrorKind::Calc, "Nested arrays are not supported")
                    }
                }
                v => v,
            },
            Err(e) => LiteralValue::Error(e),
        }
    }
}

/// Grouping identity of a key cell: text compares case-insensitively, numbers by value.
fn norm_key(v: &LiteralValue) -> String {
    match v {
        LiteralValue::Text(s) => format!("t{}", s.to_lowercase()),
        LiteralValue::Boolean(b) => format!("b{b}"),
        LiteralValue::Empty => "e".to_string(),
        LiteralValue::Error(e) => format!("x{}", e.kind),
        other => match other.as_serial_number() {
            Some(n) => format!("n{}", n.to_bits()),
            None => format!("o{other:?}"),
        },
    }
}

/// `field_headers` → (data has a header row, show headers in the result).
fn header_mode(
    arg: Option<i64>,
    values: &[Vec<LiteralValue>],
) -> Result<(bool, bool), LiteralValue> {
    match arg {
        // Automatic: "If the 1st value is text and the 2nd value is a number, then the data is
        // assumed to have headers" — which are then skipped, not shown.
        None => {
            let first = values.first().and_then(|r| r.first());
            let second = values.get(1).and_then(|r| r.first());
            let detected = matches!(first, Some(LiteralValue::Text(_)))
                && matches!(second, Some(LiteralValue::Int(_) | LiteralValue::Number(_)));
            Ok((detected, false))
        }
        Some(0) => Ok((false, false)),
        Some(1) => Ok((true, false)),
        Some(2) => Ok((false, true)),
        Some(3) => Ok((true, true)),
        Some(_) => Err(error_value(ExcelErrorKind::Value, "Invalid field_headers")),
    }
}

/// `total_depth` → (subtotal levels, grand total, totals at the top).
fn total_mode(arg: Option<i64>, fields: usize) -> Result<(usize, bool, bool), LiteralValue> {
    // Automatic: "Grand totals and, where possible, subtotals".
    let depth = arg.unwrap_or(fields.max(1) as i64);
    let levels = depth.unsigned_abs() as usize;
    if levels >= 2 && levels > fields {
        return Err(error_value(
            ExcelErrorKind::Value,
            "Subtotals need at least two field columns",
        ));
    }
    Ok((levels.saturating_sub(1), depth != 0, depth < 0))
}

/// One distinct key tuple and the data rows that carry it.
struct Group {
    keys: Vec<LiteralValue>,
    norm: Vec<String>,
    rows: Vec<usize>,
}

fn build_groups(keys: &[Vec<LiteralValue>], rows: &[usize]) -> Vec<Group> {
    let mut index: HashMap<Vec<String>, usize> = HashMap::new();
    let mut groups: Vec<Group> = Vec::new();
    for &r in rows {
        let norm: Vec<String> = keys[r].iter().map(norm_key).collect();
        let g = *index.entry(norm.clone()).or_insert_with(|| {
            groups.push(Group {
                keys: keys[r].clone(),
                norm,
                rows: Vec::new(),
            });
            groups.len() - 1
        });
        groups[g].rows.push(r);
    }
    groups
}

/// A row (or column) of the result: one group, a subtotal over a key prefix, or the grand total.
#[derive(Clone, Copy)]
enum Slot {
    Detail(usize),
    Subtotal { level: usize, group: usize },
    Grand,
}

impl Slot {
    fn matches(self, groups: &[Group], row_norm: &[String]) -> bool {
        match self {
            Slot::Detail(g) => groups[g].norm == row_norm,
            Slot::Subtotal { level, group } => groups[group].norm[..level] == row_norm[..level],
            Slot::Grand => true,
        }
    }

    /// The enclosing slot (`relative_to` 3 / 4): a detail's subtotal level above, else grand.
    fn parent(self, fields: usize) -> Slot {
        match self {
            Slot::Detail(group) if fields >= 2 => Slot::Subtotal {
                level: fields - 1,
                group,
            },
            Slot::Subtotal { level, group } if level >= 2 => Slot::Subtotal {
                level: level - 1,
                group,
            },
            _ => Slot::Grand,
        }
    }

    /// The key cells this slot shows.
    fn labels(self, groups: &[Group], fields: usize) -> Vec<LiteralValue> {
        match self {
            Slot::Detail(g) => groups[g].keys.clone(),
            Slot::Subtotal { level, group } => (0..fields)
                .map(|c| {
                    if c < level {
                        groups[group].keys[c].clone()
                    } else {
                        LiteralValue::Empty
                    }
                })
                .collect(),
            Slot::Grand => (0..fields.max(1))
                .map(|c| {
                    if c == 0 {
                        LiteralValue::Text(TOTAL_LABEL.to_string())
                    } else {
                        LiteralValue::Empty
                    }
                })
                .collect(),
        }
    }
}

/// Lay out sorted groups with subtotal rows (`levels` deep) and the grand total.
fn plan(order: &[usize], groups: &[Group], levels: usize, grand: bool, top: bool) -> Vec<Slot> {
    let mut out = Vec::new();
    if grand && top {
        out.push(Slot::Grand);
    }
    let same = |a: usize, b: usize, l: usize| groups[a].norm[..l] == groups[b].norm[..l];
    for (i, &g) in order.iter().enumerate() {
        if top {
            let prev = i.checked_sub(1).map(|p| order[p]);
            for level in 1..=levels {
                if prev.is_none_or(|p| !same(p, g, level)) {
                    out.push(Slot::Subtotal { level, group: g });
                }
            }
        }
        out.push(Slot::Detail(g));
        if !top {
            let next = order.get(i + 1).copied();
            for level in (1..=levels).rev() {
                if next.is_none_or(|n| !same(n, g, level)) {
                    out.push(Slot::Subtotal { level, group: g });
                }
            }
        }
    }
    if grand && !top {
        out.push(Slot::Grand);
    }
    out
}

/// Order groups: the subtotal prefix columns first (to keep each subtotal block contiguous), then
/// the requested sort columns, then every key column ascending. `value_key(g, j)` is the
/// aggregate a value-column sort compares.
fn sort_groups(
    groups: &[Group],
    fields: usize,
    value_cols: usize,
    spec: Option<&[i64]>,
    levels: usize,
    value_key: &dyn Fn(usize, usize) -> LiteralValue,
) -> Result<Vec<usize>, LiteralValue> {
    let mut criteria: Vec<(usize, bool)> = Vec::new();
    for &k in spec.unwrap_or(&[]) {
        let col = k.unsigned_abs() as usize;
        if col == 0 || col > fields + value_cols {
            return Err(error_value(ExcelErrorKind::Value, "Invalid sort_order"));
        }
        criteria.push((col - 1, k > 0));
    }
    let direction = |c: usize| {
        criteria
            .iter()
            .find(|(col, _)| *col == c)
            .map(|(_, asc)| *asc)
            .unwrap_or(true)
    };
    let mut full: Vec<(usize, bool)> = (0..levels).map(|c| (c, direction(c))).collect();
    full.extend(criteria.iter().copied());
    full.extend((0..fields).map(|c| (c, direction(c))));

    let values: Vec<Vec<LiteralValue>> = if criteria.iter().any(|(c, _)| *c >= fields) {
        (0..groups.len())
            .map(|g| (0..value_cols).map(|j| value_key(g, j)).collect())
            .collect()
    } else {
        Vec::new()
    };
    let mut order: Vec<usize> = (0..groups.len()).collect();
    order.sort_by(|&a, &b| {
        for &(c, asc) in &full {
            let ord = if c < fields {
                sort_ordering(&groups[a].keys[c], &groups[b].keys[c], asc)
            } else {
                sort_ordering(&values[a][c - fields], &values[b][c - fields], asc)
            };
            if ord != Ordering::Equal {
                return ord;
            }
        }
        Ordering::Equal
    });
    Ok(order)
}

/// Rows a `filter_array` keeps (its length is the data length, or the full length with headers).
fn filtered_rows(
    args: &[ArgumentHandle<'_, '_>],
    idx: usize,
    data_start: usize,
    total_rows: usize,
) -> Result<Vec<usize>, LiteralValue> {
    let all: Vec<usize> = (data_start..total_rows).collect();
    let Some(arg) = args.get(idx) else {
        return Ok(all);
    };
    if arg.is_skipped() {
        return Ok(all);
    }
    let flags: Vec<LiteralValue> = rows_of(arg)?.into_iter().flatten().collect();
    let offset = if flags.len() == total_rows {
        0
    } else if flags.len() == total_rows - data_start {
        data_start
    } else {
        return Err(error_value(
            ExcelErrorKind::Value,
            "filter_array must have one value per row",
        ));
    };
    let mut keep = Vec::new();
    for r in all {
        match &flags[r - offset] {
            LiteralValue::Error(e) => return Err(LiteralValue::Error(e.clone())),
            LiteralValue::Boolean(true) => keep.push(r),
            LiteralValue::Int(n) if *n != 0 => keep.push(r),
            LiteralValue::Number(n) if *n != 0.0 => keep.push(r),
            _ => {}
        }
    }
    Ok(keep)
}

fn header_cells(
    data: &[Vec<LiteralValue>],
    has_headers: bool,
    width: usize,
    generated: &str,
) -> Vec<LiteralValue> {
    (0..width)
        .map(|c| {
            if has_headers {
                data[0].get(c).cloned().unwrap_or(LiteralValue::Empty)
            } else {
                LiteralValue::Text(format!("{generated}{}", c + 1))
            }
        })
        .collect()
}

fn width(rows: &[Vec<LiteralValue>]) -> usize {
    rows.iter().map(Vec::len).max().unwrap_or(0)
}

/// Shared engine: `col_keys` is `None` for GROUPBY.
#[allow(clippy::too_many_arguments)]
fn summarize(
    agg: &Aggregator<'_, '_, '_>,
    row_keys: &[Vec<LiteralValue>],
    col_keys: Option<&[Vec<LiteralValue>]>,
    values: &[Vec<LiteralValue>],
    rows: &[usize],
    has_headers: bool,
    show_headers: bool,
    row_totals: (usize, bool, bool),
    row_sort: Option<&[i64]>,
    col_totals: (usize, bool, bool),
    col_sort: Option<&[i64]>,
    relative_to: i64,
) -> Result<Vec<Vec<LiteralValue>>, LiteralValue> {
    let rf = width(row_keys);
    let vc = width(values);
    let cell = |r: usize, c: usize| values[r].get(c).cloned().unwrap_or(LiteralValue::Empty);
    let column = |rs: &mut dyn Iterator<Item = usize>, j: usize| -> Vec<LiteralValue> {
        rs.map(|r| cell(r, j)).collect()
    };
    let all_rows = |j: usize| column(&mut rows.iter().copied(), j);

    let row_norm: Vec<Vec<String>> = row_keys
        .iter()
        .map(|k| k.iter().map(norm_key).collect())
        .collect();
    let row_groups = build_groups(row_keys, rows);

    let (col_groups, col_norm, cf) = match col_keys {
        Some(ck) => (
            build_groups(ck, rows),
            ck.iter()
                .map(|k| k.iter().map(norm_key).collect())
                .collect::<Vec<Vec<String>>>(),
            width(ck),
        ),
        None => (Vec::new(), Vec::new(), 0),
    };

    // Sorting by a value column compares the group's total for that column.
    let row_value = |g: usize, j: usize| {
        agg.apply(
            column(&mut row_groups[g].rows.iter().copied(), j),
            &all_rows(j),
        )
    };
    let row_order = sort_groups(&row_groups, rf, vc, row_sort, row_totals.0, &row_value)?;
    let row_plan = plan(
        &row_order,
        &row_groups,
        row_totals.0,
        row_totals.1,
        row_totals.2,
    );

    let col_plan = if col_keys.is_some() {
        let col_value = |g: usize, j: usize| {
            agg.apply(
                column(&mut col_groups[g].rows.iter().copied(), j),
                &all_rows(j),
            )
        };
        let order = sort_groups(&col_groups, cf, vc, col_sort, col_totals.0, &col_value)?;
        plan(
            &order,
            &col_groups,
            col_totals.0,
            col_totals.1,
            col_totals.2,
        )
    } else {
        vec![Slot::Grand]
    };

    let in_row = |slot: Slot, r: usize| slot.matches(&row_groups, &row_norm[r]);
    let in_col =
        |slot: Slot, r: usize| col_keys.is_none() || slot.matches(&col_groups, &col_norm[r]);

    let mut out: Vec<Vec<LiteralValue>> = Vec::new();
    let value_headers = header_cells(values, has_headers, vc, "Value");
    if col_keys.is_some() {
        // One header row per column-field level, plus a row naming the value columns when there
        // is more than one of them.
        for level in 0..cf {
            let mut line = vec![LiteralValue::Empty; rf];
            for &slot in &col_plan {
                let label = slot.labels(&col_groups, cf)[level].clone();
                line.push(label);
                line.extend(std::iter::repeat_n(LiteralValue::Empty, vc - 1));
            }
            out.push(line);
        }
        if vc > 1 || show_headers {
            let mut line = if show_headers {
                header_cells(row_keys, has_headers, rf, "Row Field ")
            } else {
                vec![LiteralValue::Empty; rf]
            };
            for _ in &col_plan {
                line.extend(value_headers.iter().cloned());
            }
            out.push(line);
        }
    } else if show_headers {
        let mut line = header_cells(row_keys, has_headers, rf, "Row Field ");
        line.extend(value_headers.iter().cloned());
        out.push(line);
    }

    for &rslot in &row_plan {
        let mut line = rslot.labels(&row_groups, rf);
        for &cslot in &col_plan {
            let members: Vec<usize> = rows
                .iter()
                .copied()
                .filter(|&r| in_row(rslot, r) && in_col(cslot, r))
                .collect();
            for j in 0..vc {
                if members.is_empty() {
                    line.push(LiteralValue::Empty);
                    continue;
                }
                let subset = column(&mut members.iter().copied(), j);
                let total = if !agg.two_args {
                    Vec::new()
                } else if col_keys.is_none() {
                    all_rows(j)
                } else {
                    let (rs, cs) = match relative_to {
                        1 => (rslot, Slot::Grand),
                        2 => (Slot::Grand, Slot::Grand),
                        3 => (Slot::Grand, cslot.parent(cf)),
                        4 => (rslot.parent(rf), Slot::Grand),
                        _ => (Slot::Grand, cslot),
                    };
                    column(
                        &mut rows
                            .iter()
                            .copied()
                            .filter(|&r| in_row(rs, r) && in_col(cs, r)),
                        j,
                    )
                };
                line.push(agg.apply(subset, &total));
            }
        }
        out.push(line);
    }
    Ok(out)
}

fn range_arg() -> ArgSchema {
    ArgSchema {
        kinds: smallvec::smallvec![ArgKind::Range],
        required: true,
        by_ref: false,
        shape: ShapeKind::Range,
        coercion: CoercionPolicy::None,
        max: None,
        repeating: None,
        default: None,
    }
}

fn any_arg(required: bool) -> ArgSchema {
    ArgSchema {
        kinds: smallvec::smallvec![ArgKind::Any],
        required,
        by_ref: false,
        shape: ShapeKind::Scalar,
        coercion: CoercionPolicy::None,
        max: None,
        repeating: None,
        default: None,
    }
}

fn result<'b>(
    out: Result<Vec<Vec<LiteralValue>>, LiteralValue>,
    ctx: &dyn FunctionContext<'b>,
) -> CalcValue<'b> {
    match out {
        Ok(rows) if rows.is_empty() => scalar(error_value(ExcelErrorKind::Calc, "Empty result")),
        Ok(rows) => collapse_if_scalar(rows, ctx.date_system()),
        Err(e) => scalar(e),
    }
}

#[derive(Debug)]
pub struct GroupByFn;

/// Groups rows by key fields and aggregates value columns with a function.
///
/// `GROUPBY(row_fields, values, function, [field_headers], [total_depth], [sort_order],
/// [filter_array], [field_relationship])`
///
/// # Remarks
/// - `function` is a LAMBDA: a function name such as `SUM` (eta-reduced) or `LAMBDA(v,SUM(v))`;
///   a two-parameter function such as `PERCENTOF` also receives all values.
/// - Rows are sorted ascending by the row fields unless `sort_order` names other columns
///   (row-field columns first, then value columns; negative sorts descending).
/// - By default a `Total` row closes the result (subtotals too with two or more row-field
///   columns); `total_depth` 0 removes it, a negative depth puts totals at the top.
/// - Headers are detected automatically (text above a number) and skipped; `field_headers` 3
///   shows them.
///
/// # Examples
/// ```yaml,sandbox
/// title: "Sum by key"
/// grid:
///   A1: "a"
///   A2: "b"
///   A3: "a"
///   B1: 2
///   B2: 3
///   B3: 4
/// formula: "=GROUPBY(A1:A3,B1:B3,SUM)"
/// expected: [["a",6],["b",3],["Total",9]]
/// ```
///
/// ```yaml,docs
/// related:
///   - PIVOTBY
///   - PERCENTOF
///   - LAMBDA
/// faq:
///   - q: "Can I pass the function as text?"
///     a: "Excel takes a function such as SUM or a LAMBDA. The text form \"SUM\" is still accepted for older formulas."
/// ```
impl Function for GroupByFn {
    func_caps!(PURE, MAY_SPILL);
    fn name(&self) -> &'static str {
        "GROUPBY"
    }
    fn min_args(&self) -> usize {
        3
    }
    fn variadic(&self) -> bool {
        true
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        static SCHEMA: std::sync::LazyLock<Vec<ArgSchema>> = std::sync::LazyLock::new(|| {
            let mut v = vec![range_arg(), range_arg(), any_arg(true)];
            v.extend((0..5).map(|_| any_arg(false)));
            v
        });
        &SCHEMA
    }

    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        ctx: &dyn FunctionContext<'b>,
    ) -> Result<CalcValue<'b>, ExcelError> {
        if args.len() < 3 || args.len() > 8 {
            return Ok(scalar(error_value(
                ExcelErrorKind::Value,
                "GROUPBY takes 3 to 8 arguments",
            )));
        }
        let out = (|| {
            let keys = rows_of(&args[0])?;
            let values = rows_of(&args[1])?;
            if keys.len() != values.len() {
                return Err(error_value(
                    ExcelErrorKind::Value,
                    "row_fields and values must have the same number of rows",
                ));
            }
            let agg = Aggregator::new(&args[2])?;
            let (has, show) = header_mode(opt_int(args, 3)?, &values)?;
            let rf = width(&keys);
            let relationship = opt_int(args, 7)?.unwrap_or(0);
            if !(0..=1).contains(&relationship) {
                return Err(error_value(
                    ExcelErrorKind::Value,
                    "Invalid field_relationship",
                ));
            }
            let mut totals = total_mode(opt_int(args, 4)?, rf)?;
            if relationship == 1 {
                // Table relationship: no subtotals.
                totals.0 = 0;
            }
            let sort = opt_int_list(args, 5)?;
            let data_start = usize::from(has);
            let rows = filtered_rows(args, 6, data_start, keys.len())?;
            if rows.is_empty() {
                return Err(error_value(ExcelErrorKind::Calc, "No rows to group"));
            }
            summarize(
                &agg,
                &keys,
                None,
                &values,
                &rows,
                has,
                show,
                totals,
                sort.as_deref(),
                (0, false, false),
                None,
                0,
            )
        })();
        Ok(result(out, ctx))
    }
}

#[derive(Debug)]
pub struct PivotByFn;

/// Groups rows by row fields and columns by column fields and aggregates values at each
/// intersection.
///
/// `PIVOTBY(row_fields, col_fields, values, function, [field_headers], [row_total_depth],
/// [row_sort_order], [col_total_depth], [col_sort_order], [filter_array], [relative_to])`
///
/// # Remarks
/// - `function` is a LAMBDA such as `SUM` or `LAMBDA(v,SUM(v))`; a two-parameter function such as
///   `PERCENTOF` receives the comparison set chosen by `relative_to` (0 column totals, 1 row
///   totals, 2 grand total, 3 parent column total, 4 parent row total).
/// - Rows and columns are sorted ascending and closed by `Total` rows / columns by default.
///
/// # Examples
/// ```yaml,sandbox
/// title: "Sum by region and quarter"
/// grid:
///   A1: "East"
///   A2: "East"
///   A3: "West"
///   B1: "Q1"
///   B2: "Q2"
///   B3: "Q1"
///   C1: 100
///   C2: 150
///   C3: 120
/// formula: "=PIVOTBY(A1:A3,B1:B3,C1:C3,SUM)"
/// expected: [["","Q1","Q2","Total"],["East",100,150,250],["West",120,"",120],["Total",220,150,370]]
/// ```
///
/// ```yaml,docs
/// related:
///   - GROUPBY
///   - PERCENTOF
/// faq:
///   - q: "What does relative_to change?"
///     a: "Only a two-argument function such as PERCENTOF uses it: it picks the set the subset is compared against."
/// ```
impl Function for PivotByFn {
    func_caps!(PURE, MAY_SPILL);
    fn name(&self) -> &'static str {
        "PIVOTBY"
    }
    fn min_args(&self) -> usize {
        4
    }
    fn variadic(&self) -> bool {
        true
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        static SCHEMA: std::sync::LazyLock<Vec<ArgSchema>> = std::sync::LazyLock::new(|| {
            let mut v = vec![range_arg(), range_arg(), range_arg(), any_arg(true)];
            v.extend((0..7).map(|_| any_arg(false)));
            v
        });
        &SCHEMA
    }

    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        ctx: &dyn FunctionContext<'b>,
    ) -> Result<CalcValue<'b>, ExcelError> {
        if args.len() < 4 || args.len() > 11 {
            return Ok(scalar(error_value(
                ExcelErrorKind::Value,
                "PIVOTBY takes 4 to 11 arguments",
            )));
        }
        let out = (|| {
            let row_keys = rows_of(&args[0])?;
            let col_keys = rows_of(&args[1])?;
            let values = rows_of(&args[2])?;
            if row_keys.len() != values.len() || col_keys.len() != values.len() {
                return Err(error_value(
                    ExcelErrorKind::Value,
                    "row_fields, col_fields and values must have the same number of rows",
                ));
            }
            let agg = Aggregator::new(&args[3])?;
            let (has, show) = header_mode(opt_int(args, 4)?, &values)?;
            let row_totals = total_mode(opt_int(args, 5)?, width(&row_keys))?;
            let row_sort = opt_int_list(args, 6)?;
            let col_totals = total_mode(opt_int(args, 7)?, width(&col_keys))?;
            let col_sort = opt_int_list(args, 8)?;
            let data_start = usize::from(has);
            let rows = filtered_rows(args, 9, data_start, values.len())?;
            let relative_to = opt_int(args, 10)?.unwrap_or(0);
            if !(0..=4).contains(&relative_to) {
                return Err(error_value(ExcelErrorKind::Value, "Invalid relative_to"));
            }
            if rows.is_empty() {
                return Err(error_value(ExcelErrorKind::Calc, "No rows to group"));
            }
            summarize(
                &agg,
                &row_keys,
                Some(&col_keys),
                &values,
                &rows,
                has,
                show,
                row_totals,
                row_sort.as_deref(),
                col_totals,
                col_sort.as_deref(),
                relative_to,
            )
        })();
        Ok(result(out, ctx))
    }
}
