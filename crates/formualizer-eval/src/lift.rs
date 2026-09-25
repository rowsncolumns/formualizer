//! Element-wise lifting of scalar functions over array / range arguments.
//!
//! Excel evaluates a scalar function whose argument is an array once per element and
//! returns an array of the same shape (`LEN(A1:A3)` → `{1;2;3}`, `LEFT("abc",{1,2})` →
//! `{"a","ab"}`), broadcasting 1×1 and vector arguments against each other and padding
//! the cells a shorter vector does not cover with `#N/A`. Functions opt in through
//! [`FnCaps::ELEMENTWISE`]; reductions, lookups and anything that legitimately consumes a
//! whole range keep their range arguments untouched.

use crate::function::Function;
use crate::traits::{ArgumentHandle, CalcValue, FunctionContext};
use formualizer_common::{ExcelError, ExcelErrorKind, LiteralValue};
use formualizer_parse::parser::{ASTNode, ASTNodeType};

pub(crate) type Shape = (usize, usize);

/// An evaluated argument, split into "one value" and "a grid of values" for broadcasting.
pub(crate) enum Lifted {
    Scalar(LiteralValue),
    Array(Vec<Vec<LiteralValue>>, Shape),
}

pub(crate) fn materialize(arg: &ArgumentHandle<'_, '_>) -> Result<Lifted, ExcelError> {
    materialize_value(arg.value()?)
}

/// A 1×1 array or range counts as a scalar so single-cell references keep the scalar path.
pub(crate) fn materialize_value(value: CalcValue<'_>) -> Result<Lifted, ExcelError> {
    Ok(match value {
        CalcValue::Scalar(LiteralValue::Array(rows)) => {
            let shape = (rows.len(), rows.first().map(|r| r.len()).unwrap_or(0));
            if shape == (1, 1) {
                Lifted::Scalar(rows[0][0].clone())
            } else {
                Lifted::Array(rows, shape)
            }
        }
        CalcValue::Scalar(v) => Lifted::Scalar(v),
        CalcValue::Range(rv) => {
            let (rows, cols) = rv.dims();
            if rows == 1 && cols == 1 {
                Lifted::Scalar(rv.get_cell(0, 0))
            } else {
                let mut data = Vec::with_capacity(rows);
                rv.for_each_row(&mut |row| {
                    data.push(row.to_vec());
                    Ok(())
                })?;
                Lifted::Array(data, (rows, cols))
            }
        }
        CalcValue::Callable(_) => Lifted::Scalar(LiteralValue::Error(
            ExcelError::new(ExcelErrorKind::Calc).with_message("LAMBDA value must be invoked"),
        )),
    })
}

/// Excel's operator/function broadcasting: a dimension of 1 stretches; otherwise the
/// result takes the larger extent and cells a shorter operand does not reach become `#N/A`.
pub(crate) fn target_shape(shapes: &[Shape]) -> Shape {
    shapes.iter().fold((1, 1), |(r, c), &(sr, sc)| {
        (
            if r == 1 { sr } else { r.max(sr) },
            if c == 1 { sc } else { c.max(sc) },
        )
    })
}

pub(crate) fn pick(rows: &[Vec<LiteralValue>], shape: Shape, i: usize, j: usize) -> LiteralValue {
    let r = if shape.0 == 1 { 0 } else { i };
    let c = if shape.1 == 1 { 0 } else { j };
    match rows.get(r).and_then(|row| row.get(c)) {
        Some(v) => v.clone(),
        None => LiteralValue::Error(ExcelError::new(ExcelErrorKind::Na)),
    }
}

