use super::DependencyGraph;
use super::vertex::VertexId;
use crate::engine::virtual_deps::VirtualDeps;
use formualizer_common::ExcelError;
use rustc_hash::{FxHashMap, FxHashSet};
use std::borrow::Cow;

pub struct Scheduler<'a> {
    graph: &'a DependencyGraph,
}

#[derive(Debug, Clone)]
pub struct Layer {
    pub vertices: Vec<VertexId>,
}

/// One step of the canonical schedule walk: either an acyclic Kahn wave
/// (`Layer`, an index into `Schedule::layers`) or a cyclic SCC treated as a
/// single super-node (`Cycle`, an index into `Schedule::cycles`). Storing
/// indices keeps `Schedule::layers`/`Schedule::cycles` the single owners of
/// the vertex Vecs, so building or cloning a schedule never duplicates them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleUnit {
    Layer(u32),
    Cycle(u32),
}

#[derive(Debug, Clone)]
pub struct Schedule {
    /// Canonical walk order: condensation order over the dependency graph.
    /// Every `Cycle` unit appears after all units containing its external
    /// dependencies and before all units containing its external dependents.
    pub units: Vec<ScheduleUnit>,
    /// All cyclic SCCs (same contents as before `units` existed), for
    /// consumers that only need the set of cycles.
    pub cycles: Vec<Vec<VertexId>>,
    /// The `Layer` units in order, for consumers that only walk layers.
    pub layers: Vec<Layer>,
}

impl Schedule {
    fn from_parts(layers: Vec<Layer>, cycles: Vec<Vec<VertexId>>) -> Self {
        debug_assert!(
            cycles.is_empty(),
            "Schedule::from_parts is the cycle-free fast path"
        );
        let units = (0..layers.len() as u32).map(ScheduleUnit::Layer).collect();
        Schedule {
            units,
            cycles,
            layers,
        }
    }

    /// Resolve a `ScheduleUnit::Layer` index.
    pub fn unit_layer(&self, i: u32) -> &Layer {
        &self.layers[i as usize]
    }

    /// Resolve a `ScheduleUnit::Cycle` index.
    pub fn unit_cycle(&self, i: u32) -> &[VertexId] {
        &self.cycles[i as usize]
    }
}

impl<'a> Scheduler<'a> {
    pub fn new(graph: &'a DependencyGraph) -> Self {
        Self { graph }
    }

    pub fn create_schedule(&self, vertices: &[VertexId]) -> Result<Schedule, ExcelError> {
        #[cfg(feature = "tracing")]
        let _span = tracing::info_span!("scheduler", vertices = vertices.len()).entered();
        // 1. Find strongly connected components using Tarjan's algorithm
        #[cfg(feature = "tracing")]
        let _scc_span = tracing::info_span!("tarjan_scc").entered();
        let sccs = self.tarjan_scc(vertices)?;
        #[cfg(feature = "tracing")]
        drop(_scc_span);

        // 2. Separate cyclic from acyclic components
        let (cycles, acyclic_sccs) = self.separate_cycles(sccs);

        // 3. Topologically sort acyclic components into layers
        if self.graph.dynamic_topo_enabled() {
            // Dynamic-topo (PK) branch: pk_layers_for orders only the acyclic
            // subset, so we cannot interleave Cycle units by condensation
            // position here. Stay conservative on this experimental path and
            // emit ALL Cycle units first (matching today's stamp-cycles-first
            // semantics), then the pk layers as Layer units.
            let subset: Vec<VertexId> = acyclic_sccs.into_iter().flatten().collect();
            let layers = if subset.is_empty() {
                Vec::new()
            } else {
                self.graph
                    .pk_layers_for(&subset)
                    .unwrap_or(self.build_layers(vec![subset])?)
            };
            let mut units = Vec::with_capacity(cycles.len() + layers.len());
            let mut cycle_order: Vec<usize> = (0..cycles.len()).collect();
            cycle_order.sort_by_key(|&i| cycles[i].iter().copied().min());
            for i in cycle_order {
                units.push(ScheduleUnit::Cycle(i as u32));
            }
            units.extend((0..layers.len() as u32).map(ScheduleUnit::Layer));
            return Ok(Schedule {
                units,
                cycles,
                layers,
            });
        }

        if cycles.is_empty() {
            // Fast path: byte-for-byte today's layer construction.
            let layers = self.build_layers(acyclic_sccs)?;
            return Ok(Schedule::from_parts(layers, cycles));
        }

        // Cycle path: Kahn over the condensation (SCC-as-node).
        let (units, layers) = self.build_condensation_units(&cycles, acyclic_sccs, None)?;
        Ok(Schedule {
            units,
            cycles,
            layers,
        })
    }

