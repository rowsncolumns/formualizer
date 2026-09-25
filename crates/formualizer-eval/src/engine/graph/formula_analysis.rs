use super::*;
use formualizer_parse::parser::{ASTNode, ASTNodeType, ReferenceType};

// Type alias for complex return types (local to analysis).
type ExtractDependenciesResult = Result<
    (
        Vec<VertexId>,
        Vec<SharedRangeRef<'static>>,
        Vec<CellRef>,
        Vec<VertexId>,
    ),
    ExcelError,
>;

type ExtractDependenciesWithPendingNamesResult = Result<
    (
        Vec<VertexId>,
        Vec<SharedRangeRef<'static>>,
        Vec<CellRef>,
        Vec<VertexId>,
        Vec<String>,
    ),
    ExcelError,
>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnresolvedNamePolicy {
    Error,
    Collect,
}

impl DependencyGraph {
    // Helper methods for formula analysis / dependency extraction.

    pub(super) fn extract_dependencies(
        &mut self,
        ast: &ASTNode,
        current_sheet_id: SheetId,
    ) -> ExtractDependenciesResult {
        let (dependencies, ranges, placeholders, named_dependencies, _pending_names) =
            self.extract_dependencies_inner(ast, current_sheet_id, UnresolvedNamePolicy::Error)?;
        Ok((dependencies, ranges, placeholders, named_dependencies))
    }

    pub(super) fn extract_dependencies_with_pending_names(
        &mut self,
        ast: &ASTNode,
        current_sheet_id: SheetId,
    ) -> ExtractDependenciesWithPendingNamesResult {
        self.extract_dependencies_inner(ast, current_sheet_id, UnresolvedNamePolicy::Collect)
    }

    pub(super) fn extract_dependencies_arena(
        &mut self,
        ast_id: AstNodeId,
        current_sheet_id: SheetId,
    ) -> ExtractDependenciesResult {
        let (dependencies, ranges, placeholders, named_dependencies, _pending_names) = self
            .extract_dependencies_inner_arena(
                ast_id,
                current_sheet_id,
                UnresolvedNamePolicy::Error,
            )?;
        Ok((dependencies, ranges, placeholders, named_dependencies))
    }

    pub(super) fn extract_dependencies_with_pending_names_arena(
        &mut self,
        ast_id: AstNodeId,
        current_sheet_id: SheetId,
    ) -> ExtractDependenciesWithPendingNamesResult {
        self.extract_dependencies_inner_arena(
            ast_id,
            current_sheet_id,
            UnresolvedNamePolicy::Collect,
        )
    }

    fn extract_dependencies_inner_arena(
        &mut self,
        ast_id: AstNodeId,
        current_sheet_id: SheetId,
        unresolved_name_policy: UnresolvedNamePolicy,
    ) -> ExtractDependenciesWithPendingNamesResult {
        let mut dependencies = FxHashSet::default();
        let mut range_dependencies: Vec<SharedRangeRef<'static>> = Vec::new();
        let mut created_placeholders = Vec::new();
        let mut named_dependencies = Vec::new();
        let mut unresolved_names = FxHashSet::default();
        let mut local_scopes: Vec<FxHashSet<String>> = Vec::new();
        self.extract_dependencies_recursive_arena(
            ast_id,
            current_sheet_id,
            &mut dependencies,
            &mut range_dependencies,
            &mut created_placeholders,
            &mut named_dependencies,
            &mut unresolved_names,
            &mut local_scopes,
            unresolved_name_policy,
        )?;

        // Deduplicate range references.
        let mut deduped_ranges = Vec::new();
        for range_ref in range_dependencies {
            if !deduped_ranges.contains(&range_ref) {
                deduped_ranges.push(range_ref);
            }
        }

        named_dependencies.sort_unstable_by_key(|v| v.0);
        named_dependencies.dedup_by_key(|v| v.0);

        let mut unresolved_names: Vec<String> = unresolved_names.into_iter().collect();
        unresolved_names.sort();

        Ok((
            dependencies.into_iter().collect(),
            deduped_ranges,
            created_placeholders,
            named_dependencies,
            unresolved_names,
        ))
    }

