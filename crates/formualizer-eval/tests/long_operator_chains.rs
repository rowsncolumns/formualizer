//! Long operator chains evaluate, edit and print without recursing once per term
//! (Athena-Intel/demo-app-monorepo#49969 FX-02).
//!
//! Every infix operator is left-associative, so `=A1+A1+…+A1` is a tree whose left spine is as deep
//! as the formula is long. Excel accepts formulas up to 8,192 characters — `=1+1+…` at that length
//! is 4,096 terms — and every pass that recursed into `left` (ingest metadata, canonicalization,
//! dependency extraction, arena interning and reconstruction, the interpreter, the reference
//! adjuster, the pretty-printer, clone / eq / drop) ran out of stack long before that inside wasm.
//! The passes now walk a chain's spine with a loop.
//!
//! These tests use 20,000-term chains on a thread with a 4 MiB stack: any pass that still recursed
//! once per term would overflow it (a debug frame is hundreds of bytes), so a passing run means no
//! pass on these paths does.

use formualizer_common::LiteralValue;
use formualizer_eval::engine::{Engine, EvalConfig};
use formualizer_eval::test_workbook::TestWorkbook;
use formualizer_parse::parser::parse;
use formualizer_parse::pretty::excel_formula;

const TERMS: usize = 20_000;

fn on_small_stack(f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(4 * 1024 * 1024)
        .spawn(f)
        .expect("spawn")
        .join()
        .expect("test thread panicked (or overflowed its stack)");
}

fn chain(term: &str, op: &str, n: usize) -> String {
    format!("={}", vec![term; n].join(op))
}

fn engine() -> Engine<TestWorkbook> {
    let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
    e.add_sheet("Sheet2").unwrap();
    e
}

fn formula_text(e: &Engine<TestWorkbook>, sheet: &str, row: u32, col: u32) -> String {
    let (ast, _) = e.get_cell(sheet, row, col).expect("cell");
    excel_formula(&ast.expect("formula cell"))
}

fn value(e: &Engine<TestWorkbook>, sheet: &str, row: u32, col: u32) -> LiteralValue {
    e.get_cell_value(sheet, row, col)
        .unwrap_or(LiteralValue::Empty)
}

/// Set, evaluate, edit a precedent, insert a row above (every reference shifts), rename the sheet
/// the chain reads (every reference is rewritten) and print the formula back — for `+`, `-`, `*`,
/// `&` and comparison chains.
#[test]
fn twenty_thousand_term_chains_evaluate_shift_rename_and_print() {
    on_small_stack(|| {
        let cases: [(&str, LiteralValue, LiteralValue); 5] = [
            (
                "+",
                LiteralValue::Number(TERMS as f64),
                LiteralValue::Number(2.0 * TERMS as f64),
            ),
            (
                "-",
                LiteralValue::Number(1.0 - (TERMS as f64 - 1.0)),
                LiteralValue::Number(2.0 - 2.0 * (TERMS as f64 - 1.0)),
            ),
            (
                "*",
                LiteralValue::Number(1.0),
                LiteralValue::Number(2f64.powi(TERMS as i32)),
            ),
            (
                "&",
                LiteralValue::Text("1".repeat(TERMS)),
                LiteralValue::Text("2".repeat(TERMS)),
            ),
            // `(1=1)=1` compares TRUE with a number: FALSE, and so on down the chain.
            (
                "=",
                LiteralValue::Boolean(false),
                LiteralValue::Boolean(false),
            ),
        ];
        for (op, before, after) in cases {
            let mut e = engine();
            e.set_cell_value("Sheet1", 1, 1, LiteralValue::Number(1.0))
                .unwrap();
            let formula = chain("Sheet1!A1", op, TERMS);
            e.set_cell_formula("Sheet2", 1, 3, parse(&formula).unwrap())
                .unwrap();
            e.evaluate_all().unwrap();
            assert_eq!(value(&e, "Sheet2", 1, 3), before, "{op}");

            e.set_cell_value("Sheet1", 1, 1, LiteralValue::Number(2.0))
                .unwrap();
            e.evaluate_all().unwrap();
            if op != "*" {
                assert_eq!(value(&e, "Sheet2", 1, 3), after, "{op}");
            }

            e.insert_rows("Sheet1", 1, 1).unwrap();
            e.evaluate_all().unwrap();
            assert_eq!(
                formula_text(&e, "Sheet2", 1, 3),
                chain("Sheet1!A2", op, TERMS),
                "{op}"
            );

            let sheet1 = e.sheet_id("Sheet1").unwrap();
            e.rename_sheet(sheet1, "Data").unwrap();
            e.evaluate_all().unwrap();
            assert_eq!(
                formula_text(&e, "Sheet2", 1, 3),
                chain("Data!A2", op, TERMS),
                "{op}"
            );
            if op != "*" {
                assert_eq!(value(&e, "Sheet2", 1, 3), after, "{op}");
            }
        }
    });
}

/// The parser's own value-level operations on a long chain: clone, compare, hash, print, drop.
#[test]
fn twenty_thousand_term_ast_clones_compares_hashes_and_drops() {
    on_small_stack(|| {
        let formula = chain("1", "+", TERMS);
        let ast = parse(&formula).unwrap();
        let copy = ast.clone();
        assert_eq!(ast, copy);
        assert_eq!(ast.fingerprint(), copy.fingerprint());
        assert_eq!(excel_formula(&copy), formula);
        let other = parse(&chain("1", "-", TERMS)).unwrap();
        assert_ne!(ast, other);
        drop(ast);
        drop(copy);
    });
}

/// Mixed precedence still nests only one chain per precedence level, and evaluates left to right
/// the way Excel does (`2-1-1` is 0, `8/2/2` is 2, `2^3^2` is 64).
#[test]
fn long_mixed_precedence_chains_evaluate_left_to_right() {
    on_small_stack(|| {
        let mut e = engine();
        let sub = format!("=100{}", "-1".repeat(TERMS));
        e.set_cell_formula("Sheet1", 1, 1, parse(&sub).unwrap())
            .unwrap();
        let mixed = format!("=0{}", "+2*3^2/2-1".repeat(TERMS / 4));
        e.set_cell_formula("Sheet1", 2, 1, parse(&mixed).unwrap())
            .unwrap();
        e.set_cell_formula("Sheet1", 3, 1, parse("=2^3^2").unwrap())
            .unwrap();
        e.evaluate_all().unwrap();
        assert_eq!(
            value(&e, "Sheet1", 1, 1),
            LiteralValue::Number(100.0 - TERMS as f64)
        );
        assert_eq!(
            value(&e, "Sheet1", 2, 1),
            LiteralValue::Number(8.0 * (TERMS / 4) as f64)
        );
        assert_eq!(value(&e, "Sheet1", 3, 1), LiteralValue::Number(64.0));
        assert_eq!(formula_text(&e, "Sheet1", 2, 1), mixed);
    });
}