    /// Create a schedule considering additional ephemeral (virtual) dependencies just for this pass.
    /// `vdeps` maps a vertex to extra dependency vertices that should be considered as incoming edges.
    pub fn create_schedule_with_virtual(
        &self,
        vertices: &[VertexId],
        vdeps: &VirtualDeps,
    ) -> Result<Schedule, ExcelError> {
        #[cfg(feature = "tracing")]
        let _span = tracing::info_span!(
            "scheduler_with_virtual",
            vertices = vertices.len(),
            vdeps = vdeps.vertex_count()
        )
        .entered();
        // 1. SCC detection with virtual deps
        #[cfg(feature = "tracing")]
        let _scc_span = tracing::info_span!("tarjan_scc_with_virtual").entered();
        let sccs = self.tarjan_scc_with_virtual(vertices, vdeps)?;
        #[cfg(feature = "tracing")]
        drop(_scc_span);
        // 2. Separate cycles and acyclic components
        let (cycles, acyclic_sccs) = self.separate_cycles(sccs);
        // 3. Build layers over combined adjacency (graph + vdeps)
        #[cfg(feature = "tracing")]
        let _layers_span = tracing::info_span!("build_layers_with_virtual").entered();
        if cycles.is_empty() {
            // Fast path: byte-for-byte today's layer construction.
            let layers = self.build_layers_with_virtual(acyclic_sccs, vdeps)?;
            return Ok(Schedule::from_parts(layers, cycles));
        }
        // Cycle path: Kahn over the condensation (SCC-as-node), honoring
        // virtual deps as extra edges.
        let (units, layers) = self.build_condensation_units(&cycles, acyclic_sccs, Some(vdeps))?;
        Ok(Schedule {
            units,
            cycles,
            layers,
        })
    }

    /// Tarjan's strongly connected components algorithm
    pub fn tarjan_scc(&self, vertices: &[VertexId]) -> Result<Vec<Vec<VertexId>>, ExcelError> {
        self.tarjan_scc_impl(vertices, None)
    }

    /// Tarjan with virtual deps
    fn tarjan_scc_with_virtual(
        &self,
        vertices: &[VertexId],
        vdeps: &VirtualDeps,
    ) -> Result<Vec<Vec<VertexId>>, ExcelError> {
        self.tarjan_scc_impl(vertices, Some(vdeps))
    }

