use crate::function::{FnCaps, Function};
use crate::function_contract::{
    FunctionArgumentDependencyContract, FunctionArityRule, FunctionDependencyClass,
    FunctionDependencyContract,
};
use crate::interpreter::{LocalBinding, LocalEnv};
use crate::traits::{ArgumentHandle, CalcValue, CustomCallable, FunctionContext};
use formualizer_common::{ExcelError, ExcelErrorKind, LiteralValue};
use formualizer_parse::parser::{ASTNode, ASTNodeType, ReferenceType};
use std::cell::Cell;
use std::collections::HashSet;
use std::sync::Arc;

/// Internal function name the arena lowers `ASTNodeType::Call` (`LAMBDA(x,x*2)(3)`)
/// to: its first argument is the callee expression, the rest are the call
/// arguments. Never produced by the parser and mapped back on reconstruction.
pub const IMMEDIATE_CALL_FUNCTION: &str = "__CALL__";

/// Nested `LAMBDA` invocations deeper than this return `#NUM!` instead of
/// exhausting the native stack (Excel caps recursion the same way).
const MAX_LAMBDA_DEPTH: usize = 512;

/// Hard cap on the cells a single `MAKEARRAY` may produce (Excel is bounded by
/// the worksheet grid); larger requests return `#NUM!`.
const MAX_MAKEARRAY_CELLS: u64 = 10_000_000;

thread_local! {
    static LAMBDA_DEPTH: Cell<usize> = const { Cell::new(0) };
}

fn value_error(msg: impl Into<String>) -> ExcelError {
    ExcelError::new(ExcelErrorKind::Value).with_message(msg.into())
}

fn err_value(msg: impl Into<String>) -> CalcValue<'static> {
    CalcValue::Scalar(LiteralValue::Error(value_error(msg)))
}

fn local_name_from_ast(node: &ASTNode) -> Result<String, ExcelError> {
    match &node.node_type {
        ASTNodeType::Reference {
            reference: ReferenceType::NamedRange(name),
            ..
        } => Ok(name.clone()),
        _ => Err(value_error("Expected a local name identifier")),
    }
}

/// `LAMBDA` parameter: a bare name (`x`) or Excel's optional spelling (`[y]`).
/// The tokenizer classifies `[y]` as a bracketed structured reference with the
/// column name and no table, which is the shape matched here. The brackets are
/// documentation only: at call time Excel lets the caller leave out any trailing
/// parameter, bracketed or not, and `ISOMITTED` reports it either way.
fn lambda_param_from_ast(node: &ASTNode) -> Result<String, ExcelError> {
    match &node.node_type {
        ASTNodeType::Reference {
            reference: ReferenceType::NamedRange(name),
            ..
        } => Ok(name.clone()),
        ASTNodeType::Reference {
            reference: ReferenceType::Table(table),
            ..
        } if !table.name.is_empty() => Ok(table.name.clone()),
        _ => Err(value_error("Expected a LAMBDA parameter name")),
    }
}

fn binding_from_calc_value(cv: CalcValue<'_>) -> LocalBinding {
    match cv {
        CalcValue::Scalar(v) => LocalBinding::Value(v),
        CalcValue::Range(rv) => {
            let (rows, cols) = rv.dims();
            if rows == 1 && cols == 1 {
                LocalBinding::Value(rv.get_cell(0, 0))
            } else {
                let mut data = Vec::with_capacity(rows);
                let _ = rv.for_each_row(&mut |row| {
                    data.push(row.to_vec());
                    Ok(())
                });
                LocalBinding::Value(LiteralValue::Array(data))
            }
        }
        CalcValue::Callable(c) => LocalBinding::Callable(c),
    }
}

#[derive(Debug)]
pub struct LetFn;

/// Binds local names to values and evaluates a final expression with those bindings.
///
/// `LET` introduces lexical variables using name/value pairs, then returns the last expression.
///
/// # Remarks
/// - Arguments must be provided as `name, value` pairs followed by one final calculation expression.
/// - Names are resolved as local identifiers and can shadow workbook-level names.
/// - Bindings are evaluated left-to-right, so later values can reference earlier bindings.
/// - Invalid names or malformed arity return `#VALUE!`.
///
/// # Examples
///
/// ```yaml,sandbox
/// title: "Bind intermediate values"
/// formula: "=LET(rate,0.08,price,125,price*(1+rate))"
/// expected: 135
/// ```
///
/// ```yaml,sandbox
/// title: "Use LET with range calculations"
/// grid:
///   A1: 10
///   A2: 4
/// formula: "=LET(total,SUM(A1:A2),total*2)"
/// expected: 28
/// ```
///
/// ```yaml,sandbox
/// title: "Nested LET supports shadowing"
/// formula: "=LET(x,2,LET(x,5,x)+x)"
/// expected: 7
/// ```
///
/// ```yaml,docs
/// related:
///   - LAMBDA
///   - IF
///   - SUM
///   - INDEX
/// faq:
///   - q: "Can a LET binding reference a name defined later in the same LET?"
///     a: "No. LET evaluates name/value pairs left-to-right, so each binding can only use earlier bindings."
///   - q: "Does LET overwrite workbook or worksheet names permanently?"
///     a: "No. LET names are lexical and local to that formula evaluation; they only shadow outer names inside the LET expression."
///   - q: "Is LET itself volatile?"
///     a: "No. LET is deterministic unless one of its bound expressions calls a volatile function such as RAND."
/// ```
///
/// [formualizer-docgen:schema:start]
/// Name: LET
/// Type: LetFn
/// Min args: 3
/// Max args: variadic
/// Variadic: true
/// Signature: LET(<schema unavailable>)
/// Arg schema: <unavailable: arg_schema panicked>
/// Caps: PURE, SHORT_CIRCUIT
/// [formualizer-docgen:schema:end]
impl Function for LetFn {
    fn caps(&self) -> FnCaps {
        FnCaps::PURE | FnCaps::SHORT_CIRCUIT | FnCaps::LOCAL_ENVIRONMENT | FnCaps::MAY_SPILL
    }

    fn name(&self) -> &'static str {
        "LET"
    }

    fn min_args(&self) -> usize {
        3
    }

    fn variadic(&self) -> bool {
        true
    }

    fn dependency_contract(&self, arity: usize) -> Option<FunctionDependencyContract> {
        FunctionDependencyContract {
            class: FunctionDependencyClass::StaticScalarAllArgs,
            arity: FunctionArityRule::OddAtLeast(3),
            arguments: FunctionArgumentDependencyContract::LocalBindingPairs,
        }
        .for_arity(arity)
    }

    fn arg_schema(&self) -> &'static [crate::args::ArgSchema] {
        static SCHEMA: std::sync::LazyLock<Vec<crate::args::ArgSchema>> =
            std::sync::LazyLock::new(|| vec![crate::args::ArgSchema::any()]);
        &SCHEMA
    }

    fn dispatch<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        ctx: &dyn FunctionContext<'b>,
    ) -> Result<CalcValue<'b>, ExcelError> {
        self.eval(args, ctx)
    }

    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        _ctx: &dyn FunctionContext<'b>,
    ) -> Result<CalcValue<'b>, ExcelError> {
        if args.len() < 3 || args.len().is_multiple_of(2) {
            return Ok(err_value(
                "LET expects name/value pairs followed by a final expression",
            ));
        }

        let mut env: LocalEnv = args[0].current_env();

        for pair_idx in (0..args.len() - 1).step_by(2) {
            let name = match local_name_from_ast(args[pair_idx].ast()) {
                Ok(name) => name,
                Err(e) => return Ok(CalcValue::Scalar(LiteralValue::Error(e))),
            };

            let bound = args[pair_idx + 1].value_with_env(env.clone())?;
            env = env.with_binding(&name, binding_from_calc_value(bound));
        }

        args[args.len() - 1].value_with_env(env)
    }
}