    fn extract_dependencies_recursive_arena(
        &mut self,
        ast_id: AstNodeId,
        current_sheet_id: SheetId,
        dependencies: &mut FxHashSet<VertexId>,
        range_dependencies: &mut Vec<SharedRangeRef<'static>>,
        created_placeholders: &mut Vec<CellRef>,
        named_dependencies: &mut Vec<VertexId>,
        unresolved_names: &mut FxHashSet<String>,
        local_scopes: &mut Vec<FxHashSet<String>>,
        unresolved_name_policy: UnresolvedNamePolicy,
    ) -> Result<(), ExcelError> {
        let Some(node) = self.data_store.get_node(ast_id).cloned() else {
            return Err(
                ExcelError::new(ExcelErrorKind::Value).with_message("Missing interned formula AST")
            );
        };

        match node {
            super::super::arena::ast::AstNodeData::Reference { ref_type, .. } => {
                let reference = self
                    .data_store
                    .reconstruct_reference_type_for_eval(&ref_type, &self.sheet_reg);
                match &reference {
                    ReferenceType::External(ext) => match ext.kind {
                        formualizer_parse::parser::ExternalRefKind::Cell { .. } => {
                            let name = ext.raw.as_str();
                            if let Some(source) = self.resolve_source_scalar_entry(name) {
                                dependencies.insert(source.vertex);
                            } else {
                                return Err(ExcelError::new(ExcelErrorKind::Name)
                                    .with_message(format!("Undefined name: {name}")));
                            }
                        }
                        formualizer_parse::parser::ExternalRefKind::Range { .. } => {
                            let name = ext.raw.as_str();
                            if let Some(source) = self.resolve_source_table_entry(name) {
                                dependencies.insert(source.vertex);
                            } else {
                                return Err(ExcelError::new(ExcelErrorKind::Name)
                                    .with_message(format!("Undefined table: {name}")));
                            }
                        }
                    },
                    ReferenceType::Cell { .. } => {
                        let vertex_id = self.get_or_create_vertex_for_reference(
                            &reference,
                            current_sheet_id,
                            created_placeholders,
                        )?;
                        dependencies.insert(vertex_id);
                    }
                    ReferenceType::Range {
                        sheet,
                        start_row,
                        start_col,
                        end_row,
                        end_col,
                        ..
                    } => {
                        // If any bound is missing (infinite/partial range), always keep compressed.
                        let has_unbounded = start_row.is_none()
                            || end_row.is_none()
                            || start_col.is_none()
                            || end_col.is_none();
                        if has_unbounded {
                            if let Some(SharedRef::Range(range)) = reference.to_sheet_ref_lossy() {
                                let owned = range.into_owned();
                                let sheet_id = match owned.sheet {
                                    SharedSheetLocator::Id(id) => id,
                                    SharedSheetLocator::Current => current_sheet_id,
                                    SharedSheetLocator::Name(name) => {
                                        self.resolve_existing_sheet_id(name.as_ref())?
                                    }
                                };
                                range_dependencies.push(SharedRangeRef {
                                    sheet: SharedSheetLocator::Id(sheet_id),
                                    start_row: owned.start_row,
                                    start_col: owned.start_col,
                                    end_row: owned.end_row,
                                    end_col: owned.end_col,
                                });
                            }
                        } else {
                            let (Some(sr), Some(sc), Some(er), Some(ec)) =
                                (*start_row, *start_col, *end_row, *end_col)
                            else {
                                return Err(ExcelError::new(ExcelErrorKind::Ref));
                            };

                            if sr > er || sc > ec {
                                return Err(ExcelError::new(ExcelErrorKind::Ref));
                            }

                            let height = er.saturating_sub(sr) + 1;
                            let width = ec.saturating_sub(sc) + 1;
                            let size = (width * height) as usize;

                            if size <= self.config.range_expansion_limit {
                                // Expand to individual cells.
                                let sheet_id = match sheet {
                                    Some(name) => self.resolve_existing_sheet_id(name)?,
                                    None => current_sheet_id,
                                };
                                for row in sr..=er {
                                    for col in sc..=ec {
                                        let coord = Coord::from_excel(row, col, true, true);
                                        let addr = CellRef::new(sheet_id, coord);
                                        let vertex_id =
                                            self.get_or_create_vertex(&addr, created_placeholders);
                                        dependencies.insert(vertex_id);
                                    }
                                }
                            } else {
                                // Keep as a compressed range dependency.
                                if let Some(SharedRef::Range(range)) =
                                    reference.to_sheet_ref_lossy()
                                {
                                    let owned = range.into_owned();
                                    let sheet_id = match owned.sheet {
                                        SharedSheetLocator::Id(id) => id,
                                        SharedSheetLocator::Current => current_sheet_id,
                                        SharedSheetLocator::Name(name) => {
                                            self.resolve_existing_sheet_id(name.as_ref())?
                                        }
                                    };
                                    range_dependencies.push(SharedRangeRef {
                                        sheet: SharedSheetLocator::Id(sheet_id),
                                        start_row: owned.start_row,
                                        start_col: owned.start_col,
                                        end_row: owned.end_row,
                                        end_col: owned.end_col,
                                    });
                                }
                            }
                        }
                    }
                    ReferenceType::NamedRange(name) => {
                        let key = name.to_ascii_uppercase();
                        if local_scopes.iter().rev().any(|scope| scope.contains(&key)) {
                            return Ok(());
                        }

                        if let Some(named_range) = self.resolve_name_entry(name, current_sheet_id) {
                            dependencies.insert(named_range.vertex);
                            named_dependencies.push(named_range.vertex);
                        } else if let Some(table) = self.resolve_table_entry(name) {
                            // A bare table name (`=ROWS(Table1)`) is the table's data body.
                            dependencies.insert(table.vertex);
                        } else if let Some(source) = self.resolve_source_scalar_entry(name) {
                            dependencies.insert(source.vertex);
                        } else {
                            match unresolved_name_policy {
                                UnresolvedNamePolicy::Error => {
                                    return Err(ExcelError::new(ExcelErrorKind::Name)
                                        .with_message(format!("Undefined name: {name}")));
                                }
                                UnresolvedNamePolicy::Collect => {
                                    unresolved_names.insert(name.to_string());
                                }
                            }
                        }
                    }
                    ReferenceType::Table(tref) => {
                        if let Some(table) = self.resolve_table_entry(&tref.name) {
                            dependencies.insert(table.vertex);
                        } else if let Some(source) = self.resolve_source_table_entry(&tref.name) {
                            dependencies.insert(source.vertex);
                        } else {
                            return Err(ExcelError::new(ExcelErrorKind::Name)
                                .with_message(format!("Undefined table: {}", tref.name)));
                        }
                    }
                    // A 3-D reference depends on the same rectangle on every
                    // sheet between its endpoints (tab order, inclusive).
                    ReferenceType::Cell3D {
                        sheet_first,
                        sheet_last,
                        row,
                        col,
                        ..
                    } => {
                        self.push_three_d_dependencies(
                            sheet_first,
                            sheet_last,
                            (Some(*row), Some(*col), Some(*row), Some(*col)),
                            dependencies,
                            range_dependencies,
                            created_placeholders,
                        )?;
                    }
                    ReferenceType::Range3D {
                        sheet_first,
                        sheet_last,
                        start_row,
                        start_col,
                        end_row,
                        end_col,
                        ..
                    } => {
                        self.push_three_d_dependencies(
                            sheet_first,
                            sheet_last,
                            (*start_row, *start_col, *end_row, *end_col),
                            dependencies,
                            range_dependencies,
                            created_placeholders,
                        )?;
                    }
                }
            }
            super::super::arena::ast::AstNodeData::BinaryOp {
                left_id, right_id, ..
            } => {
                self.extract_dependencies_recursive_arena(
                    left_id,
                    current_sheet_id,
                    dependencies,
                    range_dependencies,
                    created_placeholders,
                    named_dependencies,
                    unresolved_names,
                    local_scopes,
                    unresolved_name_policy,
                )?;
                self.extract_dependencies_recursive_arena(
                    right_id,
                    current_sheet_id,
                    dependencies,
                    range_dependencies,
                    created_placeholders,
                    named_dependencies,
                    unresolved_names,
                    local_scopes,
                    unresolved_name_policy,
                )?;
            }
            super::super::arena::ast::AstNodeData::UnaryOp { expr_id, .. } => {
                self.extract_dependencies_recursive_arena(
                    expr_id,
                    current_sheet_id,
                    dependencies,
                    range_dependencies,
                    created_placeholders,
                    named_dependencies,
                    unresolved_names,
                    local_scopes,
                    unresolved_name_policy,
                )?;
            }
            super::super::arena::ast::AstNodeData::Function { name_id, .. } => {
                let name = self.data_store.resolve_ast_string(name_id).to_string();
                let args: Vec<AstNodeId> = self
                    .data_store
                    .get_args(ast_id)
                    .map_or_else(Vec::new, |args| args.to_vec());
                // See the AST walker: a named-lambda call depends on the name; LAMBDA / LET
                // parameters are local names, never dependencies (spreadsheet#939 G-07).
                self.push_named_callable_dependency(
                    &name,
                    current_sheet_id,
                    dependencies,
                    named_dependencies,
                    local_scopes,
                );
                if let Some((names_at, body_at)) = local_binding_layout(&name, args.len()) {
                    let mut scope = FxHashSet::default();
                    for &i in &names_at {
                        if let Some(n) = self.arena_local_binding_name(args[i]) {
                            scope.insert(n.to_ascii_uppercase());
                        }
                    }
                    local_scopes.push(scope);
                    let mut result = Ok(());
                    for (i, arg) in args.iter().enumerate() {
                        if names_at.contains(&i) || !body_at.contains(&i) {
                            continue;
                        }
                        result = self.extract_dependencies_recursive_arena(
                            *arg,
                            current_sheet_id,
                            dependencies,
                            range_dependencies,
                            created_placeholders,
                            named_dependencies,
                            unresolved_names,
                            local_scopes,
                            unresolved_name_policy,
                        );
                        if result.is_err() {
                            break;
                        }
                    }
                    local_scopes.pop();
                    result?;
                } else {
                    for arg in args {
                        self.extract_dependencies_recursive_arena(
                            arg,
                            current_sheet_id,
                            dependencies,
                            range_dependencies,
                            created_placeholders,
                            named_dependencies,
                            unresolved_names,
                            local_scopes,
                            unresolved_name_policy,
                        )?;
                    }
                }
            }
            super::super::arena::ast::AstNodeData::Array { .. } => {
                let elements: Vec<AstNodeId> = self
                    .data_store
                    .get_array_elems(ast_id)
                    .map_or_else(Vec::new, |(_, _, elems)| elems.to_vec());
                for cell in elements {
                    self.extract_dependencies_recursive_arena(
                        cell,
                        current_sheet_id,
                        dependencies,
                        range_dependencies,
                        created_placeholders,
                        named_dependencies,
                        unresolved_names,
                        local_scopes,
                        unresolved_name_policy,
                    )?;
                }
            }
            super::super::arena::ast::AstNodeData::Literal(_) => {}
        }
        Ok(())
    }

