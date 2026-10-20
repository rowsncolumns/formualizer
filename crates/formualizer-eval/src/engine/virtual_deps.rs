use crate::engine::VertexId;
use crate::engine::VertexKind;
use crate::engine::eval::Engine;
use crate::formula_plane::region_index::Region;
use crate::structured::{Rect, lower_structured_rect};
use crate::traits::{
    EvaluationContext, FunctionProvider, NamedRangeResolver, Range, RangeResolver,
    ReferenceResolver, Resolver, SourceResolver, Table, TableResolver,
};
use formualizer_common::{ExcelError, LiteralValue};
use formualizer_parse::parser::{ReferenceType, TableReference};
use rustc_hash::{FxHashMap, FxHashSet};
use std::sync::{Arc, Mutex};

/// Virtual (range-derived) scheduling dependencies of one schedule build: the dirty / volatile
/// formula vertices each candidate must be ordered after that its graph edges do not already
/// carry.
///
/// Member lists are shared per rectangle. A template-shaped workbook has tens of thousands of
/// formulas reading the same few blocks (`SUMIFS` over a 4,000-row key column, filled down);
/// listing those thousands of vertices once per formula is hundreds of millions of edges, which
/// the scheduler then sorted and inverted (23 GB and minutes for a 1M-formula model, before a
/// single formula was evaluated). Here a vertex names the groups it depends on and the scheduler
/// treats each group as one pass-through node — every member first, then every dependent —
/// which orders exactly as the expanded edges did.
#[derive(Clone, Debug, Default)]
pub struct VirtualDeps {
    /// Per-vertex owned extras: dynamic (INDIRECT / OFFSET) dependencies and, for a formula that
    /// sits inside a rectangle it reads, that rectangle's members minus itself.
    per_vertex: FxHashMap<VertexId, Vec<VertexId>>,
    /// Distinct non-empty rectangle member lists, each sorted by vertex id.
    groups: Vec<Arc<[VertexId]>>,
    /// Vertex -> the groups it depends on (sorted, deduplicated group ids).
    vertex_groups: FxHashMap<VertexId, Vec<u32>>,
}

impl VirtualDeps {
    /// The legacy shape: every vertex's full dependency list, owned. Lists are kept verbatim
    /// (order included) so a schedule built from them walks its edges exactly as before.
    pub fn from_per_vertex(per_vertex: FxHashMap<VertexId, Vec<VertexId>>) -> Self {
        Self {
            per_vertex,
            ..Self::default()
        }
    }

    pub fn is_empty(&self) -> bool {
        self.per_vertex.is_empty() && self.vertex_groups.is_empty()
    }

    /// Vertices with at least one virtual dependency.
    pub fn vertex_count(&self) -> usize {
        self.vertices().count()
    }

    /// The number of shared rectangle member lists.
    pub fn group_count(&self) -> usize {
        self.groups.len()
    }

    /// The expanded edge count — what the per-vertex lists would hold.
    pub fn edge_count(&self) -> usize {
        self.per_vertex.values().map(Vec::len).sum::<usize>()
            + self
                .vertex_groups
                .values()
                .flatten()
                .map(|&g| self.groups[g as usize].len())
                .sum::<usize>()
    }

    pub fn has_vertex(&self, v: VertexId) -> bool {
        self.per_vertex.contains_key(&v) || self.vertex_groups.contains_key(&v)
    }