#[derive(Clone)]
struct LambdaClosure {
    params: Vec<String>,
    body: ASTNode,
    captured_env: LocalEnv,
}

struct DepthGuard;

impl DepthGuard {
    fn enter() -> Option<Self> {
        LAMBDA_DEPTH.with(|d| {
            if d.get() >= MAX_LAMBDA_DEPTH {
                None
            } else {
                d.set(d.get() + 1);
                Some(DepthGuard)
            }
        })
    }
}

impl Drop for DepthGuard {
    fn drop(&mut self) {
        LAMBDA_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
    }
}

impl CustomCallable for LambdaClosure {
    fn arity(&self) -> usize {
        self.params.len()
    }

    fn invoke<'ctx>(
        &self,
        interp: &crate::interpreter::Interpreter<'ctx>,
        args: &[LiteralValue],
    ) -> Result<CalcValue<'ctx>, ExcelError> {
        // Excel rejects surplus arguments but accepts a call with fewer
        // arguments than parameters: the missing ones are bound as omitted,
        // so `ISOMITTED(p)` is TRUE and reading `p` as a value is `#VALUE!`.
        if args.len() > self.params.len() {
            return Ok(err_value(format!(
                "LAMBDA expected at most {} argument(s), got {}",
                self.params.len(),
                args.len()
            )));
        }

        let Some(_depth) = DepthGuard::enter() else {
            return Ok(CalcValue::Scalar(LiteralValue::Error(
                ExcelError::new(ExcelErrorKind::Num).with_message(format!(
                    "LAMBDA recursion exceeded {MAX_LAMBDA_DEPTH} levels"
                )),
            )));
        };

        let mut env = self.captured_env.clone();
        for (idx, param) in self.params.iter().enumerate() {
            let binding = match args.get(idx) {
                Some(value) => LocalBinding::Value(value.clone()),
                None => LocalBinding::Omitted,
            };
            env = env.with_binding(param, binding);
        }

        let scoped = interp.with_local_env(env);
        scoped.evaluate_ast(&self.body)
    }
}

#[derive(Debug)]
pub struct LambdaFn;

/// Creates an anonymous callable that can be invoked with spreadsheet arguments.
///
/// `LAMBDA` captures its defining local scope and returns a reusable function value.
///
/// # Remarks
/// - All arguments except the last are parameter names; the last argument is the body expression.
/// - A parameter may be written in brackets (`[name]`) to document it as optional; `ISOMITTED(name)` tells whether the caller supplied it.
/// - Parameter names must be unique (case-insensitive), or `#VALUE!` is returned.
/// - Invocation may supply fewer arguments than parameters (the rest are omitted) but never more than the declared count.
/// - Reading an omitted parameter as a value yields `#VALUE!`; `ISOMITTED` is the only function that accepts one.
/// - Returning an uninvoked lambda as a final cell value yields a `#CALC!` in evaluation.
///
/// # Examples
///
/// ```yaml,sandbox
/// title: "Inline lambda invocation"
/// formula: "=LAMBDA(x,x+1)(41)"
/// expected: 42
/// ```
///
/// ```yaml,sandbox
/// title: "Lambda captures outer LET bindings"
/// formula: "=LET(k,10,addk,LAMBDA(n,n+k),addk(5))"
/// expected: 15
/// ```
///
/// ```yaml,sandbox
/// title: "Optional parameter left out"
/// formula: "=LAMBDA(x,[y],IF(ISOMITTED(y),x,x+y))(5)"
/// expected: 5
/// ```
///
/// ```yaml,sandbox
/// title: "Duplicate parameter names are invalid"
/// formula: "=LAMBDA(x,x,x+1)"
/// expected: "#VALUE!"
/// ```
///
/// ```yaml,docs
/// related:
///   - LET
///   - ISOMITTED
///   - MAP
///   - REDUCE
/// faq:
///   - q: "Why does =LAMBDA(x,x+1) return #CALC! instead of a number?"
///     a: "LAMBDA returns a callable value. In a cell result position, it must be invoked, for example =LAMBDA(x,x+1)(1)."
///   - q: "Does a LAMBDA read outer LET variables at call time or definition time?"
///     a: "Definition time. The closure captures its lexical environment when created."
///   - q: "Can I call a LAMBDA with fewer or extra arguments?"
///     a: "Fewer is allowed: the trailing parameters are omitted, ISOMITTED(param) is TRUE and using one as a value is #VALUE!. Extra arguments are rejected with #VALUE!."
/// ```
///
/// [formualizer-docgen:schema:start]
/// Name: LAMBDA
/// Type: LambdaFn
/// Min args: 1
/// Max args: variadic
/// Variadic: true
/// Signature: LAMBDA(<schema unavailable>)
/// Arg schema: <unavailable: arg_schema panicked>
/// Caps: PURE, SHORT_CIRCUIT
/// [formualizer-docgen:schema:end]
impl Function for LambdaFn {
    fn caps(&self) -> FnCaps {
        FnCaps::PURE | FnCaps::SHORT_CIRCUIT | FnCaps::LOCAL_ENVIRONMENT | FnCaps::MAY_SPILL
    }

    fn name(&self) -> &'static str {
        "LAMBDA"
    }

    fn min_args(&self) -> usize {
        1
    }

    fn variadic(&self) -> bool {
        true
    }

    fn dependency_contract(&self, arity: usize) -> Option<FunctionDependencyContract> {
        FunctionDependencyContract {
            class: FunctionDependencyClass::StaticScalarAllArgs,
            arity: FunctionArityRule::AtLeast(1),
            arguments: FunctionArgumentDependencyContract::LambdaParameters,
        }
        .for_arity(arity)
    }

    fn arg_schema(&self) -> &'static [crate::args::ArgSchema] {
        static SCHEMA: std::sync::LazyLock<Vec<crate::args::ArgSchema>> =
            std::sync::LazyLock::new(|| vec![crate::args::ArgSchema::any()]);
        &SCHEMA
    }

    fn dispatch<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        ctx: &dyn FunctionContext<'b>,
    ) -> Result<CalcValue<'b>, ExcelError> {
        self.eval(args, ctx)
    }

    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        _ctx: &dyn FunctionContext<'b>,
    ) -> Result<CalcValue<'b>, ExcelError> {
        if args.is_empty() {
            return Ok(err_value(
                "LAMBDA requires at least a calculation expression",
            ));
        }

        let mut params = Vec::new();
        let mut seen = HashSet::new();
        for arg in &args[..args.len() - 1] {
            let param = match lambda_param_from_ast(arg.ast()) {
                Ok(param) => param,
                Err(e) => return Ok(CalcValue::Scalar(LiteralValue::Error(e))),
            };
            let key = param.to_ascii_uppercase();
            if !seen.insert(key) {
                return Ok(err_value("LAMBDA parameter names must be unique"));
            }
            params.push(param);
        }

        let closure = LambdaClosure {
            params,
            body: args[args.len() - 1].ast().clone(),
            captured_env: args[0].current_env(),
        };

        Ok(CalcValue::Callable(Arc::new(closure)))
    }
}