    fn arena_named_range_name(&self, ast_id: AstNodeId) -> Option<String> {
        match self.data_store.get_node(ast_id)? {
            super::super::arena::ast::AstNodeData::Reference {
                ref_type: super::super::arena::ast::CompactRefType::NamedRange(name_id),
                ..
            } => Some(self.data_store.resolve_ast_string(*name_id).to_string()),
            _ => None,
        }
    }

    /// A `LAMBDA` parameter / `LET` name in arena form: a bare name (`x`) or Excel's optional
    /// spelling `[y]`, which the parser reads as a bare table reference.
    fn arena_local_binding_name(&self, ast_id: AstNodeId) -> Option<String> {
        use super::super::arena::ast::{AstNodeData, CompactRefType};
        match self.data_store.get_node(ast_id)? {
            AstNodeData::Reference {
                ref_type: CompactRefType::NamedRange(name_id),
                ..
            }
            | AstNodeData::Reference {
                ref_type:
                    CompactRefType::Table {
                        name_id,
                        specifier_id: None,
                    },
                ..
            } => Some(self.data_store.resolve_ast_string(*name_id).to_string()),
            _ => None,
        }
    }

    /// `=Dbl(21)` where `Dbl` is a defined name: record the name as a dependency of the formula so
    /// redefining the name recalculates its callers. Builtins are unaffected (a defined name that
    /// happens to spell a builtin adds a harmless edge); a LET/LAMBDA-local `f(3)` is skipped.
    fn push_named_callable_dependency(
        &self,
        name: &str,
        current_sheet_id: SheetId,
        dependencies: &mut FxHashSet<VertexId>,
        named_dependencies: &mut Vec<VertexId>,
        local_scopes: &[FxHashSet<String>],
    ) {
        let key = name.to_ascii_uppercase();
        if local_scopes.iter().rev().any(|scope| scope.contains(&key)) {
            return;
        }
        if let Some(named_range) = self.resolve_name_entry(name, current_sheet_id) {
            dependencies.insert(named_range.vertex);
            named_dependencies.push(named_range.vertex);
        }
    }