    /// Every vertex with at least one virtual dependency, each once.
    pub fn vertices(&self) -> impl Iterator<Item = VertexId> + '_ {
        self.per_vertex.keys().copied().chain(
            self.vertex_groups
                .keys()
                .filter(|v| !self.per_vertex.contains_key(v))
                .copied(),
        )
    }

    pub fn owned_of(&self, v: VertexId) -> &[VertexId] {
        self.per_vertex.get(&v).map_or(&[], Vec::as_slice)
    }

    pub fn groups_of(&self, v: VertexId) -> &[u32] {
        self.vertex_groups.get(&v).map_or(&[], Vec::as_slice)
    }

    pub fn group_members(&self, group: u32) -> &[VertexId] {
        &self.groups[group as usize]
    }

    /// `v`'s dependencies as one sorted, deduplicated list — the legacy per-vertex shape.
    pub fn flat_sorted_deps(&self, v: VertexId) -> Vec<VertexId> {
        let mut out: Vec<VertexId> = self.owned_of(v).to_vec();
        for &g in self.groups_of(v) {
            out.extend_from_slice(self.group_members(g));
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Every vertex's expanded list: the legacy map this type exists to avoid materializing.
    #[cfg(test)]
    pub fn expand(&self) -> FxHashMap<VertexId, Vec<VertexId>> {
        self.vertices()
            .map(|v| (v, self.flat_sorted_deps(v)))
            .collect()
    }
}

/// The per-build memo behind [`VirtualDeps`]: rectangle -> group (`None` for a rectangle holding
/// no dirty / volatile formula). A schedule build asks for the same few rectangles from tens of
/// thousands of formulas, so each rectangle is walked once per build; dirty / volatile flags do
/// not change while a schedule is being built, so the memo is exact.
#[derive(Default)]
pub(crate) struct RectMemo {
    by_rect: FxHashMap<(crate::SheetId, u32, u32, u32, u32), Option<u32>>,
    groups: Vec<Arc<[VertexId]>>,
}

impl RectMemo {
    fn group_for<R: EvaluationContext>(
        &mut self,
        engine: &Engine<R>,
        sheet_id: crate::SheetId,
        sr: u32,
        sc: u32,
        er: u32,
        ec: u32,
    ) -> Option<u32> {
        let key = (sheet_id, sr, sc, er, ec);
        if let Some(&group) = self.by_rect.get(&key) {
            return group;
        }
        let members = dirty_formula_vertices_in_rect(engine, sheet_id, sr, sc, er, ec);
        let group = if members.is_empty() {
            None
        } else {
            self.groups.push(Arc::from(members));
            Some((self.groups.len() - 1) as u32)
        };
        self.by_rect.insert(key, group);
        group
    }

    /// Record that `v` must follow the dirty / volatile formulas inside a 1-based inclusive
    /// rectangle: as the shared group, or — when `v` itself sits inside it — as owned extras
    /// minus `v` (a group never contains a vertex that depends on it).
    #[allow(clippy::too_many_arguments)]
    fn attach<R: EvaluationContext>(
        &mut self,
        engine: &Engine<R>,
        v: VertexId,
        sheet_id: crate::SheetId,
        sr: u32,
        sc: u32,
        er: u32,
        ec: u32,
        out: &mut CollectedVirtualDeps,
    ) {
        let Some(group) = self.group_for(engine, sheet_id, sr, sc, er, ec) else {
            return;
        };
        let members = &self.groups[group as usize];
        if members.binary_search(&v).is_ok() {
            out.owned
                .extend(members.iter().copied().filter(|&u| u != v));
        } else {
            out.groups.push(group);
        }
    }
}

/// One vertex's virtual dependencies as collected: the groups it depends on plus owned extras.
#[derive(Default)]
pub(crate) struct CollectedVirtualDeps {
    pub(crate) groups: Vec<u32>,
    pub(crate) owned: Vec<VertexId>,
}

impl CollectedVirtualDeps {
    fn flatten(self, groups: &[Arc<[VertexId]>]) -> Vec<VertexId> {
        let mut out = self.owned;
        for g in self.groups {
            out.extend_from_slice(&groups[g as usize]);
        }
        out.sort_unstable();
        out.dedup();
        out
    }
}

use crate::interpreter::Interpreter;

pub struct DynamicRefCollector<'a, R: EvaluationContext> {
    pub engine: &'a Engine<R>,
    pub current_sheet: &'a str,
    pub(crate) collected: Mutex<FxHashSet<VertexId>>,
    pub(crate) collected_regions: Mutex<FxHashSet<Region>>,
}