#[derive(Debug)]
pub struct IsOmittedFn;

/// Reports whether a `LAMBDA` parameter was left out by the caller.
///
/// `ISOMITTED` inspects the binding of a `LAMBDA` parameter, whether it was
/// declared as `[name]` or as a bare `name`.
///
/// # Remarks
/// - Returns `TRUE` when the call did not supply the parameter (any trailing parameter may be left out).
/// - Returns `FALSE` for a supplied parameter, and for any argument that is not a parameter name.
/// - A name that is not bound by an enclosing `LAMBDA` or `LET` returns `#NAME?`.
///
/// # Examples
///
/// ```yaml,sandbox
/// title: "Omitted optional parameter"
/// formula: "=LAMBDA(x,[y],IF(ISOMITTED(y),x,x+y))(5)"
/// expected: 5
/// ```
///
/// ```yaml,sandbox
/// title: "Supplied optional parameter"
/// formula: "=LAMBDA(x,[y],IF(ISOMITTED(y),x,x+y))(5,2)"
/// expected: 7
/// ```
///
/// ```yaml,docs
/// related:
///   - LAMBDA
///   - LET
/// faq:
///   - q: "What happens if I use an omitted parameter directly?"
///     a: "Reading an omitted parameter as a value returns #VALUE!; guard it with ISOMITTED first."
/// ```
///
/// [formualizer-docgen:schema:start]
/// Name: ISOMITTED
/// Type: IsOmittedFn
/// Min args: 1
/// Max args: 1
/// Variadic: false
/// Signature: ISOMITTED(<schema unavailable>)
/// Arg schema: <unavailable: arg_schema panicked>
/// Caps: PURE
/// [formualizer-docgen:schema:end]
impl Function for IsOmittedFn {
    fn caps(&self) -> FnCaps {
        FnCaps::PURE | FnCaps::LOCAL_ENVIRONMENT
    }

    fn name(&self) -> &'static str {
        "ISOMITTED"
    }

    fn min_args(&self) -> usize {
        1
    }

    fn arg_schema(&self) -> &'static [crate::args::ArgSchema] {
        static SCHEMA: std::sync::LazyLock<Vec<crate::args::ArgSchema>> =
            std::sync::LazyLock::new(|| vec![crate::args::ArgSchema::any()]);
        &SCHEMA
    }

    fn dispatch<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        ctx: &dyn FunctionContext<'b>,
    ) -> Result<CalcValue<'b>, ExcelError> {
        self.eval(args, ctx)
    }

    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        _ctx: &dyn FunctionContext<'b>,
    ) -> Result<CalcValue<'b>, ExcelError> {
        if args.len() != 1 {
            return Ok(err_value("ISOMITTED expects exactly one argument"));
        }
        let name = match &args[0].ast().node_type {
            ASTNodeType::Reference {
                reference: ReferenceType::NamedRange(name),
                ..
            } => name.clone(),
            _ => return Ok(CalcValue::Scalar(LiteralValue::Boolean(false))),
        };
        Ok(CalcValue::Scalar(
            match args[0].current_env().lookup(&name) {
                Some(LocalBinding::Omitted) => LiteralValue::Boolean(true),
                Some(_) => LiteralValue::Boolean(false),
                None => LiteralValue::Error(
                    ExcelError::new(ExcelErrorKind::Name)
                        .with_message(format!("{name} is not a LAMBDA parameter")),
                ),
            },
        ))
    }
}

/// Internal: immediate invocation lowered by the arena (see [`IMMEDIATE_CALL_FUNCTION`]).
/// `__CALL__(callee, arg1, …)` evaluates `callee` to a `LAMBDA` value and invokes it.
#[derive(Debug)]
pub struct ImmediateCallFn;

impl Function for ImmediateCallFn {
    fn caps(&self) -> FnCaps {
        FnCaps::PURE | FnCaps::LOCAL_ENVIRONMENT | FnCaps::MAY_SPILL
    }

    fn name(&self) -> &'static str {
        IMMEDIATE_CALL_FUNCTION
    }

    fn min_args(&self) -> usize {
        1
    }

    fn variadic(&self) -> bool {
        true
    }

    fn arg_schema(&self) -> &'static [crate::args::ArgSchema] {
        static SCHEMA: std::sync::LazyLock<Vec<crate::args::ArgSchema>> =
            std::sync::LazyLock::new(|| vec![crate::args::ArgSchema::any()]);
        &SCHEMA
    }

    fn dispatch<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        ctx: &dyn FunctionContext<'b>,
    ) -> Result<CalcValue<'b>, ExcelError> {
        self.eval(args, ctx)
    }

    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        _ctx: &dyn FunctionContext<'b>,
    ) -> Result<CalcValue<'b>, ExcelError> {
        let Some((callee, call_args)) = args.split_first() else {
            return Ok(err_value("Nothing to invoke"));
        };
        let callable = match callee.value()? {
            CalcValue::Callable(c) => c,
            CalcValue::Scalar(LiteralValue::Error(e)) => {
                return Ok(CalcValue::Scalar(LiteralValue::Error(e)));
            }
            _ => return Ok(err_value("Only a LAMBDA value can be invoked")),
        };
        let mut values = Vec::with_capacity(call_args.len());
        for arg in call_args {
            values.push(arg.value()?.into_literal());
        }
        callee.invoke_callable(&callable, &values)
    }
}

/* ───────────────────────── higher-order helpers ───────────────────────── */

fn calc_error() -> ExcelError {
    ExcelError::new(ExcelErrorKind::Calc)
}

/// The `LAMBDA` argument of a helper function, or the error value to return.
fn callable_arg(arg: &ArgumentHandle<'_, '_>) -> Result<Arc<dyn CustomCallable>, LiteralValue> {
    match arg.value() {
        Ok(CalcValue::Callable(c)) => Ok(c),
        Ok(CalcValue::Scalar(LiteralValue::Error(e))) => Err(LiteralValue::Error(e)),
        Ok(_) => Err(LiteralValue::Error(value_error(
            "Expected a LAMBDA as the function argument",
        ))),
        Err(e) => Err(LiteralValue::Error(e)),
    }
}

/// Materialise an array argument as rows (a scalar is a 1×1 array).
fn rows_of(arg: &ArgumentHandle<'_, '_>) -> Result<Vec<Vec<LiteralValue>>, ExcelError> {
    Ok(match arg.value()? {
        CalcValue::Range(rv) => {
            let (rows, _) = rv.dims();
            let mut data = Vec::with_capacity(rows);
            rv.for_each_row(&mut |row| {
                data.push(row.to_vec());
                Ok(())
            })?;
            data
        }
        CalcValue::Scalar(LiteralValue::Array(rows)) => rows,
        CalcValue::Scalar(v) => vec![vec![v]],
        CalcValue::Callable(_) => {
            return Err(calc_error().with_message("LAMBDA value must be invoked"));
        }
    })
}

fn dims(rows: &[Vec<LiteralValue>]) -> (usize, usize) {
    (rows.len(), rows.iter().map(Vec::len).max().unwrap_or(0))
}

