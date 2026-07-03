//! Structured (table) reference lowering shared by formula ingest, the
//! interpreter, and the engine's range resolution.
//!
//! The parser accepts the full MS-XLSX §18.17.6.2 grammar; this module turns
//! the row/column specifiers that need table geometry — and, for `#This Row`
//! / `@`, the evaluating cell — into concrete rectangles. Plain forms the
//! resolution paths already implement (`Table[Col]`, `Table[#Data]`,
//! `Table[A]:[B]`, …) are untouched; see [`needs_lowering`].

use formualizer_common::{ExcelError, ExcelErrorKind};
use formualizer_parse::parser::{ReferenceType, SpecialItem, TableRowSpecifier, TableSpecifier};

/// 1-based, inclusive geometry of a defined workbook table.
#[derive(Debug, Clone)]
pub struct TableGeometry {
    /// Sheet the table lives on, when the producing context knows it by name.
    pub sheet: Option<String>,
    pub start_row: u32,
    pub start_col: u32,
    pub end_row: u32,
    pub end_col: u32,
    pub header_row: bool,
    pub totals_row: bool,
    /// Column names in header order.
    pub columns: Vec<String>,
}

/// 1-based, inclusive rectangle produced by lowering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub start_row: u32,
    pub start_col: u32,
    pub end_row: u32,
    pub end_col: u32,
}

impl TableGeometry {
    pub fn data_start_row(&self) -> u32 {
        if self.header_row {
            self.start_row + 1
        } else {
            self.start_row
        }
    }

    pub fn data_end_row(&self) -> u32 {
        if self.totals_row {
            self.end_row.saturating_sub(1)
        } else {
            self.end_row
        }
    }

    pub fn col_index(&self, name: &str) -> Option<u32> {
        let key = name.to_lowercase();
        self.columns
            .iter()
            .position(|c| c.to_lowercase() == key)
            .map(|i| i as u32)
    }
}

/// Does this specifier need the lowering implemented here (vs the plain
/// column/special arms the resolution paths already implement)?
pub fn needs_lowering(spec: Option<&TableSpecifier>) -> bool {
    matches!(
        spec,
        Some(TableSpecifier::SpecialItem(SpecialItem::ThisRow))
            | Some(TableSpecifier::Row(_))
            | Some(TableSpecifier::Combination(_))
    )
}

/// Does this specifier select the evaluating cell's row (`@` / `#This Row`)?
pub fn involves_this_row(spec: Option<&TableSpecifier>) -> bool {
    fn walk(s: &TableSpecifier) -> bool {
        match s {
            TableSpecifier::SpecialItem(SpecialItem::ThisRow) => true,
            TableSpecifier::Row(TableRowSpecifier::Current) => true,
            TableSpecifier::Combination(parts) => parts.iter().any(|p| walk(p)),
            _ => false,
        }
    }
    spec.is_some_and(walk)
}

#[derive(Default)]
struct Selection {
    this_row: bool,
    all: bool,
    data: bool,
    headers: bool,
    totals: bool,
    row_index: Option<u32>,
    /// 0-based column offsets within the table, inclusive.
    col_intervals: Vec<(u32, u32)>,
}

fn unknown_column(name: &str) -> ExcelError {
    ExcelError::new(ExcelErrorKind::Ref).with_message(format!(
        "Unknown table column in structured reference: {name}"
    ))
}

fn analyze(
    geom: &TableGeometry,
    spec: &TableSpecifier,
    out: &mut Selection,
) -> Result<(), ExcelError> {
    match spec {
        TableSpecifier::SpecialItem(SpecialItem::ThisRow) => out.this_row = true,
        TableSpecifier::SpecialItem(SpecialItem::All) | TableSpecifier::All => out.all = true,
        TableSpecifier::SpecialItem(SpecialItem::Data) | TableSpecifier::Data => out.data = true,
        TableSpecifier::SpecialItem(SpecialItem::Headers) | TableSpecifier::Headers => {
            out.headers = true
        }
        TableSpecifier::SpecialItem(SpecialItem::Totals) | TableSpecifier::Totals => {
            out.totals = true
        }
        TableSpecifier::Row(rs) => match rs {
            TableRowSpecifier::Current => out.this_row = true,
            TableRowSpecifier::All => out.all = true,
            TableRowSpecifier::Data => out.data = true,
            TableRowSpecifier::Headers => out.headers = true,
            TableRowSpecifier::Totals => out.totals = true,
            TableRowSpecifier::Index(n) => out.row_index = Some(*n),
        },
        TableSpecifier::Column(c) => {
            let i = geom.col_index(c).ok_or_else(|| unknown_column(c))?;
            out.col_intervals.push((i, i));
        }
        TableSpecifier::ColumnRange(a, b) => {
            let ia = geom.col_index(a).ok_or_else(|| unknown_column(a))?;
            let ib = geom.col_index(b).ok_or_else(|| unknown_column(b))?;
            out.col_intervals.push((ia.min(ib), ia.max(ib)));
        }
        TableSpecifier::Combination(parts) => {
            for p in parts {
                analyze(geom, p, out)?;
            }
        }
    }
    Ok(())
}

