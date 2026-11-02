//! Dynamic / modern lookup & array helpers: XLOOKUP, FILTER, UNIQUE (initial sprint subset)
//!
//! Notes / Simplifications (documented for future refinement):
//! - XLOOKUP supports: lookup_value, lookup_array, return_array, [if_not_found], [match_mode], [search_mode]
//!   * match_mode: 0 exact (default), -1 exact-or-next-smaller, 1 exact-or-next-larger, 2 wildcard (basic * ?)
//!   * search_mode: 1 forward (default), -1 reverse; (2 / -2 binary not yet implemented -> treated as 1 / -1)
//!   * Wildcard mode (2) is case-insensitive and supports Excel-style escapes (~).
//! - FILTER supports: array, include, [if_empty]; Shapes must be broadcast-compatible by rows (include is 1-D).
//!   * include may be vertical column vector OR same sized 2D; we reduce any non-zero truthy cell to include row.
//!   * if_empty omitted -> #CALC! per Excel when no matches.
//! - UNIQUE supports: array, [by_col], [exactly_once]
//!   * by_col TRUE -> operate column-wise returning the unique columns (kept as columns)
//!   * exactly_once TRUE returns only values with count == 1 (supported in row-wise primitive set)
//! - All functions return Array literal values (spills) – engine handles spill placement later.
//!
//! TODO(backlog):
//! - Binary search for XLOOKUP approximate modes; currently linear scan.
//! - Better type coercion parity with Excel (booleans/text vs numbers nuances).
//! - Match unsorted detection for approximate modes (#N/A) and wildcard escaping.
//! - PERFORMANCE: streaming FILTER without full materialization; UNIQUE using smallvec for tiny sets.

use super::super::utils::collapse_if_scalar;
use super::lookup_utils::{PreparedLookupMatcher, cmp_for_approx_lookup, sort_ordering};
use crate::args::{ArgSchema, CoercionPolicy, ShapeKind};
use crate::engine::lookup_index_cache::LookupAxis;
use crate::function::Function; // FnCaps imported via macro
use crate::traits::{ArgumentHandle, FunctionContext};
use formualizer_common::{ArgKind, ExcelError, ExcelErrorKind, LiteralValue};
use formualizer_macros::func_caps;
use std::collections::HashMap;

/* ─────────────────── generated-array allocation guard ───────────────────
 *
 * Generator functions (SEQUENCE, RANDARRAY) materialize their full result
 * as `Vec<Vec<LiteralValue>>` before the engine ever sees it, so dimension
 * args taken from user input must be guarded BEFORE allocating — e.g.
 * `=SEQUENCE(1e6,1e6)` would otherwise attempt a 10^12-cell allocation.
 *
 * Cap rationale:
 * - Per-dimension: Excel sheet limits (1,048,576 rows × 16,384 cols — the
 *   same values as `EvalConfig::default().max_sheet_rows/max_sheet_cols`).
 *   A generated array larger than a sheet can never spill successfully
 *   (`SpillBoundsPolicy::Strict`), so it is `#SPILL!` unconditionally —
 *   the error Excel shows for `=SEQUENCE(2000000)` ("spill range is too
 *   big") — without allocating the array first.
 * - Total cells: 2^24 (16,777,216). `LiteralValue` is ≥32 bytes, so this
 *   already bounds the transient allocation near ~0.5 GiB. Anything larger
 *   risks OOM before the engine's spill planner can reject the result, so
 *   it fails closed as `#NUM!`.
 */
const GENERATED_ARRAY_MAX_ROWS: i64 = 1_048_576;
const GENERATED_ARRAY_MAX_COLS: i64 = 16_384;
const GENERATED_ARRAY_MAX_CELLS: i64 = 1 << 24;

/// Returns `Some(#SPILL!)` when a `rows x cols` generated array is taller or
/// wider than a sheet, `Some(#NUM!)` when it exceeds the allocation guard;
/// uses checked arithmetic so overflowing products fail closed. Callers have
/// already rejected `rows <= 0 || cols <= 0`.
fn generated_array_too_large(rows: i64, cols: i64) -> Option<ExcelError> {
    if rows > GENERATED_ARRAY_MAX_ROWS || cols > GENERATED_ARRAY_MAX_COLS {
        return Some(
            ExcelError::new(ExcelErrorKind::Spill).with_message("Spill exceeds sheet bounds"),
        );
    }
    match rows.checked_mul(cols) {
        Some(total) if total <= GENERATED_ARRAY_MAX_CELLS => None,
        _ => Some(ExcelError::new(ExcelErrorKind::Num)),
    }
}

/* ───────────────────────── helpers ───────────────────────── */

pub fn super_wildcard_match(pattern: &str, text: &str) -> bool {
    super::lookup_utils::wildcard_pattern_match(pattern, text)
}

/// Is `arg` a reference whose extent is not fully declared — a whole-row / whole-column range, or a
/// table / external / 3-D form the range view sizes on its own? Value arguments (array literals,
/// function results) are fully sized by their view and return `false`.
fn is_open_ended_reference(arg: &ArgumentHandle<'_, '_>) -> bool {
    use formualizer_parse::parser::ReferenceType;
    match arg.as_reference_or_eval() {
        Ok(ReferenceType::Cell { .. }) => false,
        Ok(ReferenceType::Range {
            start_row,
            start_col,
            end_row,
            end_col,
            ..
        }) => start_row.is_none() || start_col.is_none() || end_row.is_none() || end_col.is_none(),
        Ok(_) => true,
        Err(_) => false,
    }
}

/* ───────────────────────── XLOOKUP() ───────────────────────── */

#[derive(Debug)]
pub struct XLookupFn;

/// Looks up a value in one array and returns the aligned value from another array.
///
/// `XLOOKUP` supports exact, approximate, and wildcard matching with forward or reverse search.
///
/// # Remarks
/// - Defaults: `match_mode=0` (exact), `search_mode=1` (first-to-last).
/// - `if_not_found` is optional; if omitted and no match exists, returns `#N/A`.
/// - `match_mode`: `0` exact, `-1` exact-or-next-smaller, `1` exact-or-next-larger, `2` wildcard.
/// - `search_mode`: `1` forward, `-1` reverse. Other modes are accepted with current fallback behavior.
/// - `lookup_array` must be 1D. Invalid shape returns `#VALUE!`.
/// - If `return_array` is multi-column or multi-row, the matched row/column is returned as a spill.
///
/// # Examples
/// ```yaml,sandbox
/// title: "Exact key lookup with fallback"
/// grid:
///   A1: "id"
///   A2: 101
///   A3: 102
///   B1: "name"
///   B2: "Ana"
///   B3: "Bo"
/// formula: '=XLOOKUP(102,A2:A3,B2:B3,"Missing")'
/// expected: "Bo"
/// ```
///
/// ```yaml,sandbox
/// title: "Return a full row from a matched key"
/// grid:
///   A1: 1
///   A2: 2
///   B1: "East"
///   C1: 120
///   B2: "West"
///   C2: 140
/// formula: '=XLOOKUP(2,A1:A2,B1:C2)'
/// expected: [["West",140]]
/// ```
///
/// ```yaml,docs
/// related:
///   - XMATCH
///   - MATCH
///   - FILTER
/// faq:
///   - q: "How do match_mode and search_mode interact?"
///     a: "match_mode controls exact/approximate/wildcard behavior, while search_mode controls scan direction; reverse search (-1) returns the last matching position."
///   - q: "What happens when no match is found?"
///     a: "If if_not_found is provided, XLOOKUP returns that value; otherwise it returns #N/A."
///   - q: "Why do I get #VALUE! from XLOOKUP on valid ranges?"
///     a: "The lookup_array must be one-dimensional (single row or single column); multi-row-and-column lookup ranges return #VALUE!."
/// ```
/// [formualizer-docgen:schema:start]
/// Name: XLOOKUP
/// Type: XLookupFn
/// Min args: 3
/// Max args: variadic
/// Variadic: true
/// Signature: XLOOKUP(arg1: any@scalar, arg2: range@range, arg3: range@range, arg4?: any@scalar, arg5?: number@scalar, arg6?...: number@scalar)
/// Arg schema: arg1{kinds=any,required=true,shape=scalar,by_ref=false,coercion=None,max=None,repeating=None,default=false}; arg2{kinds=range,required=true,shape=range,by_ref=true,coercion=None,max=None,repeating=None,default=false}; arg3{kinds=range,required=true,shape=range,by_ref=true,coercion=None,max=None,repeating=None,default=false}; arg4{kinds=any,required=false,shape=scalar,by_ref=false,coercion=None,max=None,repeating=None,default=false}; arg5{kinds=number,required=false,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=true}; arg6{kinds=number,required=false,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=true}
/// Caps: PURE, LOOKUP
/// [formualizer-docgen:schema:end]
impl Function for XLookupFn {
    func_caps!(PURE, LOOKUP, MAY_SPILL);
    fn name(&self) -> &'static str {
        "XLOOKUP"
    }
    fn min_args(&self) -> usize {
        3
    }
    fn variadic(&self) -> bool {
        true
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        use once_cell::sync::Lazy;
        static SCHEMA: Lazy<Vec<ArgSchema>> = Lazy::new(|| {
            vec![
                // lookup_value
                ArgSchema {
                    kinds: smallvec::smallvec![ArgKind::Any],
                    required: true,
                    by_ref: false,
                    shape: ShapeKind::Scalar,
                    coercion: CoercionPolicy::None,
                    max: None,
                    repeating: None,
                    default: None,
                },
                // lookup_array (range)
                ArgSchema {
                    kinds: smallvec::smallvec![ArgKind::Range],
                    required: true,
                    by_ref: true,
                    shape: ShapeKind::Range,
                    coercion: CoercionPolicy::None,
                    max: None,
                    repeating: None,
                    default: None,
                },
                // return_array (range)
                ArgSchema {
                    kinds: smallvec::smallvec![ArgKind::Range],
                    required: true,
                    by_ref: true,
                    shape: ShapeKind::Range,
                    coercion: CoercionPolicy::None,
                    max: None,
                    repeating: None,
                    default: None,
                },
                // if_not_found (any optional)
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
                // match_mode (number) default 0
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
                // search_mode (number) default 1
                ArgSchema {
                    kinds: smallvec::smallvec![ArgKind::Number],
                    required: false,
                    by_ref: false,
                    shape: ShapeKind::Scalar,
                    coercion: CoercionPolicy::NumberLenientText,
                    max: None,
                    repeating: None,
                    default: Some(LiteralValue::Int(1)),
                },
            ]
        });
        &SCHEMA
    }
    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        _ctx: &dyn FunctionContext<'b>,
    ) -> Result<crate::traits::CalcValue<'b>, ExcelError> {
        if args.len() < 3 {
            return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                ExcelError::new(ExcelErrorKind::Value),
            )));
        }
        let lookup_value = args[0].value()?.into_literal();
        if let LiteralValue::Error(ref e) = lookup_value {
            return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                e.clone(),
            )));
        }
        let lookup_view = match args[1].range_view() {
            Ok(v) => v,
            Err(e) => return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(e))),
        };
        let ret_view = match args[2].range_view() {
            Ok(v) => v,
            Err(e) => return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(e))),
        };

        let (lookup_rows, lookup_cols) = lookup_view.dims();
        let (ret_rows, ret_cols) = ret_view.dims();

        // XLOOKUP requires a 1-D lookup array (single row or single column).
        // If the lookup range is completely empty (used-region trimmed to 0),
        // fall back to the return range's used-region length and treat missing lookup
        // cells as Empty.
        let vertical = if lookup_cols == 1 {
            true
        } else if lookup_rows == 1 {
            false
        } else if lookup_rows == 0 && lookup_cols == 0 {
            if ret_cols == 1 {
                true
            } else if ret_rows == 1 {
                false
            } else {
                return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                    ExcelError::new(ExcelErrorKind::Value),
                )));
            }
        } else {
            return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                ExcelError::new(ExcelErrorKind::Value),
            )));
        };

        // Excel: the return array must span the lookup array's length along the lookup axis
        // (`XLOOKUP(v,A1:A5,B1:B3)` is #VALUE!). Only checked when neither side is a whole-row /
        // whole-column reference — those views are trimmed to the sheet's used region, so their
        // dims are not the declared extents.
        if !is_open_ended_reference(&args[1]) && !is_open_ended_reference(&args[2]) {
            let (need, have) = if vertical {
                (lookup_rows, ret_rows)
            } else {
                (lookup_cols, ret_cols)
            };
            if need > 0 && need != have {
                return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                    ExcelError::new(ExcelErrorKind::Value),
                )));
            }
        }

        let lookup_len = {
            let raw = if vertical { lookup_rows } else { lookup_cols };
            if raw == 0 {
                if vertical { ret_rows } else { ret_cols }
            } else {
                raw
            }
        };

        if lookup_len == 0 {
            return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                ExcelError::new(ExcelErrorKind::Na),
            )));
        }

        let match_mode = if args.len() >= 5 {
            match args[4].value()?.into_literal() {
                LiteralValue::Int(i) => i,
                LiteralValue::Number(n) => n as i64,
                _ => 0,
            }
        } else {
            0
        };
        let search_mode = if args.len() >= 6 {
            match args[5].value()?.into_literal() {
                LiteralValue::Int(i) => i,
                LiteralValue::Number(n) => n as i64,
                _ => 1,
            }
        } else {
            1
        };

        let wildcard = match_mode == 2;

        let mut found: Option<usize> = None;
        let needle = lookup_value;
        let prepared_matcher = PreparedLookupMatcher::new(&needle, wildcard);
        if match_mode == 0 || wildcard {
            if match_mode == 0 && search_mode == 1 && lookup_rows > 0 && lookup_cols > 0 {
                let axis = if vertical {
                    LookupAxis::ColumnInView(0)
                } else {
                    LookupAxis::RowInView(0)
                };
                if let Some(index) = _ctx.get_lookup_index(&lookup_view, axis) {
                    found = index.find_first_exact(&needle);
                } else {
                    found = super::lookup_utils::find_exact_index_in_view(
                        &lookup_view,
                        &needle,
                        false,
                    )?;
                }
            } else if search_mode == 1 && lookup_rows > 0 && lookup_cols > 0 {
                found =
                    super::lookup_utils::find_exact_index_in_view(&lookup_view, &needle, wildcard)?;
            } else if search_mode == -1 {
                for i in (0..lookup_len).rev() {
                    let cand = if vertical {
                        lookup_view.get_cell(i, 0)
                    } else {
                        lookup_view.get_cell(0, i)
                    };
                    if prepared_matcher.matches(&cand) {
                        found = Some(i);
                        break;
                    }
                }
            } else {
                // Fallback linear scan (also used when the lookup view is empty and
                // we are treating missing cells as Empty).
                for i in 0..lookup_len {
                    let cand = if vertical {
                        lookup_view.get_cell(i, 0)
                    } else {
                        lookup_view.get_cell(0, i)
                    };
                    if prepared_matcher.matches(&cand) {
                        found = Some(i);
                        break;
                    }
                }
            }
        } else if match_mode == -1 || match_mode == 1 {
            // Excel's approximate-match order (numbers < text < logicals, no cross-type
            // coercion) decides "next smaller" / "next larger" — spreadsheet#939 Z-08.
            let mut best: Option<(usize, LiteralValue)> = None;

            let mut prev: Option<LiteralValue> = None;
            for i in 0..lookup_len {
                let cand = if vertical {
                    lookup_view.get_cell(i, 0)
                } else {
                    lookup_view.get_cell(0, i)
                };

                if let Some(p) = prev.as_ref()
                    && cmp_for_approx_lookup(p, &cand) == std::cmp::Ordering::Greater
                {
                    return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                        ExcelError::new(ExcelErrorKind::Na),
                    )));
                }
                prev = Some(cand.clone());

                match cmp_for_approx_lookup(&cand, &needle) {
                    std::cmp::Ordering::Equal => {
                        found = Some(i);
                        break;
                    }
                    side => {
                        if approx_candidate_improves(match_mode, side, &cand, best.as_ref()) {
                            best = Some((i, cand));
                        }
                    }
                }
            }

            if found.is_none() {
                found = best.map(|(i, _)| i);
            }
        } else {
            return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                ExcelError::new(ExcelErrorKind::Value),
            )));
        }

        if let Some(idx) = found {
            let (ret_rows, ret_cols) = ret_view.dims();
            if ret_rows == 0 || ret_cols == 0 {
                return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Empty));
            }

            if vertical {
                if ret_cols == 1 {
                    return Ok(crate::traits::CalcValue::Scalar(ret_view.get_cell(idx, 0)));
                }
                let mut row_out: Vec<LiteralValue> = Vec::with_capacity(ret_cols);
                for c in 0..ret_cols {
                    row_out.push(ret_view.get_cell(idx, c));
                }
                return Ok(crate::traits::CalcValue::Range(
                    crate::engine::range_view::RangeView::from_owned_rows(
                        vec![row_out],
                        _ctx.date_system(),
                    ),
                ));
            }

            // Horizontal orientation: treat idx as column.
            if ret_rows == 1 {
                return Ok(crate::traits::CalcValue::Scalar(ret_view.get_cell(0, idx)));
            }

            let mut col_out: Vec<Vec<LiteralValue>> = Vec::with_capacity(ret_rows);
            for r in 0..ret_rows {
                col_out.push(vec![ret_view.get_cell(r, idx)]);
            }
            return Ok(crate::traits::CalcValue::Range(
                crate::engine::range_view::RangeView::from_owned_rows(col_out, _ctx.date_system()),
            ));
        }

        // `XLOOKUP(v,rng,ret,,-1)` skips if_not_found: no match is `#N/A`, not the empty marker.
        if args.len() >= 4 && !args[3].is_skipped() {
            return args[3].value();
        }
        Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
            ExcelError::new(ExcelErrorKind::Na),
        )))
    }
}

/* ───────────────────────── XMATCH() ───────────────────────── */