/// Invoke the lambda and reduce its result to one cell value: a nested array
/// result cannot be placed in a single cell (`#CALC!`, as in Excel).
fn call_scalar(
    handle: &ArgumentHandle<'_, '_>,
    callable: &Arc<dyn CustomCallable>,
    args: &[LiteralValue],
) -> LiteralValue {
    match handle.invoke_callable(callable, args) {
        Ok(cv) => match cv.into_literal() {
            LiteralValue::Array(_) => {
                LiteralValue::Error(calc_error().with_message("Nested arrays are not supported"))
            }
            v => v,
        },
        Err(e) => LiteralValue::Error(e),
    }
}

fn array_result<'b>(mut rows: Vec<Vec<LiteralValue>>) -> CalcValue<'b> {
    if rows.len() == 1 && rows[0].len() == 1 {
        return CalcValue::Scalar(rows.pop().and_then(|mut r| r.pop()).unwrap());
    }
    CalcValue::Scalar(LiteralValue::Array(rows))
}

fn count_arg(arg: &ArgumentHandle<'_, '_>) -> Result<i64, LiteralValue> {
    match arg.value().map(CalcValue::into_literal) {
        Ok(LiteralValue::Int(i)) => Ok(i),
        Ok(LiteralValue::Number(n)) if n.is_finite() => Ok(n.floor() as i64),
        Ok(LiteralValue::Boolean(b)) => Ok(b as i64),
        Ok(LiteralValue::Empty) => Ok(0),
        Ok(LiteralValue::Text(t)) => {
            match crate::coercion::to_number_lenient(&LiteralValue::Text(t)) {
                Ok(n) => Ok(n.floor() as i64),
                Err(e) => Err(LiteralValue::Error(e)),
            }
        }
        Ok(LiteralValue::Error(e)) => Err(LiteralValue::Error(e)),
        Ok(_) => Err(LiteralValue::Error(value_error("Expected a number"))),
        Err(e) => Err(LiteralValue::Error(e)),
    }
}

macro_rules! hof_boilerplate {
    ($ty:ident, $name:expr, $min:expr, $variadic:expr) => {
        fn caps(&self) -> FnCaps {
            FnCaps::PURE | FnCaps::LOCAL_ENVIRONMENT | FnCaps::MAY_SPILL
        }

        fn name(&self) -> &'static str {
            $name
        }

        fn min_args(&self) -> usize {
            $min
        }

        fn variadic(&self) -> bool {
            $variadic
        }

        fn arg_schema(&self) -> &'static [crate::args::ArgSchema] {
            static SCHEMA: std::sync::LazyLock<Vec<crate::args::ArgSchema>> =
                std::sync::LazyLock::new(|| vec![crate::args::ArgSchema::any()]);
            &SCHEMA
        }

        fn dispatch<'a, 'b, 'c>(
            &self,
            args: &'c [ArgumentHandle<'a, 'b>],
            ctx: &dyn FunctionContext<'b>,
        ) -> Result<CalcValue<'b>, ExcelError> {
            if args.len() < self.min_args() {
                return Ok(err_value(format!(
                    "{} expects at least {} argument(s), got {}",
                    self.name(),
                    self.min_args(),
                    args.len()
                )));
            }
            self.eval(args, ctx)
        }
    };
}

#[derive(Debug)]
pub struct MapFn;

/// Applies a `LAMBDA` to every element of one or more arrays and returns the results.
///
/// `MAP(array1, [array2, …], lambda)` calls `lambda` once per position with the
/// corresponding element of each array.
///
/// # Remarks
/// - The result has the largest row count and largest column count of the inputs; positions a smaller array does not cover receive `#N/A`.
/// - A lambda that returns an array yields `#CALC!` for that position.
///
/// # Examples
///
/// ```yaml,sandbox
/// title: "Double each value"
/// grid:
///   A1: 1
///   A2: 2
///   A3: 3
/// formula: "=MAP(A1:A3,LAMBDA(v,v*2))"
/// expected: [[2],[4],[6]]
/// ```
///
/// ```yaml,sandbox
/// title: "Combine two arrays"
/// grid:
///   A1: 1
///   A2: 2
///   B1: 10
///   B2: 20
/// formula: "=MAP(A1:A2,B1:B2,LAMBDA(a,b,a+b))"
/// expected: [[11],[22]]
/// ```
///
/// ```yaml,docs
/// related:
///   - LAMBDA
///   - REDUCE
///   - SCAN
///   - BYROW
/// ```
///
/// [formualizer-docgen:schema:start]
/// Name: MAP
/// Type: MapFn
/// Min args: 2
/// Max args: variadic
/// Variadic: true
/// Signature: MAP(<schema unavailable>)
/// Arg schema: <unavailable: arg_schema panicked>
/// Caps: PURE, MAY_SPILL
/// [formualizer-docgen:schema:end]
impl Function for MapFn {
    hof_boilerplate!(MapFn, "MAP", 2, true);

    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        _ctx: &dyn FunctionContext<'b>,
    ) -> Result<CalcValue<'b>, ExcelError> {
        let (lambda_arg, array_args) = args.split_last().expect("min_args");
        let callable = match callable_arg(lambda_arg) {
            Ok(c) => c,
            Err(e) => return Ok(CalcValue::Scalar(e)),
        };
        let mut arrays = Vec::with_capacity(array_args.len());
        for arg in array_args {
            arrays.push(rows_of(arg)?);
        }
        let (rows, cols) = arrays.iter().fold((0, 0), |(r, c), a| {
            let (ar, ac) = dims(a);
            (r.max(ar), c.max(ac))
        });
        let mut out = Vec::with_capacity(rows);
        for r in 0..rows {
            let mut row = Vec::with_capacity(cols);
            for c in 0..cols {
                let call_args: Vec<LiteralValue> = arrays
                    .iter()
                    .map(|a| {
                        a.get(r)
                            .and_then(|row| row.get(c))
                            .cloned()
                            .unwrap_or_else(|| {
                                LiteralValue::Error(ExcelError::new(ExcelErrorKind::Na))
                            })
                    })
                    .collect();
                row.push(call_scalar(lambda_arg, &callable, &call_args));
            }
            out.push(row);
        }
        Ok(array_result(out))
    }
}

/// Shared shape of `REDUCE` / `SCAN`: `([initial], array, lambda(accumulator, value))`.
fn fold_args<'a, 'b, 'c>(
    fname: &str,
    args: &'c [ArgumentHandle<'a, 'b>],
) -> Result<
    (
        LiteralValue,
        &'c ArgumentHandle<'a, 'b>,
        &'c ArgumentHandle<'a, 'b>,
    ),
    LiteralValue,
> {
    match args.len() {
        2 => Ok((LiteralValue::Empty, &args[0], &args[1])),
        3 => {
            let initial = match args[0].value() {
                Ok(cv) => cv.into_literal(),
                Err(e) => return Err(LiteralValue::Error(e)),
            };
            Ok((initial, &args[1], &args[2]))
        }
        n => Err(LiteralValue::Error(value_error(format!(
            "{fname} expects 2 or 3 arguments, got {n}"
        )))),
    }
}

#[derive(Debug)]
pub struct ReduceFn;

