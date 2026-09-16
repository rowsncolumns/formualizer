use crate::{
    CellRef,
    broadcast::{broadcast_shape, project_index},
    coercion,
    traits::{ArgumentHandle, DefaultFunctionContext, EvaluationContext},
};
use formualizer_common::{ExcelError, ExcelErrorKind, LiteralValue};
use formualizer_parse::parser::{ASTNode, ASTNodeType, ReferenceType};
use rustc_hash::FxHashMap;
use std::{borrow::Cow, sync::Arc};

use crate::engine::arena::ast::SheetKey;
use crate::engine::arena::{AstNodeData, AstNodeId, CompactRefType, DataStore};
use crate::engine::sheet_registry::SheetRegistry;
use crate::formula_plane::template_canonical::LiteralSlotId;

#[derive(Clone)]
pub enum LocalBinding {
    Value(LiteralValue),
    Callable(Arc<dyn crate::traits::CustomCallable>),
    /// An optional `LAMBDA` parameter (`[name]`) the caller did not supply.
    /// `ISOMITTED(name)` reports it; reading it as a value is `#VALUE!`.
    Omitted,
}

#[derive(Clone, Default)]
pub struct LocalEnv {
    head: Option<Arc<EnvFrame>>,
}

#[derive(Clone)]
struct EnvFrame {
    parent: Option<Arc<EnvFrame>>,
    bindings: FxHashMap<String, LocalBinding>,
}

impl LocalEnv {
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.head.is_none()
    }

    fn norm(name: &str) -> String {
        name.to_ascii_uppercase()
    }

    pub fn lookup(&self, name: &str) -> Option<LocalBinding> {
        self.head.as_ref()?;
        let key = Self::norm(name);
        let mut cur = self.head.as_ref().cloned();
        while let Some(frame) = cur {
            if let Some(v) = frame.bindings.get(&key) {
                return Some(v.clone());
            }
            cur = frame.parent.clone();
        }
        None
    }

    pub fn with_binding(&self, name: &str, value: LocalBinding) -> Self {
        let mut bindings = FxHashMap::default();
        bindings.insert(Self::norm(name), value);
        Self {
            head: Some(Arc::new(EnvFrame {
                parent: self.head.clone(),
                bindings,
            })),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct InterpreterParameterBindings<'a> {
    pub(crate) literal_slots_by_node: &'a FxHashMap<AstNodeId, LiteralSlotId>,
    pub(crate) literal_values: &'a [LiteralValue],
}

/// A union `(A1:A2,B1:B2)` has no single rectangle; by-reference consumers fall back to the value
/// path, which stacks the areas (see `Interpreter::union_calc_values`).
fn multi_area_reference_error() -> ExcelError {
    ExcelError::new(ExcelErrorKind::Ref)
        .with_message("A multi-area reference cannot be used as a single reference")
}

pub struct Interpreter<'a> {
    pub context: &'a dyn EvaluationContext,
    current_sheet: &'a str,
    current_cell: Option<crate::CellRef>,
    local_env: LocalEnv,
    reference_row_delta: i64,
    reference_col_delta: i64,
    disable_ast_planner: bool,
    parameter_bindings: Option<InterpreterParameterBindings<'a>>,
}

impl<'a> Interpreter<'a> {
    pub fn new(context: &'a dyn EvaluationContext, current_sheet: &'a str) -> Self {
        Self {
            context,
            current_sheet,
            current_cell: None,
            local_env: LocalEnv::default(),
            reference_row_delta: 0,
            reference_col_delta: 0,
            disable_ast_planner: false,
            parameter_bindings: None,
        }
    }

    pub fn new_with_cell(
        context: &'a dyn EvaluationContext,
        current_sheet: &'a str,
        cell: crate::CellRef,
    ) -> Self {
        Self {
            context,
            current_sheet,
            current_cell: Some(cell),
            local_env: LocalEnv::default(),
            reference_row_delta: 0,
            reference_col_delta: 0,
            disable_ast_planner: false,
            parameter_bindings: None,
        }
    }