#[derive(Debug)]
pub struct XMatchFn;
/// Returns the 1-based position of a value in a one-dimensional lookup array.
///
/// `XMATCH` extends `MATCH` with explicit search direction and wildcard mode.
///
/// # Remarks
/// - Defaults: `match_mode=0` (exact), `search_mode=1` (first-to-last).
/// - `match_mode`: `0` exact, `-1` exact-or-next-smaller, `1` exact-or-next-larger, `2` wildcard.
/// - `search_mode`: `1` forward, `-1` reverse, `2` ascending binary intent, `-2` descending binary intent.
/// - `lookup_array` must be a single row or single column, otherwise returns `#VALUE!`.
/// - Not found returns `#N/A`.
///
/// # Examples
/// ```yaml,sandbox
/// title: "Exact match position"
/// grid:
///   A1: "alpha"
///   A2: "beta"
///   A3: "gamma"
/// formula: '=XMATCH("beta",A1:A3)'
/// expected: 2
/// ```
///
/// ```yaml,sandbox
/// title: "Reverse search finds last duplicate"
/// grid:
///   A1: 7
///   A2: 9
///   A3: 7
/// formula: '=XMATCH(7,A1:A3,0,-1)'
/// expected: 3
/// ```
///
/// ```yaml,docs
/// related:
///   - XLOOKUP
///   - MATCH
///   - INDEX
/// faq:
///   - q: "How do search_mode values affect duplicate matches?"
///     a: "search_mode=1 returns the first qualifying match, while search_mode=-1 scans from the end and returns the last qualifying match."
///   - q: "When do binary-intent search modes (2 or -2) return #N/A?"
///     a: "For approximate modes they require sorted data in the expected direction; unsorted arrays are treated as no valid match and return #N/A."
/// ```
/// [formualizer-docgen:schema:start]
/// Name: XMATCH
/// Type: XMatchFn
/// Min args: 2
/// Max args: variadic
/// Variadic: true
/// Signature: XMATCH(arg1: any@scalar, arg2: range@range, arg3?: number@scalar, arg4?...: number@scalar)
/// Arg schema: arg1{kinds=any,required=true,shape=scalar,by_ref=false,coercion=None,max=None,repeating=None,default=false}; arg2{kinds=range,required=true,shape=range,by_ref=true,coercion=None,max=None,repeating=None,default=false}; arg3{kinds=number,required=false,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=true}; arg4{kinds=number,required=false,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=true}
/// Caps: PURE, LOOKUP
/// [formualizer-docgen:schema:end]
impl Function for XMatchFn {
    func_caps!(PURE, LOOKUP, MAY_SPILL);
    fn name(&self) -> &'static str {
        "XMATCH"
    }
    fn min_args(&self) -> usize {
        2
    }
    fn variadic(&self) -> bool {
        true
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        use once_cell::sync::Lazy;
        static SCHEMA: Lazy<Vec<ArgSchema>> = Lazy::new(|| {
            vec![
                // lookup_value
                ArgSchema {
                    kinds: smallvec::smallvec![ArgKind::Any],
                    required: true,
                    by_ref: false,
                    shape: ShapeKind::Scalar,
                    coercion: CoercionPolicy::None,
                    max: None,
                    repeating: None,
                    default: None,
                },
                // lookup_array (range)
                ArgSchema {
                    kinds: smallvec::smallvec![ArgKind::Range],
                    required: true,
                    by_ref: true,
                    shape: ShapeKind::Range,
                    coercion: CoercionPolicy::None,
                    max: None,
                    repeating: None,
                    default: None,
                },
                // match_mode (number) default 0
                // 0 = exact (default), -1 = exact or next smaller, 1 = exact or next larger, 2 = wildcard
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
                // search_mode (number) default 1
                // 1 = first to last (default), -1 = last to first, 2 = binary ascending, -2 = binary descending
                ArgSchema {
                    kinds: smallvec::smallvec![ArgKind::Number],
                    required: false,
                    by_ref: false,
                    shape: ShapeKind::Scalar,
                    coercion: CoercionPolicy::NumberLenientText,
                    max: None,
                    repeating: None,
                    default: Some(LiteralValue::Int(1)),
                },
            ]
        });
        &SCHEMA
    }
    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        _ctx: &dyn FunctionContext<'b>,
    ) -> Result<crate::traits::CalcValue<'b>, ExcelError> {
        if args.len() < 2 {
            return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                ExcelError::new(ExcelErrorKind::Value),
            )));
        }
        let lookup_value = args[0].value()?.into_literal();
        if let LiteralValue::Error(ref e) = lookup_value {
            return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                e.clone(),
            )));
        }
        let lookup_view = match args[1].range_view() {
            Ok(v) => v,
            Err(e) => return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(e))),
        };

        let (lookup_rows, lookup_cols) = lookup_view.dims();

        // XMATCH requires a 1-D lookup array (single row or single column).
        let vertical = if lookup_cols == 1 {
            true
        } else if lookup_rows == 1 {
            false
        } else if lookup_rows == 0 || lookup_cols == 0 {
            return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                ExcelError::new(ExcelErrorKind::Na),
            )));
        } else {
            return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                ExcelError::new(ExcelErrorKind::Value),
            )));
        };

        let lookup_len = if vertical { lookup_rows } else { lookup_cols };

        if lookup_len == 0 {
            return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                ExcelError::new(ExcelErrorKind::Na),
            )));
        }

        let match_mode = if args.len() >= 3 {
            match args[2].value()?.into_literal() {
                LiteralValue::Int(i) => i,
                LiteralValue::Number(n) => n as i64,
                _ => 0,
            }
        } else {
            0
        };
        let search_mode = if args.len() >= 4 {
            match args[3].value()?.into_literal() {
                LiteralValue::Int(i) => i,
                LiteralValue::Number(n) => n as i64,
                _ => 1,
            }
        } else {
            1
        };

        let wildcard = match_mode == 2;
        let needle = lookup_value;
        let prepared_matcher = PreparedLookupMatcher::new(&needle, wildcard);

        let mut found: Option<usize> = None;

        if match_mode == 0 || wildcard {
            // Exact match or wildcard match
            if search_mode == 1 || search_mode == 2 {
                // Forward search (first to last) or binary ascending (treated as forward for exact)
                if lookup_rows > 0 && lookup_cols > 0 {
                    found = super::lookup_utils::find_exact_index_in_view(
                        &lookup_view,
                        &needle,
                        wildcard,
                    )?;
                }
            } else if search_mode == -1 || search_mode == -2 {
                // Reverse search (last to first) or binary descending (treated as reverse for exact)
                for i in (0..lookup_len).rev() {
                    let cand = if vertical {
                        lookup_view.get_cell(i, 0)
                    } else {
                        lookup_view.get_cell(0, i)
                    };
                    if prepared_matcher.matches(&cand) {
                        found = Some(i);
                        break;
                    }
                }
            } else {
                // Fallback linear scan
                for i in 0..lookup_len {
                    let cand = if vertical {
                        lookup_view.get_cell(i, 0)
                    } else {
                        lookup_view.get_cell(0, i)
                    };
                    if prepared_matcher.matches(&cand) {
                        found = Some(i);
                        break;
                    }
                }
            }
        } else if match_mode == -1 || match_mode == 1 {
            // Approximate match: -1 = exact or next smaller, 1 = exact or next larger, in
            // Excel's approximate-match order (numbers < text < logicals, no cross-type
            // coercion) — spreadsheet#939 Z-08.
            let mut best: Option<(usize, LiteralValue)> = None;

            // Determine iteration direction based on search_mode
            let use_reverse = search_mode == -1 || search_mode == -2;
            let indices: Box<dyn Iterator<Item = usize>> = if use_reverse {
                Box::new((0..lookup_len).rev())
            } else {
                Box::new(0..lookup_len)
            };

            // For binary search modes (2, -2), data should be sorted
            // We verify sorting for approximate modes
            if (search_mode == 2 || search_mode == -2) && match_mode != 0 {
                let ascending = search_mode == 2;
                let mut prev: Option<LiteralValue> = None;
                for i in 0..lookup_len {
                    let cand = if vertical {
                        lookup_view.get_cell(i, 0)
                    } else {
                        lookup_view.get_cell(0, i)
                    };
                    if let Some(p) = prev.as_ref() {
                        let order = cmp_for_approx_lookup(p, &cand);
                        let sorted_ok = if ascending {
                            order != std::cmp::Ordering::Greater
                        } else {
                            order != std::cmp::Ordering::Less
                        };
                        if !sorted_ok {
                            return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                                ExcelError::new(ExcelErrorKind::Na),
                            )));
                        }
                    }
                    prev = Some(cand);
                }
            }

            for i in indices {
                let cand = if vertical {
                    lookup_view.get_cell(i, 0)
                } else {
                    lookup_view.get_cell(0, i)
                };

                match cmp_for_approx_lookup(&cand, &needle) {
                    std::cmp::Ordering::Equal => {
                        found = Some(i);
                        break;
                    }
                    side => {
                        if approx_candidate_improves(match_mode, side, &cand, best.as_ref()) {
                            best = Some((i, cand));
                        }
                    }
                }
            }

            if found.is_none() {
                found = best.map(|(i, _)| i);
            }
        } else {
            return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                ExcelError::new(ExcelErrorKind::Value),
            )));
        }

        match found {
            Some(idx) => Ok(crate::traits::CalcValue::Scalar(LiteralValue::Int(
                (idx + 1) as i64,
            ))),
            None => Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                ExcelError::new(ExcelErrorKind::Na),
            ))),
        }
    }
}

/// `XLOOKUP` / `XMATCH` approximate selection: for match_mode -1 (next smaller) a candidate
/// BELOW the needle beats the current best when it is larger; for match_mode 1 (next larger) a
/// candidate ABOVE the needle beats it when it is smaller. `side` is the candidate's order
/// relative to the needle (never `Equal` here).
fn approx_candidate_improves(
    match_mode: i64,
    side: std::cmp::Ordering,
    cand: &LiteralValue,
    best: Option<&(usize, LiteralValue)>,
) -> bool {
    use std::cmp::Ordering;
    let (wanted_side, better) = if match_mode == -1 {
        (Ordering::Less, Ordering::Greater)
    } else {
        (Ordering::Greater, Ordering::Less)
    };
    side == wanted_side && best.is_none_or(|(_, b)| cmp_for_approx_lookup(cand, b) == better)
}

/// A numeric argument that may be a scalar or an array/range of numbers (`SORT`'s
/// `sort_index` / `sort_order`). `Ok(None)` when the argument is blank.
fn int_list_arg<'b>(arg: &ArgumentHandle<'_, 'b>) -> Result<Option<Vec<i64>>, ExcelError> {
    fn to_int(v: &LiteralValue) -> Result<Option<i64>, ExcelError> {
        Ok(match v {
            LiteralValue::Empty => None,
            LiteralValue::Int(i) => Some(*i),
            LiteralValue::Number(n) => Some(n.trunc() as i64),
            LiteralValue::Boolean(b) => Some(*b as i64),
            LiteralValue::Error(e) => return Err(e.clone()),
            other => Some(crate::coercion::to_number_lenient(other)?.trunc() as i64),
        })
    }
    let collect = |rows: Vec<Vec<LiteralValue>>| -> Result<Option<Vec<i64>>, ExcelError> {
        let mut out = Vec::new();
        for v in rows.into_iter().flatten() {
            if let Some(i) = to_int(&v)? {
                out.push(i);
            }
        }
        Ok(Some(out))
    };
    match arg.value()? {
        crate::traits::CalcValue::Scalar(LiteralValue::Array(rows)) => collect(rows),
        crate::traits::CalcValue::Scalar(v) => Ok(to_int(&v)?.map(|i| vec![i])),
        crate::traits::CalcValue::Range(rv) => {
            let mut rows = Vec::new();
            rv.for_each_row(&mut |row| {
                rows.push(row.to_vec());
                Ok(())
            })?;
            collect(rows)
        }
        crate::traits::CalcValue::Callable(_) => Err(ExcelError::new(ExcelErrorKind::Value)),
    }
}

/* ───────────────────────── SORT() ───────────────────────── */

#[derive(Debug)]
pub struct SortFn;
/// Sorts an array by a selected row or column and returns a spilled result.
///
/// `SORT` can order rows (default) or columns.
///
/// # Remarks
/// - Defaults: `sort_index=1`, `sort_order=1` (ascending), `by_col=FALSE`.
/// - `sort_index` is 1-based in the active sort axis; an array such as `{3,1}` sorts by several
///   keys in order, and `sort_order` may be an array of matching length (`{1,-1}`).
/// - `sort_order` is `1` (ascending) or `-1` (descending); any other value returns `#VALUE!`.
/// - Invalid sort indexes return `#VALUE!`.
/// - Empty input returns an empty spill.
///
/// # Examples
/// ```yaml,sandbox
/// title: "Sort rows by second column"
/// grid:
///   A1: "C"
///   B1: 30
///   A2: "A"
///   B2: 10
///   A3: "B"
///   B3: 20
/// formula: '=SORT(A1:B3,2,1,FALSE)'
/// expected: [["A",10],["B",20],["C",30]]
/// ```
///
/// ```yaml,sandbox
/// title: "Sort columns by first row descending"
/// grid:
///   A1: 1
///   B1: 3
///   C1: 2
///   A2: "A"
///   B2: "C"
///   C2: "B"
/// formula: '=SORT(A1:C2,1,-1,TRUE)'
/// expected: [[3,2,1],["C","B","A"]]
/// ```
///
/// ```yaml,docs
/// related:
///   - SORTBY
///   - TAKE
///   - DROP
/// faq:
///   - q: "What changes when by_col is TRUE?"
///     a: "SORT reorders columns instead of rows, and sort_index is interpreted as a row index used as the sort key."
///   - q: "What causes #VALUE! in SORT?"
///     a: "If sort_index is outside the active axis (row or column axis based on by_col), SORT returns #VALUE!."
/// ```
/// [formualizer-docgen:schema:start]
/// Name: SORT
/// Type: SortFn
/// Min args: 1
/// Max args: variadic
/// Variadic: true
/// Signature: SORT(arg1: range@range, arg2?: number@scalar, arg3?: number@scalar, arg4?...: logical@scalar)
/// Arg schema: arg1{kinds=range,required=true,shape=range,by_ref=true,coercion=None,max=None,repeating=None,default=false}; arg2{kinds=number,required=false,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=true}; arg3{kinds=number,required=false,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=true}; arg4{kinds=logical,required=false,shape=scalar,by_ref=false,coercion=Logical,max=None,repeating=None,default=true}
/// Caps: PURE
/// [formualizer-docgen:schema:end]
impl Function for SortFn {
    func_caps!(PURE, MAY_SPILL);
    fn name(&self) -> &'static str {
        "SORT"
    }
    fn min_args(&self) -> usize {
        1
    }
    fn variadic(&self) -> bool {
        true
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        use once_cell::sync::Lazy;
        static SCHEMA: Lazy<Vec<ArgSchema>> = Lazy::new(|| {
            vec![
                // array
                ArgSchema {
                    kinds: smallvec::smallvec![ArgKind::Range],
                    required: true,
                    by_ref: true,
                    shape: ShapeKind::Range,
                    coercion: CoercionPolicy::None,
                    max: None,
                    repeating: None,
                    default: None,
                },
                // sort_index (default 1; a number or an array of key indexes)
                ArgSchema {
                    kinds: smallvec::smallvec![ArgKind::Number, ArgKind::Any],
                    required: false,
                    by_ref: false,
                    shape: ShapeKind::Scalar,
                    coercion: CoercionPolicy::None,
                    max: None,
                    repeating: None,
                    default: Some(LiteralValue::Int(1)),
                },
                // sort_order (default 1 = ascending, -1 = descending; number or array)
                ArgSchema {
                    kinds: smallvec::smallvec![ArgKind::Number, ArgKind::Any],
                    required: false,
                    by_ref: false,
                    shape: ShapeKind::Scalar,
                    coercion: CoercionPolicy::None,
                    max: None,
                    repeating: None,
                    default: Some(LiteralValue::Int(1)),
                },
                // by_col (default FALSE = sort rows, TRUE = sort columns)
                ArgSchema {
                    kinds: smallvec::smallvec![ArgKind::Logical],
                    required: false,
                    by_ref: false,
                    shape: ShapeKind::Scalar,
                    coercion: CoercionPolicy::Logical,
                    max: None,
                    repeating: None,
                    default: Some(LiteralValue::Boolean(false)),
                },
            ]
        });
        &SCHEMA
    }
    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        _ctx: &dyn FunctionContext<'b>,
    ) -> Result<crate::traits::CalcValue<'b>, ExcelError> {
        let view = match args[0].range_view() {
            Ok(v) => v,
            Err(e) => return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(e))),
        };
        let (rows, cols) = view.dims();
        if rows == 0 || cols == 0 {
            return Ok(crate::traits::CalcValue::Range(
                crate::engine::range_view::RangeView::from_owned_rows(vec![], _ctx.date_system()),
            ));
        }

        let value_err = || {
            Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                ExcelError::new(ExcelErrorKind::Value),
            )))
        };

        let sort_indexes: Vec<i64> = if args.len() >= 2 {
            match int_list_arg(&args[1])? {
                Some(v) if !v.is_empty() => v,
                Some(_) => return value_err(),
                None => vec![1],
            }
        } else {
            vec![1]
        };

        let sort_orders: Vec<i64> = if args.len() >= 3 {
            match int_list_arg(&args[2])? {
                Some(v) if !v.is_empty() => v,
                Some(_) => return value_err(),
                None => vec![1],
            }
        } else {
            vec![1]
        };
        // Excel accepts exactly 1 (ascending) or -1 (descending) — SORT(r,1,2) is #VALUE!.
        if sort_orders.iter().any(|o| *o != 1 && *o != -1) {
            return value_err();
        }
        // One order applies to every key; otherwise the arrays must align.
        if sort_orders.len() != 1 && sort_orders.len() != sort_indexes.len() {
            return value_err();
        }

        let by_col = if args.len() >= 4 {
            matches!(args[3].value()?.into_literal(), LiteralValue::Boolean(true))
        } else {
            false
        };

        let axis_len = if by_col { rows } else { cols };
        let mut keys: Vec<(usize, bool)> = Vec::with_capacity(sort_indexes.len());
        for (k, idx) in sort_indexes.iter().enumerate() {
            if *idx < 1 || (*idx as usize) > axis_len {
                return value_err();
            }
            let order = if sort_orders.len() == 1 {
                sort_orders[0]
            } else {
                sort_orders[k]
            };
            keys.push(((*idx - 1) as usize, order >= 0));
        }
        // Excel collation: numbers < text < logicals < errors, descending reverses the type order,
        // blanks last either way (spreadsheet#939 G-05).
        let compare_keys = |a: &[LiteralValue], b: &[LiteralValue]| -> std::cmp::Ordering {
            for (key_idx, ascending) in &keys {
                let cmp = sort_ordering(&a[*key_idx], &b[*key_idx], *ascending);
                if cmp != std::cmp::Ordering::Equal {
                    return cmp;
                }
            }
            std::cmp::Ordering::Equal
        };

        if by_col {
            // Extract columns as vectors
            let mut columns: Vec<(usize, Vec<LiteralValue>)> = Vec::with_capacity(cols);
            for c in 0..cols {
                let mut col_vals: Vec<LiteralValue> = Vec::with_capacity(rows);
                for r in 0..rows {
                    col_vals.push(view.get_cell(r, c));
                }
                columns.push((c, col_vals));
            }

            // Stable sort of the columns by the key rows
            columns.sort_by(|a, b| compare_keys(&a.1, &b.1));

            // Reconstruct the array with sorted columns
            let mut out: Vec<Vec<LiteralValue>> = vec![Vec::with_capacity(cols); rows];
            for (_orig_idx, col_vals) in columns {
                for (r, val) in col_vals.into_iter().enumerate() {
                    out[r].push(val);
                }
            }

            Ok(collapse_if_scalar(out, _ctx.date_system()))
        } else {
            // Extract rows
            let mut row_data: Vec<Vec<LiteralValue>> = Vec::with_capacity(rows);
            for r in 0..rows {
                let mut row_vals: Vec<LiteralValue> = Vec::with_capacity(cols);
                for c in 0..cols {
                    row_vals.push(view.get_cell(r, c));
                }
                row_data.push(row_vals);
            }

            // Stable sort of the rows by the key columns
            row_data.sort_by(|a, b| compare_keys(a, b));

            Ok(collapse_if_scalar(row_data, _ctx.date_system()))
        }
    }
}