/// Folds an array into a single value by applying a `LAMBDA` to an accumulator and each element.
///
/// `REDUCE([initial_value], array, lambda(accumulator, value))` walks the array in
/// row-major order, feeding each result back in as the accumulator.
///
/// # Remarks
/// - When `initial_value` is omitted the accumulator starts empty (treated as 0 in arithmetic).
/// - Errors produced by the lambda are carried through to the result.
///
/// # Examples
///
/// ```yaml,sandbox
/// title: "Sum with REDUCE"
/// grid:
///   A1: 1
///   A2: 2
///   A3: 3
/// formula: "=REDUCE(0,A1:A3,LAMBDA(a,v,a+v))"
/// expected: 6
/// ```
///
/// ```yaml,docs
/// related:
///   - LAMBDA
///   - SCAN
///   - MAP
/// ```
///
/// [formualizer-docgen:schema:start]
/// Name: REDUCE
/// Type: ReduceFn
/// Min args: 2
/// Max args: 3
/// Variadic: true
/// Signature: REDUCE(<schema unavailable>)
/// Arg schema: <unavailable: arg_schema panicked>
/// Caps: PURE, MAY_SPILL
/// [formualizer-docgen:schema:end]
impl Function for ReduceFn {
    hof_boilerplate!(ReduceFn, "REDUCE", 2, true);

    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        _ctx: &dyn FunctionContext<'b>,
    ) -> Result<CalcValue<'b>, ExcelError> {
        let (mut acc, array_arg, lambda_arg) = match fold_args("REDUCE", args) {
            Ok(parts) => parts,
            Err(e) => return Ok(CalcValue::Scalar(e)),
        };
        let callable = match callable_arg(lambda_arg) {
            Ok(c) => c,
            Err(e) => return Ok(CalcValue::Scalar(e)),
        };
        for row in rows_of(array_arg)? {
            for v in row {
                acc = call_scalar(lambda_arg, &callable, &[acc, v]);
            }
        }
        Ok(CalcValue::Scalar(acc))
    }
}

#[derive(Debug)]
pub struct ScanFn;

/// Like `REDUCE`, but returns every intermediate accumulator value in the shape of the input.
///
/// `SCAN([initial_value], array, lambda(accumulator, value))` produces running results.
///
/// # Examples
///
/// ```yaml,sandbox
/// title: "Running total"
/// grid:
///   A1: 1
///   A2: 2
///   A3: 3
/// formula: "=SCAN(0,A1:A3,LAMBDA(a,v,a+v))"
/// expected: [[1],[3],[6]]
/// ```
///
/// ```yaml,docs
/// related:
///   - LAMBDA
///   - REDUCE
///   - MAP
/// ```
///
/// [formualizer-docgen:schema:start]
/// Name: SCAN
/// Type: ScanFn
/// Min args: 2
/// Max args: 3
/// Variadic: true
/// Signature: SCAN(<schema unavailable>)
/// Arg schema: <unavailable: arg_schema panicked>
/// Caps: PURE, MAY_SPILL
/// [formualizer-docgen:schema:end]
impl Function for ScanFn {
    hof_boilerplate!(ScanFn, "SCAN", 2, true);

    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        _ctx: &dyn FunctionContext<'b>,
    ) -> Result<CalcValue<'b>, ExcelError> {
        let (mut acc, array_arg, lambda_arg) = match fold_args("SCAN", args) {
            Ok(parts) => parts,
            Err(e) => return Ok(CalcValue::Scalar(e)),
        };
        let callable = match callable_arg(lambda_arg) {
            Ok(c) => c,
            Err(e) => return Ok(CalcValue::Scalar(e)),
        };
        let mut out = Vec::new();
        for row in rows_of(array_arg)? {
            let mut out_row = Vec::with_capacity(row.len());
            for v in row {
                acc = call_scalar(lambda_arg, &callable, &[acc, v]);
                out_row.push(acc.clone());
            }
            out.push(out_row);
        }
        Ok(array_result(out))
    }
}

#[derive(Debug)]
pub struct ByRowFn;

/// Applies a `LAMBDA` to each row of an array and returns one result per row.
///
/// `BYROW(array, lambda(row))` passes every row as a 1×N array; the results form an N×1 column.
///
/// # Examples
///
/// ```yaml,sandbox
/// title: "Row sums"
/// grid:
///   A1: 1
///   B1: 4
///   A2: 2
///   B2: 5
/// formula: "=BYROW(A1:B2,LAMBDA(r,SUM(r)))"
/// expected: [[5],[7]]
/// ```
///
/// ```yaml,docs
/// related:
///   - BYCOL
///   - LAMBDA
///   - MAP
/// ```
///
/// [formualizer-docgen:schema:start]
/// Name: BYROW
/// Type: ByRowFn
/// Min args: 2
/// Max args: 2
/// Variadic: false
/// Signature: BYROW(<schema unavailable>)
/// Arg schema: <unavailable: arg_schema panicked>
/// Caps: PURE, MAY_SPILL
/// [formualizer-docgen:schema:end]
impl Function for ByRowFn {
    hof_boilerplate!(ByRowFn, "BYROW", 2, false);

    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        _ctx: &dyn FunctionContext<'b>,
    ) -> Result<CalcValue<'b>, ExcelError> {
        let callable = match callable_arg(&args[1]) {
            Ok(c) => c,
            Err(e) => return Ok(CalcValue::Scalar(e)),
        };
        let out: Vec<Vec<LiteralValue>> = rows_of(&args[0])?
            .into_iter()
            .map(|row| {
                vec![call_scalar(
                    &args[1],
                    &callable,
                    &[LiteralValue::Array(vec![row])],
                )]
            })
            .collect();
        Ok(array_result(out))
    }
}

#[derive(Debug)]
pub struct ByColFn;

/// Applies a `LAMBDA` to each column of an array and returns one result per column.
///
/// `BYCOL(array, lambda(column))` passes every column as an N×1 array; the results form a 1×N row.
///
/// # Examples
///
/// ```yaml,sandbox
/// title: "Column sums"
/// grid:
///   A1: 1
///   B1: 4
///   A2: 2
///   B2: 5
/// formula: "=BYCOL(A1:B2,LAMBDA(c,SUM(c)))"
/// expected: [[3,9]]
/// ```
///
/// ```yaml,docs
/// related:
///   - BYROW
///   - LAMBDA
///   - MAP
/// ```
///
/// [formualizer-docgen:schema:start]
/// Name: BYCOL
/// Type: ByColFn
/// Min args: 2
/// Max args: 2
/// Variadic: false
/// Signature: BYCOL(<schema unavailable>)
/// Arg schema: <unavailable: arg_schema panicked>
/// Caps: PURE, MAY_SPILL
/// [formualizer-docgen:schema:end]
impl Function for ByColFn {
    hof_boilerplate!(ByColFn, "BYCOL", 2, false);

    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        _ctx: &dyn FunctionContext<'b>,
    ) -> Result<CalcValue<'b>, ExcelError> {
        let callable = match callable_arg(&args[1]) {
            Ok(c) => c,
            Err(e) => return Ok(CalcValue::Scalar(e)),
        };
        let rows = rows_of(&args[0])?;
        let (_, cols) = dims(&rows);
        let out_row: Vec<LiteralValue> = (0..cols)
            .map(|c| {
                let column: Vec<Vec<LiteralValue>> = rows
                    .iter()
                    .map(|row| vec![row.get(c).cloned().unwrap_or(LiteralValue::Empty)])
                    .collect();
                call_scalar(&args[1], &callable, &[LiteralValue::Array(column)])
            })
            .collect();
        Ok(array_result(vec![out_row]))
    }
}