impl<'a, R: EvaluationContext> DynamicRefCollector<'a, R> {
    pub fn new(engine: &'a Engine<R>, current_sheet: &'a str) -> Self {
        Self {
            engine,
            current_sheet,
            collected: Mutex::new(FxHashSet::default()),
            collected_regions: Mutex::new(FxHashSet::default()),
        }
    }

    fn collect_formula_vertices_in_rect(
        &self,
        sheet_name: &str,
        sr: u32,
        sc: u32,
        er: u32,
        ec: u32,
    ) {
        let Some(sheet_id) = self.engine.graph.sheet_id(sheet_name) else {
            return;
        };
        let sr0 = sr.saturating_sub(1);
        let er0 = er.saturating_sub(1);
        let sc0 = sc.saturating_sub(1);
        let ec0 = ec.saturating_sub(1);
        self.collected_regions
            .lock()
            .unwrap()
            .insert(Region::rect(sheet_id, sr0, er0, sc0, ec0).normalized());
        let Some(index) = self.engine.graph.sheet_index(sheet_id) else {
            return;
        };

        let mut out = self.collected.lock().unwrap();
        for u in index.vertices_in_col_range(sc0, ec0) {
            let row0 = self.engine.graph.vertex_coord(u).row();
            if row0 < sr0 || row0 > er0 {
                continue;
            }
            match self.engine.graph.get_vertex_kind(u) {
                VertexKind::FormulaScalar | VertexKind::FormulaArray => {
                    if self.engine.graph.is_dirty(u) || self.engine.graph.is_volatile(u) {
                        out.insert(u);
                    }
                }
                _ => {}
            }
        }
    }

    fn collect_formula_vertices_for_range(
        &self,
        sheet_name: &str,
        start_row: Option<u32>,
        start_col: Option<u32>,
        end_row: Option<u32>,
        end_col: Option<u32>,
    ) {
        let mut sr = start_row;
        let mut sc = start_col;
        let mut er = end_row;
        let mut ec = end_col;

        if sr.is_none() && er.is_none() {
            let scv = sc.unwrap_or(1u32);
            let ecv = ec.unwrap_or(scv);
            sr = Some(1);
            if let Some((_, max_r)) = self.engine.used_rows_for_columns(sheet_name, scv, ecv) {
                er = Some(max_r);
            } else if self.engine.sheet_bounds(sheet_name).is_some() {
                er = Some(self.engine.config.max_open_ended_rows);
            }
        }
        if sc.is_none() && ec.is_none() {
            let srv = sr.unwrap_or(1u32);
            let erv = er.unwrap_or(srv);
            sc = Some(1);
            if let Some((_, max_c)) = self.engine.used_cols_for_rows(sheet_name, srv, erv) {
                ec = Some(max_c);
            } else if self.engine.sheet_bounds(sheet_name).is_some() {
                ec = Some(self.engine.config.max_open_ended_cols);
            }
        }
        if sr.is_some() && er.is_none() {
            let scv = sc.unwrap_or(1u32);
            let ecv = ec.unwrap_or(scv);
            if let Some((_, max_r)) = self.engine.used_rows_for_columns(sheet_name, scv, ecv) {
                er = Some(max_r);
            } else if self.engine.sheet_bounds(sheet_name).is_some() {
                er = Some(self.engine.config.max_open_ended_rows);
            }
        }
        if er.is_some() && sr.is_none() {
            sr = Some(1);
        }
        if sc.is_some() && ec.is_none() {
            let srv = sr.unwrap_or(1u32);
            let erv = er.unwrap_or(srv);
            if let Some((_, max_c)) = self.engine.used_cols_for_rows(sheet_name, srv, erv) {
                ec = Some(max_c);
            } else if self.engine.sheet_bounds(sheet_name).is_some() {
                ec = Some(self.engine.config.max_open_ended_cols);
            }
        }
        if ec.is_some() && sc.is_none() {
            sc = Some(1);
        }

        let sr = sr.unwrap_or(1);
        let sc = sc.unwrap_or(1);
        let er = er.unwrap_or(sr.saturating_sub(1));
        let ec = ec.unwrap_or(sc.saturating_sub(1));
        if er < sr || ec < sc {
            return;
        }

        self.collect_formula_vertices_in_rect(sheet_name, sr, sc, er, ec);
    }
}