    /// Dependencies of a 3-D reference `first:last!<rect>`: the rectangle on
    /// every sheet the span covers, each handled like the plain `Range` arm
    /// (expanded to cells when small enough, kept compressed otherwise).
    /// An endpoint that is not a sheet is `#REF!`, as for a bad sheet name.
    fn push_three_d_dependencies(
        &mut self,
        sheet_first: &str,
        sheet_last: &str,
        bounds: (Option<u32>, Option<u32>, Option<u32>, Option<u32>),
        dependencies: &mut FxHashSet<VertexId>,
        range_dependencies: &mut Vec<SharedRangeRef<'static>>,
        created_placeholders: &mut Vec<CellRef>,
    ) -> Result<(), ExcelError> {
        let span = self
            .sheet_reg()
            .active_span_ids(sheet_first, sheet_last)
            .ok_or_else(|| {
                ExcelError::new(ExcelErrorKind::Ref)
                    .with_message(format!("Sheet not found: {sheet_first}:{sheet_last}"))
            })?;
        let (start_row, start_col, end_row, end_col) = bounds;
        for sheet_id in span {
            match (start_row, start_col, end_row, end_col) {
                (Some(sr), Some(sc), Some(er), Some(ec)) => {
                    if sr > er || sc > ec {
                        return Err(ExcelError::new(ExcelErrorKind::Ref));
                    }
                    let size = ((ec - sc + 1) * (er - sr + 1)) as usize;
                    if size <= self.config.range_expansion_limit {
                        for row in sr..=er {
                            for col in sc..=ec {
                                let coord = Coord::from_excel(row, col, true, true);
                                let addr = CellRef::new(sheet_id, coord);
                                let vertex_id =
                                    self.get_or_create_vertex(&addr, created_placeholders);
                                dependencies.insert(vertex_id);
                            }
                        }
                        continue;
                    }
                }
                _ => {}
            }
            let reference = ReferenceType::Range {
                sheet: Some(self.sheet_name(sheet_id).to_string()),
                start_row,
                start_col,
                end_row,
                end_col,
                start_row_abs: true,
                start_col_abs: true,
                end_row_abs: true,
                end_col_abs: true,
            };
            if let Some(SharedRef::Range(range)) = reference.to_sheet_ref_lossy() {
                let owned = range.into_owned();
                range_dependencies.push(SharedRangeRef {
                    sheet: SharedSheetLocator::Id(sheet_id),
                    start_row: owned.start_row,
                    start_col: owned.start_col,
                    end_row: owned.end_row,
                    end_col: owned.end_col,
                });
            }
        }
        Ok(())
    }