/// Lower a specifier into a concrete rectangle. `current_row` is the 1-based
/// row of the evaluating cell, when the caller has one; it is only required
/// for `#This Row` / `@` forms.
pub fn lower_structured_rect(
    geom: &TableGeometry,
    spec: Option<&TableSpecifier>,
    current_row: Option<u32>,
) -> Result<Rect, ExcelError> {
    let spec = spec.ok_or_else(|| {
        ExcelError::new(ExcelErrorKind::Ref)
            .with_message("Table reference without specifier is unsupported".to_string())
    })?;
    let mut sel = Selection::default();
    analyze(geom, spec, &mut sel)?;

    // Column span: union of the named columns/ranges, which must be adjacent.
    let (start_col, end_col) = if sel.col_intervals.is_empty() {
        (geom.start_col, geom.end_col)
    } else {
        sel.col_intervals.sort_unstable();
        let (first, mut last) = sel.col_intervals[0];
        for &(s, e) in &sel.col_intervals[1..] {
            if s > last + 1 {
                return Err(ExcelError::new(ExcelErrorKind::Value).with_message(
                    "Non-adjacent columns in a structured reference are not supported".to_string(),
                ));
            }
            last = last.max(e);
        }
        (geom.start_col + first, geom.start_col + last)
    };

    // Row band.
    let (start_row, end_row) = if sel.this_row {
        if sel.all || sel.data || sel.headers || sel.totals || sel.row_index.is_some() {
            return Err(ExcelError::new(ExcelErrorKind::Value).with_message(
                "#This Row cannot be combined with other row specifiers".to_string(),
            ));
        }
        let r = current_row.ok_or_else(|| {
            ExcelError::new(ExcelErrorKind::NImpl)
                .with_message("@ (This Row) requires a cell evaluation context".to_string())
        })?;
        if geom.header_row && r == geom.start_row {
            return Err(ExcelError::new(ExcelErrorKind::Ref).with_message(
                "This-row structured references are not valid in the table header row".to_string(),
            ));
        }
        if r < geom.data_start_row() || r > geom.end_row {
            return Err(ExcelError::new(ExcelErrorKind::Value)
                .with_message("@ (This Row) used outside the table's rows".to_string()));
        }
        (r, r)
    } else if let Some(n) = sel.row_index {
        if n == 0 {
            return Err(ExcelError::new(ExcelErrorKind::Ref)
                .with_message("Structured reference row index is 1-based".to_string()));
        }
        let r = geom.data_start_row() + (n - 1);
        if r > geom.data_end_row() {
            return Err(ExcelError::new(ExcelErrorKind::Ref).with_message(
                "Structured reference row index is outside the table's data rows".to_string(),
            ));
        }
        (r, r)
    } else if sel.all {
        (geom.start_row, geom.end_row)
    } else {
        let mut bands: Vec<(u32, u32)> = Vec::new();
        if sel.headers {
            if !geom.header_row {
                return Err(ExcelError::new(ExcelErrorKind::Ref)
                    .with_message("Table has no header row".to_string()));
            }
            bands.push((geom.start_row, geom.start_row));
        }
        if sel.data || (!sel.headers && !sel.totals) {
            let (ds, de) = (geom.data_start_row(), geom.data_end_row());
            if ds > de {
                return Err(ExcelError::new(ExcelErrorKind::Ref)
                    .with_message("Table has no data rows".to_string()));
            }
            bands.push((ds, de));
        }
        if sel.totals {
            if !geom.totals_row {
                return Err(ExcelError::new(ExcelErrorKind::Ref)
                    .with_message("Table has no totals row".to_string()));
            }
            bands.push((geom.end_row, geom.end_row));
        }
        bands.sort_unstable();
        let (first, mut last) = bands[0];
        for &(bs, be) in &bands[1..] {
            if bs > last + 1 {
                return Err(ExcelError::new(ExcelErrorKind::Ref).with_message(
                    "Non-adjacent row areas in a structured reference are not supported"
                        .to_string(),
                ));
            }
            last = last.max(be);
        }
        (first, last)
    };

    Ok(Rect {
        start_row,
        start_col,
        end_row,
        end_col,
    })
}

/// Turn a lowered rectangle into a concrete reference (a `Cell` when 1x1).
pub fn rect_to_reference(rect: &Rect, sheet: Option<String>) -> ReferenceType {
    if rect.start_row == rect.end_row && rect.start_col == rect.end_col {
        ReferenceType::Cell {
            sheet,
            row: rect.start_row,
            col: rect.start_col,
            row_abs: true,
            col_abs: true,
        }
    } else {
        ReferenceType::Range {
            sheet,
            start_row: Some(rect.start_row),
            start_col: Some(rect.start_col),
            end_row: Some(rect.end_row),
            end_col: Some(rect.end_col),
            start_row_abs: true,
            start_col_abs: true,
            end_row_abs: true,
            end_col_abs: true,
        }
    }
}

/// Plan the ingest-time rewrite of a this-row reference into a concrete
/// cell/range reference anchored at the evaluating cell's row. The caller has
/// already resolved the table and verified it lives on the evaluating cell's
/// sheet; `cell_row` is that cell's 1-based row.
pub fn plan_this_row_rewrite(
    geom: &TableGeometry,
    spec: Option<&TableSpecifier>,
    cell_row: u32,
) -> Result<ReferenceType, ExcelError> {
    let rect = lower_structured_rect(geom, spec, Some(cell_row))?;
    Ok(rect_to_reference(&rect, None))
}