    /// Iterative Tarjan over the scheduled subgraph, optionally honoring
    /// per-pass virtual dependency edges.
    ///
    /// The DFS uses an explicit frame stack (vertex, dependency list, next
    /// edge offset) instead of recursion, so depth is bounded by heap, not
    /// the thread stack — the recursive predecessor SIGABRTed around depth
    /// ~1500 in debug (2 MiB test stacks) on large SCCs AND on plain acyclic
    /// chains whose dependencies point at higher vertex ids (see
    /// `engine/tests/iterate_corpus_scale.rs`).
    ///
    /// Determinism contract: SCC emission order and within-SCC member order
    /// are byte-identical to the recursive version (pinned by the
    /// differential test in `engine/tests/tarjan_differential.rs` and the
    /// ordering invariants in `engine/tests/schedule_units.rs`):
    /// * dependencies are walked in adjacency order (base slice/Vec, then
    ///   the vertex's virtual deps, exactly as the recursive code iterated);
    /// * a vertex's SCC is emitted when its frame is exhausted and
    ///   `lowlink == index` (the same program point as the recursive
    ///   post-order emission);
    /// * SCC members are popped off the Tarjan stack in the same order.
    fn tarjan_scc_impl(
        &self,
        vertices: &[VertexId],
        vdeps: Option<&VirtualDeps>,
    ) -> Result<Vec<Vec<VertexId>>, ExcelError> {
        /// One edge target: a vertex (resolved through `local_of` below) or an already-local
        /// group node.
        enum Target {
            Vertex(VertexId),
            Local(u32),
        }
        /// One node's dependency list, materialized once per DFS frame.
        enum DepList<'g> {
            Slice(&'g [VertexId]),
            Owned(Vec<VertexId>),
            /// A vertex with virtual deps: its base adjacency, then its owned extras, then the
            /// group nodes it depends on — the order the expanded lists walked.
            Virtual {
                base: Cow<'g, [VertexId]>,
                owned: &'g [VertexId],
                groups: &'g [u32],
                group_local: &'g FxHashMap<u32, u32>,
            },
            /// A group node: its members.
            Members(&'g [VertexId]),
        }
        impl DepList<'_> {
            #[inline]
            fn get(&self, i: usize) -> Option<Target> {
                match self {
                    DepList::Slice(s) => s.get(i).map(|&v| Target::Vertex(v)),
                    DepList::Owned(v) => v.get(i).map(|&v| Target::Vertex(v)),
                    DepList::Members(m) => m.get(i).map(|&v| Target::Vertex(v)),
                    DepList::Virtual {
                        base,
                        owned,
                        groups,
                        group_local,
                    } => {
                        if i < base.len() {
                            return Some(Target::Vertex(base[i]));
                        }
                        let i = i - base.len();
                        if i < owned.len() {
                            return Some(Target::Vertex(owned[i]));
                        }
                        groups
                            .get(i - owned.len())
                            .map(|g| Target::Local(group_local[g]))
                    }
                }
            }
        }

        // Compact the candidate set into dense local ids once, up front. All
        // per-edge / per-frame bookkeeping (index, lowlink, on-stack) then
        // becomes a plain array op instead of an FxHashMap probe; the only
        // remaining hash per edge is the `VertexId -> local id` membership
        // lookup, which doubles as the old `vertex_set.contains` filter.
        // `VertexId` is a `u32` newtype, so `u32` local ids cannot overflow.
        const UNVISITED: u32 = u32::MAX;
        let mut local_of: FxHashMap<VertexId, u32> =
            FxHashMap::with_capacity_and_hasher(vertices.len(), Default::default());
        let mut vertex_of_local: Vec<VertexId> = Vec::with_capacity(vertices.len());
        for &v in vertices {
            if let std::collections::hash_map::Entry::Vacant(slot) = local_of.entry(v) {
                slot.insert(vertex_of_local.len() as u32);
                vertex_of_local.push(v);
            }
        }
        let n = vertex_of_local.len();

        // Group nodes: one local id (after the vertices) per rectangle group a scheduled vertex
        // depends on. A group is a pass-through — reached from every dependent, reaching every
        // member — so the vertices reachable through it are exactly those the expanded edges
        // reached, and the SCC partition of the real vertices is unchanged.
        let mut group_local: FxHashMap<u32, u32> = FxHashMap::default();
        let mut members_of_group_local: Vec<&[VertexId]> = Vec::new();
        if let Some(vd) = vdeps {
            for &v in &vertex_of_local {
                for &g in vd.groups_of(v) {
                    group_local.entry(g).or_insert_with(|| {
                        members_of_group_local.push(vd.group_members(g));
                        (n + members_of_group_local.len() - 1) as u32
                    });
                }
            }
        }
        let total = n + members_of_group_local.len();

        let deps_of = |local: u32| -> DepList<'_> {
            if let Some(members) = (local as usize)
                .checked_sub(n)
                .and_then(|i| members_of_group_local.get(i))
            {
                return DepList::Members(members);
            }
            let vertex = vertex_of_local[local as usize];
            if let Some(vd) = vdeps.filter(|vd| vd.has_vertex(vertex)) {
                let base: Cow<'_, [VertexId]> = match self.graph.dependencies_slice(vertex) {
                    Some(base) => Cow::Borrowed(base),
                    None => Cow::Owned(self.graph.get_dependencies(vertex)),
                };
                DepList::Virtual {
                    base,
                    owned: vd.owned_of(vertex),
                    groups: vd.groups_of(vertex),
                    group_local: &group_local,
                }
            } else if let Some(base) = self.graph.dependencies_slice(vertex) {
                DepList::Slice(base)
            } else {
                DepList::Owned(self.graph.get_dependencies(vertex))
            }
        };

        let mut index_counter: u32 = 0;
        let mut indices: Vec<u32> = vec![UNVISITED; total];
        let mut lowlinks: Vec<u32> = vec![0; total];
        let mut on_stack: Vec<bool> = vec![false; total];
        let mut stack: Vec<u32> = Vec::with_capacity(total);
        let mut sccs: Vec<Vec<VertexId>> = Vec::new();

        // Explicit DFS frames: (local id, its dependency list, next edge
        // offset). Pre-sized to the worst case (one frame per node, e.g. a
        // single deep chain) so deep schedules never re-grow the stack.
        let mut frames: Vec<(u32, DepList<'_>, u32)> = Vec::with_capacity(total);

        for &root_vertex in vertices {
            let root = local_of[&root_vertex];
            if indices[root as usize] != UNVISITED {
                continue;
            }
            indices[root as usize] = index_counter;
            lowlinks[root as usize] = index_counter;
            index_counter += 1;
            stack.push(root);
            on_stack[root as usize] = true;
            frames.push((root, deps_of(root), 0));

            while let Some(frame) = frames.last_mut() {
                let vertex = frame.0 as usize;
                if let Some(target) = frame.1.get(frame.2 as usize) {
                    frame.2 += 1;
                    // Only consider dependencies that are part of the current
                    // scheduling task.
                    let dep = match target {
                        Target::Vertex(dep_vertex) => match local_of.get(&dep_vertex) {
                            Some(&dep) => dep,
                            None => continue,
                        },
                        Target::Local(dep) => dep,
                    };
                    let d = dep as usize;
                    if indices[d] == UNVISITED {
                        // Not yet visited: descend (the recursive call).
                        indices[d] = index_counter;
                        lowlinks[d] = index_counter;
                        index_counter += 1;
                        stack.push(dep);
                        on_stack[d] = true;
                        frames.push((dep, deps_of(dep), 0));
                    } else if on_stack[d] {
                        // On the Tarjan stack: in the current SCC.
                        let dep_index = indices[d];
                        if dep_index < lowlinks[vertex] {
                            lowlinks[vertex] = dep_index;
                        }
                    }
                } else {
                    // Node exhausted: emit its SCC if it is a root, then
                    // return to the parent frame, folding the child lowlink in
                    // (the post-recursion `min(lowlink[v], lowlink[dep])`).
                    let vertex_lowlink = lowlinks[vertex];
                    if vertex_lowlink == indices[vertex] {
                        let mut scc = Vec::new();
                        loop {
                            let w = stack.pop().unwrap();
                            on_stack[w as usize] = false;
                            // Group nodes are scheduling scaffolding, not formulas: a cycle
                            // through one is the cycle among its real vertices.
                            if (w as usize) < n {
                                scc.push(vertex_of_local[w as usize]);
                            }
                            if w as usize == vertex {
                                break;
                            }
                        }
                        if !scc.is_empty() {
                            sccs.push(scc);
                        }
                    }
                    frames.pop();
                    if let Some(parent) = frames.last() {
                        let p = parent.0 as usize;
                        if vertex_lowlink < lowlinks[p] {
                            lowlinks[p] = vertex_lowlink;
                        }
                    }
                }
            }
        }

        Ok(sccs)
    }

    /// Test-only visibility shim over the private virtual-deps entry, used by
    /// the differential test in `engine/tests/tarjan_differential.rs`.
    #[cfg(test)]
    pub(crate) fn tarjan_scc_with_virtual_for_tests(
        &self,
        vertices: &[VertexId],
        vdeps: &FxHashMap<VertexId, Vec<VertexId>>,
    ) -> Result<Vec<Vec<VertexId>>, ExcelError> {
        let vdeps = VirtualDeps::from_per_vertex(vdeps.clone());
        self.tarjan_scc_with_virtual(vertices, &vdeps)
    }

    /// Recursive reference implementation, retained ONLY for the differential
    /// test (`engine/tests/tarjan_differential.rs`) that proves the iterative
    /// rewrite emits byte-identical SCC output. Never call on deep graphs
    /// without a large stack: it overflows around depth ~1500 in debug.
    #[cfg(test)]
    pub(crate) fn tarjan_scc_recursive_reference(
        &self,
        vertices: &[VertexId],
    ) -> Result<Vec<Vec<VertexId>>, ExcelError> {
        let mut index_counter = 0;
        let mut stack = Vec::new();
        let mut indices = FxHashMap::default();
        let mut lowlinks = FxHashMap::default();
        let mut on_stack = FxHashSet::default();
        let mut sccs = Vec::new();
        let vertex_set: FxHashSet<VertexId> = vertices.iter().copied().collect();

        for &vertex in vertices {
            if !indices.contains_key(&vertex) {
                self.tarjan_visit(
                    vertex,
                    &mut index_counter,
                    &mut stack,
                    &mut indices,
                    &mut lowlinks,
                    &mut on_stack,
                    &mut sccs,
                    &vertex_set,
                )?;
            }
        }

        Ok(sccs)
    }

    /// Recursive reference with virtual deps; see
    /// [`Self::tarjan_scc_recursive_reference`].
    #[cfg(test)]
    pub(crate) fn tarjan_scc_with_virtual_recursive_reference(
        &self,
        vertices: &[VertexId],
        vdeps: &FxHashMap<VertexId, Vec<VertexId>>,
    ) -> Result<Vec<Vec<VertexId>>, ExcelError> {
        let mut index_counter = 0;
        let mut stack = Vec::new();
        let mut indices = FxHashMap::default();
        let mut lowlinks = FxHashMap::default();
        let mut on_stack = FxHashSet::default();
        let mut sccs = Vec::new();
        let vertex_set: FxHashSet<VertexId> = vertices.iter().copied().collect();

        for &vertex in vertices {
            if !indices.contains_key(&vertex) {
                self.tarjan_visit_with_virtual(
                    vertex,
                    &mut index_counter,
                    &mut stack,
                    &mut indices,
                    &mut lowlinks,
                    &mut on_stack,
                    &mut sccs,
                    &vertex_set,
                    vdeps,
                )?;
            }
        }

        Ok(sccs)
    }

    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    fn tarjan_visit(
        &self,
        vertex: VertexId,
        index_counter: &mut usize,
        stack: &mut Vec<VertexId>,
        indices: &mut FxHashMap<VertexId, usize>,
        lowlinks: &mut FxHashMap<VertexId, usize>,
        on_stack: &mut FxHashSet<VertexId>,
        sccs: &mut Vec<Vec<VertexId>>,
        vertex_set: &FxHashSet<VertexId>,
    ) -> Result<(), ExcelError> {
        // Set the depth index for vertex to the smallest unused index
        indices.insert(vertex, *index_counter);
        lowlinks.insert(vertex, *index_counter);
        *index_counter += 1;
        stack.push(vertex);
        on_stack.insert(vertex);

        // Consider successors of vertex (dependencies)
        if let Some(dependencies) = self.graph.dependencies_slice(vertex) {
            for &dependency in dependencies {
                // Only consider dependencies that are part of the current scheduling task
                if !vertex_set.contains(&dependency) {
                    continue;
                }

                if !indices.contains_key(&dependency) {
                    // Successor dependency has not yet been visited; recurse on it
                    self.tarjan_visit(
                        dependency,
                        index_counter,
                        stack,
                        indices,
                        lowlinks,
                        on_stack,
                        sccs,
                        vertex_set,
                    )?;
                    let dep_lowlink = lowlinks[&dependency];
                    lowlinks.insert(vertex, lowlinks[&vertex].min(dep_lowlink));
                } else if on_stack.contains(&dependency) {
                    // Successor dependency is in stack and hence in the current SCC
                    let dep_index = indices[&dependency];
                    lowlinks.insert(vertex, lowlinks[&vertex].min(dep_index));
                }
            }
        } else {
            let dependencies = self.graph.get_dependencies(vertex);
            for dependency in dependencies {
                // Only consider dependencies that are part of the current scheduling task
                if !vertex_set.contains(&dependency) {
                    continue;
                }

                if !indices.contains_key(&dependency) {
                    // Successor dependency has not yet been visited; recurse on it
                    self.tarjan_visit(
                        dependency,
                        index_counter,
                        stack,
                        indices,
                        lowlinks,
                        on_stack,
                        sccs,
                        vertex_set,
                    )?;
                    let dep_lowlink = lowlinks[&dependency];
                    lowlinks.insert(vertex, lowlinks[&vertex].min(dep_lowlink));
                } else if on_stack.contains(&dependency) {
                    // Successor dependency is in stack and hence in the current SCC
                    let dep_index = indices[&dependency];
                    lowlinks.insert(vertex, lowlinks[&vertex].min(dep_index));
                }
            }
        }

        // If vertex is a root node, pop the stack and print an SCC
        if lowlinks[&vertex] == indices[&vertex] {
            let mut scc = Vec::new();
            loop {
                let w = stack.pop().unwrap();
                on_stack.remove(&w);
                scc.push(w);
                if w == vertex {
                    break;
                }
            }
            sccs.push(scc);
        }

        Ok(())
    }

    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    fn tarjan_visit_with_virtual(
        &self,
        vertex: VertexId,
        index_counter: &mut usize,
        stack: &mut Vec<VertexId>,
        indices: &mut FxHashMap<VertexId, usize>,
        lowlinks: &mut FxHashMap<VertexId, usize>,
        on_stack: &mut FxHashSet<VertexId>,
        sccs: &mut Vec<Vec<VertexId>>,
        vertex_set: &FxHashSet<VertexId>,
        vdeps: &FxHashMap<VertexId, Vec<VertexId>>,
    ) -> Result<(), ExcelError> {
        // Set the depth index for vertex to the smallest unused index
        indices.insert(vertex, *index_counter);
        lowlinks.insert(vertex, *index_counter);
        *index_counter += 1;
        stack.push(vertex);
        on_stack.insert(vertex);

        // Consider successors of vertex (dependencies) including virtual deps
        if let Some(extra) = vdeps.get(&vertex) {
            let mut dependencies: Vec<VertexId> =
                if let Some(base) = self.graph.dependencies_slice(vertex) {
                    base.to_vec()
                } else {
                    self.graph.get_dependencies(vertex)
                };
            dependencies.extend(extra.iter().copied());

            for dependency in dependencies {
                // Only consider dependencies that are part of the current scheduling task
                if !vertex_set.contains(&dependency) {
                    continue;
                }

                if !indices.contains_key(&dependency) {
                    // Successor dependency has not yet been visited; recurse on it
                    self.tarjan_visit_with_virtual(
                        dependency,
                        index_counter,
                        stack,
                        indices,
                        lowlinks,
                        on_stack,
                        sccs,
                        vertex_set,
                        vdeps,
                    )?;
                    let dep_lowlink = lowlinks[&dependency];
                    lowlinks.insert(vertex, lowlinks[&vertex].min(dep_lowlink));
                } else if on_stack.contains(&dependency) {
                    // Successor dependency is in stack and hence in the current SCC
                    let dep_index = indices[&dependency];
                    lowlinks.insert(vertex, lowlinks[&vertex].min(dep_index));
                }
            }
        } else if let Some(dependencies) = self.graph.dependencies_slice(vertex) {
            for &dependency in dependencies {
                // Only consider dependencies that are part of the current scheduling task
                if !vertex_set.contains(&dependency) {
                    continue;
                }

                if !indices.contains_key(&dependency) {
                    // Successor dependency has not yet been visited; recurse on it
                    self.tarjan_visit_with_virtual(
                        dependency,
                        index_counter,
                        stack,
                        indices,
                        lowlinks,
                        on_stack,
                        sccs,
                        vertex_set,
                        vdeps,
                    )?;
                    let dep_lowlink = lowlinks[&dependency];
                    lowlinks.insert(vertex, lowlinks[&vertex].min(dep_lowlink));
                } else if on_stack.contains(&dependency) {
                    // Successor dependency is in stack and hence in the current SCC
                    let dep_index = indices[&dependency];
                    lowlinks.insert(vertex, lowlinks[&vertex].min(dep_index));
                }
            }
        } else {
            let dependencies = self.graph.get_dependencies(vertex);
            for dependency in dependencies {
                // Only consider dependencies that are part of the current scheduling task
                if !vertex_set.contains(&dependency) {
                    continue;
                }

                if !indices.contains_key(&dependency) {
                    // Successor dependency has not yet been visited; recurse on it
                    self.tarjan_visit_with_virtual(
                        dependency,
                        index_counter,
                        stack,
                        indices,
                        lowlinks,
                        on_stack,
                        sccs,
                        vertex_set,
                        vdeps,
                    )?;
                    let dep_lowlink = lowlinks[&dependency];
                    lowlinks.insert(vertex, lowlinks[&vertex].min(dep_lowlink));
                } else if on_stack.contains(&dependency) {
                    // Successor dependency is in stack and hence in the current SCC
                    let dep_index = indices[&dependency];
                    lowlinks.insert(vertex, lowlinks[&vertex].min(dep_index));
                }
            }
        }

        // If vertex is a root node, pop the stack and produce an SCC
        if lowlinks[&vertex] == indices[&vertex] {
            let mut scc = Vec::new();
            loop {
                let w = stack.pop().unwrap();
                on_stack.remove(&w);
                scc.push(w);
                if w == vertex {
                    break;
                }
            }
            sccs.push(scc);
        }

        Ok(())
    }

    pub(crate) fn separate_cycles(
        &self,
        sccs: Vec<Vec<VertexId>>,
    ) -> (Vec<Vec<VertexId>>, Vec<Vec<VertexId>>) {
        let mut cycles = Vec::new();
        let mut acyclic = Vec::new();

        for scc in sccs {
            if scc.len() > 1 || (scc.len() == 1 && self.has_self_loop(scc[0])) {
                cycles.push(scc);
            } else {
                acyclic.push(scc);
            }
        }

        (cycles, acyclic)
    }

    fn has_self_loop(&self, vertex: VertexId) -> bool {
        self.graph.has_self_loop(vertex)
    }

    pub(crate) fn build_layers(
        &self,
        acyclic_sccs: Vec<Vec<VertexId>>,
    ) -> Result<Vec<Layer>, ExcelError> {
        let vertices: Vec<VertexId> = acyclic_sccs.into_iter().flatten().collect();
        if vertices.is_empty() {
            return Ok(Vec::new());
        }
        let vertex_set: FxHashSet<VertexId> = vertices.iter().copied().collect();

        // Calculate in-degrees for all vertices in the acyclic subgraph
        let mut in_degrees: FxHashMap<VertexId, usize> = vertices.iter().map(|&v| (v, 0)).collect();
        for &vertex_id in &vertices {
            if let Some(dependencies) = self.graph.dependencies_slice(vertex_id) {
                for &dep_id in dependencies {
                    if vertex_set.contains(&dep_id)
                        && let Some(in_degree) = in_degrees.get_mut(&vertex_id)
                    {
                        *in_degree += 1;
                    }
                }
            } else {
                let dependencies = self.graph.get_dependencies(vertex_id);
                for dep_id in dependencies {
                    if vertex_set.contains(&dep_id)
                        && let Some(in_degree) = in_degrees.get_mut(&vertex_id)
                    {
                        *in_degree += 1;
                    }
                }
            }
        }

        // Initialize the queue with all nodes having an in-degree of 0
        let mut queue: std::collections::VecDeque<VertexId> = in_degrees
            .iter()
            .filter(|&(_, &in_degree)| in_degree == 0)
            .map(|(&v, _)| v)
            .collect();

        let mut layers = Vec::new();
        let mut processed_count = 0;

        while !queue.is_empty() {
            let mut current_layer_vertices = Vec::new();
            for _ in 0..queue.len() {
                let u = queue.pop_front().unwrap();
                current_layer_vertices.push(u);
                processed_count += 1;

                // For each dependent of u, reduce its in-degree
                if let Some(dependents) = self.graph.dependents_slice(u) {
                    for &v_dep in dependents {
                        if let Some(in_degree) = in_degrees.get_mut(&v_dep) {
                            *in_degree -= 1;
                            if *in_degree == 0 {
                                queue.push_back(v_dep);
                            }
                        }
                    }
                } else {
                    for v_dep in self.graph.get_dependents(u) {
                        if let Some(in_degree) = in_degrees.get_mut(&v_dep) {
                            *in_degree -= 1;
                            if *in_degree == 0 {
                                queue.push_back(v_dep);
                            }
                        }
                    }
                }
            }
            // Sort for deterministic output in tests
            current_layer_vertices.sort();
            layers.push(Layer {
                vertices: current_layer_vertices,
            });
        }

        if processed_count != vertices.len() {
            return Err(
                ExcelError::new(formualizer_common::ExcelErrorKind::Circ).with_message(
                    "Unexpected cycle detected in acyclic components during layer construction"
                        .to_string(),
                ),
            );
        }

        Ok(layers)
    }

    /// Kahn's algorithm over the condensation of the scheduled subgraph:
    /// each cyclic SCC is a super-node, each acyclic vertex a singleton node.
    ///
    /// Per Kahn wave we emit first the wave's `Cycle` units (ordered by
    /// smallest member `VertexId` for determinism), then one `Layer` unit with
    /// the wave's singleton vertices (sorted, as in `build_layers`). Within a
    /// wave there are no inter-node edges, so this ordering is semantically
    /// free. The result guarantees every `Cycle` unit appears after all units
    /// containing its external dependencies and before all units containing
    /// its external dependents (pinned by tests in
    /// `engine/tests/schedule_units.rs`).
    ///
    /// Returns the unit walk plus the `Layer` units in order (the
    /// compatibility `Schedule::layers` view).
    fn build_condensation_units(
        &self,
        cycles: &[Vec<VertexId>],
        acyclic_sccs: Vec<Vec<VertexId>>,
        vdeps: Option<&VirtualDeps>,
    ) -> Result<(Vec<ScheduleUnit>, Vec<Layer>), ExcelError> {
        let singletons: Vec<VertexId> = acyclic_sccs.into_iter().flatten().collect();
        let cycle_node_count = cycles.len();
        let node_count = cycle_node_count + singletons.len();

        // Map every scheduled vertex to its condensation node.
        let mut node_of: FxHashMap<VertexId, usize> = FxHashMap::default();
        for (i, cycle) in cycles.iter().enumerate() {
            for &v in cycle {
                node_of.insert(v, i);
            }
        }
        for (j, &v) in singletons.iter().enumerate() {
            node_of.insert(v, cycle_node_count + j);
        }

        // Build in-degrees and the dependents adjacency from a single scan of
        // the dependency direction, so both sides are guaranteed consistent.
        // Edges to vertices outside the scheduled set are ignored
        // (membership == presence in `node_of`); intra-node edges are ignored.
        let mut in_degrees: Vec<usize> = vec![0; node_count];
        let mut node_dependents: Vec<Vec<usize>> = vec![Vec::new(); node_count];
        let scan_dep = |from_node: usize,
                        dep: VertexId,
                        in_degrees: &mut Vec<usize>,
                        node_dependents: &mut Vec<Vec<usize>>| {
            if let Some(&dep_node) = node_of.get(&dep)
                && dep_node != from_node
            {
                in_degrees[from_node] += 1;
                node_dependents[dep_node].push(from_node);
            }
        };
        // Rectangle groups are pass-through nodes beside the condensation nodes: a group
        // resolves once every member's node has, and then releases every node depending on
        // it. The one case that cannot pass through is a dependent sharing a CYCLE node with
        // a member (the expanded edge would be intra-node and ignored, but waiting on the
        // group would wait on the dependent's own node): that dependent expands the group
        // into direct edges instead.
        let mut group_local: FxHashMap<u32, u32> = FxHashMap::default();
        let mut group_in: Vec<usize> = Vec::new();
        let mut group_dependents: Vec<Vec<usize>> = Vec::new();
        let mut group_cycle_nodes: Vec<FxHashSet<usize>> = Vec::new();
        let mut node_member_groups: Vec<Vec<u32>> = vec![Vec::new(); node_count];
        for (&v, &n_v) in node_of.iter() {
            if let Some(deps) = self.graph.dependencies_slice(v) {
                for &dep in deps {
                    scan_dep(n_v, dep, &mut in_degrees, &mut node_dependents);
                }
            } else {
                for dep in self.graph.get_dependencies(v) {
                    scan_dep(n_v, dep, &mut in_degrees, &mut node_dependents);
                }
            }
            let Some(vd) = vdeps else {
                continue;
            };
            for &dep in vd.owned_of(v) {
                scan_dep(n_v, dep, &mut in_degrees, &mut node_dependents);
            }
            for &g in vd.groups_of(v) {
                let gl = match group_local.get(&g) {
                    Some(&gl) => gl,
                    None => {
                        let mut members = 0usize;
                        let mut cycle_nodes = FxHashSet::default();
                        let gl = group_in.len() as u32;
                        for m in vd.group_members(g) {
                            let Some(&mn) = node_of.get(m) else {
                                continue;
                            };
                            members += 1;
                            node_member_groups[mn].push(gl);
                            if mn < cycle_node_count {
                                cycle_nodes.insert(mn);
                            }
                        }
                        if members == 0 {
                            // No scheduled member: nothing to wait for.
                            group_local.insert(g, u32::MAX);
                            continue;
                        }
                        group_in.push(members);
                        group_dependents.push(Vec::new());
                        group_cycle_nodes.push(cycle_nodes);
                        group_local.insert(g, gl);
                        gl
                    }
                };
                if gl == u32::MAX {
                    continue;
                }
                if n_v < cycle_node_count && group_cycle_nodes[gl as usize].contains(&n_v) {
                    for &dep in vd.group_members(g) {
                        scan_dep(n_v, dep, &mut in_degrees, &mut node_dependents);
                    }
                } else {
                    in_degrees[n_v] += 1;
                    group_dependents[gl as usize].push(n_v);
                }
            }
        }

        // Kahn by waves over condensation nodes.
        let mut current: Vec<usize> = (0..node_count).filter(|&n| in_degrees[n] == 0).collect();
        let mut units = Vec::new();
        let mut layers = Vec::new();
        let mut processed_count = 0usize;
        while !current.is_empty() {
            let mut wave_cycles: Vec<usize> = current
                .iter()
                .copied()
                .filter(|&n| n < cycle_node_count)
                .collect();
            wave_cycles.sort_by_key(|&n| cycles[n].iter().copied().min());
            for n in wave_cycles {
                units.push(ScheduleUnit::Cycle(n as u32));
            }

            let mut wave_vertices: Vec<VertexId> = current
                .iter()
                .copied()
                .filter(|&n| n >= cycle_node_count)
                .map(|n| singletons[n - cycle_node_count])
                .collect();
            if !wave_vertices.is_empty() {
                // Sort for deterministic output, as in build_layers.
                wave_vertices.sort();
                units.push(ScheduleUnit::Layer(layers.len() as u32));
                layers.push(Layer {
                    vertices: wave_vertices,
                });
            }

            processed_count += current.len();
            let mut next = Vec::new();
            for &n in &current {
                for &dependent in &node_dependents[n] {
                    in_degrees[dependent] -= 1;
                    if in_degrees[dependent] == 0 {
                        next.push(dependent);
                    }
                }
                for &gl in &node_member_groups[n] {
                    group_in[gl as usize] -= 1;
                    if group_in[gl as usize] == 0 {
                        for &dependent in &group_dependents[gl as usize] {
                            in_degrees[dependent] -= 1;
                            if in_degrees[dependent] == 0 {
                                next.push(dependent);
                            }
                        }
                    }
                }
            }
            current = next;
        }

        if processed_count != node_count {
            return Err(
                ExcelError::new(formualizer_common::ExcelErrorKind::Circ).with_message(
                    "Unexpected cycle detected in condensation during unit construction"
                        .to_string(),
                ),
            );
        }

        Ok((units, layers))
    }

    pub(crate) fn build_layers_with_virtual(
        &self,
        acyclic_sccs: Vec<Vec<VertexId>>,
        vdeps: &VirtualDeps,
    ) -> Result<Vec<Layer>, ExcelError> {
        use std::collections::VecDeque;
        let vertices: Vec<VertexId> = acyclic_sccs.into_iter().flatten().collect();
        if vertices.is_empty() {
            return Ok(Vec::new());
        }
        let n = vertices.len();
        let mut local_of: FxHashMap<VertexId, u32> =
            FxHashMap::with_capacity_and_hasher(n, Default::default());
        for (i, &v) in vertices.iter().enumerate() {
            local_of.insert(v, i as u32);
        }

        // Kahn over the vertices plus one pass-through node per rectangle group with a
        // scheduled member: a vertex waits on each distinct direct dependency (graph edge or
        // owned extra) and on each of its groups; a group waits on its scheduled members and
        // then releases its dependents within the same wave — so a dependent becomes ready in
        // the wave after its last dependency resolves, exactly as with the expanded edges.
        let mut in_degrees: Vec<u32> = vec![0; n];
        let mut out: Vec<Vec<u32>> = vec![Vec::new(); n];
        let mut member_groups: Vec<Vec<u32>> = vec![Vec::new(); n];
        let mut group_local: FxHashMap<u32, u32> = FxHashMap::default();
        let mut group_in: Vec<u32> = Vec::new();
        let mut group_dependents: Vec<Vec<u32>> = Vec::new();
        let mut direct: Vec<u32> = Vec::new();
        for (i, &v) in vertices.iter().enumerate() {
            direct.clear();
            if let Some(base) = self.graph.dependencies_slice(v) {
                direct.extend(base.iter().filter_map(|d| local_of.get(d).copied()));
            } else {
                direct.extend(
                    self.graph
                        .get_dependencies(v)
                        .iter()
                        .filter_map(|d| local_of.get(d).copied()),
                );
            }
            direct.extend(
                vdeps
                    .owned_of(v)
                    .iter()
                    .filter_map(|d| local_of.get(d).copied()),
            );
            direct.sort_unstable();
            direct.dedup();
            for &d in &direct {
                out[d as usize].push(i as u32);
            }
            let mut degree = direct.len() as u32;
            for &g in vdeps.groups_of(v) {
                let gl = match group_local.get(&g) {
                    Some(&gl) => gl,
                    None => {
                        let members: Vec<u32> = vdeps
                            .group_members(g)
                            .iter()
                            .filter_map(|m| local_of.get(m).copied())
                            .collect();
                        let gl = if members.is_empty() {
                            u32::MAX
                        } else {
                            let gl = group_in.len() as u32;
                            for &m in &members {
                                member_groups[m as usize].push(gl);
                            }
                            group_in.push(members.len() as u32);
                            group_dependents.push(Vec::new());
                            gl
                        };
                        group_local.insert(g, gl);
                        gl
                    }
                };
                if gl == u32::MAX {
                    continue;
                }
                group_dependents[gl as usize].push(i as u32);
                degree += 1;
            }
            in_degrees[i] = degree;
        }

        let mut queue: VecDeque<u32> = (0..n as u32)
            .filter(|&i| in_degrees[i as usize] == 0)
            .collect();
        let mut layers = Vec::new();
        let mut processed_count = 0;
        while !queue.is_empty() {
            let mut cur = Vec::new();
            for _ in 0..queue.len() {
                let u = queue.pop_front().unwrap();
                cur.push(vertices[u as usize]);
                processed_count += 1;
                for &w in &out[u as usize] {
                    let ind = &mut in_degrees[w as usize];
                    *ind = ind.saturating_sub(1);
                    if *ind == 0 {
                        queue.push_back(w);
                    }
                }
                for &gl in &member_groups[u as usize] {
                    let pending = &mut group_in[gl as usize];
                    *pending = pending.saturating_sub(1);
                    if *pending == 0 {
                        for &w in &group_dependents[gl as usize] {
                            let ind = &mut in_degrees[w as usize];
                            *ind = ind.saturating_sub(1);
                            if *ind == 0 {
                                queue.push_back(w);
                            }
                        }
                    }
                }
            }
            cur.sort_unstable();
            layers.push(Layer { vertices: cur });
        }
        if processed_count != vertices.len() {
            return Err(
                ExcelError::new(formualizer_common::ExcelErrorKind::Circ).with_message(
                    "Unexpected cycle detected in acyclic components during layer construction (virtual)"
                        .to_string(),
                ),
            );
        }
        Ok(layers)
    }
}