/* ───────────────────────── SORTBY() ───────────────────────── */

#[derive(Debug)]
pub struct SortByFn;
/// Sorts an array based on one or more aligned sort-by arrays.
///
/// `SORTBY` separates what is returned (`array`) from what determines ordering (`by_array`).
///
/// # Remarks
/// - Requires at least one `by_array` aligned to the row count of `array`.
/// - `sort_order` is `1` (ascending, the default) or `-1` (descending); other values return `#VALUE!`.
/// - Additional `by_array`/`sort_order` pairs are processed left-to-right (`SORTBY(r,k1,1,k2,-1)`).
/// - Shape mismatches or invalid criteria return `#VALUE!`.
/// - Returns a spilled sorted array.
///
/// # Examples
/// ```yaml,sandbox
/// title: "Sort names by score"
/// grid:
///   A1: "Charlie"
///   A2: "Alice"
///   A3: "Bob"
///   B1: 3
///   B2: 1
///   B3: 2
/// formula: '=SORTBY(A1:A3,B1:B3)'
/// expected: [["Alice"],["Bob"],["Charlie"]]
/// ```
///
/// ```yaml,sandbox
/// title: "Sort descending by key"
/// grid:
///   A1: "Q1"
///   A2: "Q2"
///   A3: "Q3"
///   B1: 100
///   B2: 300
///   B3: 200
/// formula: '=SORTBY(A1:A3,B1:B3,-1)'
/// expected: [["Q2"],["Q3"],["Q1"]]
/// ```
///
/// ```yaml,docs
/// related:
///   - SORT
///   - UNIQUE
///   - FILTER
/// faq:
///   - q: "How are multiple sort criteria applied?"
///     a: "SORTBY evaluates criteria left-to-right, using later by_array values only when earlier criteria compare equal."
///   - q: "Why do I get #VALUE! with SORTBY?"
///     a: "Each by_array must be one-dimensional and aligned to the primary array row count; mismatched shapes return #VALUE!."
/// ```
/// [formualizer-docgen:schema:start]
/// Name: SORTBY
/// Type: SortByFn
/// Min args: 2
/// Max args: variadic
/// Variadic: true
/// Signature: SORTBY(arg1: range@range, arg2...: range|number|any@scalar)
/// Arg schema: arg1{kinds=range,required=true,shape=range,by_ref=true,coercion=None,max=None,repeating=None,default=false}; arg2{kinds=range|number|any,required=true,shape=scalar,by_ref=false,coercion=None,max=None,repeating=Some(1),default=false}
/// Caps: PURE
/// [formualizer-docgen:schema:end]
impl Function for SortByFn {
    func_caps!(PURE, MAY_SPILL);
    fn name(&self) -> &'static str {
        "SORTBY"
    }
    fn min_args(&self) -> usize {
        2
    }
    fn variadic(&self) -> bool {
        true
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        use once_cell::sync::Lazy;
        static SCHEMA: Lazy<Vec<ArgSchema>> = Lazy::new(|| {
            vec![
                // array
                ArgSchema {
                    kinds: smallvec::smallvec![ArgKind::Range],
                    required: true,
                    by_ref: true,
                    shape: ShapeKind::Range,
                    coercion: CoercionPolicy::None,
                    max: None,
                    repeating: None,
                    default: None,
                },
                // by_array1, [sort_order1], [by_array2, [sort_order2]], … — one repeating slot
                // that accepts either a key array (range/array) or a sort order (number); eval
                // tells them apart, so the schema stays arity-agnostic.
                ArgSchema {
                    kinds: smallvec::smallvec![ArgKind::Range, ArgKind::Number, ArgKind::Any],
                    required: true,
                    by_ref: false,
                    shape: ShapeKind::Scalar,
                    coercion: CoercionPolicy::None,
                    max: None,
                    repeating: Some(1),
                    default: None,
                },
            ]
        });
        &SCHEMA
    }
    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        _ctx: &dyn FunctionContext<'b>,
    ) -> Result<crate::traits::CalcValue<'b>, ExcelError> {
        if args.len() < 2 {
            return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                ExcelError::new(ExcelErrorKind::Value),
            )));
        }

        let view = match args[0].range_view() {
            Ok(v) => v,
            Err(e) => return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(e))),
        };
        let (rows, cols) = view.dims();
        if rows == 0 || cols == 0 {
            return Ok(crate::traits::CalcValue::Range(
                crate::engine::range_view::RangeView::from_owned_rows(vec![], _ctx.date_system()),
            ));
        }

        // Parse sort criteria: pairs of (by_array, sort_order)
        // Arguments after array: by_array1, [sort_order1], [by_array2], [sort_order2], ...
        let mut sort_criteria: Vec<(Vec<LiteralValue>, bool)> = Vec::new();
        let mut arg_idx = 1;

        while arg_idx < args.len() {
            // by_array
            let by_view = match args[arg_idx].range_view() {
                Ok(v) => v,
                Err(e) => return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(e))),
            };
            let (by_rows, by_cols) = by_view.dims();

            // The by_array should be 1-D and match the number of rows in the main array
            let by_values: Vec<LiteralValue> = if by_cols == 1 {
                if by_rows != rows {
                    return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                        ExcelError::new(ExcelErrorKind::Value),
                    )));
                }
                (0..by_rows).map(|r| by_view.get_cell(r, 0)).collect()
            } else if by_rows == 1 {
                if by_cols != rows {
                    return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                        ExcelError::new(ExcelErrorKind::Value),
                    )));
                }
                (0..by_cols).map(|c| by_view.get_cell(0, c)).collect()
            } else {
                return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                    ExcelError::new(ExcelErrorKind::Value),
                )));
            };

            arg_idx += 1;

            // sort_order (optional): a scalar number is an order for the key just read; anything
            // else (range, array) is the next by_array and is left for the next iteration.
            let ascending = if arg_idx < args.len() {
                match args[arg_idx].value()? {
                    crate::traits::CalcValue::Scalar(LiteralValue::Int(i)) => {
                        arg_idx += 1;
                        match i {
                            1 => true,
                            -1 => false,
                            _ => {
                                return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                                    ExcelError::new(ExcelErrorKind::Value),
                                )));
                            }
                        }
                    }
                    crate::traits::CalcValue::Scalar(LiteralValue::Number(n)) => {
                        arg_idx += 1;
                        if n == 1.0 {
                            true
                        } else if n == -1.0 {
                            false
                        } else {
                            return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                                ExcelError::new(ExcelErrorKind::Value),
                            )));
                        }
                    }
                    crate::traits::CalcValue::Scalar(LiteralValue::Error(e)) => {
                        return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(e)));
                    }
                    _ => true,
                }
            } else {
                true
            };

            sort_criteria.push((by_values, ascending));
        }

        if sort_criteria.is_empty() {
            return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                ExcelError::new(ExcelErrorKind::Value),
            )));
        }

        // Extract rows with their indices
        let mut indexed_rows: Vec<(usize, Vec<LiteralValue>)> = Vec::with_capacity(rows);
        for r in 0..rows {
            let mut row_vals: Vec<LiteralValue> = Vec::with_capacity(cols);
            for c in 0..cols {
                row_vals.push(view.get_cell(r, c));
            }
            indexed_rows.push((r, row_vals));
        }

        // Sort using all criteria — the same Excel collation as SORT (numbers < text < logicals <
        // errors, blanks last in both directions); each key/order pair is applied left-to-right.
        indexed_rows.sort_by(|a, b| {
            for (by_values, ascending) in &sort_criteria {
                let cmp = sort_ordering(&by_values[a.0], &by_values[b.0], *ascending);
                if cmp != std::cmp::Ordering::Equal {
                    return cmp;
                }
            }
            std::cmp::Ordering::Equal
        });

        // Extract sorted rows
        let out: Vec<Vec<LiteralValue>> = indexed_rows.into_iter().map(|(_, row)| row).collect();

        Ok(collapse_if_scalar(out, _ctx.date_system()))
    }
}

/* ───────────────────────── RANDARRAY() ───────────────────────── */

#[derive(Debug)]
pub struct RandArrayFn;
/// Generates a random spilled array of numbers.
///
/// `RANDARRAY` can return decimal values or whole numbers in a specified range.
///
/// # Remarks
/// - Defaults: `rows=1`, `columns=1`, `min=0`, `max=1`, `whole_number=FALSE`.
/// - The function is non-deterministic and recalculates to new values.
/// - For whole numbers, values are generated in an inclusive integer range.
/// - `rows <= 0`, `columns <= 0`, or invalid integer bounds return `#VALUE!`.
///
/// # Examples
/// ```yaml,sandbox
/// title: "Generate a 2x3 decimal matrix"
/// formula: '=RANDARRAY(2,3)'
/// expected: "2x3 array of decimals in [0,1)"
/// ```
///
/// ```yaml,sandbox
/// title: "Generate six integer dice rolls"
/// formula: '=RANDARRAY(6,1,1,6,TRUE)'
/// expected: "6x1 array of integers from 1 to 6"
/// ```
///
/// ```yaml,docs
/// related:
///   - SEQUENCE
///   - SORT
///   - UNIQUE
/// faq:
///   - q: "Are RANDARRAY bounds inclusive?"
///     a: "In whole_number mode, min and max are inclusive integer bounds; in decimal mode values are generated over the numeric interval from min toward max."
///   - q: "Why does RANDARRAY recalculate on every recalc pass?"
///     a: "RANDARRAY is volatile and non-deterministic by design, so its spilled results are regenerated each evaluation."
/// ```
/// [formualizer-docgen:schema:start]
/// Name: RANDARRAY
/// Type: RandArrayFn
/// Min args: 0
/// Max args: variadic
/// Variadic: true
/// Signature: RANDARRAY(arg1?: number@scalar, arg2?: number@scalar, arg3?: number@scalar, arg4?: number@scalar, arg5?...: logical@scalar)
/// Arg schema: arg1{kinds=number,required=false,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=true}; arg2{kinds=number,required=false,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=true}; arg3{kinds=number,required=false,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=true}; arg4{kinds=number,required=false,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=true}; arg5{kinds=logical,required=false,shape=scalar,by_ref=false,coercion=Logical,max=None,repeating=None,default=true}
/// Caps: none
/// [formualizer-docgen:schema:end]
impl Function for RandArrayFn {
    fn caps(&self) -> crate::function::FnCaps {
        crate::function::FnCaps::VOLATILE | crate::function::FnCaps::MAY_SPILL
    }
    fn name(&self) -> &'static str {
        "RANDARRAY"
    }
    fn min_args(&self) -> usize {
        0
    }
    fn variadic(&self) -> bool {
        true
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        use once_cell::sync::Lazy;
        static SCHEMA: Lazy<Vec<ArgSchema>> = Lazy::new(|| {
            vec![
                // rows (default 1)
                ArgSchema {
                    kinds: smallvec::smallvec![ArgKind::Number],
                    required: false,
                    by_ref: false,
                    shape: ShapeKind::Scalar,
                    coercion: CoercionPolicy::NumberLenientText,
                    max: None,
                    repeating: None,
                    default: Some(LiteralValue::Int(1)),
                },
                // columns (default 1)
                ArgSchema {
                    kinds: smallvec::smallvec![ArgKind::Number],
                    required: false,
                    by_ref: false,
                    shape: ShapeKind::Scalar,
                    coercion: CoercionPolicy::NumberLenientText,
                    max: None,
                    repeating: None,
                    default: Some(LiteralValue::Int(1)),
                },
                // min (default 0)
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
                // max (default 1)
                ArgSchema {
                    kinds: smallvec::smallvec![ArgKind::Number],
                    required: false,
                    by_ref: false,
                    shape: ShapeKind::Scalar,
                    coercion: CoercionPolicy::NumberLenientText,
                    max: None,
                    repeating: None,
                    default: Some(LiteralValue::Int(1)),
                },
                // whole_number (default FALSE)
                ArgSchema {
                    kinds: smallvec::smallvec![ArgKind::Logical],
                    required: false,
                    by_ref: false,
                    shape: ShapeKind::Scalar,
                    coercion: CoercionPolicy::Logical,
                    max: None,
                    repeating: None,
                    default: Some(LiteralValue::Boolean(false)),
                },
            ]
        });
        &SCHEMA
    }
    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        ctx: &dyn FunctionContext<'b>,
    ) -> Result<crate::traits::CalcValue<'b>, ExcelError> {
        use rand::Rng;

        // Extract numbers (allow float but coerce to i64 for dimensions)
        let num = |a: &ArgumentHandle| -> Result<f64, ExcelError> {
            Ok(match a.value()?.into_literal() {
                LiteralValue::Int(i) => i as f64,
                LiteralValue::Number(n) => n,
                LiteralValue::Error(e) => return Err(e),
                _other => {
                    return Err(ExcelError::new(ExcelErrorKind::Value));
                }
            })
        };

        let rows = if !args.is_empty() {
            num(&args[0])? as i64
        } else {
            1
        };
        let cols = if args.len() >= 2 {
            num(&args[1])? as i64
        } else {
            1
        };
        let min_val = if args.len() >= 3 { num(&args[2])? } else { 0.0 };
        let max_val = if args.len() >= 4 { num(&args[3])? } else { 1.0 };
        let whole_number = if args.len() >= 5 {
            matches!(args[4].value()?.into_literal(), LiteralValue::Boolean(true))
        } else {
            false
        };

        // Validate dimensions
        if rows <= 0 || cols <= 0 {
            return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                ExcelError::new(ExcelErrorKind::Value),
            )));
        }

        // Validate min <= max for whole numbers
        if whole_number && min_val > max_val {
            return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                ExcelError::new(ExcelErrorKind::Value),
            )));
        }

        if let Some(e) = generated_array_too_large(rows, cols) {
            return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(e)));
        }

        let mut rng = ctx.rng_for_current(self.function_salt());
        let mut out: Vec<Vec<LiteralValue>> = Vec::with_capacity(rows as usize);

        for _r in 0..rows {
            let mut row_vec: Vec<LiteralValue> = Vec::with_capacity(cols as usize);
            for _c in 0..cols {
                let value = if whole_number {
                    // Generate random integer in range [min, max] inclusive
                    let min_int = min_val.ceil() as i64;
                    let max_int = max_val.floor() as i64;
                    if min_int > max_int {
                        return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                            ExcelError::new(ExcelErrorKind::Value),
                        )));
                    }
                    let rand_int = rng.gen_range(min_int..=max_int);
                    LiteralValue::Int(rand_int)
                } else {
                    // Generate random float in range [min, max)
                    let rand_float = rng.r#gen::<f64>() * (max_val - min_val) + min_val;
                    LiteralValue::Number(rand_float)
                };
                row_vec.push(value);
            }
            out.push(row_vec);
        }

        Ok(collapse_if_scalar(out, ctx.date_system()))
    }
}

/* ───────────────────────── FILTER() ───────────────────────── */

