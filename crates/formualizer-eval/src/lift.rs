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

type Shape = (usize, usize);

enum Lifted {
    Scalar(LiteralValue),
    Array(Vec<Vec<LiteralValue>>, Shape),
}

fn materialize(arg: &ArgumentHandle<'_, '_>) -> Result<Lifted, ExcelError> {
    Ok(match arg.value()? {
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
fn target_shape(shapes: &[Shape]) -> Shape {
    shapes.iter().fold((1, 1), |(r, c), &(sr, sc)| {
        (
            if r == 1 { sr } else { r.max(sr) },
            if c == 1 { sc } else { c.max(sc) },
        )
    })
}

fn pick(rows: &[Vec<LiteralValue>], shape: Shape, i: usize, j: usize) -> LiteralValue {
    let r = if shape.0 == 1 { 0 } else { i };
    let c = if shape.1 == 1 { 0 } else { j };
    match rows.get(r).and_then(|row| row.get(c)) {
        Some(v) => v.clone(),
        None => LiteralValue::Error(ExcelError::new(ExcelErrorKind::Na)),
    }
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
