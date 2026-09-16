//! Excel parity for reference syntax and name resolution (rowsncolumns/spreadsheet#546, unit U18):
//! the union `,` and intersection ` ` operators (B-12/E-14), the spilled-range operator `A1#`
//! (E-15), `INDIRECT(text, FALSE)` R1C1 (A-12/E-17), reversed range corners (E-18), named
//! constants and named formulas (E-19), sheet-scoped names and the `Sheet!Name` qualifier (E-20),
//! and a bare table name as its data body (E-21).

use formualizer_common::{ExcelErrorKind, LiteralValue};
use formualizer_eval::engine::named_range::{NameScope, NamedDefinition};
use formualizer_eval::engine::{Engine, EvalConfig};
use formualizer_eval::reference::{CellRef, Coord, RangeRef};
use formualizer_eval::test_workbook::TestWorkbook;
use formualizer_parse::parser::parse;

fn n(v: f64) -> LiteralValue {
    LiteralValue::Number(v)
}

/// Sheet1: A1:C3 = [[1,10,100],[2,20,200],[3,30,300]]; Sheet2!A1 = 7, Sheet2!B1 = 42.
fn seeded() -> Engine<TestWorkbook> {
    let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
    for r in 1..=3u32 {
        e.set_cell_value("Sheet1", r, 1, n(r as f64)).unwrap();
        e.set_cell_value("Sheet1", r, 2, n(r as f64 * 10.0))
            .unwrap();
        e.set_cell_value("Sheet1", r, 3, n(r as f64 * 100.0))
            .unwrap();
    }
    e.add_sheet("Sheet2").unwrap();
    e.set_cell_value("Sheet2", 1, 1, n(7.0)).unwrap();
    e.set_cell_value("Sheet2", 1, 2, n(42.0)).unwrap();
    e
}

/// Evaluate `formula` at Sheet1!E10 and read the result back.
fn eval(e: &mut Engine<TestWorkbook>, formula: &str) -> LiteralValue {
    eval_at(e, "Sheet1", 10, 5, formula)
}