#[derive(Debug)]
pub struct FilterFn;
/// Filters rows from an array using a Boolean include mask.
///
/// `FILTER` returns only rows where the include condition evaluates to true.
///
/// # Remarks
/// - `include` must have the same row count as `array`, or a single row used as broadcast.
/// - A row is kept if any include cell for that row is truthy.
/// - If no rows match, `if_empty` is returned when supplied; otherwise `#CALC!`.
/// - Dimension mismatches return `#VALUE!`.
/// - Results spill as dynamic arrays.
///
/// # Examples
/// ```yaml,sandbox
/// title: "Filter active records"
/// grid:
///   A1: "Ana"
///   B1: true
///   A2: "Bo"
///   B2: false
///   A3: "Cy"
///   B3: true
/// formula: '=FILTER(A1:A3,B1:B3)'
/// expected: [["Ana"],["Cy"]]
/// ```
///
/// ```yaml,sandbox
/// title: "Return fallback when no rows match"
/// grid:
///   A1: 10
///   A2: 20
///   B1: false
///   B2: false
/// formula: '=FILTER(A1:A2,B1:B2,"No matches")'
/// expected: "No matches"
/// ```
///
/// ```yaml,docs
/// related:
///   - XLOOKUP
///   - UNIQUE
///   - SORT
/// faq:
///   - q: "What happens when include has no TRUE rows?"
///     a: "FILTER returns if_empty when provided; otherwise it returns #CALC! to signal an empty result set."
///   - q: "How strict is include shape matching?"
///     a: "include must match array row count or be a single broadcast row; incompatible dimensions return #VALUE!."
/// ```
/// [formualizer-docgen:schema:start]
/// Name: FILTER
/// Type: FilterFn
/// Min args: 2
/// Max args: variadic
/// Variadic: true
/// Signature: FILTER(arg1: range@range, arg2: range@range, arg3?...: any@scalar)
/// Arg schema: arg1{kinds=range,required=true,shape=range,by_ref=true,coercion=None,max=None,repeating=None,default=false}; arg2{kinds=range,required=true,shape=range,by_ref=true,coercion=None,max=None,repeating=None,default=false}; arg3{kinds=any,required=false,shape=scalar,by_ref=false,coercion=None,max=None,repeating=None,default=false}
/// Caps: PURE
/// [formualizer-docgen:schema:end]
impl Function for FilterFn {
    func_caps!(PURE, MAY_SPILL);
    fn name(&self) -> &'static str {
        "FILTER"
    }
    fn min_args(&self) -> usize {
        2
    }
    fn variadic(&self) -> bool {
        true
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        use once_cell::sync::Lazy;
        static SCHEMA: Lazy<Vec<ArgSchema>> = Lazy::new(|| {
            vec![
                // array
                ArgSchema {
                    kinds: smallvec::smallvec![ArgKind::Range],
                    required: true,
                    by_ref: true,
                    shape: ShapeKind::Range,
                    coercion: CoercionPolicy::None,
                    max: None,
                    repeating: None,
                    default: None,
                },
                // include
                ArgSchema {
                    kinds: smallvec::smallvec![ArgKind::Range],
                    required: true,
                    by_ref: true,
                    shape: ShapeKind::Range,
                    coercion: CoercionPolicy::None,
                    max: None,
                    repeating: None,
                    default: None,
                },
                // if_empty optional scalar
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
    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        _ctx: &dyn FunctionContext<'b>,
    ) -> Result<crate::traits::CalcValue<'b>, ExcelError> {
        if args.len() < 2 {
            return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                ExcelError::new(ExcelErrorKind::Value),
            )));
        }
        let array_view = args[0].range_view()?;
        let include_view = args[1].range_view()?;

        let (array_rows, array_cols) = array_view.dims();
        if array_rows == 0 || array_cols == 0 {
            return Ok(crate::traits::CalcValue::Range(
                crate::engine::range_view::RangeView::from_owned_rows(vec![], _ctx.date_system()),
            ));
        }

        let (include_rows, include_cols) = include_view.dims();
        if include_rows != array_rows && include_rows != 1 {
            return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                ExcelError::new(ExcelErrorKind::Value),
            )));
        }

        let mut result: Vec<Vec<LiteralValue>> = Vec::new();
        for r in 0..array_rows {
            let include_r = if include_rows == array_rows { r } else { 0 };
            let mut include = false;
            for c in 0..include_cols {
                if include_view.get_cell(include_r, c).is_truthy() {
                    include = true;
                    break;
                }
            }

            if include {
                let mut row_out: Vec<LiteralValue> = Vec::with_capacity(array_cols);
                for c in 0..array_cols {
                    row_out.push(array_view.get_cell(r, c));
                }
                result.push(row_out);
            }
        }

        if result.is_empty() {
            if args.len() >= 3 {
                return args[2].value();
            }
            return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                ExcelError::new(ExcelErrorKind::Calc),
            )));
        }

        Ok(crate::traits::CalcValue::Range(
            crate::engine::range_view::RangeView::from_owned_rows(result, _ctx.date_system()),
        ))
    }
}

/* ───────────────────────── UNIQUE() ───────────────────────── */

#[derive(Debug)]
pub struct UniqueFn;
/// Returns distinct rows or columns from a range.
///
/// `UNIQUE` preserves first-occurrence order and can optionally return only values that appear once.
///
/// # Remarks
/// - Defaults: `by_col=FALSE`, `exactly_once=FALSE`.
/// - With `by_col=FALSE`, uniqueness is evaluated by full rows.
/// - With `by_col=TRUE`, uniqueness is evaluated by full columns.
/// - With `exactly_once=TRUE`, only entries with frequency 1 are returned.
/// - Result spills as an array.
///
/// # Examples
/// ```yaml,sandbox
/// title: "Unique values by row"
/// grid:
///   A1: "A"
///   A2: "A"
///   A3: "B"
///   A4: "C"
/// formula: '=UNIQUE(A1:A4)'
/// expected: [["A"],["B"],["C"]]
/// ```
///
/// ```yaml,sandbox
/// title: "Only values that appear once"
/// grid:
///   A1: 1
///   A2: 1
///   A3: 2
///   A4: 3
/// formula: '=UNIQUE(A1:A4,FALSE,TRUE)'
/// expected: [[2],[3]]
/// ```
///
/// ```yaml,docs
/// related:
///   - FILTER
///   - SORT
///   - SORTBY
/// faq:
///   - q: "What does exactly_once=TRUE change?"
///     a: "Instead of returning first occurrences, UNIQUE returns only rows or columns whose full key appears exactly one time."
///   - q: "How does by_col affect uniqueness checks?"
///     a: "by_col=FALSE compares entire rows, while by_col=TRUE compares entire columns and spills unique columns."
/// ```
/// [formualizer-docgen:schema:start]
/// Name: UNIQUE
/// Type: UniqueFn
/// Min args: 1
/// Max args: variadic
/// Variadic: true
/// Signature: UNIQUE(arg1: range@range, arg2?: logical@scalar, arg3?...: logical@scalar)
/// Arg schema: arg1{kinds=range,required=true,shape=range,by_ref=true,coercion=None,max=None,repeating=None,default=false}; arg2{kinds=logical,required=false,shape=scalar,by_ref=false,coercion=Logical,max=None,repeating=None,default=true}; arg3{kinds=logical,required=false,shape=scalar,by_ref=false,coercion=Logical,max=None,repeating=None,default=true}
/// Caps: PURE
/// [formualizer-docgen:schema:end]
impl Function for UniqueFn {
    func_caps!(PURE, MAY_SPILL);
    fn name(&self) -> &'static str {
        "UNIQUE"
    }
    fn min_args(&self) -> usize {
        1
    }
    fn variadic(&self) -> bool {
        true
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        use once_cell::sync::Lazy;
        static SCHEMA: Lazy<Vec<ArgSchema>> = Lazy::new(|| {
            vec![
                ArgSchema {
                    kinds: smallvec::smallvec![ArgKind::Range],
                    required: true,
                    by_ref: true,
                    shape: ShapeKind::Range,
                    coercion: CoercionPolicy::None,
                    max: None,
                    repeating: None,
                    default: None,
                },
                ArgSchema {
                    kinds: smallvec::smallvec![ArgKind::Logical],
                    required: false,
                    by_ref: false,
                    shape: ShapeKind::Scalar,
                    coercion: CoercionPolicy::Logical,
                    max: None,
                    repeating: None,
                    default: Some(LiteralValue::Boolean(false)),
                },
                ArgSchema {
                    kinds: smallvec::smallvec![ArgKind::Logical],
                    required: false,
                    by_ref: false,
                    shape: ShapeKind::Scalar,
                    coercion: CoercionPolicy::Logical,
                    max: None,
                    repeating: None,
                    default: Some(LiteralValue::Boolean(false)),
                },
            ]
        });
        &SCHEMA
    }
    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        _ctx: &dyn FunctionContext<'b>,
    ) -> Result<crate::traits::CalcValue<'b>, ExcelError> {
        let view = match args[0].range_view() {
            Ok(v) => v,
            Err(e) => return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(e))),
        };
        let (rows, cols) = view.dims();
        if rows == 0 || cols == 0 {
            return Ok(crate::traits::CalcValue::Range(
                crate::engine::range_view::RangeView::from_owned_rows(vec![], _ctx.date_system()),
            ));
        }

        let by_col = if args.len() >= 2 {
            matches!(args[1].value()?.into_literal(), LiteralValue::Boolean(true))
        } else {
            false
        };
        let exactly_once = if args.len() >= 3 {
            matches!(args[2].value()?.into_literal(), LiteralValue::Boolean(true))
        } else {
            false
        };

        if by_col {
            #[derive(Hash, Eq, PartialEq, Clone)]
            struct ColKey(Vec<LiteralValue>);

            let mut order: Vec<ColKey> = Vec::new();
            let mut counts: HashMap<ColKey, usize> = HashMap::new();

            for c in 0..cols {
                let mut col_vals: Vec<LiteralValue> = Vec::with_capacity(rows);
                for r in 0..rows {
                    col_vals.push(view.get_cell(r, c));
                }
                let key = ColKey(col_vals);
                if !counts.contains_key(&key) {
                    order.push(key.clone());
                }
                *counts.entry(key).or_insert(0) += 1;
            }

            let kept: Vec<Vec<LiteralValue>> = order
                .into_iter()
                .filter(|k| !exactly_once || counts.get(k) == Some(&1))
                .map(|k| k.0)
                .collect();
            // Unique columns stay columns: transpose the kept column vectors back into rows.
            let out: Vec<Vec<LiteralValue>> = (0..rows)
                .map(|r| kept.iter().map(|col| col[r].clone()).collect())
                .collect();
            return Ok(collapse_if_scalar(out, _ctx.date_system()));
        }

        #[derive(Hash, Eq, PartialEq, Clone)]
        struct RowKey(Vec<LiteralValue>);

        let mut order: Vec<RowKey> = Vec::new();
        let mut counts: HashMap<RowKey, usize> = HashMap::new();
        for r in 0..rows {
            let mut row_vals: Vec<LiteralValue> = Vec::with_capacity(cols);
            for c in 0..cols {
                row_vals.push(view.get_cell(r, c));
            }
            let key = RowKey(row_vals);
            if !counts.contains_key(&key) {
                order.push(key.clone());
            }
            *counts.entry(key).or_insert(0) += 1;
        }

        let mut out: Vec<Vec<LiteralValue>> = Vec::new();
        for k in order {
            if !exactly_once || counts.get(&k) == Some(&1) {
                out.push(k.0);
            }
        }
        Ok(collapse_if_scalar(out, _ctx.date_system()))
    }
}

/* ───────────────────────── SEQUENCE() ───────────────────────── */

#[derive(Debug)]
pub struct SequenceFn;
/// Generates a sequential numeric array with configurable size, start, and step.
///
/// `SEQUENCE` fills values row-by-row and returns a dynamic spill.
///
/// # Remarks
/// - Defaults: `columns=1`, `start=1`, `step=1`.
/// - `rows` and `columns` must be positive; otherwise returns `#VALUE!`.
/// - Values are emitted as integers when integral, otherwise as floating-point numbers.
/// - Result spills to the requested dimensions.
///
/// # Examples
/// ```yaml,sandbox
/// title: "Simple vertical sequence"
/// formula: '=SEQUENCE(5)'
/// expected: [[1],[2],[3],[4],[5]]
/// ```
///
/// ```yaml,sandbox
/// title: "2x3 sequence with custom start and step"
/// formula: '=SEQUENCE(2,3,10,5)'
/// expected: [[10,15,20],[25,30,35]]
/// ```
///
/// ```yaml,docs
/// related:
///   - RANDARRAY
///   - TAKE
///   - DROP
/// faq:
///   - q: "What input values are invalid for SEQUENCE?"
///     a: "rows and columns must be positive numbers; zero or negative sizes return #VALUE!."
///   - q: "Does SEQUENCE fill by rows or by columns first?"
///     a: "SEQUENCE fills row-by-row across columns, then continues on the next row using the same step increment."
/// ```
/// [formualizer-docgen:schema:start]
/// Name: SEQUENCE
/// Type: SequenceFn
/// Min args: 1
/// Max args: variadic
/// Variadic: true
/// Signature: SEQUENCE(arg1: number@scalar, arg2?: number@scalar, arg3?: number@scalar, arg4?...: number@scalar)
/// Arg schema: arg1{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg2{kinds=number,required=false,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=true}; arg3{kinds=number,required=false,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=true}; arg4{kinds=number,required=false,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=true}
/// Caps: PURE
/// [formualizer-docgen:schema:end]
impl Function for SequenceFn {
    func_caps!(PURE, MAY_SPILL);
    fn name(&self) -> &'static str {
        "SEQUENCE"
    }
    fn min_args(&self) -> usize {
        1
    }
    fn variadic(&self) -> bool {
        true
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        use once_cell::sync::Lazy;
        static SCHEMA: Lazy<Vec<ArgSchema>> = Lazy::new(|| {
            vec![
                // rows
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
                // columns (default 1)
                ArgSchema {
                    kinds: smallvec::smallvec![ArgKind::Number],
                    required: false,
                    by_ref: false,
                    shape: ShapeKind::Scalar,
                    coercion: CoercionPolicy::NumberLenientText,
                    max: None,
                    repeating: None,
                    default: Some(LiteralValue::Int(1)),
                },
                // start (default 1)
                ArgSchema {
                    kinds: smallvec::smallvec![ArgKind::Number],
                    required: false,
                    by_ref: false,
                    shape: ShapeKind::Scalar,
                    coercion: CoercionPolicy::NumberLenientText,
                    max: None,
                    repeating: None,
                    default: Some(LiteralValue::Int(1)),
                },
                // step (default 1)
                ArgSchema {
                    kinds: smallvec::smallvec![ArgKind::Number],
                    required: false,
                    by_ref: false,
                    shape: ShapeKind::Scalar,
                    coercion: CoercionPolicy::NumberLenientText,
                    max: None,
                    repeating: None,
                    default: Some(LiteralValue::Int(1)),
                },
            ]
        });
        &SCHEMA
    }
    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        _ctx: &dyn FunctionContext<'b>,
    ) -> Result<crate::traits::CalcValue<'b>, ExcelError> {
        // Extract numbers (allow float but coerce to i64 for dimensions)
        let num = |a: &ArgumentHandle| -> Result<f64, ExcelError> {
            Ok(match a.value()?.into_literal() {
                LiteralValue::Int(i) => i as f64,
                LiteralValue::Number(n) => n,
                _other => {
                    return Err(ExcelError::new(ExcelErrorKind::Value));
                }
            })
        };
        let rows_f = num(&args[0])?;
        let rows = rows_f as i64;
        let cols = if args.len() >= 2 {
            num(&args[1])? as i64
        } else {
            1
        };
        let start = if args.len() >= 3 { num(&args[2])? } else { 1.0 };
        let step = if args.len() >= 4 { num(&args[3])? } else { 1.0 };
        // Excel: a zero-sized SEQUENCE is an empty array (#CALC!); negative dims are #VALUE!.
        if rows < 0 || cols < 0 {
            return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                ExcelError::new(ExcelErrorKind::Value),
            )));
        }
        if rows == 0 || cols == 0 {
            return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                ExcelError::new(ExcelErrorKind::Calc),
            )));
        }
        if let Some(e) = generated_array_too_large(rows, cols) {
            return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(e)));
        }
        let mut out: Vec<Vec<LiteralValue>> = Vec::with_capacity(rows as usize);
        let mut current = start;
        for _r in 0..rows {
            let mut row_vec: Vec<LiteralValue> = Vec::with_capacity(cols as usize);
            for _c in 0..cols {
                // Use Int when value integral & within i64 range
                if (current.fract().abs() < 1e-12) && current.abs() < (i64::MAX as f64) {
                    row_vec.push(LiteralValue::Int(current as i64));
                } else {
                    row_vec.push(LiteralValue::Number(current));
                }
                current += step;
            }
            out.push(row_vec);
        }

        Ok(collapse_if_scalar(out, _ctx.date_system()))
    }
}

/* ───────────────────────── TRANSPOSE() ───────────────────────── */

#[derive(Debug)]
pub struct TransposeFn;
/// Swaps rows and columns in an input range.
///
/// `TRANSPOSE` returns a spilled array whose shape is the inverse of the source shape.
///
/// # Remarks
/// - Input with shape `R x C` returns output shape `C x R`.
/// - Empty input returns an empty spill.
/// - Errors in source cells are preserved in transposed positions.
///
/// # Examples
/// ```yaml,sandbox
/// title: "Transpose a row into a column"
/// grid:
///   A1: 10
///   B1: 20
///   C1: 30
/// formula: '=TRANSPOSE(A1:C1)'
/// expected: [[10],[20],[30]]
/// ```
///
/// ```yaml,sandbox
/// title: "Transpose a 2x2 matrix"
/// grid:
///   A1: 1
///   B1: 2
///   A2: 3
///   B2: 4
/// formula: '=TRANSPOSE(A1:B2)'
/// expected: [[1,3],[2,4]]
/// ```
///
/// ```yaml,docs
/// related:
///   - TAKE
///   - DROP
///   - HSTACK
/// faq:
///   - q: "What happens to errors inside the source array?"
///     a: "TRANSPOSE preserves error values and only changes their position in the output matrix."
///   - q: "Does TRANSPOSE return a scalar for 1x1 inputs?"
///     a: "Yes. After transposition, a 1x1 result collapses to a scalar in this engine."
/// ```
/// [formualizer-docgen:schema:start]
/// Name: TRANSPOSE
/// Type: TransposeFn
/// Min args: 1
/// Max args: 1
/// Variadic: false
/// Signature: TRANSPOSE(arg1: range@range)
/// Arg schema: arg1{kinds=range,required=true,shape=range,by_ref=true,coercion=None,max=None,repeating=None,default=false}
/// Caps: PURE
/// [formualizer-docgen:schema:end]
impl Function for TransposeFn {
    func_caps!(PURE, MAY_SPILL);
    fn name(&self) -> &'static str {
        "TRANSPOSE"
    }
    fn min_args(&self) -> usize {
        1
    }
    fn variadic(&self) -> bool {
        false
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        use once_cell::sync::Lazy;
        static SCHEMA: Lazy<Vec<ArgSchema>> = Lazy::new(|| {
            vec![ArgSchema {
                kinds: smallvec::smallvec![ArgKind::Range],
                required: true,
                by_ref: true,
                shape: ShapeKind::Range,
                coercion: CoercionPolicy::None,
                max: None,
                repeating: None,
                default: None,
            }]
        });
        &SCHEMA
    }
    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        _ctx: &dyn FunctionContext<'b>,
    ) -> Result<crate::traits::CalcValue<'b>, ExcelError> {
        let view = match args[0].range_view() {
            Ok(v) => v,
            Err(e) => return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(e))),
        };
        let (rows, cols) = view.dims();
        if rows == 0 || cols == 0 {
            return Ok(crate::traits::CalcValue::Range(
                crate::engine::range_view::RangeView::from_owned_rows(vec![], _ctx.date_system()),
            ));
        }

        let mut out: Vec<Vec<LiteralValue>> = vec![Vec::with_capacity(rows); cols];
        for (c, col) in out.iter_mut().enumerate().take(cols) {
            for r in 0..rows {
                col.push(view.get_cell(r, c));
            }
        }
        Ok(collapse_if_scalar(out, _ctx.date_system()))
    }
}