impl<'a, R: EvaluationContext> ReferenceResolver for DynamicRefCollector<'a, R> {
    fn resolve_cell_reference(
        &self,
        sheet: Option<&str>,
        row: u32,
        col: u32,
    ) -> Result<LiteralValue, ExcelError> {
        let sheet_name = sheet.unwrap_or(self.current_sheet);
        if let Some(sheet_id) = self.engine.graph.sheet_id(sheet_name) {
            self.collected_regions.lock().unwrap().insert(Region::point(
                sheet_id,
                row.saturating_sub(1),
                col.saturating_sub(1),
            ));
        }
        if let Some(&vid) = self
            .engine
            .graph
            .get_vertex_id_for_address(&self.engine.graph.make_cell_ref(sheet_name, row, col))
        {
            self.collected.lock().unwrap().insert(vid);
        }
        self.engine.resolve_cell_reference(sheet, row, col)
    }
}

impl<'a, R: EvaluationContext> RangeResolver for DynamicRefCollector<'a, R> {
    fn resolve_range_reference(
        &self,
        sheet: Option<&str>,
        sr: Option<u32>,
        sc: Option<u32>,
        er: Option<u32>,
        ec: Option<u32>,
    ) -> Result<Box<dyn Range>, ExcelError> {
        let sheet_name = sheet.unwrap_or(self.current_sheet);
        self.collect_formula_vertices_for_range(sheet_name, sr, sc, er, ec);
        self.engine.resolve_range_reference(sheet, sr, sc, er, ec)
    }
}

impl<'a, R: EvaluationContext> NamedRangeResolver for DynamicRefCollector<'a, R> {
    fn resolve_named_range_reference(
        &self,
        name: &str,
    ) -> Result<Vec<Vec<LiteralValue>>, ExcelError> {
        self.engine.resolve_named_range_reference(name)
    }

    fn named_range_reference_definition(
        &self,
        name: &str,
    ) -> Option<formualizer_parse::parser::ReferenceType> {
        self.engine.named_range_reference_definition(name)
    }

    fn named_range_is_constant(&self, name: &str) -> bool {
        self.engine.named_range_is_constant(name)
    }

    fn named_lambda_ast(
        &self,
        name: &str,
        current_sheet: &str,
    ) -> Option<formualizer_parse::parser::ASTNode> {
        self.engine.named_lambda_ast(name, current_sheet)
    }
}

impl<'a, R: EvaluationContext> TableResolver for DynamicRefCollector<'a, R> {
    fn resolve_table_reference(&self, tref: &TableReference) -> Result<Box<dyn Table>, ExcelError> {
        self.engine.resolve_table_reference(tref)
    }
}

impl<'a, R: EvaluationContext> SourceResolver for DynamicRefCollector<'a, R> {
    fn source_scalar_version(&self, name: &str) -> Option<u64> {
        self.engine.source_scalar_version(name)
    }
    fn resolve_source_scalar(&self, name: &str) -> Result<LiteralValue, ExcelError> {
        self.engine.resolve_source_scalar(name)
    }
    fn source_table_version(&self, name: &str) -> Option<u64> {
        self.engine.source_table_version(name)
    }
    fn resolve_source_table(&self, name: &str) -> Result<Box<dyn Table>, ExcelError> {
        self.engine.resolve_source_table(name)
    }
}