#[derive(Debug)]
pub struct MakeArrayFn;

/// Builds an array of the given size by calling a `LAMBDA` with each row and column index.
///
/// `MAKEARRAY(rows, columns, lambda(row, column))` uses 1-based indexes, as Excel does.
///
/// # Remarks
/// - `rows` and `columns` must be at least 1, otherwise `#VALUE!`.
/// - Requests larger than the worksheet grid return `#NUM!`.
///
/// # Examples
///
/// ```yaml,sandbox
/// title: "Multiplication table"
/// formula: "=MAKEARRAY(2,3,LAMBDA(r,c,r*c))"
/// expected: [[1,2,3],[2,4,6]]
/// ```
///
/// ```yaml,docs
/// related:
///   - LAMBDA
///   - SEQUENCE
///   - MAP
/// ```
///
/// [formualizer-docgen:schema:start]
/// Name: MAKEARRAY
/// Type: MakeArrayFn
/// Min args: 3
/// Max args: 3
/// Variadic: false
/// Signature: MAKEARRAY(<schema unavailable>)
/// Arg schema: <unavailable: arg_schema panicked>
/// Caps: PURE, MAY_SPILL
/// [formualizer-docgen:schema:end]
impl Function for MakeArrayFn {
    hof_boilerplate!(MakeArrayFn, "MAKEARRAY", 3, false);

    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        _ctx: &dyn FunctionContext<'b>,
    ) -> Result<CalcValue<'b>, ExcelError> {
        let rows = match count_arg(&args[0]) {
            Ok(n) => n,
            Err(e) => return Ok(CalcValue::Scalar(e)),
        };
        let cols = match count_arg(&args[1]) {
            Ok(n) => n,
            Err(e) => return Ok(CalcValue::Scalar(e)),
        };
        if rows < 1 || cols < 1 {
            return Ok(err_value("MAKEARRAY rows and columns must be at least 1"));
        }
        if (rows as u64).saturating_mul(cols as u64) > MAX_MAKEARRAY_CELLS {
            return Ok(CalcValue::Scalar(LiteralValue::Error(
                ExcelError::new(ExcelErrorKind::Num)
                    .with_message("MAKEARRAY result exceeds the worksheet size"),
            )));
        }
        let callable = match callable_arg(&args[2]) {
            Ok(c) => c,
            Err(e) => return Ok(CalcValue::Scalar(e)),
        };
        let out: Vec<Vec<LiteralValue>> = (1..=rows)
            .map(|r| {
                (1..=cols)
                    .map(|c| {
                        call_scalar(
                            &args[2],
                            &callable,
                            &[LiteralValue::Int(r), LiteralValue::Int(c)],
                        )
                    })
                    .collect()
            })
            .collect();
        Ok(array_result(out))
    }
}