    fn extract_dependencies_inner(
        &mut self,
        ast: &ASTNode,
        current_sheet_id: SheetId,
        unresolved_name_policy: UnresolvedNamePolicy,
    ) -> ExtractDependenciesWithPendingNamesResult {
        let mut dependencies = FxHashSet::default();
        let mut range_dependencies: Vec<SharedRangeRef<'static>> = Vec::new();
        let mut created_placeholders = Vec::new();
        let mut named_dependencies = Vec::new();
        let mut unresolved_names = FxHashSet::default();
        let mut local_scopes: Vec<FxHashSet<String>> = Vec::new();
        self.extract_dependencies_recursive(
            ast,
            current_sheet_id,
            &mut dependencies,
            &mut range_dependencies,
            &mut created_placeholders,
            &mut named_dependencies,
            &mut unresolved_names,
            &mut local_scopes,
            unresolved_name_policy,
        )?;

        // Deduplicate range references.
        let mut deduped_ranges = Vec::new();
        for range_ref in range_dependencies {
            if !deduped_ranges.contains(&range_ref) {
                deduped_ranges.push(range_ref);
            }
        }

        named_dependencies.sort_unstable_by_key(|v| v.0);
        named_dependencies.dedup_by_key(|v| v.0);

        let mut unresolved_names: Vec<String> = unresolved_names.into_iter().collect();
        unresolved_names.sort();

        Ok((
            dependencies.into_iter().collect(),
            deduped_ranges,
            created_placeholders,
            named_dependencies,
            unresolved_names,
        ))
    }

