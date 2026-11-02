//! Information functions and data-provider functions without a provider
//! (demo-app-monorepo#49969 FX-13 / FX-15).
//!
//! - ISFORMULA is TRUE on a formula cell (it was always FALSE); a non-reference is `#VALUE!`.
//! - CELL("filename") is `{directory}[{file}]{sheet}` once the host supplies the workbook's identity
//!   (`""` for an unsaved workbook, as in Excel), so the `MID(CELL("filename"),FIND("]",…)+1,…)`
//!   sheet-name idiom works. INFO answers `directory` / `osversion` / `system` from the host and
//!   `memavail` / `memused` / `totmem` with `#N/A`.
//! - Provider functions return the error Excel documents when the provider is missing; EUROCONVERT
//!   needs no provider (fixed EU rates) and computes. `A1.Price` with no linked data type is
//!   `#FIELD!` (the parser keeps it as a name token so hosts can resolve their own dot accessors).

use formualizer_common::LiteralValue;
use formualizer_eval::engine::{Engine, EvalConfig, WorkbookIdentity};
use formualizer_eval::test_workbook::TestWorkbook;
use formualizer_parse::parser::parse;
use formualizer_parse::pretty::excel_formula;

fn engine(identity: Option<WorkbookIdentity>) -> Engine<TestWorkbook> {
    let config = EvalConfig {
        workbook_identity: identity,
        ..Default::default()
    };
    let mut e = Engine::new(TestWorkbook::new(), config);
    e.set_cell_value("Sheet1", 1, 1, LiteralValue::Number(10.0))
        .unwrap();
    e.set_cell_formula("Sheet1", 2, 1, parse("=A1*2").unwrap())
        .unwrap();
    e
}