/* ───────────────────────── TAKE() ───────────────────────── */

/// Excel's `#CALC!` for a function whose array result has no rows or no columns (an "empty
/// array"); TAKE / DROP return it instead of a `0×n` shape (rowsncolumns/spreadsheet#939 G-01).
fn empty_array_calc_error<'b>() -> crate::traits::CalcValue<'b> {
    crate::traits::CalcValue::Scalar(LiteralValue::Error(
        ExcelError::new(ExcelErrorKind::Calc).with_message("Empty array"),
    ))
}

#[derive(Debug)]
pub struct TakeFn;
/// Returns a subset from the start or end of rows and optional columns.
///
/// `TAKE` extracts leading or trailing portions of an array based on signed counts.
///
/// # Remarks
/// - Positive counts take from the start; negative counts take from the end.
/// - `rows` is required; `columns` is optional.
/// - Absolute counts larger than the source dimension return `#VALUE!`.
/// - Empty selections return an empty spill.
///
/// # Examples
/// ```yaml,sandbox
/// title: "Take top two rows"
/// grid:
///   A1: 10
///   A2: 20
///   A3: 30
/// formula: '=TAKE(A1:A3,2)'
/// expected: [[10],[20]]
/// ```
///
/// ```yaml,sandbox
/// title: "Take last row and last two columns"
/// grid:
///   A1: 1
///   B1: 2
///   C1: 3
///   A2: 4
///   B2: 5
///   C2: 6
/// formula: '=TAKE(A1:C2,-1,-2)'
/// expected: [[5,6]]
/// ```
///
/// ```yaml,docs
/// related:
///   - DROP
///   - CHOOSEROWS
///   - CHOOSECOLS
/// faq:
///   - q: "How are negative rows or columns interpreted?"
///     a: "Negative counts take from the end of the array, so TAKE(...,-1) returns the last row and TAKE(...,,-1) returns the last column."
///   - q: "When does TAKE return #VALUE!?"
///     a: "If the absolute requested row or column count exceeds source dimensions, TAKE returns #VALUE!."
/// ```
/// [formualizer-docgen:schema:start]
/// Name: TAKE
/// Type: TakeFn
/// Min args: 2
/// Max args: variadic
/// Variadic: true
/// Signature: TAKE(arg1: range@range, arg2: number@scalar, arg3?...: number@scalar)
/// Arg schema: arg1{kinds=range,required=true,shape=range,by_ref=true,coercion=None,max=None,repeating=None,default=false}; arg2{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg3{kinds=number,required=false,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}
/// Caps: PURE
/// [formualizer-docgen:schema:end]
impl Function for TakeFn {
    func_caps!(PURE, MAY_SPILL);
    fn name(&self) -> &'static str {
        "TAKE"
    }
    fn min_args(&self) -> usize {
        2
    }
    fn variadic(&self) -> bool {
        true
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        use once_cell::sync::Lazy;
        static SCHEMA: Lazy<Vec<ArgSchema>> = Lazy::new(|| {
            vec![
                ArgSchema {
                    kinds: smallvec::smallvec![ArgKind::Range],
                    required: true,
                    by_ref: true,
                    shape: ShapeKind::Range,
                    coercion: CoercionPolicy::None,
                    max: None,
                    repeating: None,
                    default: None,
                },
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
                ArgSchema {
                    kinds: smallvec::smallvec![ArgKind::Number],
                    required: false,
                    by_ref: false,
                    shape: ShapeKind::Scalar,
                    coercion: CoercionPolicy::NumberLenientText,
                    max: None,
                    repeating: None,
                    default: None,
                },
            ]
        });
        &SCHEMA
    }
    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        _ctx: &dyn FunctionContext<'b>,
    ) -> Result<crate::traits::CalcValue<'b>, ExcelError> {
        let view = match args[0].range_view() {
            Ok(v) => v,
            Err(e) => return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(e))),
        };
        let (rows, cols) = view.dims();
        if rows == 0 || cols == 0 {
            return Ok(empty_array_calc_error());
        }

        let height = rows as i64;
        let width = cols as i64;

        let num = |a: &ArgumentHandle| -> Result<i64, ExcelError> {
            Ok(match a.value()?.into_literal() {
                LiteralValue::Int(i) => i,
                LiteralValue::Number(n) => n as i64,
                _ => 0,
            })
        };
        // `TAKE(rng,,2)`: a skipped rows slot keeps every row; a skipped cols slot keeps every column.
        let take_rows = if args[1].is_skipped() {
            height
        } else {
            num(&args[1])?
        };
        let take_cols = if args.len() >= 3 && !args[2].is_skipped() {
            Some(num(&args[2])?)
        } else {
            None
        };

        if take_rows.abs() > height {
            return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                ExcelError::new(ExcelErrorKind::Value),
            )));
        }

        let (row_start, row_end) = if take_rows >= 0 {
            (0usize, take_rows as usize)
        } else {
            ((height + take_rows) as usize, height as usize)
        };

        let (col_start, col_end) = if let Some(tc) = take_cols {
            if tc.abs() > width {
                return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                    ExcelError::new(ExcelErrorKind::Value),
                )));
            }
            if tc >= 0 {
                (0usize, tc as usize)
            } else {
                ((width + tc) as usize, width as usize)
            }
        } else {
            (0usize, width as usize)
        };

        // Excel: TAKE(rng,0) / DROP(rng,<every row>) is `#CALC!` — there is no zero-sized array.
        if row_start >= row_end || col_start >= col_end {
            return Ok(empty_array_calc_error());
        }

        let mut out: Vec<Vec<LiteralValue>> = Vec::with_capacity(row_end - row_start);
        for r in row_start..row_end {
            let mut row_out: Vec<LiteralValue> = Vec::with_capacity(col_end - col_start);
            for c in col_start..col_end {
                row_out.push(view.get_cell(r, c));
            }
            out.push(row_out);
        }

        Ok(collapse_if_scalar(out, _ctx.date_system()))
    }
}

/* ───────────────────────── DROP() ───────────────────────── */

#[derive(Debug)]
pub struct DropFn;
/// Removes rows and optional columns from the start or end of an array.
///
/// `DROP` is the complement of `TAKE` and returns the remaining spilled subset.
///
/// # Remarks
/// - Positive counts drop from the start; negative counts drop from the end.
/// - `rows` is required; `columns` is optional.
/// - Dropping all rows or columns yields an empty spill.
/// - Invalid source references propagate errors.
///
/// # Examples
/// ```yaml,sandbox
/// title: "Drop header row"
/// grid:
///   A1: "Month"
///   A2: "Jan"
///   A3: "Feb"
/// formula: '=DROP(A1:A3,1)'
/// expected: [["Jan"],["Feb"]]
/// ```
///
/// ```yaml,sandbox
/// title: "Drop last column"
/// grid:
///   A1: 10
///   B1: 20
///   C1: 30
/// formula: '=DROP(A1:C1,0,-1)'
/// expected: [[10,20]]
/// ```
///
/// ```yaml,docs
/// related:
///   - TAKE
///   - CHOOSEROWS
///   - CHOOSECOLS
/// faq:
///   - q: "What does a negative drop count mean?"
///     a: "Negative counts drop from the end, so DROP(array,0,-1) removes the last column and keeps the leading columns."
///   - q: "What if DROP removes every row or column?"
///     a: "The result is an empty spill rather than an error when all rows or all columns are removed."
/// ```
/// [formualizer-docgen:schema:start]
/// Name: DROP
/// Type: DropFn
/// Min args: 2
/// Max args: variadic
/// Variadic: true
/// Signature: DROP(arg1: range@range, arg2: number@scalar, arg3?...: number@scalar)
/// Arg schema: arg1{kinds=range,required=true,shape=range,by_ref=true,coercion=None,max=None,repeating=None,default=false}; arg2{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg3{kinds=number,required=false,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}
/// Caps: PURE
/// [formualizer-docgen:schema:end]
impl Function for DropFn {
    func_caps!(PURE, MAY_SPILL);
    fn name(&self) -> &'static str {
        "DROP"
    }
    fn min_args(&self) -> usize {
        2
    }
    fn variadic(&self) -> bool {
        true
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        use once_cell::sync::Lazy;
        static SCHEMA: Lazy<Vec<ArgSchema>> = Lazy::new(|| {
            vec![
                ArgSchema {
                    kinds: smallvec::smallvec![ArgKind::Range],
                    required: true,
                    by_ref: true,
                    shape: ShapeKind::Range,
                    coercion: CoercionPolicy::None,
                    max: None,
                    repeating: None,
                    default: None,
                },
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
                ArgSchema {
                    kinds: smallvec::smallvec![ArgKind::Number],
                    required: false,
                    by_ref: false,
                    shape: ShapeKind::Scalar,
                    coercion: CoercionPolicy::NumberLenientText,
                    max: None,
                    repeating: None,
                    default: None,
                },
            ]
        });
        &SCHEMA
    }
    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        _ctx: &dyn FunctionContext<'b>,
    ) -> Result<crate::traits::CalcValue<'b>, ExcelError> {
        let view = match args[0].range_view() {
            Ok(v) => v,
            Err(e) => return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(e))),
        };
        let (rows, cols) = view.dims();
        if rows == 0 || cols == 0 {
            return Ok(empty_array_calc_error());
        }

        let height = rows as i64;
        let width = cols as i64;

        let num = |a: &ArgumentHandle| -> Result<i64, ExcelError> {
            Ok(match a.value()?.into_literal() {
                LiteralValue::Int(i) => i,
                LiteralValue::Number(n) => n as i64,
                _ => 0,
            })
        };
        // `DROP(rng,,-2)`: a skipped rows slot drops nothing.
        let drop_rows = if args[1].is_skipped() {
            0
        } else {
            num(&args[1])?
        };
        let drop_cols = if args.len() >= 3 && !args[2].is_skipped() {
            Some(num(&args[2])?)
        } else {
            None
        };

        let (row_start, row_end) = if drop_rows >= 0 {
            ((drop_rows as usize).min(height as usize), height as usize)
        } else {
            (0usize, (height + drop_rows).max(0) as usize)
        };

        let (col_start, col_end) = if let Some(dc) = drop_cols {
            if dc >= 0 {
                ((dc as usize).min(width as usize), width as usize)
            } else {
                (0usize, (width + dc).max(0) as usize)
            }
        } else {
            (0usize, width as usize)
        };

        // Excel: TAKE(rng,0) / DROP(rng,<every row>) is `#CALC!` — there is no zero-sized array.
        if row_start >= row_end || col_start >= col_end {
            return Ok(empty_array_calc_error());
        }

        let mut out: Vec<Vec<LiteralValue>> = Vec::with_capacity(row_end - row_start);
        for r in row_start..row_end {
            let mut row_out: Vec<LiteralValue> = Vec::with_capacity(col_end - col_start);
            for c in col_start..col_end {
                row_out.push(view.get_cell(r, c));
            }
            out.push(row_out);
        }

        Ok(collapse_if_scalar(out, _ctx.date_system()))
    }
}

/* ───────────────────────── TRIMRANGE() ───────────────────────── */

#[derive(Debug)]
pub struct TrimRangeFn;
/// Excludes the blank rows and columns at the edges of a range or array.
///
/// `TRIMRANGE` scans in from each edge until it meets a non-blank cell and drops the blank
/// rows / columns outside it, so a formula over a whole column (`TRIMRANGE(A:A)`) works on the
/// data actually present. It is the function form of Excel's trim-ref operators
/// (`A1.:.E10`, `A1:.E10`, `A1.:E10`).
///
/// # Remarks
/// - `trim_rows` / `trim_cols`: `0` keeps every row / column, `1` trims leading blanks, `2`
///   trims trailing blanks, `3` (default) trims both. Any other value is `#VALUE!`.
/// - Only truly empty cells are blank; a formula result of `""` and a `0` keep their row.
/// - A range with no non-blank cell trims to nothing and is `#REF!`, as in Excel.
///
/// # Examples
/// ```yaml,sandbox
/// title: "Trim trailing blanks"
/// grid:
///   A1: 1
///   A2: 2
///   A3: 3
/// formula: '=TRIMRANGE(A1:A5)'
/// expected: [[1],[2],[3]]
/// ```
///
/// ```yaml,sandbox
/// title: "Keep leading blank rows"
/// grid:
///   A2: 5
///   A3: 6
/// formula: '=TRIMRANGE(A1:A4,2)'
/// expected: [[null],[5],[6]]
/// ```
///
/// ```yaml,docs
/// related:
///   - DROP
///   - TAKE
///   - FILTER
/// faq:
///   - q: "Why is TRIMRANGE(A1:A5) #REF! on an empty range?"
///     a: "Every row is blank, so nothing is left after trimming; Excel reports the empty result as #REF!."
///   - q: "Does a cell containing an empty string count as blank?"
///     a: "No. Only cells with no value are blank; a formula returning \"\" keeps its row and column."
/// ```
/// [formualizer-docgen:schema:start]
/// Name: TRIMRANGE
/// Type: TrimRangeFn
/// Min args: 1
/// Max args: variadic
/// Variadic: true
/// Signature: TRIMRANGE(arg1: range@range, arg2?: number@scalar, arg3?...: number@scalar)
/// Arg schema: arg1{kinds=range,required=true,shape=range,by_ref=true,coercion=None,max=None,repeating=None,default=false}; arg2{kinds=number,required=false,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg3{kinds=number,required=false,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}
/// Caps: PURE
/// [formualizer-docgen:schema:end]
impl Function for TrimRangeFn {
    func_caps!(PURE, MAY_SPILL);
    fn name(&self) -> &'static str {
        "TRIMRANGE"
    }
    fn min_args(&self) -> usize {
        1
    }
    fn variadic(&self) -> bool {
        true
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        use once_cell::sync::Lazy;
        static SCHEMA: Lazy<Vec<ArgSchema>> = Lazy::new(|| {
            let mode = || ArgSchema {
                kinds: smallvec::smallvec![ArgKind::Number],
                required: false,
                by_ref: false,
                shape: ShapeKind::Scalar,
                coercion: CoercionPolicy::NumberLenientText,
                max: None,
                repeating: None,
                default: None,
            };
            vec![
                ArgSchema {
                    kinds: smallvec::smallvec![ArgKind::Range],
                    required: true,
                    by_ref: true,
                    shape: ShapeKind::Range,
                    coercion: CoercionPolicy::None,
                    max: None,
                    repeating: None,
                    default: None,
                },
                mode(),
                mode(),
            ]
        });
        &SCHEMA
    }
    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        ctx: &dyn FunctionContext<'b>,
    ) -> Result<crate::traits::CalcValue<'b>, ExcelError> {
        let scalar_err =
            |e: ExcelError| Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(e)));
        if args.is_empty() || args.len() > 3 {
            return scalar_err(ExcelError::new_value());
        }
        // Trim mode: 0 none, 1 leading, 2 trailing, 3 both (default). Only an omitted or
        // skipped slot (`TRIMRANGE(rng,,2)`) is the default; a blank cell in the slot is a
        // number argument like any other and coerces to 0 (keep every row / column).
        let mode = |idx: usize| -> Result<u8, ExcelError> {
            let Some(arg) = args.get(idx) else {
                return Ok(3);
            };
            if arg.is_skipped() {
                return Ok(3);
            }
            match arg.value()?.into_literal() {
                LiteralValue::Error(e) => Err(e),
                v => {
                    let n = crate::builtins::utils::coerce_num_for(arg, &v)?.trunc();
                    if (0.0..=3.0).contains(&n) {
                        Ok(n as u8)
                    } else {
                        Err(ExcelError::new_value())
                    }
                }
            }
        };
        let trim_rows = match mode(1) {
            Ok(m) => m,
            Err(e) => return scalar_err(e),
        };
        let trim_cols = match mode(2) {
            Ok(m) => m,
            Err(e) => return scalar_err(e),
        };
        let view = match args[0].range_view() {
            Ok(v) => v,
            Err(e) => return scalar_err(e),
        };
        let (rows, cols) = view.dims();
        if rows == 0 || cols == 0 {
            return scalar_err(ExcelError::new_ref());
        }
        let blank_row =
            |r: usize| (0..cols).all(|c| matches!(view.get_cell(r, c), LiteralValue::Empty));
        let blank_col =
            |c: usize| (0..rows).all(|r| matches!(view.get_cell(r, c), LiteralValue::Empty));

        let (mut row_start, mut row_end) = (0usize, rows);
        if trim_rows & 1 != 0 {
            while row_start < row_end && blank_row(row_start) {
                row_start += 1;
            }
        }
        if trim_rows & 2 != 0 {
            while row_end > row_start && blank_row(row_end - 1) {
                row_end -= 1;
            }
        }
        let (mut col_start, mut col_end) = (0usize, cols);
        if trim_cols & 1 != 0 {
            while col_start < col_end && blank_col(col_start) {
                col_start += 1;
            }
        }
        if trim_cols & 2 != 0 {
            while col_end > col_start && blank_col(col_end - 1) {
                col_end -= 1;
            }
        }
        if row_start >= row_end || col_start >= col_end {
            return scalar_err(ExcelError::new_ref());
        }

        let mut out: Vec<Vec<LiteralValue>> = Vec::with_capacity(row_end - row_start);
        for r in row_start..row_end {
            let mut row_out: Vec<LiteralValue> = Vec::with_capacity(col_end - col_start);
            for c in col_start..col_end {
                row_out.push(view.get_cell(r, c));
            }
            out.push(row_out);
        }
        Ok(collapse_if_scalar(out, ctx.date_system()))
    }
}