    fn extract_dependencies_recursive(
        &mut self,
        ast: &ASTNode,
        current_sheet_id: SheetId,
        dependencies: &mut FxHashSet<VertexId>,
        range_dependencies: &mut Vec<SharedRangeRef<'static>>,
        created_placeholders: &mut Vec<CellRef>,
        named_dependencies: &mut Vec<VertexId>,
        unresolved_names: &mut FxHashSet<String>,
        local_scopes: &mut Vec<FxHashSet<String>>,
        unresolved_name_policy: UnresolvedNamePolicy,
    ) -> Result<(), ExcelError> {
        match &ast.node_type {
            ASTNodeType::Reference { reference, .. } => match reference {
                ReferenceType::External(ext) => match ext.kind {
                    formualizer_parse::parser::ExternalRefKind::Cell { .. } => {
                        let name = ext.raw.as_str();
                        if let Some(source) = self.resolve_source_scalar_entry(name) {
                            dependencies.insert(source.vertex);
                        } else {
                            return Err(ExcelError::new(ExcelErrorKind::Name)
                                .with_message(format!("Undefined name: {name}")));
                        }
                    }
                    formualizer_parse::parser::ExternalRefKind::Range { .. } => {
                        let name = ext.raw.as_str();
                        if let Some(source) = self.resolve_source_table_entry(name) {
                            dependencies.insert(source.vertex);
                        } else {
                            return Err(ExcelError::new(ExcelErrorKind::Name)
                                .with_message(format!("Undefined table: {name}")));
                        }
                    }
                },
                ReferenceType::Cell { .. } => {
                    let vertex_id = self.get_or_create_vertex_for_reference(
                        reference,
                        current_sheet_id,
                        created_placeholders,
                    )?;
                    dependencies.insert(vertex_id);
                }
                ReferenceType::Range {
                    sheet,
                    start_row,
                    start_col,
                    end_row,
                    end_col,
                    ..
                } => {
                    // If any bound is missing (infinite/partial range), always keep compressed.
                    let has_unbounded = start_row.is_none()
                        || end_row.is_none()
                        || start_col.is_none()
                        || end_col.is_none();
                    if has_unbounded {
                        if let Some(SharedRef::Range(range)) = reference.to_sheet_ref_lossy() {
                            let owned = range.into_owned();
                            let sheet_id = match owned.sheet {
                                SharedSheetLocator::Id(id) => id,
                                SharedSheetLocator::Current => current_sheet_id,
                                SharedSheetLocator::Name(name) => {
                                    self.resolve_existing_sheet_id(name.as_ref())?
                                }
                            };
                            range_dependencies.push(SharedRangeRef {
                                sheet: SharedSheetLocator::Id(sheet_id),
                                start_row: owned.start_row,
                                start_col: owned.start_col,
                                end_row: owned.end_row,
                                end_col: owned.end_col,
                            });
                        }
                    } else {
                        let (Some(sr), Some(sc), Some(er), Some(ec)) =
                            (*start_row, *start_col, *end_row, *end_col)
                        else {
                            return Err(ExcelError::new(ExcelErrorKind::Ref));
                        };

                        if sr > er || sc > ec {
                            return Err(ExcelError::new(ExcelErrorKind::Ref));
                        }

                        let height = er.saturating_sub(sr) + 1;
                        let width = ec.saturating_sub(sc) + 1;
                        let size = (width * height) as usize;

                        if size <= self.config.range_expansion_limit {
                            // Expand to individual cells.
                            let sheet_id = match sheet {
                                Some(name) => self.resolve_existing_sheet_id(name)?,
                                None => current_sheet_id,
                            };
                            for row in sr..=er {
                                for col in sc..=ec {
                                    let coord = Coord::from_excel(row, col, true, true);
                                    let addr = CellRef::new(sheet_id, coord);
                                    let vertex_id =
                                        self.get_or_create_vertex(&addr, created_placeholders);
                                    dependencies.insert(vertex_id);
                                }
                            }
                        } else {
                            // Keep as a compressed range dependency.
                            if let Some(SharedRef::Range(range)) = reference.to_sheet_ref_lossy() {
                                let owned = range.into_owned();
                                let sheet_id = match owned.sheet {
                                    SharedSheetLocator::Id(id) => id,
                                    SharedSheetLocator::Current => current_sheet_id,
                                    SharedSheetLocator::Name(name) => {
                                        self.resolve_existing_sheet_id(name.as_ref())?
                                    }
                                };
                                range_dependencies.push(SharedRangeRef {
                                    sheet: SharedSheetLocator::Id(sheet_id),
                                    start_row: owned.start_row,
                                    start_col: owned.start_col,
                                    end_row: owned.end_row,
                                    end_col: owned.end_col,
                                });
                            }
                        }
                    }
                }
                ReferenceType::NamedRange(name) => {
                    let key = name.to_ascii_uppercase();
                    if local_scopes.iter().rev().any(|scope| scope.contains(&key)) {
                        return Ok(());
                    }

                    if let Some(named_range) = self.resolve_name_entry(name, current_sheet_id) {
                        dependencies.insert(named_range.vertex);
                        named_dependencies.push(named_range.vertex);
                    } else if let Some(table) = self.resolve_table_entry(name) {
                        // A bare table name (`=ROWS(Table1)`) is the table's data body.
                        dependencies.insert(table.vertex);
                    } else if let Some(source) = self.resolve_source_scalar_entry(name) {
                        dependencies.insert(source.vertex);
                    } else {
                        match unresolved_name_policy {
                            UnresolvedNamePolicy::Error => {
                                return Err(ExcelError::new(ExcelErrorKind::Name)
                                    .with_message(format!("Undefined name: {name}")));
                            }
                            UnresolvedNamePolicy::Collect => {
                                unresolved_names.insert(name.to_string());
                            }
                        }
                    }
                }
                ReferenceType::Table(tref) => {
                    if let Some(table) = self.resolve_table_entry(&tref.name) {
                        dependencies.insert(table.vertex);
                    } else if let Some(source) = self.resolve_source_table_entry(&tref.name) {
                        dependencies.insert(source.vertex);
                    } else {
                        return Err(ExcelError::new(ExcelErrorKind::Name)
                            .with_message(format!("Undefined table: {}", tref.name)));
                    }
                }
                // A 3-D reference depends on the same rectangle on every
                // sheet between its endpoints (tab order, inclusive).
                ReferenceType::Cell3D {
                    sheet_first,
                    sheet_last,
                    row,
                    col,
                    ..
                } => {
                    self.push_three_d_dependencies(
                        sheet_first,
                        sheet_last,
                        (Some(*row), Some(*col), Some(*row), Some(*col)),
                        dependencies,
                        range_dependencies,
                        created_placeholders,
                    )?;
                }
                ReferenceType::Range3D {
                    sheet_first,
                    sheet_last,
                    start_row,
                    start_col,
                    end_row,
                    end_col,
                    ..
                } => {
                    self.push_three_d_dependencies(
                        sheet_first,
                        sheet_last,
                        (*start_row, *start_col, *end_row, *end_col),
                        dependencies,
                        range_dependencies,
                        created_placeholders,
                    )?;
                }
            },
            ASTNodeType::BinaryOp { left, right, .. } => {
                self.extract_dependencies_recursive(
                    left,
                    current_sheet_id,
                    dependencies,
                    range_dependencies,
                    created_placeholders,
                    named_dependencies,
                    unresolved_names,
                    local_scopes,
                    unresolved_name_policy,
                )?;
                self.extract_dependencies_recursive(
                    right,
                    current_sheet_id,
                    dependencies,
                    range_dependencies,
                    created_placeholders,
                    named_dependencies,
                    unresolved_names,
                    local_scopes,
                    unresolved_name_policy,
                )?;
            }
            ASTNodeType::UnaryOp { expr, .. } => {
                self.extract_dependencies_recursive(
                    expr,
                    current_sheet_id,
                    dependencies,
                    range_dependencies,
                    created_placeholders,
                    named_dependencies,
                    unresolved_names,
                    local_scopes,
                    unresolved_name_policy,
                )?;
            }
            ASTNodeType::Function { name, args } => {
                // A call to a lambda-valued defined name (`=Dbl(21)`) depends on that name, so
                // redefining it recalculates the callers (spreadsheet#939 G-07).
                self.push_named_callable_dependency(
                    name,
                    current_sheet_id,
                    dependencies,
                    named_dependencies,
                    local_scopes,
                );
                // `LAMBDA(x,[y],body)` / `LET(n1,v1,…,body)` bind LOCAL names: they are not
                // workbook names (an undefined one must not fail the formula) and never
                // dependencies. Only the body and the LET values are walked.
                let local = local_binding_layout(name, args.len());
                if let Some((names_at, body_at)) = local {
                    let mut scope = FxHashSet::default();
                    for &i in &names_at {
                        if let Some(n) = local_binding_name(&args[i]) {
                            scope.insert(n.to_ascii_uppercase());
                        }
                    }
                    local_scopes.push(scope);
                    let walked = args
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| !names_at.contains(i))
                        .filter(|(i, _)| body_at.contains(i));
                    let mut result = Ok(());
                    for (_, arg) in walked {
                        result = self.extract_dependencies_recursive(
                            arg,
                            current_sheet_id,
                            dependencies,
                            range_dependencies,
                            created_placeholders,
                            named_dependencies,
                            unresolved_names,
                            local_scopes,
                            unresolved_name_policy,
                        );
                        if result.is_err() {
                            break;
                        }
                    }
                    local_scopes.pop();
                    result?;
                } else {
                    for arg in args {
                        self.extract_dependencies_recursive(
                            arg,
                            current_sheet_id,
                            dependencies,
                            range_dependencies,
                            created_placeholders,
                            named_dependencies,
                            unresolved_names,
                            local_scopes,
                            unresolved_name_policy,
                        )?;
                    }
                }
            }
            ASTNodeType::Call { callee, args } => {
                // Walk both the callee and the call arguments so any references
                // they contain are tracked. Full evaluator semantics for
                // immediate-invocation calls are not yet implemented, but
                // dependency collection must still cover them.
                self.extract_dependencies_recursive(
                    callee,
                    current_sheet_id,
                    dependencies,
                    range_dependencies,
                    created_placeholders,
                    named_dependencies,
                    unresolved_names,
                    local_scopes,
                    unresolved_name_policy,
                )?;
                for arg in args {
                    self.extract_dependencies_recursive(
                        arg,
                        current_sheet_id,
                        dependencies,
                        range_dependencies,
                        created_placeholders,
                        named_dependencies,
                        unresolved_names,
                        local_scopes,
                        unresolved_name_policy,
                    )?;
                }
            }
            ASTNodeType::Array(rows) => {
                for item in rows.iter().flatten() {
                    self.extract_dependencies_recursive(
                        item,
                        current_sheet_id,
                        dependencies,
                        range_dependencies,
                        created_placeholders,
                        named_dependencies,
                        unresolved_names,
                        local_scopes,
                        unresolved_name_policy,
                    )?;
                }
            }
            ASTNodeType::Literal(_) => {}
        }
        Ok(())
    }

    /// Gets the VertexId for a reference, creating a placeholder vertex if it doesn't exist.
    fn get_or_create_vertex_for_reference(
        &mut self,
        reference: &ReferenceType,
        current_sheet_id: SheetId,
        created_placeholders: &mut Vec<CellRef>,
    ) -> Result<VertexId, ExcelError> {
        match reference {
            ReferenceType::Cell {
                sheet, row, col, ..
            } => {
                let sheet_id = match sheet {
                    Some(name) => self.resolve_existing_sheet_id(name)?,
                    None => current_sheet_id,
                };
                let coord = Coord::from_excel(*row, *col, true, true);
                let addr = CellRef::new(sheet_id, coord);
                Ok(self.get_or_create_vertex(&addr, created_placeholders))
            }
            _ => Err(ExcelError::new(ExcelErrorKind::Value)
                .with_message("Expected a cell reference, but got a range or other type.")),
        }
    }

    #[inline]
    pub(super) fn is_ast_volatile(&self, ast: &ASTNode) -> bool {
        if ast.contains_volatile() {
            return true;
        }

        use formualizer_parse::parser::ASTNodeType;

        match &ast.node_type {
            ASTNodeType::Function { name, args } => {
                if let Some(func) = crate::function_registry::get("", name)
                    && func.caps().contains(crate::function::FnCaps::VOLATILE)
                {
                    return true;
                }
                args.iter().any(|arg| self.is_ast_volatile(arg))
            }
            ASTNodeType::BinaryOp { left, right, .. } => {
                self.is_ast_volatile(left) || self.is_ast_volatile(right)
            }
            ASTNodeType::UnaryOp { expr, .. } => self.is_ast_volatile(expr),
            ASTNodeType::Array(rows) => rows
                .iter()
                .any(|row| row.iter().any(|cell| self.is_ast_volatile(cell))),
            ASTNodeType::Call { callee, args } => {
                self.is_ast_volatile(callee) || args.iter().any(|a| self.is_ast_volatile(a))
            }
            _ => false,
        }
    }

    pub fn is_ast_dynamic(&self, ast: &ASTNode) -> bool {
        use formualizer_parse::parser::ASTNodeType;

        match &ast.node_type {
            ASTNodeType::Function { name, args } => {
                if let Some(func) = crate::function_registry::get("", name)
                    && func
                        .caps()
                        .contains(crate::function::FnCaps::DYNAMIC_DEPENDENCY)
                {
                    return true;
                }
                args.iter().any(|arg| self.is_ast_dynamic(arg))
            }
            ASTNodeType::BinaryOp { left, right, .. } => {
                self.is_ast_dynamic(left) || self.is_ast_dynamic(right)
            }
            ASTNodeType::UnaryOp { expr, .. } => self.is_ast_dynamic(expr),
            ASTNodeType::Array(rows) => rows
                .iter()
                .any(|row| row.iter().any(|cell| self.is_ast_dynamic(cell))),
            ASTNodeType::Call { callee, args } => {
                self.is_ast_dynamic(callee) || args.iter().any(|a| self.is_ast_dynamic(a))
            }
            _ => false,
        }
    }
}