pub fn register_builtins() {
    crate::function_registry::register_builtin(Arc::new(LetFn));
    crate::function_registry::register_builtin(Arc::new(LambdaFn));
    crate::function_registry::register_builtin(Arc::new(IsOmittedFn));
    crate::function_registry::register_builtin(Arc::new(ImmediateCallFn));
    crate::function_registry::register_builtin(Arc::new(MapFn));
    crate::function_registry::register_builtin(Arc::new(ReduceFn));
    crate::function_registry::register_builtin(Arc::new(ScanFn));
    crate::function_registry::register_builtin(Arc::new(ByRowFn));
    crate::function_registry::register_builtin(Arc::new(ByColFn));
    crate::function_registry::register_builtin(Arc::new(MakeArrayFn));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_workbook::TestWorkbook;
    use formualizer_parse::parser::parse;

    fn test_wb() -> TestWorkbook {
        TestWorkbook::new()
            .with_function(Arc::new(LetFn))
            .with_function(Arc::new(LambdaFn))
            .with_function(Arc::new(IsOmittedFn))
            .with_function(Arc::new(ImmediateCallFn))
            .with_function(Arc::new(MapFn))
            .with_function(Arc::new(ReduceFn))
            .with_function(Arc::new(ScanFn))
            .with_function(Arc::new(ByRowFn))
            .with_function(Arc::new(ByColFn))
            .with_function(Arc::new(MakeArrayFn))
            .with_function(Arc::new(crate::builtins::math::SumFn))
            .with_function(Arc::new(crate::builtins::logical::IfFn))
    }

    /// F1:F3 = 1,2,3 ; G1:G3 = 4,5,6
    fn grid_wb() -> TestWorkbook {
        test_wb().with_range(
            "Sheet1",
            1,
            6,
            vec![
                vec![LiteralValue::Int(1), LiteralValue::Int(4)],
                vec![LiteralValue::Int(2), LiteralValue::Int(5)],
                vec![LiteralValue::Int(3), LiteralValue::Int(6)],
            ],
        )
    }

    fn eval(src: &str) -> LiteralValue {
        eval_result(src).expect("eval")
    }

    fn eval_result(src: &str) -> Result<LiteralValue, ExcelError> {
        eval_result_with_wb(src, test_wb())
    }

    fn eval_with_wb(src: &str, wb: TestWorkbook) -> LiteralValue {
        eval_result_with_wb(src, wb).expect("eval")
    }

    fn eval_result_with_wb(src: &str, wb: TestWorkbook) -> Result<LiteralValue, ExcelError> {
        let interp = wb.interpreter();
        let ast = parse(src).expect("parse");
        interp.evaluate_ast(&ast).map(|v| v.into_literal())
    }

    fn nums(rows: &[&[f64]]) -> LiteralValue {
        LiteralValue::Array(
            rows.iter()
                .map(|r| r.iter().map(|v| LiteralValue::Number(*v)).collect())
                .collect(),
        )
    }

    fn assert_error(v: LiteralValue, kind: ExcelErrorKind) {
        match v {
            LiteralValue::Error(e) => assert_eq!(e.kind, kind),
            other => panic!("expected {kind:?}, got {other:?}"),
        }
    }

    #[test]
    fn let_binds_values() {
        assert_eq!(eval("=LET(x,2,x+3)"), LiteralValue::Number(5.0));
    }

    #[test]
    fn let_nested_shadowing() {
        assert_eq!(eval("=LET(x,2,LET(x,5,x)+x)"), LiteralValue::Number(7.0));
    }

    #[test]
    fn lambda_can_be_bound_and_invoked() {
        assert_eq!(
            eval("=LET(inc,LAMBDA(n,n+1),inc(41))"),
            LiteralValue::Number(42.0)
        );
    }

    #[test]
    fn lambda_closure_captures_outer_bindings() {
        assert_eq!(
            eval("=LET(k,10,addk,LAMBDA(n,n+k),addk(5))"),
            LiteralValue::Number(15.0)
        );
    }

    #[test]
    fn lambda_arity_errors() {
        assert_error(
            eval("=LET(inc,LAMBDA(n,n+1),inc(1,2))"),
            ExcelErrorKind::Value,
        );
        // Fewer arguments is a legal call; the error comes from reading the
        // omitted `n` as a value, not from the call itself.
        assert_error(eval("=LET(inc,LAMBDA(n,n+1),inc())"), ExcelErrorKind::Value);
        assert_eq!(
            eval("=LET(inc,LAMBDA(n,ISOMITTED(n)),inc())"),
            LiteralValue::Boolean(true)
        );
    }

    #[test]
    fn lambda_value_requires_invocation() {
        assert_error(eval("=LAMBDA(x,x+1)"), ExcelErrorKind::Calc);
    }

    #[test]
    fn let_rejects_non_identifier_name() {
        assert_error(eval("=LET(A1,2,A1)"), ExcelErrorKind::Value);
    }

    #[test]
    fn lambda_rejects_duplicate_params() {
        assert_error(eval("=LAMBDA(x,x,x+1)"), ExcelErrorKind::Value);
    }

    #[test]
    fn let_and_lambda_names_are_case_insensitive() {
        assert_eq!(eval("=LET(x,1,X+1)"), LiteralValue::Number(2.0));
        assert_eq!(
            eval("=LET(F,LAMBDA(n,n+1),f(1))"),
            LiteralValue::Number(2.0)
        );
    }

    #[test]
    fn let_shadows_workbook_named_range() {
        let wb = test_wb().with_named_range("x", vec![vec![LiteralValue::Number(100.0)]]);
        assert_eq!(eval_with_wb("=LET(X,1,x+1)", wb), LiteralValue::Number(2.0));
    }

    #[test]
    fn lambda_param_shadows_outer_scope() {
        assert_eq!(
            eval("=LET(n,5,f,LAMBDA(n,n+1),f(10))"),
            LiteralValue::Number(11.0)
        );
    }

    #[test]
    fn lambda_closure_snapshot_semantics() {
        assert_eq!(
            eval("=LET(k,1,f,LAMBDA(x,x+k),k,2,f(0))"),
            LiteralValue::Number(1.0)
        );
    }

    #[test]
    fn let_undefined_symbol_before_binding_errors() {
        // A `#NAME?` *value* (not a hard error) so IFERROR/ISERROR can catch it.
        assert_error(eval("=LET(x,y,y,2,x)"), ExcelErrorKind::Name);
    }

    #[test]
    fn non_invoked_lambda_in_let_is_calc_error() {
        assert_error(eval("=LET(f,LAMBDA(x,x+1),f)"), ExcelErrorKind::Calc);
    }

    /* ── immediate invocation (spreadsheet#546 A-01 / C-R04) ── */

    #[test]
    fn lambda_immediate_invocation() {
        assert_eq!(eval("=LAMBDA(x,x*2)(3)"), LiteralValue::Number(6.0));
        assert_eq!(eval("=LAMBDA(a,b,a+b)(2,3)"), LiteralValue::Number(5.0));
        assert_eq!(
            eval("=LET(f,LAMBDA(x,x+1),f)(41)"),
            LiteralValue::Number(42.0)
        );
    }

    #[test]
    fn immediate_invocation_of_non_lambda_is_value_error() {
        assert_error(eval("=LET(x,5,x)(3)"), ExcelErrorKind::Value);
    }

    #[test]
    fn immediate_invocation_round_trips_through_the_arena() {
        // The engine stores formulas in the AST arena; `LAMBDA(...)(...)` must survive
        // lowering to `__CALL__` and evaluate there too.
        let mut engine =
            crate::engine::Engine::new(TestWorkbook::new(), crate::engine::EvalConfig::default());
        engine
            .set_cell_formula("Sheet1", 1, 1, parse("=LAMBDA(x,x*2)(21)").unwrap())
            .unwrap();
        engine.evaluate_all().unwrap();
        assert_eq!(
            engine.get_cell_value("Sheet1", 1, 1),
            Some(LiteralValue::Number(42.0))
        );
    }

    /* ── optional parameters / ISOMITTED (A-02) ── */

    #[test]
    fn optional_parameter_can_be_omitted() {
        assert_eq!(
            eval("=LAMBDA(x,[y],IF(ISOMITTED(y),x,x+y))(5)"),
            LiteralValue::Number(5.0)
        );
        assert_eq!(
            eval("=LAMBDA(x,[y],IF(ISOMITTED(y),x,x+y))(5,2)"),
            LiteralValue::Number(7.0)
        );
        assert_eq!(
            eval("=LET(f,LAMBDA(x,[y],IF(ISOMITTED(y),x,x+y)),f(5))"),
            LiteralValue::Number(5.0)
        );
    }

    #[test]
    fn reading_an_omitted_parameter_is_value_error() {
        assert_error(eval("=LAMBDA(x,[y],x+y)(5)"), ExcelErrorKind::Value);
    }

    #[test]
    fn isomitted_on_supplied_or_literal_is_false() {
        assert_eq!(
            eval("=LAMBDA(x,[y],ISOMITTED(y))(1,2)"),
            LiteralValue::Boolean(false)
        );
        assert_eq!(
            eval("=LAMBDA(x,ISOMITTED(x))(1)"),
            LiteralValue::Boolean(false)
        );
        assert_eq!(eval("=ISOMITTED(1)"), LiteralValue::Boolean(false));
    }

    #[test]
    fn isomitted_on_unbound_name_is_name_error() {
        assert_error(eval("=ISOMITTED(zzz)"), ExcelErrorKind::Name);
    }

    #[test]
    fn surplus_arguments_are_rejected_omitted_ones_are_bound() {
        assert_error(eval("=LAMBDA(x,[y],x)(1,2,3)"), ExcelErrorKind::Value);
        // `x` is omitted and read as a value → #VALUE! (the call itself is fine).
        assert_error(eval("=LAMBDA(x,[y],x)()"), ExcelErrorKind::Value);
        assert_eq!(
            eval("=LAMBDA(x,[y],ISOMITTED(x))()"),
            LiteralValue::Boolean(true)
        );
    }

    /* ── fewer arguments than parameters (rowsncolumns/spreadsheet#546 W5-C) ── */

    #[test]
    fn unbracketed_parameters_can_be_omitted_too() {
        assert_eq!(
            eval("=LAMBDA(x,ISOMITTED(x))()"),
            LiteralValue::Boolean(true)
        );
        assert_eq!(
            eval("=LAMBDA(x,y,ISOMITTED(y))(1)"),
            LiteralValue::Boolean(true)
        );
        assert_eq!(
            eval("=LAMBDA(x,y,ISOMITTED(x))(1)"),
            LiteralValue::Boolean(false)
        );
        // An omitted parameter that the body never reads is harmless.
        assert_eq!(eval("=LAMBDA(x,y,x*2)(4)"), LiteralValue::Number(8.0));
        assert_eq!(
            eval("=LAMBDA(x,y,IF(ISOMITTED(y),x,x+y))(1)"),
            LiteralValue::Number(1.0)
        );
        assert_eq!(
            eval("=LAMBDA(x,y,IF(ISOMITTED(y),x,x+y))(1,2)"),
            LiteralValue::Number(3.0)
        );
        // LET-bound and named-call paths share the closure invocation.
        assert_eq!(
            eval("=LET(f,LAMBDA(x,y,ISOMITTED(y)),f(1))"),
            LiteralValue::Boolean(true)
        );
    }

    #[test]
    fn omitted_parameter_used_as_a_value_is_value_error() {
        assert_error(eval("=LAMBDA(x,y,x+y)(1)"), ExcelErrorKind::Value);
        assert_error(eval("=LAMBDA(x,x+1)()"), ExcelErrorKind::Value);
        assert_error(eval("=LET(f,LAMBDA(x,y,x+y),f(1))"), ExcelErrorKind::Value);
    }

    #[test]
    fn hofs_call_lambdas_that_declare_more_parameters_than_supplied() {
        assert_eq!(
            eval("=SUM(MAP({1,2,3},LAMBDA(v,k,IF(ISOMITTED(k),v,v+k))))"),
            LiteralValue::Number(6.0)
        );
        assert_eq!(
            eval("=REDUCE(0,{1,2,3},LAMBDA(a,v,w,a+v))"),
            LiteralValue::Number(6.0)
        );
        assert_eq!(
            eval("=SUM(BYROW({1,2;3,4},LAMBDA(r,extra,SUM(r))))"),
            LiteralValue::Number(10.0)
        );
        // …but the body still fails if it actually uses the missing value.
        assert_error(
            eval("=SUM(MAP({1,2,3},LAMBDA(v,k,v+k)))"),
            ExcelErrorKind::Value,
        );
    }

    /* ── LET / LAMBDA locals bound to arrays (C-R06) ── */

    #[test]
    fn let_can_bind_a_range_and_aggregate_it() {
        assert_eq!(
            eval_with_wb("=LET(a,F1:F3,SUM(a))", grid_wb()),
            LiteralValue::Number(6.0)
        );
        assert_eq!(
            eval_with_wb("=LET(a,{1,2,3},SUM(a))", grid_wb()),
            LiteralValue::Number(6.0)
        );
    }

    /* ── helper functions ── */

    #[test]
    fn map_one_array() {
        assert_eq!(
            eval_with_wb("=MAP(F1:F3,LAMBDA(v,v*2))", grid_wb()),
            nums(&[&[2.0], &[4.0], &[6.0]])
        );
    }

    #[test]
    fn map_two_arrays() {
        assert_eq!(
            eval_with_wb("=MAP(F1:F3,G1:G3,LAMBDA(a,b,a+b))", grid_wb()),
            nums(&[&[5.0], &[7.0], &[9.0]])
        );
    }

    #[test]
    fn map_pads_shorter_array_with_na() {
        let v = eval_with_wb("=MAP(F1:F3,G1:G2,LAMBDA(a,b,a+b))", grid_wb());
        match v {
            LiteralValue::Array(rows) => {
                assert_eq!(rows.len(), 3);
                assert_eq!(rows[0][0], LiteralValue::Number(5.0));
                assert_error(rows[2][0].clone(), ExcelErrorKind::Na);
            }
            other => panic!("expected array, got {other:?}"),
        }
    }

    #[test]
    fn map_with_array_constant_and_scalar() {
        assert_eq!(
            eval("=MAP({1,2,3},LAMBDA(v,v*10))"),
            nums(&[&[10.0, 20.0, 30.0]])
        );
        assert_eq!(eval("=MAP(4,LAMBDA(v,v*10))"), LiteralValue::Number(40.0));
    }

    #[test]
    fn map_rejects_non_lambda() {
        assert_error(
            eval_with_wb("=MAP(F1:F3,42)", grid_wb()),
            ExcelErrorKind::Value,
        );
    }

    #[test]
    fn map_lambda_returning_array_is_calc_error() {
        let v = eval_with_wb("=MAP(F1:F2,LAMBDA(v,F1:F3))", grid_wb());
        match v {
            LiteralValue::Array(rows) => assert_error(rows[0][0].clone(), ExcelErrorKind::Calc),
            other => panic!("expected array, got {other:?}"),
        }
    }

    #[test]
    fn reduce_sums_with_and_without_initial() {
        assert_eq!(
            eval_with_wb("=REDUCE(0,F1:F3,LAMBDA(a,v,a+v))", grid_wb()),
            LiteralValue::Number(6.0)
        );
        assert_eq!(
            eval_with_wb("=REDUCE(100,F1:F3,LAMBDA(a,v,a+v))", grid_wb()),
            LiteralValue::Number(106.0)
        );
        assert_eq!(
            eval_with_wb("=REDUCE(F1:F3,LAMBDA(a,v,a+v))", grid_wb()),
            LiteralValue::Number(6.0)
        );
    }

    #[test]
    fn reduce_walks_row_major() {
        // ((("" & 1) & 4) & 2) & 5 & 3 & 6 → "142536"
        assert_eq!(
            eval_with_wb("=REDUCE(\"\",F1:G3,LAMBDA(a,v,a&v))", grid_wb()),
            LiteralValue::Text("142536".to_string())
        );
    }

    #[test]
    fn scan_running_totals() {
        assert_eq!(
            eval_with_wb("=SCAN(0,F1:F3,LAMBDA(a,v,a+v))", grid_wb()),
            nums(&[&[1.0], &[3.0], &[6.0]])
        );
        assert_eq!(
            eval_with_wb("=SCAN(0,F1:G2,LAMBDA(a,v,a+v))", grid_wb()),
            nums(&[&[1.0, 5.0], &[7.0, 12.0]])
        );
    }

    #[test]
    fn byrow_and_bycol_receive_whole_slices() {
        assert_eq!(
            eval_with_wb("=BYROW(F1:G2,LAMBDA(r,SUM(r)))", grid_wb()),
            nums(&[&[5.0], &[7.0]])
        );
        assert_eq!(
            eval_with_wb("=BYCOL(F1:G2,LAMBDA(c,SUM(c)))", grid_wb()),
            nums(&[&[3.0, 9.0]])
        );
    }

    #[test]
    fn makearray_builds_indexed_grid() {
        assert_eq!(
            eval("=MAKEARRAY(2,3,LAMBDA(r,c,r*c))"),
            nums(&[&[1.0, 2.0, 3.0], &[2.0, 4.0, 6.0]])
        );
        assert_eq!(
            eval("=MAKEARRAY(1,1,LAMBDA(r,c,r+c))"),
            LiteralValue::Number(2.0)
        );
    }

    #[test]
    fn makearray_rejects_bad_sizes() {
        assert_error(
            eval("=MAKEARRAY(0,3,LAMBDA(r,c,r*c))"),
            ExcelErrorKind::Value,
        );
        assert_error(
            eval("=MAKEARRAY(100000,100000,LAMBDA(r,c,r*c))"),
            ExcelErrorKind::Num,
        );
    }

    #[test]
    fn helpers_accept_let_bound_lambdas() {
        assert_eq!(
            eval_with_wb("=LET(dbl,LAMBDA(v,v*2),MAP(F1:F3,dbl))", grid_wb()),
            nums(&[&[2.0], &[4.0], &[6.0]])
        );
    }

    #[test]
    fn helper_arity_errors_are_value_errors() {
        assert_error(eval("=MAP(LAMBDA(v,v))"), ExcelErrorKind::Value);
        assert_error(
            eval_with_wb("=REDUCE(F1:F3)", grid_wb()),
            ExcelErrorKind::Value,
        );
        assert_error(
            eval_with_wb("=REDUCE(0,F1:F3,LAMBDA(a,v,a+v),1)", grid_wb()),
            ExcelErrorKind::Value,
        );
    }
}