    pub fn current_sheet(&self) -> &'a str {
        self.current_sheet
    }

    pub fn local_env(&self) -> &LocalEnv {
        &self.local_env
    }

    pub(crate) fn with_current_cell(&self, cell: crate::CellRef) -> Self {
        Self {
            context: self.context,
            current_sheet: self.current_sheet,
            current_cell: Some(cell),
            local_env: self.local_env.clone(),
            reference_row_delta: self.reference_row_delta,
            reference_col_delta: self.reference_col_delta,
            disable_ast_planner: self.disable_ast_planner,
            parameter_bindings: self.parameter_bindings,
        }
    }

    pub fn with_local_env(&self, env: LocalEnv) -> Self {
        Self {
            context: self.context,
            current_sheet: self.current_sheet,
            current_cell: self.current_cell,
            local_env: env,
            reference_row_delta: self.reference_row_delta,
            reference_col_delta: self.reference_col_delta,
            disable_ast_planner: self.disable_ast_planner,
            parameter_bindings: self.parameter_bindings,
        }
    }

    pub(crate) fn with_parameter_bindings(
        &self,
        bindings: InterpreterParameterBindings<'a>,
    ) -> Self {
        Self {
            context: self.context,
            current_sheet: self.current_sheet,
            current_cell: self.current_cell,
            local_env: self.local_env.clone(),
            reference_row_delta: self.reference_row_delta,
            reference_col_delta: self.reference_col_delta,
            disable_ast_planner: self.disable_ast_planner,
            parameter_bindings: Some(bindings),
        }
    }

    fn effective_reference<'r>(
        &self,
        reference: &'r ReferenceType,
    ) -> Result<Cow<'r, ReferenceType>, ExcelError> {
        if self.reference_row_delta == 0 && self.reference_col_delta == 0 {
            return Ok(Cow::Borrowed(reference));
        }

        Ok(Cow::Owned(relocate_reference_for_offset(
            reference,
            self.reference_row_delta,
            self.reference_col_delta,
        )?))
    }

    pub(crate) fn resolve_local_reference(
        &self,
        reference: &ReferenceType,
    ) -> Option<crate::traits::CalcValue<'a>> {
        if self.local_env.is_empty() {
            return None;
        }
        let name = match reference {
            ReferenceType::NamedRange(name) => name,
            _ => return None,
        };
        match self.local_env.lookup(name)? {
            LocalBinding::Value(v) => Some(crate::traits::CalcValue::Scalar(v)),
            LocalBinding::Callable(c) => Some(crate::traits::CalcValue::Callable(c)),
            LocalBinding::Omitted => Some(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                ExcelError::new(ExcelErrorKind::Value)
                    .with_message(format!("LAMBDA parameter {name} was omitted")),
            ))),
        }
    }

    fn resolve_local_callable(&self, name: &str) -> Option<Arc<dyn crate::traits::CustomCallable>> {
        if self.local_env.is_empty() {
            return None;
        }
        match self.local_env.lookup(name)? {
            LocalBinding::Callable(c) => Some(c),
            LocalBinding::Value(_) | LocalBinding::Omitted => None,
        }
    }

    pub fn resolve_local_name(&self, name: &str) -> Option<LocalBinding> {
        self.local_env.lookup(name)
    }

    /// Lower a structured table reference that needs row context or
    /// combination handling into a concrete cell/range reference. Returns
    /// `Ok(None)` when the reference is not such a form (or the context does
    /// not track the table) — callers then resolve the original reference.
    ///
    /// Table geometry — and, for `#This Row`/`@`, the evaluating cell — is
    /// only available here; the context's resolution path is cell-agnostic.
    /// Graph ingest rewrites this-row forms in stored formulas before they
    /// reach evaluation; this covers direct AST evaluation (conditional
    /// formats, data validation, INDIRECT) and context-free combination forms.
    fn lower_structured_table_ref(
        &self,
        reference: &ReferenceType,
        current_sheet: &str,
    ) -> Result<Option<ReferenceType>, ExcelError> {
        let ReferenceType::Table(tref) = reference else {
            return Ok(None);
        };
        if !crate::structured::needs_lowering(tref.specifier.as_ref()) {
            return Ok(None);
        }
        let Some(geom) = self.context.table_geometry(&tref.name) else {
            return Ok(None);
        };
        if crate::structured::involves_this_row(tref.specifier.as_ref())
            && geom.sheet.as_deref() != Some(current_sheet)
        {
            return Err(ExcelError::new(ExcelErrorKind::Value)
                .with_message("@ (This Row) used outside the table's rows".to_string()));
        }
        let current_row = self.current_cell.map(|c| c.coord.row() + 1);
        let rect =
            crate::structured::lower_structured_rect(&geom, tref.specifier.as_ref(), current_row)?;
        Ok(Some(crate::structured::rect_to_reference(
            &rect,
            geom.sheet.clone(),
        )))
    }

    pub fn resolve_range_view<'c>(
        &'c self,
        reference: &ReferenceType,
        current_sheet: &str,
    ) -> Result<crate::engine::range_view::RangeView<'c>, ExcelError> {
        if let Some(lowered) = self.lower_structured_table_ref(reference, current_sheet)? {
            return self.context.resolve_range_view(&lowered, current_sheet);
        }
        self.context.resolve_range_view(reference, current_sheet)
    }

    /// Evaluate an AST node in a reference context and return a ReferenceType.
    /// This is used for range combinators (e.g., ":"), by-ref argument flows,
    /// and spill planning. Functions that can return references must set
    /// `FnCaps::RETURNS_REFERENCE` and override `eval_reference`.
    pub fn evaluate_ast_as_reference(&self, node: &ASTNode) -> Result<ReferenceType, ExcelError> {
        match &node.node_type {
            ASTNodeType::Reference { reference, .. } => {
                self.reference_for_current_offset(reference)
            }
            ASTNodeType::Function { name, args } => {
                if let Some(fun) = self.context.get_function("", name) {
                    // Build handles; allow function to decide reference semantics
                    let handles: Vec<ArgumentHandle> =
                        args.iter().map(|n| ArgumentHandle::new(n, self)).collect();
                    let fctx = DefaultFunctionContext::new_with_sheet(
                        self.context,
                        self.current_cell,
                        self.current_sheet,
                    );
                    if let Some(res) = fun.eval_reference(&handles, &fctx) {
                        res
                    } else {
                        Err(ExcelError::new(ExcelErrorKind::Ref)
                            .with_message("Function does not return a reference"))
                    }
                } else {
                    Err(ExcelError::new(ExcelErrorKind::Name)
                        .with_message(format!("Unknown function: {name}")))
                }
            }
            ASTNodeType::BinaryOp { op, left, right } if op == ":" => {
                let lref = self.evaluate_ast_as_reference(left)?;
                let rref = self.evaluate_ast_as_reference(right)?;
                crate::reference::combine_references(&lref, &rref)
            }
            ASTNodeType::BinaryOp { op, left, right } if op == " " => {
                let lref = self.evaluate_ast_as_reference(left)?;
                let rref = self.evaluate_ast_as_reference(right)?;
                self.intersect_reference_operands(&lref, &rref)
            }
            ASTNodeType::BinaryOp { op, .. } if op == "," => Err(multi_area_reference_error()),
            ASTNodeType::UnaryOp { op, expr } if op == "#" => {
                let anchor = self.evaluate_ast_as_reference(expr)?;
                self.spill_range_of_anchor_reference(&anchor)
            }
            ASTNodeType::Array(_)
            | ASTNodeType::UnaryOp { .. }
            | ASTNodeType::BinaryOp { .. }
            | ASTNodeType::Call { .. }
            | ASTNodeType::Literal(_) => Err(ExcelError::new(ExcelErrorKind::Ref)
                .with_message("Expression cannot be used as a reference")),
        }
    }

    pub(crate) fn evaluate_arena_ast_as_reference(
        &self,
        node_id: AstNodeId,
        data_store: &DataStore,
        sheet_registry: &SheetRegistry,
    ) -> Result<ReferenceType, ExcelError> {
        let node = data_store.get_node(node_id).ok_or_else(|| {
            ExcelError::new(ExcelErrorKind::Value).with_message("Missing AST node")
        })?;

        match node {
            AstNodeData::Reference { ref_type, .. } => {
                let reference =
                    data_store.reconstruct_reference_type_for_eval(ref_type, sheet_registry);
                self.reference_for_current_offset(&reference)
            }
            AstNodeData::Function { name_id, .. } => {
                let name = data_store.resolve_ast_string(*name_id);
                let fun = self.context.get_function("", name).ok_or_else(|| {
                    ExcelError::new(ExcelErrorKind::Name)
                        .with_message(format!("Unknown function: {name}"))
                })?;

                let args = data_store.get_args(node_id).ok_or_else(|| {
                    ExcelError::new(ExcelErrorKind::Value).with_message("Missing function args")
                })?;

                let handles: Vec<ArgumentHandle> = args
                    .iter()
                    .copied()
                    .map(|arg_id| {
                        ArgumentHandle::new_arena(arg_id, self, data_store, sheet_registry)
                    })
                    .collect();

                let fctx = DefaultFunctionContext::new_with_sheet(
                    self.context,
                    self.current_cell,
                    self.current_sheet,
                );

                fun.eval_reference(&handles, &fctx).ok_or_else(|| {
                    ExcelError::new(ExcelErrorKind::Ref)
                        .with_message("Function does not return a reference")
                })?
            }
            AstNodeData::BinaryOp {
                op_id,
                left_id,
                right_id,
            } => {
                let op = data_store.resolve_ast_string(*op_id);
                if op == "," {
                    return Err(multi_area_reference_error());
                }
                if op != ":" && op != " " {
                    return Err(ExcelError::new(ExcelErrorKind::Ref)
                        .with_message("Expression cannot be used as a reference"));
                }
                let lref =
                    self.evaluate_arena_ast_as_reference(*left_id, data_store, sheet_registry)?;
                let rref =
                    self.evaluate_arena_ast_as_reference(*right_id, data_store, sheet_registry)?;
                if op == ":" {
                    crate::reference::combine_references(&lref, &rref)
                } else {
                    self.intersect_reference_operands(&lref, &rref)
                }
            }
            AstNodeData::UnaryOp { op_id, expr_id }
                if data_store.resolve_ast_string(*op_id) == "#" =>
            {
                let anchor =
                    self.evaluate_arena_ast_as_reference(*expr_id, data_store, sheet_registry)?;
                self.spill_range_of_anchor_reference(&anchor)
            }
            _ => Err(ExcelError::new(ExcelErrorKind::Ref)
                .with_message("Expression cannot be used as a reference")),
        }
    }

    /// An intersection operand read in value context: a literal or array constant is not a
    /// reference at all (`#VALUE!`, as in Excel); anything else resolves as a reference and
    /// keeps its own error (`INDIRECT("bad") A1:B2` stays `#REF!`).
    fn set_operand_reference(&self, node: &ASTNode) -> Result<ReferenceType, ExcelError> {
        match &node.node_type {
            ASTNodeType::Literal(_) | ASTNodeType::Array(_) => {
                Err(ExcelError::new(ExcelErrorKind::Value)
                    .with_message("Intersection operands must be references"))
            }
            _ => self.evaluate_ast_as_reference(node),
        }
    }

    fn set_operand_arena_reference(
        &self,
        node_id: AstNodeId,
        data_store: &DataStore,
        sheet_registry: &SheetRegistry,
    ) -> Result<ReferenceType, ExcelError> {
        match data_store.get_node(node_id) {
            Some(AstNodeData::Literal(_)) | Some(AstNodeData::Array { .. }) => {
                Err(ExcelError::new(ExcelErrorKind::Value)
                    .with_message("Intersection operands must be references"))
            }
            _ => self.evaluate_arena_ast_as_reference(node_id, data_store, sheet_registry),
        }
    }

    /// `A1:B2 B1:C3` — the rectangle both operands share (`#NULL!` when disjoint). Named ranges
    /// and structured references take part through their concrete definition; anything that is
    /// not a reference is `#VALUE!`, as in Excel.
    fn intersect_reference_operands(
        &self,
        left: &ReferenceType,
        right: &ReferenceType,
    ) -> Result<ReferenceType, ExcelError> {
        let left = self.concrete_reference_for_set_op(left)?;
        let right = self.concrete_reference_for_set_op(right)?;
        crate::reference::intersect_references(&left, &right, self.current_sheet)
    }

    fn concrete_reference_for_set_op(
        &self,
        reference: &ReferenceType,
    ) -> Result<ReferenceType, ExcelError> {
        match reference {
            ReferenceType::Cell { .. } | ReferenceType::Range { .. } => Ok(reference.clone()),
            ReferenceType::NamedRange(name) => self
                .context
                .named_range_reference_definition(name)
                .ok_or_else(|| {
                    ExcelError::new(ExcelErrorKind::Value)
                        .with_message(format!("{name} is not a range-valued name"))
                }),
            ReferenceType::Table(tref) => {
                let geom = self.context.table_geometry(&tref.name).ok_or_else(|| {
                    ExcelError::new(ExcelErrorKind::Value)
                        .with_message(format!("Unknown table: {}", tref.name))
                })?;
                let current_row = self.current_cell.map(|c| c.coord.row() + 1);
                let rect = crate::structured::lower_structured_rect(
                    &geom,
                    tref.specifier.as_ref(),
                    current_row,
                )?;
                Ok(crate::structured::rect_to_reference(&rect, geom.sheet))
            }
            _ => Err(ExcelError::new(ExcelErrorKind::Value)
                .with_message("Intersection operands must be references")),
        }
    }

    /// The range the spilled-range operator `A1#` denotes: the dynamic-array spill anchored at
    /// the operand cell (a formula that does not spill is its own 1×1 region). A non-formula
    /// anchor is `#REF!`, as in Excel.
    fn spill_range_of_anchor_reference(
        &self,
        anchor: &ReferenceType,
    ) -> Result<ReferenceType, ExcelError> {
        let (sheet, row, col) = match anchor {
            ReferenceType::Cell {
                sheet, row, col, ..
            } => (sheet.clone(), *row, *col),
            ReferenceType::Range {
                sheet,
                start_row: Some(sr),
                start_col: Some(sc),
                end_row: Some(er),
                end_col: Some(ec),
                ..
            } if sr == er && sc == ec => (sheet.clone(), *sr, *sc),
            _ => {
                return Err(ExcelError::new(ExcelErrorKind::Ref)
                    .with_message("The spilled-range operator # needs a single anchor cell"));
            }
        };
        let sheet_name = sheet.as_deref().unwrap_or(self.current_sheet);
        let (sr, sc, er, ec) = self
            .context
            .spill_range_of_anchor(sheet_name, row, col)
            .ok_or_else(|| {
                ExcelError::new(ExcelErrorKind::Ref)
                    .with_message("The # operand does not anchor a spilled array")
            })?;
        Ok(ReferenceType::Range {
            sheet: Some(sheet_name.to_string()),
            start_row: Some(sr),
            start_col: Some(sc),
            end_row: Some(er),
            end_col: Some(ec),
            start_row_abs: true,
            start_col_abs: true,
            end_row_abs: true,
            end_col_abs: true,
        })
    }

    /// The union operator `(A1:A2,B1:B2)` in VALUE context: the areas stacked into one array so
    /// aggregate consumers (`SUM`, `COUNT`, `MAX`, …) see every cell of every area. Equal-width
    /// areas stack vertically, equal-height areas side by side, anything else flattens into a
    /// single column — the shape only matters to the aggregate's cell walk.
    fn union_calc_values(
        &self,
        left: crate::traits::CalcValue<'a>,
        right: crate::traits::CalcValue<'a>,
    ) -> Result<LiteralValue, ExcelError> {
        fn rows_of(cv: crate::traits::CalcValue<'_>) -> Result<Vec<Vec<LiteralValue>>, ExcelError> {
            match cv {
                crate::traits::CalcValue::Scalar(LiteralValue::Array(rows)) => Ok(rows),
                crate::traits::CalcValue::Scalar(v) => Ok(vec![vec![v]]),
                crate::traits::CalcValue::Range(view) => {
                    let (rows, cols) = view.dims();
                    let mut out: Vec<Vec<LiteralValue>> = Vec::with_capacity(rows);
                    view.for_each_row(&mut |row| {
                        out.push(
                            (0..cols)
                                .map(|c| row.get(c).cloned().unwrap_or(LiteralValue::Empty))
                                .collect(),
                        );
                        Ok(())
                    })?;
                    Ok(out)
                }
                crate::traits::CalcValue::Callable(_) => {
                    Err(ExcelError::new(ExcelErrorKind::Value)
                        .with_message("Union operands must be references"))
                }
            }
        }
        let mut left = rows_of(left)?;
        let mut right = rows_of(right)?;
        let width = |rows: &[Vec<LiteralValue>]| rows.iter().map(Vec::len).max().unwrap_or(0);
        if left.is_empty() {
            return Ok(LiteralValue::Array(right));
        }
        if right.is_empty() {
            return Ok(LiteralValue::Array(left));
        }
        if width(&left) == width(&right) {
            left.append(&mut right);
            return Ok(LiteralValue::Array(left));
        }
        if left.len() == right.len() {
            let lw = width(&left);
            for (l, mut r) in left.iter_mut().zip(right.into_iter()) {
                l.resize(lw, LiteralValue::Empty);
                l.append(&mut r);
            }
            return Ok(LiteralValue::Array(left));
        }
        let column: Vec<Vec<LiteralValue>> = left
            .into_iter()
            .chain(right)
            .flatten()
            .map(|v| vec![v])
            .collect();
        Ok(LiteralValue::Array(column))
    }

    /* ===================  public  =================== */
    pub fn evaluate_ast(&self, node: &ASTNode) -> Result<crate::traits::CalcValue<'a>, ExcelError> {
        self.evaluate_ast_uncached(node)
    }

    pub(crate) fn evaluate_ast_with_offset(
        &self,
        node: &ASTNode,
        row_delta: i64,
        col_delta: i64,
    ) -> Result<crate::traits::CalcValue<'a>, ExcelError> {
        let offset = Self {
            context: self.context,
            current_sheet: self.current_sheet,
            current_cell: self.current_cell,
            local_env: self.local_env.clone(),
            reference_row_delta: row_delta,
            reference_col_delta: col_delta,
            disable_ast_planner: true,
            parameter_bindings: self.parameter_bindings,
        };
        offset.evaluate_ast_uncached(node)
    }

    pub(crate) fn reference_for_current_offset(
        &self,
        reference: &ReferenceType,
    ) -> Result<ReferenceType, ExcelError> {
        self.effective_reference(reference)
            .map(|reference| reference.into_owned())
    }

    pub(crate) fn evaluate_arena_ast_with_offset(
        &self,
        node_id: AstNodeId,
        row_delta: i64,
        col_delta: i64,
        data_store: &DataStore,
        sheet_registry: &SheetRegistry,
    ) -> Result<crate::traits::CalcValue<'a>, ExcelError> {
        let offset = Self {
            context: self.context,
            current_sheet: self.current_sheet,
            current_cell: self.current_cell,
            local_env: self.local_env.clone(),
            reference_row_delta: row_delta,
            reference_col_delta: col_delta,
            disable_ast_planner: true,
            parameter_bindings: self.parameter_bindings,
        };
        offset.evaluate_arena_ast(node_id, data_store, sheet_registry)
    }

    pub(crate) fn evaluate_arena_ast(
        &self,
        node_id: AstNodeId,
        data_store: &DataStore,
        sheet_registry: &SheetRegistry,
    ) -> Result<crate::traits::CalcValue<'a>, ExcelError> {
        let node = data_store.get_node(node_id).ok_or_else(|| {
            ExcelError::new(ExcelErrorKind::Value).with_message("Missing AST node")
        })?;

        match node {
            AstNodeData::Literal(vref) => {
                if let Some(bindings) = self.parameter_bindings
                    && let Some(slot_id) = bindings.literal_slots_by_node.get(&node_id)
                    && let Some(value) = bindings.literal_values.get(slot_id.0 as usize)
                {
                    return Ok(crate::traits::CalcValue::Scalar(value.clone()));
                }
                Ok(crate::traits::CalcValue::Scalar(
                    data_store.retrieve_value(*vref),
                ))
            }
            AstNodeData::Reference { ref_type, .. } => {
                if self.local_env.is_empty()
                    && let CompactRefType::Cell {
                        sheet,
                        row,
                        col,
                        row_abs,
                        col_abs,
                    } = ref_type
                    && *row > 0
                    && *col > 0
                {
                    let sheet_name = match sheet {
                        Some(SheetKey::Id(id)) => Some(sheet_registry.name(*id)),
                        Some(SheetKey::Name(name_id)) => {
                            Some(data_store.resolve_ast_string(*name_id))
                        }
                        None => None,
                    };
                    let row = shift_axis_for_offset(*row, self.reference_row_delta, *row_abs)?;
                    let col = shift_axis_for_offset(*col, self.reference_col_delta, *col_abs)?;
                    let value = self.context.resolve_cell_reference_value(
                        sheet_name,
                        row,
                        col,
                        self.current_sheet,
                    )?;
                    Ok(crate::traits::CalcValue::Scalar(value))
                } else {
                    let reference =
                        data_store.reconstruct_reference_type_for_eval(ref_type, sheet_registry);
                    let reference = self.effective_reference(&reference)?;
                    if let Some(local) = self.resolve_local_reference(&reference) {
                        return Ok(local);
                    }
                    self.eval_reference_to_calc(&reference)
                }
            }
            AstNodeData::UnaryOp { op_id, expr_id } => {
                let op = data_store.resolve_ast_string(*op_id);
                if op == "#" {
                    let anchor =
                        self.evaluate_arena_ast_as_reference(*expr_id, data_store, sheet_registry)?;
                    let spill = self.spill_range_of_anchor_reference(&anchor)?;
                    return self.eval_reference_to_calc(&spill);
                }
                let expr = self.evaluate_arena_ast(*expr_id, data_store, sheet_registry)?;

                if op == "@" {
                    // Prefer reference-aware implicit intersection so we don't depend on
                    // RangeView absolute coordinates (important for lightweight test contexts).
                    if let Some(AstNodeData::Reference { ref_type, .. }) =
                        data_store.get_node(*expr_id)
                    {
                        let reference = data_store
                            .reconstruct_reference_type_for_eval(ref_type, sheet_registry);
                        let v = self.implicit_intersection_from_reference(&reference);
                        return Ok(crate::traits::CalcValue::Scalar(v));
                    }

                    let v = self.eval_implicit_intersection_calc(expr);
                    return Ok(crate::traits::CalcValue::Scalar(v));
                }
                // For now, materialize for operators. Future: virtual range ops.
                let v = expr.into_literal();
                match v {
                    LiteralValue::Array(arr) => self
                        .map_array(arr, |cell| self.eval_unary_scalar(op, cell))
                        .map(crate::traits::CalcValue::Scalar),
                    other => self
                        .eval_unary_scalar(op, other)
                        .map(crate::traits::CalcValue::Scalar),
                }
            }
            AstNodeData::BinaryOp {
                op_id,
                left_id,
                right_id,
            } => {
                let op = data_store.resolve_ast_string(*op_id);
                if op == ":" {
                    let lref =
                        self.evaluate_arena_ast_as_reference(*left_id, data_store, sheet_registry)?;
                    let rref = self.evaluate_arena_ast_as_reference(
                        *right_id,
                        data_store,
                        sheet_registry,
                    )?;
                    return match crate::reference::combine_references(&lref, &rref) {
                        Ok(_r) => Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                            ExcelError::new(ExcelErrorKind::Ref).with_message(
                                "Reference produced by ':' cannot be used directly as a value",
                            ),
                        ))),
                        Err(e) => Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(e))),
                    };
                }
                if op == " " {
                    let intersection = self
                        .set_operand_arena_reference(*left_id, data_store, sheet_registry)
                        .and_then(|lref| {
                            Ok((
                                lref,
                                self.set_operand_arena_reference(
                                    *right_id,
                                    data_store,
                                    sheet_registry,
                                )?,
                            ))
                        })
                        .and_then(|(lref, rref)| self.intersect_reference_operands(&lref, &rref));
                    return match intersection {
                        Ok(reference) => self.eval_reference_to_calc(&reference),
                        Err(e) => Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(e))),
                    };
                }
                if op == "," {
                    let left = self.evaluate_arena_ast(*left_id, data_store, sheet_registry)?;
                    let right = self.evaluate_arena_ast(*right_id, data_store, sheet_registry)?;
                    return self
                        .union_calc_values(left, right)
                        .map(crate::traits::CalcValue::Scalar);
                }

                let left = self
                    .evaluate_arena_ast(*left_id, data_store, sheet_registry)?
                    .into_literal();
                let right = self
                    .evaluate_arena_ast(*right_id, data_store, sheet_registry)?
                    .into_literal();

                if matches!(op, "=" | "<>" | ">" | "<" | ">=" | "<=") {
                    return self
                        .compare(op, left, right)
                        .map(crate::traits::CalcValue::Scalar);
                }

                match op {
                    "+" => self
                        .add_sub_date_aware('+', left, right)
                        .map(crate::traits::CalcValue::Scalar),
                    "-" => self
                        .add_sub_date_aware('-', left, right)
                        .map(crate::traits::CalcValue::Scalar),
                    "*" => self
                        .numeric_binary(left, right, |a, b| a * b)
                        .map(crate::traits::CalcValue::Scalar),
                    "/" => self
                        .divide(left, right)
                        .map(crate::traits::CalcValue::Scalar),
                    "^" => self
                        .power(left, right)
                        .map(crate::traits::CalcValue::Scalar),
                    "&" => self
                        .concat(left, right)
                        .map(crate::traits::CalcValue::Scalar),
                    _ => Err(ExcelError::new(ExcelErrorKind::NImpl)
                        .with_message(format!("Binary op '{op}'"))),
                }
            }
            AstNodeData::Array { .. } => {
                let (rows, cols, elements) =
                    data_store.get_array_elems(node_id).ok_or_else(|| {
                        ExcelError::new(ExcelErrorKind::Value).with_message("Invalid array")
                    })?;

                let rows_usize = rows as usize;
                let cols_usize = cols as usize;
                let mut out: Vec<Vec<LiteralValue>> = Vec::with_capacity(rows_usize);
                for r in 0..rows_usize {
                    let mut row = Vec::with_capacity(cols_usize);
                    for c in 0..cols_usize {
                        let idx = r * cols_usize + c;
                        if let Some(&elem_id) = elements.get(idx) {
                            row.push(
                                self.evaluate_arena_ast(elem_id, data_store, sheet_registry)?
                                    .into_literal(),
                            );
                        }
                    }
                    out.push(row);
                }

                Ok(crate::traits::CalcValue::Range(
                    crate::engine::range_view::RangeView::from_owned_rows(
                        out,
                        self.context.date_system(),
                    ),
                ))
            }
            AstNodeData::Function { name_id, .. } => {
                let name = data_store.resolve_ast_string(*name_id);
                let args = data_store.get_args(node_id).ok_or_else(|| {
                    ExcelError::new(ExcelErrorKind::Value).with_message("Missing function args")
                })?;

                if let Some(fun) = self.context.get_function("", name) {
                    let handles: Vec<ArgumentHandle> = args
                        .iter()
                        .copied()
                        .map(|arg_id| {
                            ArgumentHandle::new_arena(arg_id, self, data_store, sheet_registry)
                        })
                        .collect();

                    let fctx = DefaultFunctionContext::new_with_sheet(
                        self.context,
                        self.current_cell,
                        self.current_sheet,
                    );

                    return Self::function_error_as_value(fun.dispatch(&handles, &fctx));
                }

                if let Some(callable) = self.resolve_local_callable(name) {
                    let mut eval_args = Vec::with_capacity(args.len());
                    for arg_id in args {
                        eval_args.push(
                            self.evaluate_arena_ast(*arg_id, data_store, sheet_registry)?
                                .into_literal(),
                        );
                    }
                    return callable.invoke(self, &eval_args);
                }

                // Same contract as `eval_function_to_calc`: an unknown function is a
                // `#NAME?` value that IS*/IFERROR can catch, not a hard failure.
                Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                    ExcelError::new(ExcelErrorKind::Name)
                        .with_message(format!("Unknown function: {name}")),
                )))
            }
        }
    }

    fn evaluate_ast_uncached(
        &self,
        node: &ASTNode,
    ) -> Result<crate::traits::CalcValue<'a>, ExcelError> {
        if self.disable_ast_planner {
            return self.eval_tree_uncached(node);
        }

        // Plan-aware evaluation: build a plan for this node and execute accordingly.
        // Provide the planner with a lightweight range-dimension probe and function lookup
        // so it can select chunked reduction and arg-parallel strategies where appropriate.
        let current_sheet = self.current_sheet.to_string();
        let range_probe = |reference: &ReferenceType| -> Option<(u32, u32)> {
            // Mirror Engine::resolve_range_storage bound normalization without materialising
            use formualizer_parse::parser::ReferenceType as RT;
            match reference {
                RT::Range {
                    sheet,
                    start_row,
                    start_col,
                    end_row,
                    end_col,
                    ..
                } => {
                    let sheet_name = sheet.as_deref().unwrap_or(&current_sheet);
                    // Start with provided values, fill None from used-region or sheet bounds.
                    let mut sr = *start_row;
                    let mut sc = *start_col;
                    let mut er = *end_row;
                    let mut ec = *end_col;

                    // Column-only: rows are None on both ends
                    if sr.is_none() && er.is_none() {
                        // Full-column reference: anchor at row 1 for alignment across columns
                        let scv = sc.unwrap_or(1);
                        let ecv = ec.unwrap_or(scv);
                        sr = Some(1);
                        if let Some((_, max_r)) =
                            self.context.used_rows_for_columns(sheet_name, scv, ecv)
                        {
                            er = Some(max_r);
                        } else if let Some((max_rows, _)) = self.context.sheet_bounds(sheet_name) {
                            er = Some(max_rows);
                        }
                    }

                    // Row-only: cols are None on both ends
                    if sc.is_none() && ec.is_none() {
                        // Full-row reference: anchor at column 1 for alignment across rows
                        let srv = sr.unwrap_or(1);
                        let erv = er.unwrap_or(srv);
                        sc = Some(1);
                        if let Some((_, max_c)) =
                            self.context.used_cols_for_rows(sheet_name, srv, erv)
                        {
                            ec = Some(max_c);
                        } else if let Some((_, max_cols)) = self.context.sheet_bounds(sheet_name) {
                            ec = Some(max_cols);
                        }
                    }

                    // Partially bounded (e.g., A1:A or A:A10)
                    if sr.is_some() && er.is_none() {
                        let scv = sc.unwrap_or(1);
                        let ecv = ec.unwrap_or(scv);
                        if let Some((_, max_r)) =
                            self.context.used_rows_for_columns(sheet_name, scv, ecv)
                        {
                            er = Some(max_r);
                        } else if let Some((max_rows, _)) = self.context.sheet_bounds(sheet_name) {
                            er = Some(max_rows);
                        }
                    }
                    if er.is_some() && sr.is_none() {
                        // Open start: anchor at row 1
                        sr = Some(1);
                    }
                    if sc.is_some() && ec.is_none() {
                        let srv = sr.unwrap_or(1);
                        let erv = er.unwrap_or(srv);
                        if let Some((_, max_c)) =
                            self.context.used_cols_for_rows(sheet_name, srv, erv)
                        {
                            ec = Some(max_c);
                        } else if let Some((_, max_cols)) = self.context.sheet_bounds(sheet_name) {
                            ec = Some(max_cols);
                        }
                    }
                    if ec.is_some() && sc.is_none() {
                        // Open start: anchor at column 1
                        sc = Some(1);
                    }

                    let sr = sr.unwrap_or(1);
                    let sc = sc.unwrap_or(1);
                    let er = er.unwrap_or(sr.saturating_sub(1));
                    let ec = ec.unwrap_or(sc.saturating_sub(1));
                    if er < sr || ec < sc {
                        return Some((0, 0));
                    }
                    Some((er.saturating_sub(sr) + 1, ec.saturating_sub(sc) + 1))
                }
                RT::Cell { .. } => Some((1, 1)),
                _ => None,
            }
        };
        let fn_lookup = |ns: &str, name: &str| self.context.get_function(ns, name);

        let mut planner = crate::planner::Planner::new(crate::planner::PlanConfig::default())
            .with_range_probe(&range_probe)
            .with_function_lookup(&fn_lookup);
        let plan = planner.plan(node);
        self.eval_with_plan(node, &plan.root)
    }

    fn eval_tree_uncached(
        &self,
        node: &ASTNode,
    ) -> Result<crate::traits::CalcValue<'a>, ExcelError> {
        match &node.node_type {
            ASTNodeType::Literal(v) => Ok(crate::traits::CalcValue::Scalar(v.clone())),
            ASTNodeType::Reference { reference, .. } => self.eval_ast_reference_to_calc(reference),
            ASTNodeType::UnaryOp { op, expr } => self
                .eval_unary(op, expr)
                .map(crate::traits::CalcValue::Scalar),
            ASTNodeType::BinaryOp { op, left, right } => self
                .eval_binary(op, left, right)
                .map(crate::traits::CalcValue::Scalar),
            ASTNodeType::Function { name, args } => self.eval_function_to_calc(name, args),
            ASTNodeType::Call { callee, args } => self.eval_call_to_calc(callee, args),
            ASTNodeType::Array(rows) => self.eval_array_literal_to_calc(rows),
        }
    }

    fn eval_with_plan(
        &self,
        node: &ASTNode,
        plan_node: &crate::planner::PlanNode,
    ) -> Result<crate::traits::CalcValue<'a>, ExcelError> {
        match &node.node_type {
            ASTNodeType::Literal(v) => Ok(crate::traits::CalcValue::Scalar(v.clone())),
            ASTNodeType::Reference { reference, .. } => self.eval_ast_reference_to_calc(reference),
            ASTNodeType::UnaryOp { op, expr } => {
                // For now, reuse existing unary implementation (which recurses).
                // In a later phase, we can map plan_node.children[0].
                self.eval_unary(op, expr)
                    .map(crate::traits::CalcValue::Scalar)
            }
            ASTNodeType::BinaryOp { op, left, right } => self
                .eval_binary(op, left, right)
                .map(crate::traits::CalcValue::Scalar),
            ASTNodeType::Function { name, args } => {
                let strategy = plan_node.strategy;
                if let Some(fun) = self.context.get_function("", name) {
                    use crate::function::FnCaps;
                    use crate::planner::ExecStrategy;
                    let caps = fun.caps();

                    // Short-circuit or volatile: always sequential
                    if caps.contains(FnCaps::SHORT_CIRCUIT) || caps.contains(FnCaps::VOLATILE) {
                        return self.eval_function_to_calc(name, args);
                    }

                    // Windowed/chunked strategies are handled by the unified `eval()` path.

                    // Arg-parallel: prewarm subexpressions and then dispatch
                    if matches!(strategy, ExecStrategy::ArgParallel)
                        && caps.contains(FnCaps::PARALLEL_ARGS)
                    {
                        // Sequential prewarm of subexpressions (safe without Sync bounds)
                        for arg in args {
                            match &arg.node_type {
                                ASTNodeType::Reference { reference, .. } => {
                                    if let Ok(reference) = self.effective_reference(reference) {
                                        let _ = self
                                            .context
                                            .resolve_range_view(&reference, self.current_sheet);
                                    }
                                }
                                _ => {
                                    let _ = self.evaluate_ast(arg);
                                }
                            }
                        }
                        return self.eval_function_to_calc(name, args);
                    }

                    // Default path
                    return self.eval_function_to_calc(name, args);
                }
                self.eval_function_to_calc(name, args)
            }
            ASTNodeType::Call { callee, args } => self.eval_call_to_calc(callee, args),
            ASTNodeType::Array(rows) => self.eval_array_literal_to_calc(rows),
        }
    }

    /* ===================  reference  =================== */
    fn eval_ast_reference_to_calc(
        &self,
        reference: &ReferenceType,
    ) -> Result<crate::traits::CalcValue<'a>, ExcelError> {
        if !self.local_env.is_empty() {
            let reference = self.effective_reference(reference)?;
            if let Some(local) = self.resolve_local_reference(&reference) {
                return Ok(local);
            }
            return self.eval_reference_to_calc(&reference);
        }

        if let ReferenceType::Cell {
            sheet,
            row,
            col,
            row_abs,
            col_abs,
        } = reference
        {
            let row = shift_axis_for_offset(*row, self.reference_row_delta, *row_abs)?;
            let col = shift_axis_for_offset(*col, self.reference_col_delta, *col_abs)?;
            return Ok(crate::traits::CalcValue::Scalar(
                self.context.resolve_cell_reference_value(
                    sheet.as_deref(),
                    row,
                    col,
                    self.current_sheet,
                )?,
            ));
        }

        let reference = self.effective_reference(reference)?;
        self.eval_reference_to_calc(&reference)
    }

    fn eval_reference_to_calc(
        &self,
        reference: &ReferenceType,
    ) -> Result<crate::traits::CalcValue<'a>, ExcelError> {
        if let ReferenceType::Cell {
            sheet, row, col, ..
        } = reference
        {
            return Ok(crate::traits::CalcValue::Scalar(
                self.context.resolve_cell_reference_value(
                    sheet.as_deref(),
                    *row,
                    *col,
                    self.current_sheet,
                )?,
            ));
        }

        let lowered = self.lower_structured_table_ref(reference, self.current_sheet)?;
        let target = lowered.as_ref().unwrap_or(reference);
        let view = match self.context.resolve_range_view(target, self.current_sheet) {
            Ok(view) => view,
            // A defined name that does not resolve is Excel's `#NAME?` *value*, not a
            // failure of the formula: `=ISERROR(FOO)` is TRUE, `=IFERROR(FOO,1)` is 1 and
            // `=ERROR.TYPE(FOO)` is 5. Surfacing it as `Err` would bypass every
            // argument-level handler. Cancellation is the one genuine abort.
            Err(e)
                if matches!(target, ReferenceType::NamedRange(_))
                    && e.kind != ExcelErrorKind::Cancelled =>
            {
                return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(e)));
            }
            Err(e) => return Err(e),
        };
        let view = view.with_cancel_token(self.context.cancellation_token());
        Ok(crate::traits::CalcValue::Range(view))
    }

    fn eval_reference(&self, reference: &ReferenceType) -> Result<LiteralValue, ExcelError> {
        self.eval_reference_to_calc(reference)
            .map(|cv| cv.into_literal())
    }

    /* ===================  unary ops  =================== */
    fn eval_unary(&self, op: &str, expr: &ASTNode) -> Result<LiteralValue, ExcelError> {
        if op == "#" {
            let anchor = self.evaluate_ast_as_reference(expr)?;
            let spill = self.spill_range_of_anchor_reference(&anchor)?;
            return Ok(self.eval_reference_to_calc(&spill)?.into_literal());
        }
        if op == "@" {
            if let ASTNodeType::Reference { reference, .. } = &expr.node_type {
                let reference = self.effective_reference(reference)?;
                return Ok(self.implicit_intersection_from_reference(&reference));
            }

            let cv = self.evaluate_ast(expr)?;
            return Ok(self.eval_implicit_intersection_calc(cv));
        }

        let v = self.evaluate_ast(expr)?.into_literal();
        match v {
            LiteralValue::Array(arr) => {
                self.map_array(arr, |cell| self.eval_unary_scalar(op, cell))
            }
            other => self.eval_unary_scalar(op, other),
        }
    }

    fn eval_unary_scalar(&self, op: &str, v: LiteralValue) -> Result<LiteralValue, ExcelError> {
        match op {
            // Excel/LibreOffice treat unary `+` as a pass-through (identity) operator,
            // not as a numeric coercion. `=+"2014F"` returns the text "2014F"; only the
            // unary `-` form coerces operands to numbers. The `=+A1` idiom is common in
            // finance models (Lotus 1-2-3 carry-over) and must preserve text labels.
            "+" => Ok(v),
            "-" => self.apply_number_unary(v, |n| -n),
            "%" => self.apply_number_unary(v, |n| n / 100.0),
            _ => {
                Err(ExcelError::new(ExcelErrorKind::NImpl).with_message(format!("Unary op '{op}'")))
            }
        }
    }

    fn eval_implicit_intersection_calc(&self, cv: crate::traits::CalcValue<'a>) -> LiteralValue {
        let (cur_r0, cur_c0) = match self.current_cell {
            Some(cell) => (cell.coord.row() as usize, cell.coord.col() as usize),
            None => (0usize, 0usize),
        };

        match cv {
            crate::traits::CalcValue::Scalar(v) => match v {
                LiteralValue::Array(arr) => {
                    if arr.is_empty() || arr.first().map(|r| r.is_empty()).unwrap_or(true) {
                        return LiteralValue::Error(ExcelError::new(ExcelErrorKind::Value));
                    }
                    arr[0][0].clone()
                }
                other => other,
            },
            crate::traits::CalcValue::Range(rv) => {
                if rv.is_empty() {
                    return LiteralValue::Error(ExcelError::new(ExcelErrorKind::Value));
                }

                // Array results (array literals and many dynamic-array functions) are materialized
                // into an owned RangeView with a temporary backing sheet ("__tmp").
                // For explicit @, interpret these as anchored at the formula cell and select the
                // top-left element.
                if rv.sheet_name() == "__tmp" {
                    return rv.get_cell(0, 0);
                }

                if let Some(v) = rv.as_1x1() {
                    return v;
                }

                let (rows, cols) = rv.dims();
                let sr = rv.start_row();
                let sc = rv.start_col();
                let er = rv.end_row();
                let ec = rv.end_col();

                // Excel-compatible implicit intersection (simplified):
                // - Nx1: pick by row
                // - 1xM: pick by column
                // - NxM: pick by (row,col)
                if cols == 1 {
                    if cur_r0 < sr || cur_r0 > er {
                        return LiteralValue::Error(ExcelError::new(ExcelErrorKind::Value));
                    }
                    let rel_r = cur_r0 - sr;
                    return rv.get_cell(rel_r, 0);
                }

                if rows == 1 {
                    if cur_c0 < sc || cur_c0 > ec {
                        return LiteralValue::Error(ExcelError::new(ExcelErrorKind::Value));
                    }
                    let rel_c = cur_c0 - sc;
                    return rv.get_cell(0, rel_c);
                }

                if cur_r0 < sr || cur_r0 > er || cur_c0 < sc || cur_c0 > ec {
                    return LiteralValue::Error(ExcelError::new(ExcelErrorKind::Value));
                }
                let rel_r = cur_r0 - sr;
                let rel_c = cur_c0 - sc;
                rv.get_cell(rel_r, rel_c)
            }
            crate::traits::CalcValue::Callable(_) => LiteralValue::Error(
                ExcelError::new(ExcelErrorKind::Calc).with_message("LAMBDA value must be invoked"),
            ),
        }
    }

    fn implicit_intersection_from_reference(&self, reference: &ReferenceType) -> LiteralValue {
        let (cur_r1, cur_c1) = match self.current_cell {
            Some(cell) => (
                cell.coord.row().saturating_add(1),
                cell.coord.col().saturating_add(1),
            ),
            None => (1u32, 1u32),
        };

        match reference {
            ReferenceType::Cell {
                sheet, row, col, ..
            } => {
                let sheet_name = sheet.as_deref().unwrap_or(self.current_sheet);
                match self
                    .context
                    .resolve_cell_reference(Some(sheet_name), *row, *col)
                {
                    Ok(v) => v,
                    Err(e) => LiteralValue::Error(e),
                }
            }
            ReferenceType::Range {
                sheet,
                start_row,
                start_col,
                end_row,
                end_col,
                ..
            } => {
                let sheet_name = sheet.as_deref().unwrap_or(self.current_sheet);

                let (sr, sc, er, ec) = match (start_row, start_col, end_row, end_col) {
                    (Some(sr), Some(sc), Some(er), Some(ec)) => (*sr, *sc, *er, *ec),
                    _ => {
                        // For open-ended/infinite ranges, fall back to the RangeView-based path.
                        // This path may be less precise in minimal test contexts.
                        let cv = match self.eval_reference_to_calc(reference) {
                            Ok(cv) => cv,
                            Err(e) => return LiteralValue::Error(e),
                        };
                        return self.eval_implicit_intersection_calc(cv);
                    }
                };

                // Normalize bounds (A10:A1 is legal syntax; treat as swapped).
                let (mut sr, mut er) = (sr, er);
                let (mut sc, mut ec) = (sc, ec);
                if sr > er {
                    std::mem::swap(&mut sr, &mut er);
                }
                if sc > ec {
                    std::mem::swap(&mut sc, &mut ec);
                }

                let pick = if sc == ec {
                    // Column vector: intersect by row
                    if cur_r1 < sr || cur_r1 > er {
                        return LiteralValue::Error(ExcelError::new(ExcelErrorKind::Value));
                    }
                    (cur_r1, sc)
                } else if sr == er {
                    // Row vector: intersect by column
                    if cur_c1 < sc || cur_c1 > ec {
                        return LiteralValue::Error(ExcelError::new(ExcelErrorKind::Value));
                    }
                    (sr, cur_c1)
                } else {
                    // 2D: require both axes
                    if cur_r1 < sr || cur_r1 > er || cur_c1 < sc || cur_c1 > ec {
                        return LiteralValue::Error(ExcelError::new(ExcelErrorKind::Value));
                    }
                    (cur_r1, cur_c1)
                };

                match self
                    .context
                    .resolve_cell_reference(Some(sheet_name), pick.0, pick.1)
                {
                    Ok(v) => v,
                    Err(e) => LiteralValue::Error(e),
                }
            }
            // Named ranges / tables / external: fall back to materializing and intersecting.
            other => {
                let cv = match self.eval_reference_to_calc(other) {
                    Ok(cv) => cv,
                    Err(e) => return LiteralValue::Error(e),
                };
                self.eval_implicit_intersection_calc(cv)
            }
        }
    }

    fn apply_number_unary<F>(&self, v: LiteralValue, f: F) -> Result<LiteralValue, ExcelError>
    where
        F: Fn(f64) -> f64,
    {
        match crate::coercion::to_number_lenient_with_locale(&v, &self.context.locale()) {
            Ok(n) => match crate::coercion::sanitize_numeric(f(n)) {
                Ok(n2) => Ok(LiteralValue::Number(n2)),
                Err(e) => Ok(LiteralValue::Error(e)),
            },
            Err(e) => Ok(LiteralValue::Error(e)),
        }
    }

    /* ===================  binary ops  =================== */
    fn eval_binary(
        &self,
        op: &str,
        left: &ASTNode,
        right: &ASTNode,
    ) -> Result<LiteralValue, ExcelError> {
        // Comparisons use dedicated path.
        if matches!(op, "=" | "<>" | ">" | "<" | ">=" | "<=") {
            let l = self.evaluate_ast(left)?.into_literal();
            let r = self.evaluate_ast(right)?.into_literal();
            return self.compare(op, l, r);
        }
        // Reference set operators: intersection yields a reference (read like any range),
        // union stacks its areas for the consuming aggregate.
        if op == " " {
            let intersection = self
                .set_operand_reference(left)
                .and_then(|lref| Ok((lref, self.set_operand_reference(right)?)))
                .and_then(|(lref, rref)| self.intersect_reference_operands(&lref, &rref));
            return match intersection {
                Ok(reference) => Ok(self.eval_reference_to_calc(&reference)?.into_literal()),
                Err(e) => Ok(LiteralValue::Error(e)),
            };
        }
        if op == "," {
            let l = self.evaluate_ast(left)?;
            let r = self.evaluate_ast(right)?;
            return self.union_calc_values(l, r);
        }

        let l_val = self.evaluate_ast(left)?.into_literal();
        let r_val = self.evaluate_ast(right)?.into_literal();

        match op {
            "+" => self.add_sub_date_aware('+', l_val, r_val),
            "-" => self.add_sub_date_aware('-', l_val, r_val),
            "*" => self.numeric_binary(l_val, r_val, |a, b| a * b),
            "/" => self.divide(l_val, r_val),
            "^" => self.power(l_val, r_val),
            "&" => self.concat(l_val, r_val),
            ":" => {
                // Compute a combined reference; in value context return #REF! for now.
                let lref = self.evaluate_ast_as_reference(left)?;
                let rref = self.evaluate_ast_as_reference(right)?;
                match crate::reference::combine_references(&lref, &rref) {
                    Ok(_r) => Err(ExcelError::new(ExcelErrorKind::Ref).with_message(
                        "Reference produced by ':' cannot be used directly as a value",
                    )),
                    Err(e) => Ok(LiteralValue::Error(e)),
                }
            }
            _ => {
                Err(ExcelError::new(ExcelErrorKind::NImpl)
                    .with_message(format!("Binary op '{op}'")))
            }
        }
    }

    fn add_sub_date_aware(
        &self,
        op: char,
        left: LiteralValue,
        right: LiteralValue,
    ) -> Result<LiteralValue, ExcelError> {
        debug_assert!(op == '+' || op == '-');

        self.broadcast_apply(left, right, |l, r| {
            use LiteralValue::*;

            let date_system = self.context.date_system();

            let date_like_serial = |v: &LiteralValue| -> Option<f64> {
                match v {
                    Date(d) => Some(crate::builtins::datetime::date_to_serial_for(
                        date_system,
                        d,
                    )),
                    DateTime(dt) => Some(crate::builtins::datetime::datetime_to_serial_for(
                        date_system,
                        dt,
                    )),
                    _ => None,
                }
            };

            let to_num = |v: &LiteralValue| -> Result<f64, ExcelError> {
                crate::coercion::to_number_lenient_with_locale(v, &self.context.locale())
            };

            let serial_to_literal = |serial: f64| -> LiteralValue {
                match crate::coercion::sanitize_numeric(serial) {
                    Ok(serial) => {
                        match crate::builtins::datetime::serial_to_datetime_for(date_system, serial)
                        {
                            Ok(dt) => {
                                if dt.time() == chrono::NaiveTime::from_hms_opt(0, 0, 0).unwrap() {
                                    Date(dt.date())
                                } else {
                                    DateTime(dt)
                                }
                            }
                            Err(e) => Error(e),
                        }
                    }
                    Err(e) => Error(e),
                }
            };

            // Date +/- number => date (propagate temporal tag)
            if let Some(ls) = date_like_serial(&l) {
                match op {
                    '+' => {
                        let rn = to_num(&r)?;
                        return Ok(serial_to_literal(ls + rn));
                    }
                    '-' => {
                        // Date - Date => numeric day delta (Excel-compatible)
                        if let Some(rs) = date_like_serial(&r) {
                            return Ok(Number(ls - rs));
                        }
                        let rn = to_num(&r)?;
                        return Ok(serial_to_literal(ls - rn));
                    }
                    _ => unreachable!(),
                }
            }

            // Number + Date => date (commutative)
            if op == '+'
                && let Some(rs) = date_like_serial(&r)
            {
                let ln = to_num(&l)?;
                return Ok(serial_to_literal(ln + rs));
            }

            // Fallback: regular numeric operation
            self.numeric_binary(l, r, |a, b| if op == '+' { a + b } else { a - b })
        })
    }

    /* ===================  function calls  =================== */
    /// Immediate invocation of a callable expression: `LAMBDA(x,x*2)(3)` or
    /// `LET(f,LAMBDA(x,x+1),f)(41)`. The callee must evaluate to a `LAMBDA`
    /// value; arguments are evaluated eagerly (ranges arrive as arrays).
    pub(crate) fn eval_call_to_calc(
        &self,
        callee: &ASTNode,
        args: &[ASTNode],
    ) -> Result<crate::traits::CalcValue<'a>, ExcelError> {
        let callable = match self.evaluate_ast(callee)? {
            crate::traits::CalcValue::Callable(c) => c,
            crate::traits::CalcValue::Scalar(LiteralValue::Error(e)) => {
                return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(e)));
            }
            _ => {
                return Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
                    ExcelError::new(ExcelErrorKind::Value)
                        .with_message("Only a LAMBDA value can be invoked"),
                )));
            }
        };
        let mut eval_args = Vec::with_capacity(args.len());
        for arg in args {
            eval_args.push(self.evaluate_ast(arg)?.into_literal());
        }
        callable.invoke(self, &eval_args)
    }

    /// A worksheet function that fails with an Excel error has *produced* that error: in
    /// Excel `=ISERROR(VLOOKUP(1,FOO,1,FALSE))` is TRUE and `=IF(ISERROR(SUMIF(FOO,1)),1,2)`
    /// is 1 — the idiom every pre-IFERROR workbook uses. Builtins reach such errors through
    /// `?` on `ArgumentHandle::range_view()`/`resolve_range_view` (an undefined name, a
    /// `#REF!` reference), which would otherwise bypass every argument-level handler and
    /// make IS* disagree with IFERROR on the same expression. Cancellation is the one
    /// genuine abort.
    fn function_error_as_value(
        result: Result<crate::traits::CalcValue<'a>, ExcelError>,
    ) -> Result<crate::traits::CalcValue<'a>, ExcelError> {
        match result {
            Err(e) if e.kind != ExcelErrorKind::Cancelled => {
                Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(e)))
            }
            other => other,
        }
    }

    fn eval_function_to_calc(
        &self,
        name: &str,
        args: &[ASTNode],
    ) -> Result<crate::traits::CalcValue<'a>, ExcelError> {
        if let Some(fun) = self.context.get_function("", name) {
            let handles: Vec<ArgumentHandle> =
                args.iter().map(|n| ArgumentHandle::new(n, self)).collect();
            // Use the function's built-in dispatch method with a narrow FunctionContext
            let fctx = DefaultFunctionContext::new_with_sheet(
                self.context,
                self.current_cell,
                self.current_sheet,
            );
            return Self::function_error_as_value(fun.dispatch(&handles, &fctx));
        }

        if let Some(callable) = self.resolve_local_callable(name) {
            let mut eval_args = Vec::with_capacity(args.len());
            for arg in args {
                eval_args.push(self.evaluate_ast(arg)?.into_literal());
            }
            return callable.invoke(self, &eval_args);
        }

        // Include the function name in the error message for better debugging
        Ok(crate::traits::CalcValue::Scalar(LiteralValue::Error(
            ExcelError::new(ExcelErrorKind::Name).with_message(format!("Unknown function: {name}")),
        )))
    }

    fn eval_function(&self, name: &str, args: &[ASTNode]) -> Result<LiteralValue, ExcelError> {
        self.eval_function_to_calc(name, args)
            .map(|cv| cv.into_literal())
    }

    pub fn function_context(&self, cell_ref: Option<&CellRef>) -> DefaultFunctionContext<'_> {
        DefaultFunctionContext::new_with_sheet(self.context, cell_ref.cloned(), self.current_sheet)
    }

    /* ===================  array literal  =================== */
    fn eval_array_literal_to_calc(
        &self,
        rows: &[Vec<ASTNode>],
    ) -> Result<crate::traits::CalcValue<'a>, ExcelError> {
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let mut r = Vec::with_capacity(row.len());
            for cell in row {
                r.push(self.evaluate_ast(cell)?.into_literal());
            }
            out.push(r);
        }
        Ok(crate::traits::CalcValue::Range(
            crate::engine::range_view::RangeView::from_owned_rows(out, self.context.date_system()),
        ))
    }

    fn eval_array_literal(&self, rows: &[Vec<ASTNode>]) -> Result<LiteralValue, ExcelError> {
        self.eval_array_literal_to_calc(rows)
            .map(|cv| cv.into_literal())
    }

    /* ===================  helpers  =================== */
    fn numeric_binary<F>(
        &self,
        left: LiteralValue,
        right: LiteralValue,
        f: F,
    ) -> Result<LiteralValue, ExcelError>
    where
        F: Fn(f64, f64) -> f64 + Copy,
    {
        self.broadcast_apply(left, right, |l, r| {
            let a = crate::coercion::to_number_lenient_with_locale(&l, &self.context.locale());
            let b = crate::coercion::to_number_lenient_with_locale(&r, &self.context.locale());
            match (a, b) {
                (Ok(a), Ok(b)) => match crate::coercion::sanitize_numeric(f(a, b)) {
                    Ok(n2) => Ok(LiteralValue::Number(n2)),
                    Err(e) => Ok(LiteralValue::Error(e)),
                },
                (Err(e), _) | (_, Err(e)) => Ok(LiteralValue::Error(e)),
            }
        })
    }

    /// `&` lifts over arrays element-wise (`A1:A3&"x"` spills `10x,20x,30x`) and
    /// propagates an error operand instead of concatenating its text.
    fn concat(&self, left: LiteralValue, right: LiteralValue) -> Result<LiteralValue, ExcelError> {
        self.broadcast_apply(left, right, |l, r| {
            if let LiteralValue::Error(e) = l {
                return Ok(LiteralValue::Error(e));
            }
            if let LiteralValue::Error(e) = r {
                return Ok(LiteralValue::Error(e));
            }
            Ok(LiteralValue::Text(format!(
                "{}{}",
                crate::coercion::to_text_invariant(&l),
                crate::coercion::to_text_invariant(&r)
            )))
        })
    }

    fn divide(&self, left: LiteralValue, right: LiteralValue) -> Result<LiteralValue, ExcelError> {
        self.broadcast_apply(left, right, |l, r| {
            let ln = crate::coercion::to_number_lenient_with_locale(&l, &self.context.locale());
            let rn = crate::coercion::to_number_lenient_with_locale(&r, &self.context.locale());
            let (a, b) = match (ln, rn) {
                (Ok(a), Ok(b)) => (a, b),
                (Err(e), _) | (_, Err(e)) => return Ok(LiteralValue::Error(e)),
            };
            if b == 0.0 {
                return Ok(LiteralValue::Error(ExcelError::from_error_string(
                    "#DIV/0!",
                )));
            }
            match crate::coercion::sanitize_numeric(a / b) {
                Ok(n) => Ok(LiteralValue::Number(n)),
                Err(e) => Ok(LiteralValue::Error(e)),
            }
        })
    }

    fn power(&self, left: LiteralValue, right: LiteralValue) -> Result<LiteralValue, ExcelError> {
        self.broadcast_apply(left, right, |l, r| {
            let ln = crate::coercion::to_number_lenient_with_locale(&l, &self.context.locale());
            let rn = crate::coercion::to_number_lenient_with_locale(&r, &self.context.locale());
            let (a, b) = match (ln, rn) {
                (Ok(a), Ok(b)) => (a, b),
                (Err(e), _) | (_, Err(e)) => return Ok(LiteralValue::Error(e)),
            };
            // Excel domain: negative base with non-integer exponent -> #NUM!,
            // 0^0 -> #NUM!, 0 to a negative power -> #DIV/0!
            if a < 0.0 && b.fract() != 0.0 {
                return Ok(LiteralValue::Error(ExcelError::new_num()));
            }
            if a == 0.0 && b == 0.0 {
                return Ok(LiteralValue::Error(ExcelError::new_num()));
            }
            if a == 0.0 && b < 0.0 {
                return Ok(LiteralValue::Error(ExcelError::new_div()));
            }
            match crate::coercion::sanitize_numeric(a.powf(b)) {
                Ok(n) => Ok(LiteralValue::Number(n)),
                Err(e) => Ok(LiteralValue::Error(e)),
            }
        })
    }

    fn map_array<F>(&self, arr: Vec<Vec<LiteralValue>>, f: F) -> Result<LiteralValue, ExcelError>
    where
        F: Fn(LiteralValue) -> Result<LiteralValue, ExcelError> + Copy,
    {
        let mut out = Vec::with_capacity(arr.len());
        for row in arr {
            let mut new_row = Vec::with_capacity(row.len());
            for cell in row {
                new_row.push(match f(cell) {
                    Ok(v) => v,
                    Err(e) => LiteralValue::Error(e),
                });
            }
            out.push(new_row);
        }
        Ok(LiteralValue::Array(out))
    }

    fn combine_arrays<F>(
        &self,
        l: Vec<Vec<LiteralValue>>,
        r: Vec<Vec<LiteralValue>>,
        f: F,
    ) -> Result<LiteralValue, ExcelError>
    where
        F: Fn(LiteralValue, LiteralValue) -> Result<LiteralValue, ExcelError> + Copy,
    {
        // Excel operator broadcasting: a dimension of 1 stretches; when both operands
        // are wider than 1 the result takes the larger extent and the cells the shorter
        // operand does not reach evaluate to #N/A (`{1;2;3}+{1;2}` → `{2;4;#N/A}`).
        let l_shape = (l.len(), l.first().map(|r| r.len()).unwrap_or(0));
        let r_shape = (r.len(), r.first().map(|r| r.len()).unwrap_or(0));
        let dim = |a: usize, b: usize| {
            if a == 1 {
                b
            } else if b == 1 {
                a
            } else {
                a.max(b)
            }
        };
        let target = (dim(l_shape.0, r_shape.0), dim(l_shape.1, r_shape.1));

        let mut out = Vec::with_capacity(target.0);
        for i in 0..target.0 {
            let mut row = Vec::with_capacity(target.1);
            for j in 0..target.1 {
                let (li, lj) = project_index((i, j), l_shape);
                let (ri, rj) = project_index((i, j), r_shape);
                let (lv, rv) = match (
                    l.get(li).and_then(|r| r.get(lj)),
                    r.get(ri).and_then(|r| r.get(rj)),
                ) {
                    (Some(lv), Some(rv)) => (lv.clone(), rv.clone()),
                    _ => {
                        row.push(LiteralValue::Error(ExcelError::new(ExcelErrorKind::Na)));
                        continue;
                    }
                };
                row.push(match f(lv, rv) {
                    Ok(v) => v,
                    Err(e) => LiteralValue::Error(e),
                });
            }
            out.push(row);
        }
        Ok(LiteralValue::Array(out))
    }

    fn broadcast_apply<F>(
        &self,
        left: LiteralValue,
        right: LiteralValue,
        f: F,
    ) -> Result<LiteralValue, ExcelError>
    where
        F: Fn(LiteralValue, LiteralValue) -> Result<LiteralValue, ExcelError> + Copy,
    {
        use LiteralValue::*;
        match (left, right) {
            (Array(l), Array(r)) => self.combine_arrays(l, r, f),
            (Array(arr), v) => {
                let shape_l = (arr.len(), arr.first().map(|r| r.len()).unwrap_or(0));
                let shape_r = (1usize, 1usize);
                let target = match broadcast_shape(&[shape_l, shape_r]) {
                    Ok(s) => s,
                    Err(e) => return Ok(LiteralValue::Error(e)),
                };
                let mut out = Vec::with_capacity(target.0);
                for i in 0..target.0 {
                    let mut row = Vec::with_capacity(target.1);
                    for j in 0..target.1 {
                        let (li, lj) = project_index((i, j), shape_l);
                        let lv = arr
                            .get(li)
                            .and_then(|r| r.get(lj))
                            .cloned()
                            .unwrap_or(LiteralValue::Empty);
                        row.push(match f(lv, v.clone()) {
                            Ok(vv) => vv,
                            Err(e) => LiteralValue::Error(e),
                        });
                    }
                    out.push(row);
                }
                Ok(LiteralValue::Array(out))
            }
            (v, Array(arr)) => {
                let shape_l = (1usize, 1usize);
                let shape_r = (arr.len(), arr.first().map(|r| r.len()).unwrap_or(0));
                let target = match broadcast_shape(&[shape_l, shape_r]) {
                    Ok(s) => s,
                    Err(e) => return Ok(LiteralValue::Error(e)),
                };
                let mut out = Vec::with_capacity(target.0);
                for i in 0..target.0 {
                    let mut row = Vec::with_capacity(target.1);
                    for j in 0..target.1 {
                        let (ri, rj) = project_index((i, j), shape_r);
                        let rv = arr
                            .get(ri)
                            .and_then(|r| r.get(rj))
                            .cloned()
                            .unwrap_or(LiteralValue::Empty);
                        row.push(match f(v.clone(), rv) {
                            Ok(vv) => vv,
                            Err(e) => LiteralValue::Error(e),
                        });
                    }
                    out.push(row);
                }
                Ok(LiteralValue::Array(out))
            }
            (l, r) => f(l, r),
        }
    }

    /* ---------- coercion helpers ---------- */
    fn coerce_number(&self, v: &LiteralValue) -> Result<f64, ExcelError> {
        coercion::to_number_lenient(v)
    }

    fn coerce_text(&self, v: &LiteralValue) -> String {
        coercion::to_text_invariant(v)
    }

    /* ---------- comparison ---------- */
    fn compare(
        &self,
        op: &str,
        left: LiteralValue,
        right: LiteralValue,
    ) -> Result<LiteralValue, ExcelError> {
        use LiteralValue::*;
        if matches!(left, Error(_)) {
            return Ok(left);
        }
        if matches!(right, Error(_)) {
            return Ok(right);
        }

        // arrays: element‑wise with broadcasting
        match (left, right) {
            (Array(l), Array(r)) => self.combine_arrays(l, r, |a, b| self.compare(op, a, b)),
            (Array(arr), v) => self.broadcast_apply(Array(arr), v, |a, b| self.compare(op, a, b)),
            (v, Array(arr)) => self.broadcast_apply(v, Array(arr), |a, b| self.compare(op, a, b)),
            (l, r) => {
                // Excel never coerces across types in a comparison: a blank
                // takes the counterpart's type (0 / "" / FALSE), same-type
                // values compare by value, and mixed types compare by rank
                // (numbers < text < booleans) so `="1"=1` and `=TRUE=1` are
                // FALSE while `=TRUE>"z"` and `="a">1` are TRUE.
                let (l, r) = match (l, r) {
                    (Empty, Empty) => (Int(0), Int(0)),
                    (Empty, r) => (Self::blank_as(&r), r),
                    (l, Empty) => {
                        let blank = Self::blank_as(&l);
                        (l, blank)
                    }
                    pair => pair,
                };
                let res = match (Self::compare_rank(&l), Self::compare_rank(&r)) {
                    (Some(1), Some(1)) => {
                        // Excel compares numbers at 15 significant digits, so
                        // `=0.1+0.2=0.3` is TRUE.
                        let a = crate::coercion::to_excel_precision(
                            crate::coercion::to_number_strict(&l)?,
                        );
                        let b = crate::coercion::to_excel_precision(
                            crate::coercion::to_number_strict(&r)?,
                        );
                        self.cmp_f64(a, b, op)
                    }
                    (Some(2), Some(2)) => match (&l, &r) {
                        (Text(a), Text(b)) => self.cmp_text(a, b, op),
                        _ => unreachable!(),
                    },
                    (Some(3), Some(3)) => match (&l, &r) {
                        (Boolean(a), Boolean(b)) => {
                            self.cmp_f64(if *a { 1.0 } else { 0.0 }, if *b { 1.0 } else { 0.0 }, op)
                        }
                        _ => unreachable!(),
                    },
                    (Some(ra), Some(rb)) => self.cmp_f64(ra as f64, rb as f64, op),
                    _ => {
                        // Pending / other placeholders: keep the lenient
                        // numeric-then-text fallback.
                        let an = crate::coercion::to_number_lenient_with_locale(
                            &l,
                            &self.context.locale(),
                        )
                        .ok();
                        let bn = crate::coercion::to_number_lenient_with_locale(
                            &r,
                            &self.context.locale(),
                        )
                        .ok();
                        if let (Some(a), Some(b)) = (an, bn) {
                            self.cmp_f64(a, b, op)
                        } else {
                            self.cmp_text(
                                &crate::coercion::to_text_invariant(&l),
                                &crate::coercion::to_text_invariant(&r),
                                op,
                            )
                        }
                    }
                };
                Ok(LiteralValue::Boolean(res))
            }
        }
    }

    /// Excel's comparison type rank: numbers (incl. date/time serials) 1,
    /// text 2, booleans 3. `None` for values that have no Excel rank.
    fn compare_rank(v: &LiteralValue) -> Option<u8> {
        use LiteralValue::*;
        match v {
            Int(_) | Number(_) | Date(_) | DateTime(_) | Time(_) | Duration(_) => Some(1),
            Text(_) => Some(2),
            Boolean(_) => Some(3),
            _ => None,
        }
    }

    /// The value a blank cell compares as against `counterpart` (Excel: 0
    /// against numbers, "" against text, FALSE against booleans).
    fn blank_as(counterpart: &LiteralValue) -> LiteralValue {
        use LiteralValue::*;
        match counterpart {
            Text(_) => Text(String::new()),
            Boolean(_) => Boolean(false),
            _ => Int(0),
        }
    }

    fn cmp_f64(&self, a: f64, b: f64, op: &str) -> bool {
        match op {
            "=" => a == b,
            "<>" => a != b,
            ">" => a > b,
            "<" => a < b,
            ">=" => a >= b,
            "<=" => a <= b,
            _ => unreachable!(),
        }
    }
    fn cmp_text(&self, a: &str, b: &str, op: &str) -> bool {
        let loc = self.context.locale();
        let (a, b) = (loc.fold_case_invariant(a), loc.fold_case_invariant(b));
        self.cmp_f64(
            a.cmp(&b) as i32 as f64,
            0.0,
            match op {
                "=" => "=",
                "<>" => "<>",
                ">" => ">",
                "<" => "<",
                ">=" => ">=",
                "<=" => "<=",
                _ => unreachable!(),
            },
        )
    }
}