/// Evaluate `eval` once per broadcast element when at least one of the arguments at
/// `positions` is a multi-cell array or range, substituting that element as a literal while
/// every other argument keeps its original (lazy, reference-capable) handle. Returns `None`
/// when none of those arguments is array-shaped so the caller takes its normal path.
///
/// This is Excel's array lift for the *criteria* slot of the criteria aggregates:
/// `COUNTIF(A1:A8,{1,2})` is `{2,1}`, `COUNTIF(C1:C5,C1:C5)` counts each element,
/// `SUMIF(rng,{"x","y"},sum)` and `SUMIFS(sum,rng,{"x","y"})` spill one total per criterion,
/// and `SUM(COUNTIF(rng,{…}))` / `SUMPRODUCT(SUMIF(…,{…}))` fold the spill. The range
/// arguments are never materialized — only the criteria are.
pub(crate) fn lift_array_arguments<'a, 'b>(
    args: &[ArgumentHandle<'a, 'b>],
    positions: &[usize],
    eval: &dyn for<'x> Fn(&[ArgumentHandle<'x, 'b>]) -> Result<CalcValue<'b>, ExcelError>,
) -> Result<Option<CalcValue<'b>>, ExcelError> {
    let mut arrays: Vec<(usize, Vec<Vec<LiteralValue>>, Shape)> = Vec::new();
    for &pos in positions {
        let Some(arg) = args.get(pos) else { continue };
        if let Lifted::Array(rows, shape) = materialize(arg)? {
            arrays.push((pos, rows, shape));
        }
    }
    if arrays.is_empty() {
        return Ok(None);
    }
    let shapes: Vec<Shape> = arrays.iter().map(|(_, _, s)| *s).collect();
    let (rows, cols) = target_shape(&shapes);
    let interp = args[0].interp();

    let mut out = Vec::with_capacity(rows);
    for i in 0..rows {
        let mut row = Vec::with_capacity(cols);
        for j in 0..cols {
            let nodes: Vec<ASTNode> = arrays
                .iter()
                .map(|(_, data, shape)| {
                    ASTNode::new(ASTNodeType::Literal(pick(data, *shape, i, j)), None)
                })
                .collect();
            let handles: Vec<ArgumentHandle<'_, 'b>> = args
                .iter()
                .enumerate()
                .map(
                    |(k, h)| match arrays.iter().position(|(pos, _, _)| *pos == k) {
                        Some(idx) => ArgumentHandle::new(&nodes[idx], interp),
                        None => h.rebound(),
                    },
                )
                .collect();
            let cell = match eval(&handles) {
                Ok(cv) => match cv.into_literal() {
                    LiteralValue::Array(inner) => inner
                        .first()
                        .and_then(|r| r.first())
                        .cloned()
                        .unwrap_or(LiteralValue::Empty),
                    v => v,
                },
                Err(e) => LiteralValue::Error(e),
            };
            row.push(cell);
        }
        out.push(row);
    }
    Ok(Some(CalcValue::Scalar(LiteralValue::Array(out))))
}

/// Evaluate `fun` once per broadcast element when at least one argument is a multi-cell
/// array or range. Returns `None` when every argument is scalar-shaped so the caller can
/// take the normal path.
pub(crate) fn lift_elementwise<'a, 'b, 'c, F: Function + ?Sized>(
    fun: &F,
    args: &'c [ArgumentHandle<'a, 'b>],
    ctx: &dyn FunctionContext<'b>,
) -> Result<Option<CalcValue<'b>>, ExcelError> {
    if args.is_empty() {
        return Ok(None);
    }
    let mut lifted = Vec::with_capacity(args.len());
    let mut shapes = Vec::new();
    for arg in args {
        let l = materialize(arg)?;
        if let Lifted::Array(_, shape) = &l {
            shapes.push(*shape);
        }
        lifted.push(l);
    }
    if shapes.is_empty() {
        return Ok(None);
    }
    let (rows, cols) = target_shape(&shapes);
    let interp = args[0].interp();

    let mut out = Vec::with_capacity(rows);
    for i in 0..rows {
        let mut row = Vec::with_capacity(cols);
        for j in 0..cols {
            let nodes: Vec<ASTNode> = lifted
                .iter()
                .map(|l| {
                    let v = match l {
                        Lifted::Scalar(v) => v.clone(),
                        Lifted::Array(data, shape) => pick(data, *shape, i, j),
                    };
                    ASTNode::new(ASTNodeType::Literal(v), None)
                })
                .collect();
            let handles: Vec<ArgumentHandle<'_, 'b>> = nodes
                .iter()
                .map(|n| ArgumentHandle::new(n, interp))
                .collect();
            let cell = match fun.eval(&handles, ctx) {
                Ok(cv) => match cv.into_literal() {
                    // A function that itself produced an array for one element keeps
                    // the result shape sane by contributing its top-left value.
                    LiteralValue::Array(inner) => inner
                        .first()
                        .and_then(|r| r.first())
                        .cloned()
                        .unwrap_or(LiteralValue::Empty),
                    v => v,
                },
                Err(e) => LiteralValue::Error(e),
            };
            row.push(cell);
        }
        out.push(row);
    }
    Ok(Some(CalcValue::Scalar(LiteralValue::Array(out))))
}