fn eval_with(e: &mut Engine<TestWorkbook>, formula: &str) -> LiteralValue {
    e.set_cell_formula("Sheet1", 1, 5, parse(formula).unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    e.get_cell_value("Sheet1", 1, 5)
        .unwrap_or(LiteralValue::Empty)
}

fn eval(formula: &str) -> LiteralValue {
    eval_with(&mut engine(None), formula)
}

fn athena_identity() -> WorkbookIdentity {
    WorkbookIdentity {
        directory: "https://app.athenaintel.com/files/".to_string(),
        file_name: "Budget.xlsx".to_string(),
        os_version: Some("Web".to_string()),
        system: Some("pcdos".to_string()),
    }
}

fn text(s: &str) -> LiteralValue {
    LiteralValue::Text(s.into())
}

/// The error code a value carries.
fn code(v: &LiteralValue) -> String {
    match v {
        LiteralValue::Error(e) => e.kind.to_string(),
        other => panic!("expected an error, got {other:?}"),
    }
}

#[test]
fn isformula_reads_the_referenced_cell() {
    assert_eq!(eval("=ISFORMULA(A2)"), LiteralValue::Boolean(true));
    assert_eq!(eval("=ISFORMULA(A1)"), LiteralValue::Boolean(false));
    assert_eq!(eval("=ISFORMULA(C9)"), LiteralValue::Boolean(false));
    assert_eq!(eval("=ISFORMULA(A2:A3)"), LiteralValue::Boolean(true));
    assert_eq!(code(&eval("=ISFORMULA(10)")), "#VALUE!");
    // A formula that is not a reference: not a cell, so #VALUE!.
    assert_eq!(code(&eval("=ISFORMULA(1+1)")), "#VALUE!");
}

#[test]
fn cell_filename_and_the_sheet_name_idiom() {
    let mut e = engine(Some(athena_identity()));
    assert_eq!(
        eval_with(&mut e, "=CELL(\"filename\")"),
        text("https://app.athenaintel.com/files/[Budget.xlsx]Sheet1")
    );
    assert_eq!(
        eval_with(&mut e, "=CELL(\"filename\",A1)"),
        text("https://app.athenaintel.com/files/[Budget.xlsx]Sheet1")
    );
    assert_eq!(
        eval_with(
            &mut e,
            "=MID(CELL(\"filename\",A1),FIND(\"]\",CELL(\"filename\",A1))+1,255)"
        ),
        text("Sheet1")
    );
    // Unsaved workbook: "".
    assert_eq!(eval("=CELL(\"filename\")"), text(""));

    // CELL is volatile (as in Excel): a new identity reaches an existing cell on the next recalc.
    eval_with(&mut e, "=CELL(\"filename\")");
    e.config.workbook_identity = Some(WorkbookIdentity {
        file_name: "Renamed.xlsx".to_string(),
        directory: String::new(),
        ..Default::default()
    });
    e.evaluate_all().unwrap();
    assert_eq!(
        e.get_cell_value("Sheet1", 1, 5),
        Some(text("[Renamed.xlsx]Sheet1"))
    );
}

#[test]
fn info_answers_from_the_host() {
    let mut e = engine(Some(athena_identity()));
    assert_eq!(
        eval_with(&mut e, "=INFO(\"directory\")"),
        text("https://app.athenaintel.com/files/")
    );
    assert_eq!(eval_with(&mut e, "=INFO(\"osversion\")"), text("Web"));
    assert_eq!(eval_with(&mut e, "=INFO(\"system\")"), text("pcdos"));
    assert_eq!(eval("=INFO(\"recalc\")"), text("Automatic"));
    for removed in ["memavail", "memused", "totmem"] {
        assert_eq!(code(&eval(&format!("=INFO(\"{removed}\")"))), "#N/A");
    }
    assert_eq!(code(&eval("=INFO(\"nonsense\")")), "#VALUE!");
}

#[test]
fn provider_functions_return_excels_errors() {
    for (formula, want) in [
        ("=STOCKHISTORY(\"MSFT\",\"1/1/2024\")", "#CONNECT!"),
        ("=TRANSLATE(\"hello\",\"en\",\"fr\")", "#CONNECT!"),
        ("=DETECTLANGUAGE(\"hello\")", "#CONNECT!"),
        ("=WEBSERVICE(\"https://example.com\")", "#VALUE!"),
        ("=RTD(\"prog.id\",,\"topic\")", "#N/A"),
        ("=CUBEVALUE(\"Sales\",\"[Measures].[Amount]\")", "#NAME?"),
        ("=CUBEMEMBER(\"Sales\",\"[Time].[2024]\")", "#NAME?"),
        ("=CUBESETCOUNT(A1)", "#NAME?"),
        ("=CALL(\"Kernel32\",\"GetTickCount\",\"J\")", "#BLOCKED!"),
        ("=IMPORTTEXT(\"C:\\data\\a.txt\")", "#BLOCKED!"),
        ("=IMPORTCSV(\"C:\\data\\a.csv\")", "#BLOCKED!"),
        ("=PY(\"1+1\",0)", "#BLOCKED!"),
        ("=FIELDVALUE(A1,\"Price\")", "#FIELD!"),
        ("=A1.Price", "#FIELD!"),
        ("=A1.Price*2", "#FIELD!"),
        ("=_FV(A1,\"Price\")", "#FIELD!"),
    ] {
        assert_eq!(code(&eval(formula)), want, "{formula}");
    }
    // Catchable like any error value.
    assert_eq!(
        eval("=IFERROR(STOCKHISTORY(\"MSFT\",1),\"none\")"),
        text("none")
    );
    assert_eq!(eval("=ISERROR(A1.Price)"), LiteralValue::Boolean(true));
    // ERROR.TYPE knows the codes: #CONNECT! 10, #BLOCKED! 11, #FIELD! 13.
    let n = |f: &str| match eval(f) {
        LiteralValue::Int(i) => i,
        LiteralValue::Number(n) => n as i64,
        other => panic!("{f}: {other:?}"),
    };
    assert_eq!(n("=ERROR.TYPE(STOCKHISTORY(\"MSFT\",1))"), 10);
    assert_eq!(n("=ERROR.TYPE(PY(\"1\",0))"), 11);
    assert_eq!(n("=ERROR.TYPE(A1.Price)"), 13);
}

#[test]
fn field_access_text_is_kept_as_typed() {
    for f in ["=A1.Price", "=Sheet1!$A$1.Price", "=A1.Price*2"] {
        assert_eq!(excel_formula(&parse(f).unwrap()), f);
    }
}

#[test]
fn euroconvert_uses_the_fixed_rates() {
    let num = |v: LiteralValue| match v {
        LiteralValue::Number(n) => n,
        LiteralValue::Int(i) => i as f64,
        other => panic!("expected a number, got {other:?}"),
    };
    // The examples on the EUROCONVERT support page.
    assert_eq!(num(eval("=EUROCONVERT(1.2,\"DEM\",\"EUR\")")), 0.61);
    assert_eq!(num(eval("=EUROCONVERT(1,\"FRF\",\"EUR\",TRUE,3)")), 0.152);
    assert_eq!(num(eval("=EUROCONVERT(1,\"FRF\",\"EUR\",FALSE,3)")), 0.15);
    assert_eq!(
        num(eval("=EUROCONVERT(1,\"FRF\",\"DEM\",TRUE,3)")),
        0.29728616
    );
    // From euros, rounded to the target's calculation decimals (BEF: 0).
    assert_eq!(num(eval("=EUROCONVERT(1,\"EUR\",\"BEF\")")), 40.0);
    assert_eq!(num(eval("=EUROCONVERT(5,\"dem\",\"DEM\")")), 5.0);
    assert_eq!(code(&eval("=EUROCONVERT(1,\"USD\",\"EUR\")")), "#VALUE!");
    assert_eq!(
        code(&eval("=EUROCONVERT(1,\"FRF\",\"DEM\",TRUE,2)")),
        "#VALUE!"
    );
}