fn relocate_reference_for_offset(
    reference: &ReferenceType,
    row_delta: i64,
    col_delta: i64,
) -> Result<ReferenceType, ExcelError> {
    match reference {
        ReferenceType::Cell {
            sheet,
            row,
            col,
            row_abs,
            col_abs,
        } => Ok(ReferenceType::Cell {
            sheet: sheet.clone(),
            row: shift_axis_for_offset(*row, row_delta, *row_abs)?,
            col: shift_axis_for_offset(*col, col_delta, *col_abs)?,
            row_abs: *row_abs,
            col_abs: *col_abs,
        }),
        ReferenceType::Range {
            sheet,
            start_row,
            start_col,
            end_row,
            end_col,
            start_row_abs,
            start_col_abs,
            end_row_abs,
            end_col_abs,
        } => Ok(ReferenceType::Range {
            sheet: sheet.clone(),
            start_row: shift_optional_axis_for_offset(*start_row, row_delta, *start_row_abs)?,
            start_col: shift_optional_axis_for_offset(*start_col, col_delta, *start_col_abs)?,
            end_row: shift_optional_axis_for_offset(*end_row, row_delta, *end_row_abs)?,
            end_col: shift_optional_axis_for_offset(*end_col, col_delta, *end_col_abs)?,
            start_row_abs: *start_row_abs,
            start_col_abs: *start_col_abs,
            end_row_abs: *end_row_abs,
            end_col_abs: *end_col_abs,
        }),
        // Defined names are placement-invariant: a relocated copy of the
        // formula references the same name, resolved at evaluation time.
        ReferenceType::NamedRange(name) => Ok(ReferenceType::NamedRange(name.clone())),
        // 3-D references relocate exactly like their 2-D counterparts: the
        // sheet span is fixed, the rectangle follows the copy offset.
        ReferenceType::Cell3D {
            sheet_first,
            sheet_last,
            row,
            col,
            row_abs,
            col_abs,
        } => Ok(ReferenceType::Cell3D {
            sheet_first: sheet_first.clone(),
            sheet_last: sheet_last.clone(),
            row: shift_axis_for_offset(*row, row_delta, *row_abs)?,
            col: shift_axis_for_offset(*col, col_delta, *col_abs)?,
            row_abs: *row_abs,
            col_abs: *col_abs,
        }),
        ReferenceType::Range3D {
            sheet_first,
            sheet_last,
            start_row,
            start_col,
            end_row,
            end_col,
            start_row_abs,
            start_col_abs,
            end_row_abs,
            end_col_abs,
        } => Ok(ReferenceType::Range3D {
            sheet_first: sheet_first.clone(),
            sheet_last: sheet_last.clone(),
            start_row: shift_optional_axis_for_offset(*start_row, row_delta, *start_row_abs)?,
            start_col: shift_optional_axis_for_offset(*start_col, col_delta, *start_col_abs)?,
            end_row: shift_optional_axis_for_offset(*end_row, row_delta, *end_row_abs)?,
            end_col: shift_optional_axis_for_offset(*end_col, col_delta, *end_col_abs)?,
            start_row_abs: *start_row_abs,
            start_col_abs: *start_col_abs,
            end_row_abs: *end_row_abs,
            end_col_abs: *end_col_abs,
        }),
        ReferenceType::Table(_) | ReferenceType::External(_) => {
            Err(unsupported_reference_relocation_error())
        }
    }
}

fn shift_optional_axis_for_offset(
    value: Option<u32>,
    delta: i64,
    is_absolute: bool,
) -> Result<Option<u32>, ExcelError> {
    value
        .map(|value| shift_axis_for_offset(value, delta, is_absolute))
        .transpose()
}

fn shift_axis_for_offset(value: u32, delta: i64, is_absolute: bool) -> Result<u32, ExcelError> {
    if is_absolute {
        return Ok(value);
    }
    let shifted = i64::from(value) + delta;
    if shifted < 1 || shifted > i64::from(u32::MAX) {
        return Err(unsupported_reference_relocation_error());
    }
    Ok(shifted as u32)
}

fn unsupported_reference_relocation_error() -> ExcelError {
    ExcelError::new(ExcelErrorKind::Ref)
        .with_message("Unsupported reference relocation for FormulaPlane span evaluation")
}