fn eval_at(
    e: &mut Engine<TestWorkbook>,
    sheet: &str,
    row: u32,
    col: u32,
    formula: &str,
) -> LiteralValue {
    e.set_cell_formula(sheet, row, col, parse(formula).unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    e.get_cell_value(sheet, row, col)
        .unwrap_or(LiteralValue::Empty)
}

fn num(v: &LiteralValue) -> f64 {
    match v {
        LiteralValue::Number(x) => *x,
        LiteralValue::Int(i) => *i as f64,
        other => panic!("expected a number, got {other:?}"),
    }
}

fn err_kind(v: &LiteralValue) -> ExcelErrorKind {
    match v {
        LiteralValue::Error(e) => e.kind,
        other => panic!("expected an error, got {other:?}"),
    }
}

fn range(sheet: formualizer_common::SheetId, r1: u32, c1: u32, r2: u32, c2: u32) -> RangeRef {
    RangeRef::new(
        CellRef::new(sheet, Coord::from_excel(r1, c1, true, true)),
        CellRef::new(sheet, Coord::from_excel(r2, c2, true, true)),
    )
}

// ───────────────────────── union `,` and intersection ` ` (B-12 / E-14) ─────────────────────────

#[test]
fn union_inside_parentheses_feeds_every_area_to_the_aggregate() {
    let mut e = seeded();
    assert_eq!(num(&eval(&mut e, "=SUM((A1,B1))")), 11.0);
    assert_eq!(num(&eval(&mut e, "=SUM((A1:A2,B1:B2))")), 33.0);
    // Areas of different shapes still contribute every cell.
    assert_eq!(
        num(&eval(&mut e, "=SUM((A1:A3,B1:C1))")),
        6.0 + 10.0 + 100.0
    );
    assert_eq!(num(&eval(&mut e, "=COUNT((A1:A3,C1:C2))")), 5.0);
    assert_eq!(num(&eval(&mut e, "=MAX((A1:A3,B1:B3))")), 30.0);
    // A union of a range and a named range.
    let sheet = e.sheet_id("Sheet1").unwrap();
    e.define_name(
        "Data",
        NamedDefinition::Range(range(sheet, 1, 3, 3, 3)),
        NameScope::Workbook,
    )
    .unwrap();
    assert_eq!(num(&eval(&mut e, "=SUM((A1:A3,Data))")), 606.0);
}

#[test]
fn intersection_is_the_shared_rectangle() {
    let mut e = seeded();
    // A1:B2 ∩ B1:C3 = B1:B2
    assert_eq!(num(&eval(&mut e, "=SUM(A1:B2 B1:C3)")), 30.0);
    // Full column ∩ full row = one cell.
    assert_eq!(num(&eval(&mut e, "=SUM(B:B 2:2)")), 20.0);
    assert_eq!(num(&eval(&mut e, "=B:B 3:3")), 30.0);
    // A single-cell intersection used as a value.
    assert_eq!(num(&eval(&mut e, "=A1:C3 B2:B2")), 20.0);
    // Intersection with a named range.
    let sheet = e.sheet_id("Sheet1").unwrap();
    e.define_name(
        "Rows12",
        NamedDefinition::Range(range(sheet, 1, 1, 2, 3)),
        NameScope::Workbook,
    )
    .unwrap();
    assert_eq!(num(&eval(&mut e, "=SUM(Rows12 C:C)")), 300.0);
}

#[test]
fn disjoint_intersection_is_null_error() {
    let mut e = seeded();
    assert_eq!(
        err_kind(&eval(&mut e, "=SUM(A1:B1 C1:D1)")),
        ExcelErrorKind::Null
    );
    assert_eq!(
        err_kind(&eval(&mut e, "=A1:A3 B1:B3")),
        ExcelErrorKind::Null
    );
    // Different sheets never intersect.
    assert_eq!(
        err_kind(&eval(&mut e, "=SUM(Sheet1!A1:C3 Sheet2!A1:C3)")),
        ExcelErrorKind::Null
    );
}

#[test]
fn intersection_of_a_non_reference_is_value_error() {
    let mut e = seeded();
    assert_eq!(
        err_kind(&eval(&mut e, "=SUM({1,2} A1:B2)")),
        ExcelErrorKind::Value
    );
}

// ───────────────────────────── spilled-range operator `A1#` (E-15) ─────────────────────────────

#[test]
fn spill_operator_covers_the_whole_dynamic_array() {
    let mut e = seeded();
    eval_at(&mut e, "Sheet1", 1, 8, "=SEQUENCE(3)");
    assert_eq!(num(&eval(&mut e, "=SUM(H1#)")), 6.0);
    assert_eq!(num(&eval(&mut e, "=ROWS(H1#)")), 3.0);
    assert_eq!(num(&eval(&mut e, "=COUNT(Sheet1!H1#)")), 3.0);
    // Arithmetic over the spill lifts element-wise and spills itself.
    eval_at(&mut e, "Sheet1", 1, 10, "=H1#*10");
    assert_eq!(num(&e.get_cell_value("Sheet1", 3, 10).unwrap()), 30.0);
    // The spill growing re-feeds the dependent.
    eval_at(&mut e, "Sheet1", 1, 8, "=SEQUENCE(4)");
    assert_eq!(num(&eval(&mut e, "=SUM(H1#)")), 10.0);
}

#[test]
fn spill_operator_on_a_non_spilling_formula_is_the_cell_itself() {
    let mut e = seeded();
    eval_at(&mut e, "Sheet1", 1, 8, "=A1+A2");
    assert_eq!(num(&eval(&mut e, "=SUM(H1#)")), 3.0);
}

#[test]
fn spill_operator_on_a_plain_value_is_ref_error() {
    let mut e = seeded();
    assert_eq!(err_kind(&eval(&mut e, "=SUM(A1#)")), ExcelErrorKind::Ref);
    assert_eq!(err_kind(&eval(&mut e, "=A1#")), ExcelErrorKind::Ref);
}

// ───────────────────────────── INDIRECT(text, FALSE) R1C1 (A-12 / E-17) ─────────────────────────

#[test]
fn indirect_r1c1_absolute_and_ranges() {
    let mut e = seeded();
    assert_eq!(num(&eval(&mut e, r#"=INDIRECT("R2C1",FALSE)"#)), 2.0);
    assert_eq!(num(&eval(&mut e, r#"=INDIRECT("r3c2",FALSE)"#)), 30.0);
    assert_eq!(
        num(&eval(&mut e, r#"=SUM(INDIRECT("R1C1:R3C1",FALSE))"#)),
        6.0
    );
    // Reversed corners normalise like A1 ranges.
    assert_eq!(
        num(&eval(&mut e, r#"=SUM(INDIRECT("R3C2:R1C1",FALSE))"#)),
        66.0
    );
    assert_eq!(
        num(&eval(&mut e, r#"=INDIRECT("Sheet2!R1C2",FALSE)"#)),
        42.0
    );
    assert_eq!(num(&eval(&mut e, r#"=SUM(INDIRECT("R2",FALSE))"#)), 222.0);
    assert_eq!(num(&eval(&mut e, r#"=SUM(INDIRECT("C3",FALSE))"#)), 600.0);
}

#[test]
fn indirect_r1c1_relative_parts_offset_from_the_formula_cell() {
    let mut e = seeded();
    // At E10: R[-8]C[-4] = A2.
    assert_eq!(num(&eval(&mut e, r#"=INDIRECT("R[-8]C[-4]",FALSE)"#)), 2.0);
    // Bare `R` / `C` = the formula's own row / column: at A4, RC[1] is B4... use row 4 col 1.
    e.set_cell_value("Sheet1", 4, 2, n(99.0)).unwrap();
    assert_eq!(
        num(&eval_at(
            &mut e,
            "Sheet1",
            4,
            1,
            r#"=INDIRECT("RC[1]",FALSE)"#
        )),
        99.0
    );
}

#[test]
fn indirect_r1c1_malformed_or_out_of_sheet_is_ref_error() {
    let mut e = seeded();
    assert_eq!(
        err_kind(&eval(&mut e, r#"=INDIRECT("R0C1",FALSE)"#)),
        ExcelErrorKind::Ref
    );
    assert_eq!(
        err_kind(&eval(&mut e, r#"=INDIRECT("R[-20]C1",FALSE)"#)),
        ExcelErrorKind::Ref
    );
    assert_eq!(
        err_kind(&eval(&mut e, r#"=INDIRECT("R2CX",FALSE)"#)),
        ExcelErrorKind::Ref
    );
    // A1 text under the R1C1 flag is not a reference.
    assert_eq!(
        err_kind(&eval(&mut e, r#"=INDIRECT("A1",FALSE)"#)),
        ExcelErrorKind::Ref
    );
}

#[test]
fn indirect_false_still_resolves_defined_names() {
    let mut e = seeded();
    let sheet = e.sheet_id("Sheet1").unwrap();
    e.define_name(
        "Data",
        NamedDefinition::Range(range(sheet, 1, 1, 3, 1)),
        NameScope::Workbook,
    )
    .unwrap();
    assert_eq!(num(&eval(&mut e, r#"=SUM(INDIRECT("Data",FALSE))"#)), 6.0);
}

// ───────────────────────────────── reversed range corners (E-18) ────────────────────────────────

#[test]
fn reversed_corners_evaluate_like_the_normalised_range() {
    let mut e = seeded();
    assert_eq!(num(&eval(&mut e, "=SUM(B2:A1)")), 33.0);
    assert_eq!(num(&eval(&mut e, "=SUM(A3:A1)")), 6.0);
    assert_eq!(num(&eval(&mut e, "=ROWS(C3:A1)")), 3.0);
}

// ───────────────────────── named constants and named formulas (E-19) ────────────────────────────

#[test]
fn named_constant_and_named_formula_evaluate() {
    let mut e = seeded();
    e.define_name(
        "MyRate",
        NamedDefinition::Literal(n(0.1)),
        NameScope::Workbook,
    )
    .unwrap();
    e.define_name(
        "Total",
        NamedDefinition::Formula {
            ast: parse("=SUM(Sheet1!$A$1:$A$3)*2").unwrap(),
            dependencies: Vec::new(),
            range_deps: Vec::new(),
        },
        NameScope::Workbook,
    )
    .unwrap();
    assert!((num(&eval(&mut e, "=MyRate*2")) - 0.2).abs() < 1e-12);
    assert_eq!(num(&eval(&mut e, "=Total")), 12.0);
    // The named formula tracks its precedents.
    e.set_cell_value("Sheet1", 1, 1, n(11.0)).unwrap();
    e.evaluate_all().unwrap();
    assert_eq!(num(&e.get_cell_value("Sheet1", 10, 5).unwrap()), 32.0);
}

// ───────────────────── sheet-scoped names and the `Sheet!Name` qualifier (E-20) ──────────────────

#[test]
fn sheet_scoped_name_is_invisible_from_other_sheets_unless_qualified() {
    let mut e = seeded();
    let sheet2 = e.sheet_id("Sheet2").unwrap();
    e.define_name(
        "Local",
        NamedDefinition::Cell(CellRef::new(sheet2, Coord::from_excel(1, 2, true, true))),
        NameScope::Sheet(sheet2),
    )
    .unwrap();
    assert_eq!(num(&eval_at(&mut e, "Sheet2", 2, 1, "=Local")), 42.0);
    assert_eq!(err_kind(&eval(&mut e, "=Local")), ExcelErrorKind::Name);
    assert_eq!(num(&eval(&mut e, "=Sheet2!Local")), 42.0);
    assert_eq!(num(&eval(&mut e, "=Sheet2!Local*2")), 84.0);
}

#[test]
fn quoted_sheet_qualifier_and_qualified_workbook_name() {
    let mut e = seeded();
    e.add_sheet("My Sheet").unwrap();
    e.set_cell_value("My Sheet", 1, 1, n(5.0)).unwrap();
    let my_sheet = e.sheet_id("My Sheet").unwrap();
    e.define_name(
        "Local",
        NamedDefinition::Cell(CellRef::new(my_sheet, Coord::from_excel(1, 1, true, true))),
        NameScope::Sheet(my_sheet),
    )
    .unwrap();
    e.define_name(
        "Global",
        NamedDefinition::Literal(n(3.0)),
        NameScope::Workbook,
    )
    .unwrap();
    assert_eq!(num(&eval(&mut e, "='My Sheet'!Local")), 5.0);
    // Excel accepts a sheet qualifier in front of a workbook-level name.
    assert_eq!(num(&eval(&mut e, "=Sheet2!Global")), 3.0);
    assert_eq!(
        err_kind(&eval(&mut e, "=Sheet2!Nope")),
        ExcelErrorKind::Name
    );
}

#[test]
fn sheet_scoped_name_shadows_the_workbook_name_on_its_sheet() {
    let mut e = seeded();
    let sheet2 = e.sheet_id("Sheet2").unwrap();
    e.define_name(
        "Rate",
        NamedDefinition::Literal(n(1.0)),
        NameScope::Workbook,
    )
    .unwrap();
    e.define_name(
        "Rate",
        NamedDefinition::Literal(n(2.0)),
        NameScope::Sheet(sheet2),
    )
    .unwrap();
    assert_eq!(num(&eval(&mut e, "=Rate")), 1.0);
    assert_eq!(num(&eval_at(&mut e, "Sheet2", 2, 1, "=Rate")), 2.0);
    assert_eq!(num(&eval(&mut e, "=Sheet2!Rate")), 2.0);
    assert_eq!(num(&eval(&mut e, "=Sheet1!Rate")), 1.0);
}

// ─────────────────────────────── bare table name as a range (E-21) ──────────────────────────────

#[test]
fn bare_table_name_is_the_data_body() {
    let mut e = seeded();
    // Table1 over A1:B4 with a header row: headers at row 1, data rows 2..4.
    e.set_cell_value("Sheet1", 1, 1, LiteralValue::Text("Col1".into()))
        .unwrap();
    e.set_cell_value("Sheet1", 1, 2, LiteralValue::Text("Col 2".into()))
        .unwrap();
    e.set_cell_value("Sheet1", 4, 1, n(4.0)).unwrap();
    e.set_cell_value("Sheet1", 4, 2, n(40.0)).unwrap();
    let sheet = e.sheet_id("Sheet1").unwrap();
    e.define_table(
        "Table1",
        range(sheet, 1, 1, 4, 2),
        true,
        vec!["Col1".into(), "Col 2".into()],
        false,
    )
    .unwrap();
    assert_eq!(num(&eval(&mut e, "=ROWS(Table1)")), 3.0);
    assert_eq!(num(&eval(&mut e, "=COLUMNS(Table1)")), 2.0);
    assert_eq!(
        num(&eval(&mut e, "=SUM(Table1)")),
        2.0 + 3.0 + 4.0 + 20.0 + 30.0 + 40.0
    );
    assert_eq!(
        num(&eval(&mut e, "=SUM(Table1[#Data])")),
        num(&eval(&mut e, "=SUM(Table1)"))
    );
    assert_eq!(num(&eval(&mut e, "=INDEX(Table1,2,2)")), 30.0);
    // Edits inside the body reach the dependent.
    e.set_cell_value("Sheet1", 4, 2, n(400.0)).unwrap();
    e.evaluate_all().unwrap();
    assert_eq!(
        num(&eval(&mut e, "=SUM(Table1)")),
        2.0 + 3.0 + 4.0 + 20.0 + 30.0 + 400.0
    );
}