pub fn register_builtins() {
    use crate::function_registry::register_builtin;
    use std::sync::Arc;
    register_builtin(Arc::new(XLookupFn));
    register_builtin(Arc::new(FilterFn));
    register_builtin(Arc::new(UniqueFn));
    register_builtin(Arc::new(SequenceFn));
    register_builtin(Arc::new(TransposeFn));
    register_builtin(Arc::new(TakeFn));
    register_builtin(Arc::new(DropFn));
    register_builtin(Arc::new(TrimRangeFn));
    register_builtin(Arc::new(XMatchFn));
    register_builtin(Arc::new(SortFn));
    register_builtin(Arc::new(SortByFn));
    register_builtin(Arc::new(RandArrayFn));
    register_builtin(Arc::new(super::groupby::GroupByFn));
    register_builtin(Arc::new(super::groupby::PivotByFn));
}

/* ───────────────────────── tests ───────────────────────── */

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_workbook::TestWorkbook;
    use crate::traits::ArgumentHandle;
    use formualizer_parse::parser::{ASTNode, ASTNodeType, ReferenceType};
    use std::sync::Arc;

    #[test]
    fn test_all_dynamic_functions_registered() {
        // Ensure builtins are registered
        crate::builtins::load_builtins();

        let functions = [
            "XLOOKUP",
            "FILTER",
            "UNIQUE",
            "SEQUENCE",
            "TRANSPOSE",
            "TAKE",
            "DROP",
            "TRIMRANGE",
            "XMATCH",
            "SORT",
            "SORTBY",
            "RANDARRAY",
            "GROUPBY",
            "PIVOTBY",
        ];

        for name in &functions {
            let result = crate::function_registry::get("", name);
            assert!(result.is_some(), "Function {} should be registered", name);
        }
    }

    fn lit(v: LiteralValue) -> ASTNode {
        ASTNode::new(ASTNodeType::Literal(v), None)
    }

    fn range(r: &str, sr: u32, sc: u32, er: u32, ec: u32) -> ASTNode {
        ASTNode::new(
            ASTNodeType::Reference {
                original: r.into(),
                reference: ReferenceType::range(None, Some(sr), Some(sc), Some(er), Some(ec)),
            },
            None,
        )
    }

    #[test]
    fn xlookup_basic_exact_and_if_not_found() {
        let wb = TestWorkbook::new().with_function(Arc::new(XLookupFn));
        let wb = wb
            .with_cell_a1("Sheet1", "A1", LiteralValue::Text("a".into()))
            .with_cell_a1("Sheet1", "A2", LiteralValue::Text("b".into()))
            .with_cell_a1("Sheet1", "B1", LiteralValue::Int(10))
            .with_cell_a1("Sheet1", "B2", LiteralValue::Int(20));
        let ctx = wb.interpreter();
        let lookup_range = range("A1:A2", 1, 1, 2, 1);
        let return_range = range("B1:B2", 1, 2, 2, 2);
        let f = ctx.context.get_function("", "XLOOKUP").unwrap();
        let key_b = lit(LiteralValue::Text("b".into()));
        let args = vec![
            ArgumentHandle::new(&key_b, &ctx),
            ArgumentHandle::new(&lookup_range, &ctx),
            ArgumentHandle::new(&return_range, &ctx),
        ];
        let v = f
            .dispatch(&args, &ctx.function_context(None))
            .unwrap()
            .into_literal();
        assert_eq!(v, LiteralValue::Number(20.0));
        let key_missing = lit(LiteralValue::Text("z".into()));
        let if_nf = lit(LiteralValue::Text("NF".into()));
        let args_nf = vec![
            ArgumentHandle::new(&key_missing, &ctx),
            ArgumentHandle::new(&lookup_range, &ctx),
            ArgumentHandle::new(&return_range, &ctx),
            ArgumentHandle::new(&if_nf, &ctx),
        ];
        let v_nf = f
            .dispatch(&args_nf, &ctx.function_context(None))
            .unwrap()
            .into_literal();
        assert_eq!(v_nf, LiteralValue::Text("NF".into()));
    }

    #[test]
    fn xlookup_match_modes_next_smaller_larger() {
        let wb = TestWorkbook::new().with_function(Arc::new(XLookupFn));
        let wb = wb
            .with_cell_a1("Sheet1", "A1", LiteralValue::Int(10))
            .with_cell_a1("Sheet1", "A2", LiteralValue::Int(20))
            .with_cell_a1("Sheet1", "A3", LiteralValue::Int(30))
            .with_cell_a1("Sheet1", "B1", LiteralValue::Int(1))
            .with_cell_a1("Sheet1", "B2", LiteralValue::Int(2))
            .with_cell_a1("Sheet1", "B3", LiteralValue::Int(3));
        let ctx = wb.interpreter();
        let lookup_range = range("A1:A3", 1, 1, 3, 1);
        let return_range = range("B1:B3", 1, 2, 3, 2);
        let f = ctx.context.get_function("", "XLOOKUP").unwrap();
        let needle_25 = lit(LiteralValue::Int(25));
        let mm_next_smaller = lit(LiteralValue::Int(-1));
        let nf_text = lit(LiteralValue::Text("NF".into()));
        let args_smaller = vec![
            ArgumentHandle::new(&needle_25, &ctx),
            ArgumentHandle::new(&lookup_range, &ctx),
            ArgumentHandle::new(&return_range, &ctx),
            ArgumentHandle::new(&nf_text, &ctx),
            ArgumentHandle::new(&mm_next_smaller, &ctx),
        ];
        let v_smaller = f
            .dispatch(&args_smaller, &ctx.function_context(None))
            .unwrap()
            .into_literal();
        assert_eq!(v_smaller, LiteralValue::Number(2.0));
        let mm_next_larger = lit(LiteralValue::Int(1));
        let nf_text2 = lit(LiteralValue::Text("NF".into()));
        let args_larger = vec![
            ArgumentHandle::new(&needle_25, &ctx),
            ArgumentHandle::new(&lookup_range, &ctx),
            ArgumentHandle::new(&return_range, &ctx),
            ArgumentHandle::new(&nf_text2, &ctx),
            ArgumentHandle::new(&mm_next_larger, &ctx),
        ];
        let v_larger = f
            .dispatch(&args_larger, &ctx.function_context(None))
            .unwrap()
            .into_literal();
        assert_eq!(v_larger, LiteralValue::Number(3.0));
    }

    #[test]
    fn xlookup_wildcard_and_not_found_default_na() {
        let wb = TestWorkbook::new().with_function(Arc::new(XLookupFn));
        let wb = wb
            .with_cell_a1("Sheet1", "A1", LiteralValue::Text("Alpha".into()))
            .with_cell_a1("Sheet1", "A2", LiteralValue::Text("Beta".into()))
            .with_cell_a1("Sheet1", "A3", LiteralValue::Text("Gamma".into()))
            .with_cell_a1("Sheet1", "B1", LiteralValue::Int(100))
            .with_cell_a1("Sheet1", "B2", LiteralValue::Int(200))
            .with_cell_a1("Sheet1", "B3", LiteralValue::Int(300));
        let ctx = wb.interpreter();
        let lookup_range = range("A1:A3", 1, 1, 3, 1);
        let return_range = range("B1:B3", 1, 2, 3, 2);
        let f = ctx.context.get_function("", "XLOOKUP").unwrap();
        // Wildcard should match Beta (*et*) with match_mode 2
        let pattern = lit(LiteralValue::Text("*et*".into()));
        let match_mode_wild = lit(LiteralValue::Int(2));
        let nf_binding = lit(LiteralValue::Text("NF".into()));
        let args_wild = vec![
            ArgumentHandle::new(&pattern, &ctx),
            ArgumentHandle::new(&lookup_range, &ctx),
            ArgumentHandle::new(&return_range, &ctx),
            ArgumentHandle::new(&nf_binding, &ctx),
            ArgumentHandle::new(&match_mode_wild, &ctx),
        ];
        let v_wild = f
            .dispatch(&args_wild, &ctx.function_context(None))
            .unwrap()
            .into_literal();
        assert_eq!(v_wild, LiteralValue::Number(200.0));
        // Escaped wildcard literal ~* should not match Beta
        let pattern_lit_star = lit(LiteralValue::Text("~*eta".into()));
        let args_lit = vec![
            ArgumentHandle::new(&pattern_lit_star, &ctx),
            ArgumentHandle::new(&lookup_range, &ctx),
            ArgumentHandle::new(&return_range, &ctx),
            ArgumentHandle::new(&nf_binding, &ctx),
            ArgumentHandle::new(&match_mode_wild, &ctx),
        ];
        let v_lit = f
            .dispatch(&args_lit, &ctx.function_context(None))
            .unwrap()
            .into_literal();
        match v_lit {
            LiteralValue::Text(s) => assert_eq!(s, "NF"),
            other => panic!("expected NF text got {other:?}"),
        }
        // Not found without if_not_found -> #N/A
        let missing = lit(LiteralValue::Text("Zeta".into()));
        let args_nf = vec![
            ArgumentHandle::new(&missing, &ctx),
            ArgumentHandle::new(&lookup_range, &ctx),
            ArgumentHandle::new(&return_range, &ctx),
        ];
        let v_nf = f
            .dispatch(&args_nf, &ctx.function_context(None))
            .unwrap()
            .into_literal();
        match v_nf {
            LiteralValue::Error(e) => assert_eq!(e.kind, ExcelErrorKind::Na),
            other => panic!("expected #N/A got {other:?}"),
        }
    }

    #[test]
    fn xlookup_unicode_exact_and_wildcard_are_case_insensitive() {
        let wb = TestWorkbook::new().with_function(Arc::new(XLookupFn));
        let wb = wb
            .with_cell_a1("Sheet1", "A1", LiteralValue::Text("ИВАН".into()))
            .with_cell_a1("Sheet1", "A2", LiteralValue::Text("Петр".into()))
            .with_cell_a1("Sheet1", "A3", LiteralValue::Text("Иванов".into()))
            .with_cell_a1("Sheet1", "B1", LiteralValue::Int(100))
            .with_cell_a1("Sheet1", "B2", LiteralValue::Int(200))
            .with_cell_a1("Sheet1", "B3", LiteralValue::Int(300));
        let ctx = wb.interpreter();
        let lookup_range = range("A1:A3", 1, 1, 3, 1);
        let return_range = range("B1:B3", 1, 2, 3, 2);
        let f = ctx.context.get_function("", "XLOOKUP").unwrap();
        let nf = lit(LiteralValue::Text("NF".into()));

        let exact = lit(LiteralValue::Text("иван".into()));
        let exact_args = vec![
            ArgumentHandle::new(&exact, &ctx),
            ArgumentHandle::new(&lookup_range, &ctx),
            ArgumentHandle::new(&return_range, &ctx),
            ArgumentHandle::new(&nf, &ctx),
        ];
        let exact_v = f
            .dispatch(&exact_args, &ctx.function_context(None))
            .unwrap()
            .into_literal();
        assert_eq!(exact_v, LiteralValue::Number(100.0));

        let wildcard = lit(LiteralValue::Text("ив?н*".into()));
        let wildcard_mode = lit(LiteralValue::Int(2));
        let wildcard_args = vec![
            ArgumentHandle::new(&wildcard, &ctx),
            ArgumentHandle::new(&lookup_range, &ctx),
            ArgumentHandle::new(&return_range, &ctx),
            ArgumentHandle::new(&nf, &ctx),
            ArgumentHandle::new(&wildcard_mode, &ctx),
        ];
        let wildcard_v = f
            .dispatch(&wildcard_args, &ctx.function_context(None))
            .unwrap()
            .into_literal();
        assert_eq!(wildcard_v, LiteralValue::Number(100.0));
    }

    #[test]
    fn xlookup_unicode_reverse_search_uses_prepared_matcher() {
        let wb = TestWorkbook::new().with_function(Arc::new(XLookupFn));
        let wb = wb
            .with_cell_a1("Sheet1", "A1", LiteralValue::Text("ИВАН".into()))
            .with_cell_a1("Sheet1", "A2", LiteralValue::Text("Петр".into()))
            .with_cell_a1("Sheet1", "A3", LiteralValue::Text("Иванов".into()))
            .with_cell_a1("Sheet1", "A4", LiteralValue::Text("ИВАН".into()))
            .with_cell_a1("Sheet1", "B1", LiteralValue::Int(100))
            .with_cell_a1("Sheet1", "B2", LiteralValue::Int(200))
            .with_cell_a1("Sheet1", "B3", LiteralValue::Int(300))
            .with_cell_a1("Sheet1", "B4", LiteralValue::Int(400));
        let ctx = wb.interpreter();
        let lookup_range = range("A1:A4", 1, 1, 4, 1);
        let return_range = range("B1:B4", 1, 2, 4, 2);
        let f = ctx.context.get_function("", "XLOOKUP").unwrap();
        let nf = lit(LiteralValue::Text("NF".into()));
        let reverse = lit(LiteralValue::Int(-1));
        let exact_mode = lit(LiteralValue::Int(0));

        let exact = lit(LiteralValue::Text("иван".into()));
        let exact_args = vec![
            ArgumentHandle::new(&exact, &ctx),
            ArgumentHandle::new(&lookup_range, &ctx),
            ArgumentHandle::new(&return_range, &ctx),
            ArgumentHandle::new(&nf, &ctx),
            ArgumentHandle::new(&exact_mode, &ctx),
            ArgumentHandle::new(&reverse, &ctx),
        ];
        let exact_v = f
            .dispatch(&exact_args, &ctx.function_context(None))
            .unwrap()
            .into_literal();
        assert_eq!(exact_v, LiteralValue::Number(400.0));

        let wildcard = lit(LiteralValue::Text("ив?н*".into()));
        let wildcard_mode = lit(LiteralValue::Int(2));
        let wildcard_args = vec![
            ArgumentHandle::new(&wildcard, &ctx),
            ArgumentHandle::new(&lookup_range, &ctx),
            ArgumentHandle::new(&return_range, &ctx),
            ArgumentHandle::new(&nf, &ctx),
            ArgumentHandle::new(&wildcard_mode, &ctx),
            ArgumentHandle::new(&reverse, &ctx),
        ];
        let wildcard_v = f
            .dispatch(&wildcard_args, &ctx.function_context(None))
            .unwrap()
            .into_literal();
        assert_eq!(wildcard_v, LiteralValue::Number(400.0));
    }

    #[test]
    fn xlookup_reverse_search_mode_picks_last() {
        let wb = TestWorkbook::new().with_function(Arc::new(XLookupFn));
        let wb = wb
            .with_cell_a1("Sheet1", "A1", LiteralValue::Int(1))
            .with_cell_a1("Sheet1", "A2", LiteralValue::Int(2))
            .with_cell_a1("Sheet1", "A3", LiteralValue::Int(1))
            .with_cell_a1("Sheet1", "B1", LiteralValue::Text("First".into()))
            .with_cell_a1("Sheet1", "B2", LiteralValue::Text("Mid".into()))
            .with_cell_a1("Sheet1", "B3", LiteralValue::Text("Last".into()));
        let ctx = wb.interpreter();
        let lookup_range = range("A1:A3", 1, 1, 3, 1);
        let return_range = range("B1:B3", 1, 2, 3, 2);
        let f = ctx.context.get_function("", "XLOOKUP").unwrap();
        let needle_one = lit(LiteralValue::Int(1));
        let search_rev = lit(LiteralValue::Int(-1));
        let nf_binding2 = lit(LiteralValue::Text("NF".into()));
        let match_mode_zero = lit(LiteralValue::Int(0));
        let args_rev = vec![
            ArgumentHandle::new(&needle_one, &ctx),
            ArgumentHandle::new(&lookup_range, &ctx),
            ArgumentHandle::new(&return_range, &ctx),
            ArgumentHandle::new(&nf_binding2, &ctx),
            /* match_mode default */ ArgumentHandle::new(&match_mode_zero, &ctx),
            ArgumentHandle::new(&search_rev, &ctx),
        ];
        let v_rev = f
            .dispatch(&args_rev, &ctx.function_context(None))
            .unwrap()
            .into_literal();
        assert_eq!(v_rev, LiteralValue::Text("Last".into()));
    }

    #[test]
    fn xlookup_horizontal_returns_column_vector_for_matrix_return() {
        let wb = TestWorkbook::new().with_function(Arc::new(XLookupFn));
        let wb = wb
            .with_cell_a1("Sheet1", "A1", LiteralValue::Int(10))
            .with_cell_a1("Sheet1", "B1", LiteralValue::Int(20))
            .with_cell_a1("Sheet1", "C1", LiteralValue::Int(30))
            .with_cell_a1("Sheet1", "A2", LiteralValue::Int(1))
            .with_cell_a1("Sheet1", "B2", LiteralValue::Int(2))
            .with_cell_a1("Sheet1", "C2", LiteralValue::Int(3))
            .with_cell_a1("Sheet1", "A3", LiteralValue::Int(4))
            .with_cell_a1("Sheet1", "B3", LiteralValue::Int(5))
            .with_cell_a1("Sheet1", "C3", LiteralValue::Int(6));
        let ctx = wb.interpreter();
        let lookup_range = range("A1:C1", 1, 1, 1, 3);
        let return_range = range("A2:C3", 2, 1, 3, 3);
        let f = ctx.context.get_function("", "XLOOKUP").unwrap();
        let needle = lit(LiteralValue::Int(20));
        let args = vec![
            ArgumentHandle::new(&needle, &ctx),
            ArgumentHandle::new(&lookup_range, &ctx),
            ArgumentHandle::new(&return_range, &ctx),
        ];
        let v = f
            .dispatch(&args, &ctx.function_context(None))
            .unwrap()
            .into_literal();
        match v {
            LiteralValue::Array(a) => {
                assert_eq!(
                    a,
                    vec![
                        vec![LiteralValue::Number(2.0)],
                        vec![LiteralValue::Number(5.0)]
                    ]
                );
            }
            other => panic!("expected array got {other:?}"),
        }
    }

    #[test]
    fn xlookup_vertical_returns_row_vector_for_matrix_return() {
        let wb = TestWorkbook::new().with_function(Arc::new(XLookupFn));
        let wb = wb
            .with_cell_a1("Sheet1", "A1", LiteralValue::Int(10))
            .with_cell_a1("Sheet1", "A2", LiteralValue::Int(20))
            .with_cell_a1("Sheet1", "A3", LiteralValue::Int(30))
            .with_cell_a1("Sheet1", "B1", LiteralValue::Int(101))
            .with_cell_a1("Sheet1", "C1", LiteralValue::Int(102))
            .with_cell_a1("Sheet1", "D1", LiteralValue::Int(103))
            .with_cell_a1("Sheet1", "B2", LiteralValue::Int(201))
            .with_cell_a1("Sheet1", "C2", LiteralValue::Int(202))
            .with_cell_a1("Sheet1", "D2", LiteralValue::Int(203))
            .with_cell_a1("Sheet1", "B3", LiteralValue::Int(301))
            .with_cell_a1("Sheet1", "C3", LiteralValue::Int(302))
            .with_cell_a1("Sheet1", "D3", LiteralValue::Int(303));
        let ctx = wb.interpreter();
        let lookup_range = range("A1:A3", 1, 1, 3, 1);
        let return_range = range("B1:D3", 1, 2, 3, 4);
        let f = ctx.context.get_function("", "XLOOKUP").unwrap();
        let needle = lit(LiteralValue::Int(20));
        let args = vec![
            ArgumentHandle::new(&needle, &ctx),
            ArgumentHandle::new(&lookup_range, &ctx),
            ArgumentHandle::new(&return_range, &ctx),
        ];
        let v = f
            .dispatch(&args, &ctx.function_context(None))
            .unwrap()
            .into_literal();
        match v {
            LiteralValue::Array(a) => {
                assert_eq!(
                    a,
                    vec![vec![
                        LiteralValue::Number(201.0),
                        LiteralValue::Number(202.0),
                        LiteralValue::Number(203.0)
                    ]]
                );
            }
            other => panic!("expected array got {other:?}"),
        }
    }

    #[test]
    fn filter_basic_and_if_empty() {
        let wb = TestWorkbook::new().with_function(Arc::new(FilterFn));
        let wb = wb
            .with_cell_a1("Sheet1", "A1", LiteralValue::Int(1))
            .with_cell_a1("Sheet1", "A2", LiteralValue::Int(2))
            .with_cell_a1("Sheet1", "B1", LiteralValue::Int(10))
            .with_cell_a1("Sheet1", "B2", LiteralValue::Int(20))
            .with_cell_a1("Sheet1", "C1", LiteralValue::Boolean(true))
            .with_cell_a1("Sheet1", "C2", LiteralValue::Boolean(false));
        let ctx = wb.interpreter();
        let array_range = range("A1:B2", 1, 1, 2, 2);
        let include_range = range("C1:C2", 1, 3, 2, 3);
        let f = ctx.context.get_function("", "FILTER").unwrap();
        let args = vec![
            ArgumentHandle::new(&array_range, &ctx),
            ArgumentHandle::new(&include_range, &ctx),
        ];
        let v = f
            .dispatch(&args, &ctx.function_context(None))
            .unwrap()
            .into_literal();
        match v {
            LiteralValue::Array(a) => {
                assert_eq!(a.len(), 1);
                assert_eq!(
                    a[0],
                    vec![LiteralValue::Number(1.0), LiteralValue::Number(10.0)]
                );
            }
            other => panic!("expected array got {other:?}"),
        }
        let wb2 = wb
            .with_cell_a1("Sheet1", "C1", LiteralValue::Boolean(false))
            .with_cell_a1("Sheet1", "C2", LiteralValue::Boolean(false));
        let ctx2 = wb2.interpreter();
        let f2 = ctx2.context.get_function("", "FILTER").unwrap();
        let empty_text = lit(LiteralValue::Text("EMPTY".into()));
        let args_empty = vec![
            ArgumentHandle::new(&array_range, &ctx2),
            ArgumentHandle::new(&include_range, &ctx2),
            ArgumentHandle::new(&empty_text, &ctx2),
        ];
        let v_empty = f2
            .dispatch(&args_empty, &ctx2.function_context(None))
            .unwrap()
            .into_literal();
        assert_eq!(v_empty, LiteralValue::Text("EMPTY".into()));
    }

    #[test]
    fn unique_basic_and_exactly_once() {
        let wb = TestWorkbook::new().with_function(Arc::new(UniqueFn));
        let wb = wb
            .with_cell_a1("Sheet1", "A1", LiteralValue::Int(1))
            .with_cell_a1("Sheet1", "A2", LiteralValue::Int(1))
            .with_cell_a1("Sheet1", "A3", LiteralValue::Int(2))
            .with_cell_a1("Sheet1", "A4", LiteralValue::Int(3));
        let ctx = wb.interpreter();
        let range = range("A1:A4", 1, 1, 4, 1);
        let f = ctx.context.get_function("", "UNIQUE").unwrap();
        let args = vec![ArgumentHandle::new(&range, &ctx)];
        let v = f
            .dispatch(&args, &ctx.function_context(None))
            .unwrap()
            .into_literal();
        match v {
            LiteralValue::Array(a) => {
                assert_eq!(a.len(), 3);
                assert_eq!(a[0][0], LiteralValue::Number(1.0));
            }
            _ => panic!("expected array"),
        }
    }

    #[test]
    fn generated_array_guard_boundaries() {
        // Exactly at the total-cells cap: allowed (4096 * 4096 == 2^24).
        assert!(generated_array_too_large(4096, 4096).is_none());
        // One past the cap: #NUM!.
        assert!(generated_array_too_large(4096, 4097).is_some());
        assert_eq!(
            generated_array_too_large(4096, 4097).map(|e| e.kind),
            Some(formualizer_common::ExcelErrorKind::Num)
        );
        // Per-dimension Excel sheet limits: can never spill → #SPILL! (Excel), not #NUM!.
        assert!(generated_array_too_large(1_048_576, 1).is_none());
        assert_eq!(
            generated_array_too_large(1_048_577, 1).map(|e| e.kind),
            Some(formualizer_common::ExcelErrorKind::Spill)
        );
        assert!(generated_array_too_large(1, 16_384).is_none());
        assert_eq!(
            generated_array_too_large(1, 16_385).map(|e| e.kind),
            Some(formualizer_common::ExcelErrorKind::Spill)
        );
        // checked_mul overflow path fails closed.
        assert!(generated_array_too_large(i64::MAX, i64::MAX).is_some());
    }

    #[test]
    fn sequence_oversized_returns_spill_error_quickly() {
        let wb = TestWorkbook::new().with_function(Arc::new(SequenceFn));
        let ctx = wb.interpreter();
        let f = ctx.context.get_function("", "SEQUENCE").unwrap();
        // =SEQUENCE(1e6, 1e6): 10^12 cells must fail fast, not allocate.
        let rows = lit(LiteralValue::Number(1e6));
        let cols = lit(LiteralValue::Number(1e6));
        let args = vec![
            ArgumentHandle::new(&rows, &ctx),
            ArgumentHandle::new(&cols, &ctx),
        ];
        let started = std::time::Instant::now();
        let v = f
            .dispatch(&args, &ctx.function_context(None))
            .unwrap()
            .into_literal();
        let elapsed = started.elapsed();
        match v {
            LiteralValue::Error(e) => {
                assert_eq!(e.kind, formualizer_common::ExcelErrorKind::Spill)
            }
            other => panic!("expected #SPILL! got {other:?}"),
        }
        assert!(
            elapsed.as_millis() < 250,
            "oversized SEQUENCE must short-circuit, took {elapsed:?}"
        );
        // Taller than the sheet but a legal total: still #SPILL! (Excel), without allocating.
        let rows = lit(LiteralValue::Int(2_000_000));
        let args = vec![ArgumentHandle::new(&rows, &ctx)];
        match f
            .dispatch(&args, &ctx.function_context(None))
            .unwrap()
            .into_literal()
        {
            LiteralValue::Error(e) => {
                assert_eq!(e.kind, formualizer_common::ExcelErrorKind::Spill)
            }
            other => panic!("expected #SPILL! got {other:?}"),
        }

        // Total-cells cap: full-sheet request (1,048,576 x 16,384) is #NUM!.
        let rows = lit(LiteralValue::Int(1_048_576));
        let cols = lit(LiteralValue::Int(16_384));
        let args = vec![
            ArgumentHandle::new(&rows, &ctx),
            ArgumentHandle::new(&cols, &ctx),
        ];
        match f
            .dispatch(&args, &ctx.function_context(None))
            .unwrap()
            .into_literal()
        {
            LiteralValue::Error(e) => {
                assert_eq!(e.kind, formualizer_common::ExcelErrorKind::Num)
            }
            other => panic!("expected #NUM! got {other:?}"),
        }
    }

    #[test]
    fn sequence_large_but_legal_succeeds() {
        // A full Excel column (1,048,576 x 1) stays under the cap and works.
        let wb = TestWorkbook::new().with_function(Arc::new(SequenceFn));
        let ctx = wb.interpreter();
        let f = ctx.context.get_function("", "SEQUENCE").unwrap();
        let rows = lit(LiteralValue::Int(1_048_576));
        let args = vec![ArgumentHandle::new(&rows, &ctx)];
        match f
            .dispatch(&args, &ctx.function_context(None))
            .unwrap()
            .into_literal()
        {
            LiteralValue::Array(a) => {
                assert_eq!(a.len(), 1_048_576);
                assert_eq!(a[0][0], LiteralValue::Number(1.0));
            }
            other => panic!("expected array got {other:?}"),
        }
    }

    #[test]
    fn sequence_negative_dims_are_value_error_and_zero_dims_are_calc_error() {
        let wb = TestWorkbook::new().with_function(Arc::new(SequenceFn));
        let ctx = wb.interpreter();
        let f = ctx.context.get_function("", "SEQUENCE").unwrap();
        // Excel: a zero-sized result is an empty array (#CALC!); negative dims are #VALUE!.
        for (r, c, kind) in [
            (0i64, 5i64, formualizer_common::ExcelErrorKind::Calc),
            (5, 0, formualizer_common::ExcelErrorKind::Calc),
            (-3, 5, formualizer_common::ExcelErrorKind::Value),
            (5, -1, formualizer_common::ExcelErrorKind::Value),
        ] {
            let rows = lit(LiteralValue::Int(r));
            let cols = lit(LiteralValue::Int(c));
            let args = vec![
                ArgumentHandle::new(&rows, &ctx),
                ArgumentHandle::new(&cols, &ctx),
            ];
            match f
                .dispatch(&args, &ctx.function_context(None))
                .unwrap()
                .into_literal()
            {
                LiteralValue::Error(e) => assert_eq!(e.kind, kind, "SEQUENCE({r},{c})"),
                other => panic!("expected {kind:?} for SEQUENCE({r},{c}), got {other:?}"),
            }
        }
    }

    #[test]
    fn randarray_oversized_returns_spill_error_quickly() {
        let wb = TestWorkbook::new().with_function(Arc::new(RandArrayFn));
        let ctx = wb.interpreter();
        let f = ctx.context.get_function("", "RANDARRAY").unwrap();
        // Wider than a sheet → #SPILL! (Excel), without allocating.
        let rows = lit(LiteralValue::Number(1e6));
        let cols = lit(LiteralValue::Number(1e6));
        let args = vec![
            ArgumentHandle::new(&rows, &ctx),
            ArgumentHandle::new(&cols, &ctx),
        ];
        let started = std::time::Instant::now();
        let v = f
            .dispatch(&args, &ctx.function_context(None))
            .unwrap()
            .into_literal();
        let elapsed = started.elapsed();
        match v {
            LiteralValue::Error(e) => {
                assert_eq!(e.kind, formualizer_common::ExcelErrorKind::Spill)
            }
            other => panic!("expected #SPILL! got {other:?}"),
        }
        assert!(
            elapsed.as_millis() < 250,
            "oversized RANDARRAY must short-circuit, took {elapsed:?}"
        );
        // Fits the sheet's dimensions but exceeds the allocation guard → #NUM!.
        let rows = lit(LiteralValue::Int(8192));
        let cols = lit(LiteralValue::Int(8192));
        let args = vec![
            ArgumentHandle::new(&rows, &ctx),
            ArgumentHandle::new(&cols, &ctx),
        ];
        match f
            .dispatch(&args, &ctx.function_context(None))
            .unwrap()
            .into_literal()
        {
            LiteralValue::Error(e) => {
                assert_eq!(e.kind, formualizer_common::ExcelErrorKind::Num)
            }
            other => panic!("expected #NUM! got {other:?}"),
        }
    }

    #[test]
    fn sequence_basic_rows_cols_step() {
        let wb = TestWorkbook::new().with_function(Arc::new(SequenceFn));
        let ctx = wb.interpreter();
        let f = ctx.context.get_function("", "SEQUENCE").unwrap();
        let rows = lit(LiteralValue::Int(2));
        let cols = lit(LiteralValue::Int(3));
        let start = lit(LiteralValue::Int(5));
        let step = lit(LiteralValue::Int(2));
        let args = vec![
            ArgumentHandle::new(&rows, &ctx),
            ArgumentHandle::new(&cols, &ctx),
            ArgumentHandle::new(&start, &ctx),
            ArgumentHandle::new(&step, &ctx),
        ];
        let v = f
            .dispatch(&args, &ctx.function_context(None))
            .unwrap()
            .into_literal();
        match v {
            LiteralValue::Array(a) => {
                assert_eq!(a.len(), 2);
                assert_eq!(a[0][0], LiteralValue::Number(5.0));
            }
            other => panic!("expected array got {other:?}"),
        }
    }

    #[test]
    fn transpose_basic() {
        let wb = TestWorkbook::new().with_function(Arc::new(TransposeFn));
        let wb = wb
            .with_cell_a1("Sheet1", "A1", LiteralValue::Int(1))
            .with_cell_a1("Sheet1", "A2", LiteralValue::Int(2))
            .with_cell_a1("Sheet1", "B1", LiteralValue::Int(10))
            .with_cell_a1("Sheet1", "B2", LiteralValue::Int(20));
        let ctx = wb.interpreter();
        let arr = range("A1:B2", 1, 1, 2, 2);
        let f = ctx.context.get_function("", "TRANSPOSE").unwrap();
        let args = vec![ArgumentHandle::new(&arr, &ctx)];
        let v = f
            .dispatch(&args, &ctx.function_context(None))
            .unwrap()
            .into_literal();
        match v {
            LiteralValue::Array(a) => {
                assert_eq!(a.len(), 2);
                assert_eq!(
                    a[0],
                    vec![LiteralValue::Number(1.0), LiteralValue::Number(2.0)]
                );
            }
            other => panic!("expected array got {other:?}"),
        }
    }

    #[test]
    fn take_basic() {
        let wb = TestWorkbook::new().with_function(Arc::new(TakeFn));
        let wb = wb
            .with_cell_a1("Sheet1", "A1", LiteralValue::Int(1))
            .with_cell_a1("Sheet1", "A2", LiteralValue::Int(2));
        let ctx = wb.interpreter();
        let arr = range("A1:A2", 1, 1, 2, 1);
        let f = ctx.context.get_function("", "TAKE").unwrap();
        let one = lit(LiteralValue::Int(1));
        let args = vec![
            ArgumentHandle::new(&arr, &ctx),
            ArgumentHandle::new(&one, &ctx),
        ];
        let v = f
            .dispatch(&args, &ctx.function_context(None))
            .unwrap()
            .into_literal();
        assert_eq!(v, LiteralValue::Number(1.0));
    }

    /// rowsncolumns/spreadsheet#939 G-01: Excel has no zero-sized array — `TAKE(rng,0)`,
    /// `TAKE(rng,,0)` and `DROP(rng,<every row>)` show `#CALC!`. Returning a `0×n` range instead
    /// reached the spill planner with an inverted rectangle and trapped the engine.
    #[test]
    fn take_and_drop_empty_result_is_calc_error() {
        let wb = TestWorkbook::new()
            .with_function(Arc::new(TakeFn))
            .with_function(Arc::new(DropFn))
            .with_cell_a1("Sheet1", "A1", LiteralValue::Int(1))
            .with_cell_a1("Sheet1", "A2", LiteralValue::Int(2))
            .with_cell_a1("Sheet1", "B1", LiteralValue::Int(3))
            .with_cell_a1("Sheet1", "B2", LiteralValue::Int(4));
        let ctx = wb.interpreter();
        let arr = range("A1:B2", 1, 1, 2, 2);
        let fctx = ctx.function_context(None);
        let calc = |name: &str, args: &[ASTNode]| {
            let f = ctx.context.get_function("", name).unwrap();
            let handles: Vec<ArgumentHandle> =
                args.iter().map(|a| ArgumentHandle::new(a, &ctx)).collect();
            f.dispatch(&handles, &fctx).unwrap().into_literal()
        };
        let zero = lit(LiteralValue::Int(0));
        let two = lit(LiteralValue::Int(2));
        let neg_two = lit(LiteralValue::Int(-2));
        let five = lit(LiteralValue::Int(5));
        for (label, v) in [
            ("TAKE(A1:B2,0)", calc("TAKE", &[arr.clone(), zero.clone()])),
            (
                "TAKE(A1:B2,2,0)",
                calc("TAKE", &[arr.clone(), two.clone(), zero.clone()]),
            ),
            ("DROP(A1:B2,2)", calc("DROP", &[arr.clone(), two.clone()])),
            (
                "DROP(A1:B2,-2)",
                calc("DROP", &[arr.clone(), neg_two.clone()]),
            ),
            ("DROP(A1:B2,5)", calc("DROP", &[arr.clone(), five.clone()])),
            (
                "DROP(A1:B2,0,2)",
                calc("DROP", &[arr.clone(), zero.clone(), two.clone()]),
            ),
        ] {
            assert!(
                matches!(&v, LiteralValue::Error(e) if e.kind == ExcelErrorKind::Calc),
                "{label} → {v:?}"
            );
        }
        // A non-empty result is unchanged.
        assert_eq!(
            calc("TAKE", &[arr.clone(), two.clone(), two.clone()]),
            LiteralValue::Array(vec![
                vec![LiteralValue::Number(1.0), LiteralValue::Number(3.0)],
                vec![LiteralValue::Number(2.0), LiteralValue::Number(4.0)],
            ])
        );
    }

    #[test]
    fn drop_basic() {
        let wb = TestWorkbook::new().with_function(Arc::new(DropFn));
        let wb = wb
            .with_cell_a1("Sheet1", "A1", LiteralValue::Int(1))
            .with_cell_a1("Sheet1", "A2", LiteralValue::Int(2));
        let ctx = wb.interpreter();
        let arr = range("A1:A2", 1, 1, 2, 1);
        let f = ctx.context.get_function("", "DROP").unwrap();
        let one = lit(LiteralValue::Int(1));
        let args = vec![
            ArgumentHandle::new(&arr, &ctx),
            ArgumentHandle::new(&one, &ctx),
        ];
        let v = f
            .dispatch(&args, &ctx.function_context(None))
            .unwrap()
            .into_literal();
        assert_eq!(v, LiteralValue::Number(2.0));
    }

    #[test]
    fn xmatch_exact_match_default() {
        let wb = TestWorkbook::new().with_function(Arc::new(XMatchFn));
        let wb = wb
            .with_cell_a1("Sheet1", "A1", LiteralValue::Text("apple".into()))
            .with_cell_a1("Sheet1", "A2", LiteralValue::Text("banana".into()))
            .with_cell_a1("Sheet1", "A3", LiteralValue::Text("cherry".into()));
        let ctx = wb.interpreter();
        let lookup_range = range("A1:A3", 1, 1, 3, 1);
        let f = ctx.context.get_function("", "XMATCH").unwrap();
        let key = lit(LiteralValue::Text("banana".into()));
        let args = vec![
            ArgumentHandle::new(&key, &ctx),
            ArgumentHandle::new(&lookup_range, &ctx),
        ];
        let v = f
            .dispatch(&args, &ctx.function_context(None))
            .unwrap()
            .into_literal();
        assert_eq!(v, LiteralValue::Int(2));
    }

    #[test]
    fn xmatch_exact_or_next_smaller() {
        let wb = TestWorkbook::new().with_function(Arc::new(XMatchFn));
        let wb = wb
            .with_cell_a1("Sheet1", "A1", LiteralValue::Int(10))
            .with_cell_a1("Sheet1", "A2", LiteralValue::Int(20))
            .with_cell_a1("Sheet1", "A3", LiteralValue::Int(30));
        let ctx = wb.interpreter();
        let lookup_range = range("A1:A3", 1, 1, 3, 1);
        let f = ctx.context.get_function("", "XMATCH").unwrap();
        let needle = lit(LiteralValue::Int(25));
        let match_mode = lit(LiteralValue::Int(-1)); // exact or next smaller
        let args = vec![
            ArgumentHandle::new(&needle, &ctx),
            ArgumentHandle::new(&lookup_range, &ctx),
            ArgumentHandle::new(&match_mode, &ctx),
        ];
        let v = f
            .dispatch(&args, &ctx.function_context(None))
            .unwrap()
            .into_literal();
        assert_eq!(v, LiteralValue::Int(2)); // 20 is the largest <= 25
    }

    #[test]
    fn xmatch_exact_or_next_larger() {
        let wb = TestWorkbook::new().with_function(Arc::new(XMatchFn));
        let wb = wb
            .with_cell_a1("Sheet1", "A1", LiteralValue::Int(10))
            .with_cell_a1("Sheet1", "A2", LiteralValue::Int(20))
            .with_cell_a1("Sheet1", "A3", LiteralValue::Int(30));
        let ctx = wb.interpreter();
        let lookup_range = range("A1:A3", 1, 1, 3, 1);
        let f = ctx.context.get_function("", "XMATCH").unwrap();
        let needle = lit(LiteralValue::Int(25));
        let match_mode = lit(LiteralValue::Int(1)); // exact or next larger
        let args = vec![
            ArgumentHandle::new(&needle, &ctx),
            ArgumentHandle::new(&lookup_range, &ctx),
            ArgumentHandle::new(&match_mode, &ctx),
        ];
        let v = f
            .dispatch(&args, &ctx.function_context(None))
            .unwrap()
            .into_literal();
        assert_eq!(v, LiteralValue::Int(3)); // 30 is the smallest >= 25
    }

    #[test]
    fn xmatch_wildcard() {
        let wb = TestWorkbook::new().with_function(Arc::new(XMatchFn));
        let wb = wb
            .with_cell_a1("Sheet1", "A1", LiteralValue::Text("alpha".into()))
            .with_cell_a1("Sheet1", "A2", LiteralValue::Text("beta".into()))
            .with_cell_a1("Sheet1", "A3", LiteralValue::Text("gamma".into()));
        let ctx = wb.interpreter();
        let lookup_range = range("A1:A3", 1, 1, 3, 1);
        let f = ctx.context.get_function("", "XMATCH").unwrap();
        let pattern = lit(LiteralValue::Text("*eta".into()));
        let match_mode = lit(LiteralValue::Int(2)); // wildcard
        let args = vec![
            ArgumentHandle::new(&pattern, &ctx),
            ArgumentHandle::new(&lookup_range, &ctx),
            ArgumentHandle::new(&match_mode, &ctx),
        ];
        let v = f
            .dispatch(&args, &ctx.function_context(None))
            .unwrap()
            .into_literal();
        assert_eq!(v, LiteralValue::Int(2)); // "beta" matches "*eta"
    }

    #[test]
    fn xmatch_unicode_wildcard_is_case_insensitive() {
        let wb = TestWorkbook::new().with_function(Arc::new(XMatchFn));
        let wb = wb
            .with_cell_a1("Sheet1", "A1", LiteralValue::Text("ИВАН".into()))
            .with_cell_a1("Sheet1", "A2", LiteralValue::Text("Петр".into()))
            .with_cell_a1("Sheet1", "A3", LiteralValue::Text("Иванов".into()));
        let ctx = wb.interpreter();
        let lookup_range = range("A1:A3", 1, 1, 3, 1);
        let f = ctx.context.get_function("", "XMATCH").unwrap();
        let pattern = lit(LiteralValue::Text("ив?н*".into()));
        let match_mode = lit(LiteralValue::Int(2));
        let args = vec![
            ArgumentHandle::new(&pattern, &ctx),
            ArgumentHandle::new(&lookup_range, &ctx),
            ArgumentHandle::new(&match_mode, &ctx),
        ];
        let v = f
            .dispatch(&args, &ctx.function_context(None))
            .unwrap()
            .into_literal();
        assert_eq!(v, LiteralValue::Int(1));
    }

    #[test]
    fn xmatch_reverse_search() {
        let wb = TestWorkbook::new().with_function(Arc::new(XMatchFn));
        let wb = wb
            .with_cell_a1("Sheet1", "A1", LiteralValue::Int(1))
            .with_cell_a1("Sheet1", "A2", LiteralValue::Int(2))
            .with_cell_a1("Sheet1", "A3", LiteralValue::Int(1)); // duplicate
        let ctx = wb.interpreter();
        let lookup_range = range("A1:A3", 1, 1, 3, 1);
        let f = ctx.context.get_function("", "XMATCH").unwrap();
        let needle = lit(LiteralValue::Int(1));
        let match_mode = lit(LiteralValue::Int(0));
        let search_mode = lit(LiteralValue::Int(-1)); // last to first
        let args = vec![
            ArgumentHandle::new(&needle, &ctx),
            ArgumentHandle::new(&lookup_range, &ctx),
            ArgumentHandle::new(&match_mode, &ctx),
            ArgumentHandle::new(&search_mode, &ctx),
        ];
        let v = f
            .dispatch(&args, &ctx.function_context(None))
            .unwrap()
            .into_literal();
        assert_eq!(v, LiteralValue::Int(3)); // last occurrence of 1
    }

    #[test]
    fn xmatch_not_found() {
        let wb = TestWorkbook::new().with_function(Arc::new(XMatchFn));
        let wb = wb
            .with_cell_a1("Sheet1", "A1", LiteralValue::Int(1))
            .with_cell_a1("Sheet1", "A2", LiteralValue::Int(2))
            .with_cell_a1("Sheet1", "A3", LiteralValue::Int(3));
        let ctx = wb.interpreter();
        let lookup_range = range("A1:A3", 1, 1, 3, 1);
        let f = ctx.context.get_function("", "XMATCH").unwrap();
        let needle = lit(LiteralValue::Int(5));
        let args = vec![
            ArgumentHandle::new(&needle, &ctx),
            ArgumentHandle::new(&lookup_range, &ctx),
        ];
        let v = f
            .dispatch(&args, &ctx.function_context(None))
            .unwrap()
            .into_literal();
        match v {
            LiteralValue::Error(e) => assert_eq!(e.kind, ExcelErrorKind::Na),
            other => panic!("expected #N/A got {other:?}"),
        }
    }

    #[test]
    fn sort_basic_ascending() {
        let wb = TestWorkbook::new().with_function(Arc::new(SortFn));
        let wb = wb
            .with_cell_a1("Sheet1", "A1", LiteralValue::Int(30))
            .with_cell_a1("Sheet1", "A2", LiteralValue::Int(10))
            .with_cell_a1("Sheet1", "A3", LiteralValue::Int(20));
        let ctx = wb.interpreter();
        let arr = range("A1:A3", 1, 1, 3, 1);
        let f = ctx.context.get_function("", "SORT").unwrap();
        let args = vec![ArgumentHandle::new(&arr, &ctx)];
        let v = f
            .dispatch(&args, &ctx.function_context(None))
            .unwrap()
            .into_literal();
        match v {
            LiteralValue::Array(a) => {
                assert_eq!(a.len(), 3);
                assert_eq!(a[0][0], LiteralValue::Number(10.0));
                assert_eq!(a[1][0], LiteralValue::Number(20.0));
                assert_eq!(a[2][0], LiteralValue::Number(30.0));
            }
            other => panic!("expected array got {other:?}"),
        }
    }

    #[test]
    fn sort_descending() {
        let wb = TestWorkbook::new().with_function(Arc::new(SortFn));
        let wb = wb
            .with_cell_a1("Sheet1", "A1", LiteralValue::Int(30))
            .with_cell_a1("Sheet1", "A2", LiteralValue::Int(10))
            .with_cell_a1("Sheet1", "A3", LiteralValue::Int(20));
        let ctx = wb.interpreter();
        let arr = range("A1:A3", 1, 1, 3, 1);
        let f = ctx.context.get_function("", "SORT").unwrap();
        let sort_index = lit(LiteralValue::Int(1));
        let sort_order = lit(LiteralValue::Int(-1)); // descending
        let args = vec![
            ArgumentHandle::new(&arr, &ctx),
            ArgumentHandle::new(&sort_index, &ctx),
            ArgumentHandle::new(&sort_order, &ctx),
        ];
        let v = f
            .dispatch(&args, &ctx.function_context(None))
            .unwrap()
            .into_literal();
        match v {
            LiteralValue::Array(a) => {
                assert_eq!(a.len(), 3);
                assert_eq!(a[0][0], LiteralValue::Number(30.0));
                assert_eq!(a[1][0], LiteralValue::Number(20.0));
                assert_eq!(a[2][0], LiteralValue::Number(10.0));
            }
            other => panic!("expected array got {other:?}"),
        }
    }

    #[test]
    fn sort_by_column() {
        let wb = TestWorkbook::new().with_function(Arc::new(SortFn));
        let wb = wb
            .with_cell_a1("Sheet1", "A1", LiteralValue::Text("Charlie".into()))
            .with_cell_a1("Sheet1", "B1", LiteralValue::Int(30))
            .with_cell_a1("Sheet1", "A2", LiteralValue::Text("Alice".into()))
            .with_cell_a1("Sheet1", "B2", LiteralValue::Int(10))
            .with_cell_a1("Sheet1", "A3", LiteralValue::Text("Bob".into()))
            .with_cell_a1("Sheet1", "B3", LiteralValue::Int(20));
        let ctx = wb.interpreter();
        let arr = range("A1:B3", 1, 1, 3, 2);
        let f = ctx.context.get_function("", "SORT").unwrap();
        let sort_index = lit(LiteralValue::Int(2)); // sort by column B
        let args = vec![
            ArgumentHandle::new(&arr, &ctx),
            ArgumentHandle::new(&sort_index, &ctx),
        ];
        let v = f
            .dispatch(&args, &ctx.function_context(None))
            .unwrap()
            .into_literal();
        match v {
            LiteralValue::Array(a) => {
                assert_eq!(a.len(), 3);
                // Should be sorted by column B: Alice(10), Bob(20), Charlie(30)
                assert_eq!(a[0][0], LiteralValue::Text("Alice".into()));
                assert_eq!(a[1][0], LiteralValue::Text("Bob".into()));
                assert_eq!(a[2][0], LiteralValue::Text("Charlie".into()));
            }
            other => panic!("expected array got {other:?}"),
        }
    }

    #[test]
    fn sortby_basic() {
        let wb = TestWorkbook::new().with_function(Arc::new(SortByFn));
        let wb = wb
            .with_cell_a1("Sheet1", "A1", LiteralValue::Text("Charlie".into()))
            .with_cell_a1("Sheet1", "A2", LiteralValue::Text("Alice".into()))
            .with_cell_a1("Sheet1", "A3", LiteralValue::Text("Bob".into()))
            .with_cell_a1("Sheet1", "B1", LiteralValue::Int(3))
            .with_cell_a1("Sheet1", "B2", LiteralValue::Int(1))
            .with_cell_a1("Sheet1", "B3", LiteralValue::Int(2));
        let ctx = wb.interpreter();
        let arr = range("A1:A3", 1, 1, 3, 1);
        let by_arr = range("B1:B3", 1, 2, 3, 2);
        let f = ctx.context.get_function("", "SORTBY").unwrap();
        let args = vec![
            ArgumentHandle::new(&arr, &ctx),
            ArgumentHandle::new(&by_arr, &ctx),
        ];
        let v = f
            .dispatch(&args, &ctx.function_context(None))
            .unwrap()
            .into_literal();
        match v {
            LiteralValue::Array(a) => {
                assert_eq!(a.len(), 3);
                // Should be sorted by B values: Alice(1), Bob(2), Charlie(3)
                assert_eq!(a[0][0], LiteralValue::Text("Alice".into()));
                assert_eq!(a[1][0], LiteralValue::Text("Bob".into()));
                assert_eq!(a[2][0], LiteralValue::Text("Charlie".into()));
            }
            other => panic!("expected array got {other:?}"),
        }
    }

    #[test]
    fn sortby_descending() {
        let wb = TestWorkbook::new().with_function(Arc::new(SortByFn));
        let wb = wb
            .with_cell_a1("Sheet1", "A1", LiteralValue::Text("Charlie".into()))
            .with_cell_a1("Sheet1", "A2", LiteralValue::Text("Alice".into()))
            .with_cell_a1("Sheet1", "A3", LiteralValue::Text("Bob".into()))
            .with_cell_a1("Sheet1", "B1", LiteralValue::Int(3))
            .with_cell_a1("Sheet1", "B2", LiteralValue::Int(1))
            .with_cell_a1("Sheet1", "B3", LiteralValue::Int(2));
        let ctx = wb.interpreter();
        let arr = range("A1:A3", 1, 1, 3, 1);
        let by_arr = range("B1:B3", 1, 2, 3, 2);
        let sort_order = lit(LiteralValue::Int(-1)); // descending
        let f = ctx.context.get_function("", "SORTBY").unwrap();
        let args = vec![
            ArgumentHandle::new(&arr, &ctx),
            ArgumentHandle::new(&by_arr, &ctx),
            ArgumentHandle::new(&sort_order, &ctx),
        ];
        let v = f
            .dispatch(&args, &ctx.function_context(None))
            .unwrap()
            .into_literal();
        match v {
            LiteralValue::Array(a) => {
                assert_eq!(a.len(), 3);
                // Should be sorted by B values descending: Charlie(3), Bob(2), Alice(1)
                assert_eq!(a[0][0], LiteralValue::Text("Charlie".into()));
                assert_eq!(a[1][0], LiteralValue::Text("Bob".into()));
                assert_eq!(a[2][0], LiteralValue::Text("Alice".into()));
            }
            other => panic!("expected array got {other:?}"),
        }
    }

    #[test]
    fn randarray_basic() {
        let wb = TestWorkbook::new().with_function(Arc::new(RandArrayFn));
        let ctx = wb.interpreter();
        let f = ctx.context.get_function("", "RANDARRAY").unwrap();

        // Test basic 2x3 array with defaults
        let rows = lit(LiteralValue::Int(2));
        let cols = lit(LiteralValue::Int(3));
        let args = vec![
            ArgumentHandle::new(&rows, &ctx),
            ArgumentHandle::new(&cols, &ctx),
        ];
        let v = f
            .dispatch(&args, &ctx.function_context(None))
            .unwrap()
            .into_literal();
        match v {
            LiteralValue::Array(a) => {
                assert_eq!(a.len(), 2);
                assert_eq!(a[0].len(), 3);
                // Check all values are between 0 and 1
                for row in &a {
                    for cell in row {
                        match cell {
                            LiteralValue::Number(n) => {
                                assert!(*n >= 0.0 && *n < 1.0, "Value {n} not in [0, 1)");
                            }
                            other => panic!("expected Number got {other:?}"),
                        }
                    }
                }
            }
            other => panic!("expected array got {other:?}"),
        }
    }

    #[test]
    fn randarray_whole_numbers() {
        let wb = TestWorkbook::new().with_function(Arc::new(RandArrayFn));
        let ctx = wb.interpreter();
        let f = ctx.context.get_function("", "RANDARRAY").unwrap();

        // Test 3x2 array with whole numbers between 1 and 10
        let rows = lit(LiteralValue::Int(3));
        let cols = lit(LiteralValue::Int(2));
        let min = lit(LiteralValue::Int(1));
        let max = lit(LiteralValue::Int(10));
        let whole = lit(LiteralValue::Boolean(true));
        let args = vec![
            ArgumentHandle::new(&rows, &ctx),
            ArgumentHandle::new(&cols, &ctx),
            ArgumentHandle::new(&min, &ctx),
            ArgumentHandle::new(&max, &ctx),
            ArgumentHandle::new(&whole, &ctx),
        ];
        let v = f
            .dispatch(&args, &ctx.function_context(None))
            .unwrap()
            .into_literal();
        match v {
            LiteralValue::Array(a) => {
                assert_eq!(a.len(), 3);
                assert_eq!(a[0].len(), 2);
                // Check all values are integers between 1 and 10
                for row in &a {
                    for cell in row {
                        let n = match cell {
                            LiteralValue::Int(n) => *n as f64,
                            LiteralValue::Number(n) => *n,
                            other => panic!("expected Int or Number got {other:?}"),
                        };
                        assert!((1.0..=10.0).contains(&n), "Value {n} not in [1, 10]");
                        // Verify it's actually a whole number
                        assert!(n.fract() == 0.0, "Value {n} is not a whole number");
                    }
                }
            }
            other => panic!("expected array got {other:?}"),
        }
    }
}