impl<'a, R: EvaluationContext> Resolver for DynamicRefCollector<'a, R> {}

impl<'a, R: EvaluationContext> FunctionProvider for DynamicRefCollector<'a, R> {
    fn planning_semantic_revision(&self) -> Option<u64> {
        self.engine.planning_semantic_revision()
    }

    fn get_function(
        &self,
        ns: &str,
        name: &str,
    ) -> Option<std::sync::Arc<dyn crate::traits::Function>> {
        self.engine.get_function(ns, name)
    }

    fn get_function_for_planning(
        &self,
        ns: &str,
        name: &str,
    ) -> Option<std::sync::Arc<dyn crate::traits::Function>> {
        self.engine.get_function_for_planning(ns, name)
    }
}

impl<'a, R: EvaluationContext> EvaluationContext for DynamicRefCollector<'a, R> {
    fn resolve_range_view<'c>(
        &'c self,
        reference: &ReferenceType,
        current_sheet: &str,
    ) -> Result<crate::engine::range_view::RangeView<'c>, ExcelError> {
        // Collect vertices directly
        match reference {
            ReferenceType::Cell {
                sheet, row, col, ..
            } => {
                let sheet_name = sheet.as_deref().unwrap_or(current_sheet);
                self.collect_formula_vertices_in_rect(sheet_name, *row, *col, *row, *col);
            }
            ReferenceType::Range {
                sheet,
                start_row,
                start_col,
                end_row,
                end_col,
                ..
            } => {
                let sheet_name = sheet.as_deref().unwrap_or(current_sheet);
                self.collect_formula_vertices_for_range(
                    sheet_name, *start_row, *start_col, *end_row, *end_col,
                );
            }
            ReferenceType::NamedRange(name) => {
                let sid = self.engine.sheet_id(current_sheet);
                if let Some(s) = sid
                    && let Some(nr) = self.engine.graph.resolve_name_entry(name, s)
                {
                    let vid = nr.vertex;
                    self.collected.lock().unwrap().insert(vid);
                }
            }
            ReferenceType::Table(tref) => {
                // The table vertex only dirties dependents; ordering needs the concrete
                // rectangle the reference selects (this-row forms were rewritten at ingest).
                if let Some((sheet_id, rect)) = structured_reference_rect(self.engine, tref, None) {
                    let sheet_name = self.engine.graph.sheet_name(sheet_id);
                    self.collect_formula_vertices_in_rect(
                        sheet_name,
                        rect.start_row,
                        rect.start_col,
                        rect.end_row,
                        rect.end_col,
                    );
                }
            }
            _ => {}
        }

        self.engine.resolve_range_view(reference, current_sheet)
    }
}

pub struct RangeVirtualDepProvider;

impl RangeVirtualDepProvider {
    /// `v`'s range-derived dependencies as one flat, sorted list.
    pub fn get_virtual_deps<R: EvaluationContext>(
        engine: &Engine<R>,
        v: VertexId,
    ) -> Vec<VertexId> {
        let mut memo = RectMemo::default();
        let collected = Self::collect(engine, v, &mut memo);
        collected.flatten(&memo.groups)
    }