/// A `LAMBDA` parameter / `LET` name in AST form: a bare name (`x`) or Excel's optional spelling
/// `[y]`, which the parser reads as a bare table reference.
fn local_binding_name(node: &ASTNode) -> Option<&str> {
    match &node.node_type {
        ASTNodeType::Reference {
            reference: ReferenceType::NamedRange(name),
            ..
        } => Some(name.as_str()),
        ASTNodeType::Reference {
            reference: ReferenceType::Table(table),
            ..
        } if table.specifier.is_none() && !table.name.is_empty() => Some(table.name.as_str()),
        _ => None,
    }
}

/// Which arguments of a local-binding function are NAMES and which are expressions to walk,
/// read from the function's own dependency contract (the registry is the single authority for
/// `LET` / `LAMBDA` semantics): `LocalBindingPairs` → names at the even indices before the body,
/// values at the odd ones plus the body; `LambdaParameters` → every argument but the body is a
/// name. `None` for any other function or an arity the contract rejects.
fn local_binding_layout(name: &str, arg_count: usize) -> Option<(Vec<usize>, Vec<usize>)> {
    use crate::function_contract::FunctionArgumentDependencyContract as Arguments;
    let contract = crate::function_registry::get("", name)?.dependency_contract(arg_count)?;
    if arg_count == 0 {
        return None;
    }
    let body = arg_count - 1;
    match contract.arguments {
        Arguments::LambdaParameters => Some(((0..body).collect(), vec![body])),
        Arguments::LocalBindingPairs if arg_count >= 3 && arg_count % 2 == 1 => {
            let names: Vec<usize> = (0..body).step_by(2).collect();
            let mut walked: Vec<usize> = (1..body).step_by(2).collect();
            walked.push(body);
            Some((names, walked))
        }
        _ => None,
    }
}