    /// The rectangle groups and owned extras `v` must be scheduled after, sharing `memo` across
    /// the formulas of one schedule build.
    pub(crate) fn collect<R: EvaluationContext>(
        engine: &Engine<R>,
        v: VertexId,
        memo: &mut RectMemo,
    ) -> CollectedVirtualDeps {
        let mut collected = CollectedVirtualDeps::default();
        if let Some(ranges) = engine.graph.get_range_dependencies(v) {
            let current_sheet_id = engine.graph.get_vertex_sheet_id(v);
            for r in ranges {
                let sheet_id = match r.sheet {
                    formualizer_common::SheetLocator::Id(id) => id,
                    _ => current_sheet_id,
                };
                let sheet_name = engine.graph.sheet_name(sheet_id);

                let mut sr = r.start_row.map(|b| b.index + 1);
                let mut sc = r.start_col.map(|b| b.index + 1);
                let mut er = r.end_row.map(|b| b.index + 1);
                let mut ec = r.end_col.map(|b| b.index + 1);

                if sr.is_none() && er.is_none() {
                    let scv = sc.unwrap_or(1u32);
                    let ecv = ec.unwrap_or(scv);
                    if let Some((min_r, max_r)) = engine.used_rows_for_columns(sheet_name, scv, ecv)
                    {
                        sr = Some(min_r);
                        er = Some(max_r);
                    } else if let Some((_max_rows, _)) = engine.sheet_bounds(sheet_name) {
                        sr = Some(1);
                        er = Some(engine.config.max_open_ended_rows);
                    }
                }
                if sc.is_none() && ec.is_none() {
                    let srv = sr.unwrap_or(1u32);
                    let erv = er.unwrap_or(srv);
                    if let Some((min_c, max_c)) = engine.used_cols_for_rows(sheet_name, srv, erv) {
                        sc = Some(min_c);
                        ec = Some(max_c);
                    } else if let Some((_, _max_cols)) = engine.sheet_bounds(sheet_name) {
                        sc = Some(1);
                        ec = Some(engine.config.max_open_ended_cols);
                    }
                }
                if sr.is_some() && er.is_none() {
                    let scv = sc.unwrap_or(1u32);
                    let ecv = ec.unwrap_or(scv);
                    if let Some((_, max_r)) = engine.used_rows_for_columns(sheet_name, scv, ecv) {
                        er = Some(max_r);
                    } else if let Some((_max_rows, _)) = engine.sheet_bounds(sheet_name) {
                        er = Some(engine.config.max_open_ended_rows);
                    }
                }
                if er.is_some() && sr.is_none() {
                    let scv = sc.unwrap_or(1u32);
                    let ecv = ec.unwrap_or(scv);
                    if let Some((min_r, _)) = engine.used_rows_for_columns(sheet_name, scv, ecv) {
                        sr = Some(min_r);
                    } else {
                        sr = Some(1);
                    }
                }
                if sc.is_some() && ec.is_none() {
                    let srv = sr.unwrap_or(1u32);
                    let erv = er.unwrap_or(srv);
                    if let Some((_, max_c)) = engine.used_cols_for_rows(sheet_name, srv, erv) {
                        ec = Some(max_c);
                    } else if let Some((_, _max_cols)) = engine.sheet_bounds(sheet_name) {
                        ec = Some(engine.config.max_open_ended_cols);
                    }
                }
                if ec.is_some() && sc.is_none() {
                    let srv = sr.unwrap_or(1u32);
                    let erv = er.unwrap_or(srv);
                    if let Some((min_c, _)) = engine.used_cols_for_rows(sheet_name, srv, erv) {
                        sc = Some(min_c);
                    } else {
                        sc = Some(1);
                    }
                }

                let sr = sr.unwrap_or(1);
                let sc = sc.unwrap_or(1);
                let er = er.unwrap_or(sr.saturating_sub(1));
                let ec = ec.unwrap_or(sc.saturating_sub(1));
                if er < sr || ec < sc {
                    continue;
                }

                memo.attach(engine, v, sheet_id, sr, sc, er, ec, &mut collected);
            }
        }
        // Structured references: the formula's graph edge points at the TABLE vertex, whose
        // range deps are stripes (dirty propagation only). Without the rectangle the reference
        // actually selects, a `=SUM(Table[Calc])` over a calculated column is dirtied by the
        // column's edits but scheduled in the same layer as the column's own formulas — it
        // reads their stale values (or blanks on a fresh graph). Lower each table reference
        // to its rectangle and order the formula after the dirty formulas inside it.
        if depends_on_table(engine, v) {
            let current_row = engine.graph.get_cell_ref(v).map(|c| c.coord.row() + 1);
            for (sheet_id, rect) in table_reference_rects(engine, v, current_row) {
                memo.attach(
                    engine,
                    v,
                    sheet_id,
                    rect.start_row,
                    rect.start_col,
                    rect.end_row,
                    rect.end_col,
                    &mut collected,
                );
            }
        }
        collected
    }
}

/// Every dirty / volatile formula vertex inside a 1-based inclusive rectangle, in vertex order.
fn dirty_formula_vertices_in_rect<R: EvaluationContext>(
    engine: &Engine<R>,
    sheet_id: crate::SheetId,
    sr: u32,
    sc: u32,
    er: u32,
    ec: u32,
) -> Vec<VertexId> {
    let Some(index) = engine.graph.sheet_index(sheet_id) else {
        return Vec::new();
    };
    let sr0 = sr.saturating_sub(1);
    let er0 = er.saturating_sub(1);
    let sc0 = sc.saturating_sub(1);
    let ec0 = ec.saturating_sub(1);
    let mut inside = Vec::new();
    for u in index.vertices_in_col_range(sc0, ec0) {
        let pc = engine.graph.vertex_coord(u);
        let row0 = pc.row();
        if row0 < sr0 || row0 > er0 {
            continue;
        }
        match engine.graph.get_vertex_kind(u) {
            VertexKind::FormulaScalar | VertexKind::FormulaArray => {
                if engine.graph.is_dirty(u) || engine.graph.is_volatile(u) {
                    inside.push(u);
                }
            }
            _ => {}
        }
    }
    inside.sort_unstable();
    inside
}

/// Does `v` carry a graph edge to a table vertex (i.e. its formula holds a structured reference
/// the ingest did not rewrite into plain cell references)?
fn depends_on_table<R: EvaluationContext>(engine: &Engine<R>, v: VertexId) -> bool {
    if !engine.graph.has_tables() {
        return false;
    }
    let is_table = |d: &VertexId| engine.graph.get_vertex_kind(*d) == VertexKind::Table;
    match engine.graph.dependencies_slice(v) {
        Some(deps) => deps.iter().any(is_table),
        None => engine.graph.get_dependencies(v).iter().any(is_table),
    }
}

/// The concrete rectangles selected by the structured references in `v`'s formula, with the
/// table's sheet. References that fail to lower (unknown table / column, a this-row form with
/// no row context) are skipped — evaluation reports those errors itself.
fn table_reference_rects<R: EvaluationContext>(
    engine: &Engine<R>,
    v: VertexId,
    current_row: Option<u32>,
) -> Vec<(crate::SheetId, Rect)> {
    let Some(ast_id) = engine.graph.get_formula_id(v) else {
        return Vec::new();
    };
    let policy = formualizer_parse::parser::CollectPolicy {
        expand_small_ranges: false,
        range_expansion_limit: 0,
        include_names: false,
    };
    let Ok(refs) = crate::engine::plan::collect_references_arena(
        engine.graph.data_store(),
        ast_id,
        engine.graph.sheet_reg(),
        &policy,
    ) else {
        return Vec::new();
    };
    refs.iter()
        .filter_map(|r| match r {
            ReferenceType::Table(tref) => structured_reference_rect(engine, tref, current_row),
            _ => None,
        })
        .collect()
}

/// Lower one structured reference against the engine's table definition: the table's sheet id
/// plus the 1-based inclusive rectangle it selects. `None` when the table is undefined or the
/// specifier does not lower (see [`crate::structured::lower_structured_rect`]).
fn structured_reference_rect<R: EvaluationContext>(
    engine: &Engine<R>,
    tref: &TableReference,
    current_row: Option<u32>,
) -> Option<(crate::SheetId, Rect)> {
    if tref.name.is_empty() {
        return None;
    }
    let geom = engine.table_geometry(&tref.name)?;
    let sheet_id = engine.graph.sheet_id(geom.sheet.as_deref()?)?;
    let rect = lower_structured_rect(&geom, tref.specifier.as_ref(), current_row).ok()?;
    Some((sheet_id, rect))
}

pub struct VirtualDepBuilder<'a, R: EvaluationContext> {
    engine: &'a Engine<R>,
}

impl<'a, R: EvaluationContext> VirtualDepBuilder<'a, R> {
    pub fn new(engine: &'a Engine<R>) -> Self {
        Self { engine }
    }
    pub fn build(&self, candidates: &[VertexId]) -> (VirtualDeps, Vec<VertexId>) {
        let augmented_vertices: Vec<VertexId> = Vec::new(); // Will be populated in Phase 3
        let mut memo = RectMemo::default();
        let mut per_vertex: FxHashMap<VertexId, Vec<VertexId>> = FxHashMap::default();
        let mut vertex_groups: FxHashMap<VertexId, Vec<u32>> = FxHashMap::default();

        for &v in candidates {
            let mut collected = RangeVirtualDepProvider::collect(self.engine, v, &mut memo);
            collected
                .owned
                .extend(DynamicRefVirtualDepProvider::get_virtual_deps(
                    self.engine,
                    v,
                ));
            collected.owned.sort_unstable();
            collected.owned.dedup();
            collected.groups.sort_unstable();
            collected.groups.dedup();
            if !collected.owned.is_empty() {
                per_vertex.insert(v, collected.owned);
            }
            if !collected.groups.is_empty() {
                vertex_groups.insert(v, collected.groups);
            }
        }

        (
            VirtualDeps {
                per_vertex,
                groups: memo.groups,
                vertex_groups,
            },
            augmented_vertices,
        )
    }
}

pub struct DynamicRefVirtualDepProvider;

impl DynamicRefVirtualDepProvider {
    fn collect<R: EvaluationContext>(
        engine: &Engine<R>,
        v: VertexId,
    ) -> (Vec<VertexId>, Vec<Region>) {
        if !engine.graph.is_dynamic(v) {
            return (Vec::new(), Vec::new());
        }
        let Some(ast_id) = engine.graph.get_formula_id(v) else {
            return (Vec::new(), Vec::new());
        };
        let sheet_id = engine.graph.get_vertex_sheet_id(v);
        let sheet_name = engine.graph.sheet_name(sheet_id);
        let collector = DynamicRefCollector::new(engine, sheet_name);
        let cell_ref = engine
            .graph
            .get_cell_ref(v)
            .unwrap_or_else(|| engine.graph.make_cell_ref(sheet_name, 0, 0));
        let interpreter = Interpreter::new_with_cell(&collector, sheet_name, cell_ref);
        let _ = interpreter.evaluate_arena_ast(
            ast_id,
            engine.graph.data_store(),
            engine.graph.sheet_reg(),
        );
        let mut deps = collector
            .collected
            .lock()
            .unwrap()
            .iter()
            .copied()
            .filter(|&dependency| dependency != v)
            .collect::<Vec<_>>();
        deps.sort_unstable();
        deps.dedup();
        let mut regions = collector
            .collected_regions
            .lock()
            .unwrap()
            .iter()
            .copied()
            .collect::<Vec<_>>();
        regions.sort_by_key(|region| {
            let (rows, cols) = region.axis_ranges();
            (region.sheet_id(), rows.query_bounds(), cols.query_bounds())
        });
        regions.dedup();
        (deps, regions)
    }

    pub fn get_virtual_deps<R: EvaluationContext>(
        engine: &Engine<R>,
        v: VertexId,
    ) -> Vec<VertexId> {
        Self::collect(engine, v).0
    }

    pub(crate) fn get_virtual_regions<R: EvaluationContext>(
        engine: &Engine<R>,
        v: VertexId,
    ) -> Vec<Region> {
        Self::collect(engine, v).1
    }
}
