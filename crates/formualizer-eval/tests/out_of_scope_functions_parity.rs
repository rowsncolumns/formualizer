//! rowsncolumns/spreadsheet#546 U28 (A-17, A-22, A-23, A-24) — out-of-scope functions degrade
//! gracefully. Functions Excel evaluates through an external service or add-in (`CUBE*`, `RTD`,
//! `STOCKHISTORY`, `CALL`, `REGISTER.ID`, `EUROCONVERT`, `TRANSLATE`, `DETECTLANGUAGE`,
//! `WEBSERVICE`) are recognised and yield `#N/A` (catchable with `IFNA`) instead of `#NAME?`;
//! `FILTERXML`, `PERMUTATIONA`, `BAHTTEXT`, `DBCS` / `JIS` and `PHONETIC` are implemented; `N`
//! reduces an array argument to its top-left element; and the registry exposes no test helpers.

use formualizer_common::{ExcelErrorKind, LiteralValue};
use formualizer_eval::engine::{Engine, EvalConfig};
use formualizer_eval::test_workbook::TestWorkbook;
use formualizer_parse::parser::parse;

/// Evaluate `formula` in Sheet1!E2 with A1:A3 = 10, 20, 30 and B1 = "x".
fn eval(formula: &str) -> LiteralValue {
    let mut e = Engine::new(TestWorkbook::new(), EvalConfig::default());
    for (i, v) in [10.0, 20.0, 30.0].into_iter().enumerate() {
        e.set_cell_value("Sheet1", i as u32 + 1, 1, LiteralValue::Number(v))
            .unwrap();
    }
    e.set_cell_value("Sheet1", 1, 2, LiteralValue::Text("x".into()))
        .unwrap();
    e.set_cell_formula("Sheet1", 2, 5, parse(formula).unwrap())
        .unwrap();
    e.evaluate_all().unwrap();
    e.get_cell_value("Sheet1", 2, 5)
        .unwrap_or(LiteralValue::Empty)
}

fn text(s: &str) -> LiteralValue {
    LiteralValue::Text(s.into())
}

fn err_kind(v: &LiteralValue) -> ExcelErrorKind {
    match v {
        LiteralValue::Error(e) => e.kind,
        other => panic!("expected error, got {other:?}"),
    }
}

fn num(v: &LiteralValue) -> f64 {
    match v {
        LiteralValue::Number(n) => *n,
        LiteralValue::Int(i) => *i as f64,
        other => panic!("expected number, got {other:?}"),
    }
}

const EXTERNAL_SERVICE_CALLS: &[&str] = &[
    "=CUBEKPIMEMBER(\"Sales\",\"[KPI]\",1)",
    "=CUBEMEMBER(\"Sales\",\"[Time].[2024]\")",
    "=CUBEMEMBERPROPERTY(\"Sales\",\"[Store].[1]\",\"Name\")",
    "=CUBERANKEDMEMBER(\"Sales\",A1,1)",
    "=CUBESET(\"Sales\",\"[Product].Children\",\"Products\")",
    "=CUBESETCOUNT(A1)",
    "=CUBEVALUE(\"Sales\",\"[Measures].[Amount]\")",
    "=CUBEVALUE(\"Sales\")",
    "=RTD(\"prog.id\",,\"topic\")",
    "=STOCKHISTORY(\"MSFT\",\"1/1/2024\")",
    "=STOCKHISTORY(\"MSFT\",\"1/1/2024\",\"1/31/2024\",0,1,0,1)",
    "=CALL(\"Kernel32\",\"GetTickCount\",\"J\")",
    "=REGISTER.ID(\"Kernel32\",\"GetTickCount\",\"J\")",
    "=EUROCONVERT(100,\"DEM\",\"EUR\")",
    "=TRANSLATE(\"hello\",\"en\",\"fr\")",
    "=DETECTLANGUAGE(\"hello\")",
    "=WEBSERVICE(\"https://example.com/api\")",
];

#[test]
fn external_service_functions_yield_na_not_name() {
    for formula in EXTERNAL_SERVICE_CALLS {
        assert_eq!(
            err_kind(&eval(formula)),
            ExcelErrorKind::Na,
            "{formula} must be a recognised call that degrades to #N/A"
        );
    }
}

#[test]
fn external_service_na_is_catchable_with_ifna() {
    assert_eq!(
        eval("=IFNA(CUBEVALUE(\"Sales\",\"[Measures].[Amount]\"),\"offline\")"),
        text("offline")
    );
    assert_eq!(
        eval("=IFERROR(STOCKHISTORY(\"MSFT\",\"1/1/2024\"),0)"),
        LiteralValue::Number(0.0)
    );
    assert_eq!(
        eval("=ISNA(RTD(\"prog.id\",,\"topic\"))"),
        LiteralValue::Boolean(true)
    );
    // #NAME? stays reserved for genuinely unknown identifiers.
    assert_eq!(
        err_kind(&eval("=NOTAREALFUNCTION(1)")),
        ExcelErrorKind::Name
    );
    assert_eq!(
        err_kind(&eval("=IFNA(NOTAREALFUNCTION(1),\"na\")")),
        ExcelErrorKind::Name
    );
}

#[test]
fn filterxml_supports_element_and_attribute_paths() {
    assert_eq!(
        eval("=FILTERXML(\"<r><a id=\"\"x\"\"/></r>\",\"//a/@id\")"),
        text("x")
    );
    assert_eq!(
        eval("=INDEX(FILTERXML(\"<r><a>1</a><a>2</a></r>\",\"//a\"),2,1)"),
        LiteralValue::Number(2.0)
    );
    assert_eq!(
        eval("=FILTERXML(\"<r><name> Ada </name></r>\",\"//name\")"),
        text("Ada")
    );
    assert_eq!(
        err_kind(&eval("=FILTERXML(\"<r><a>1</a></r>\",\"//b\")")),
        ExcelErrorKind::Value
    );
    assert_eq!(
        err_kind(&eval("=FILTERXML(\"<r/>\",\"/r[1]/a\")")),
        ExcelErrorKind::Value
    );
}

/// Only text Excel itself reads as a number becomes one; `inf` / `NaN` / hex spellings that Rust's
/// `f64` parser would accept stay text, so no non-finite value can enter the grid.
#[test]
fn filterxml_coerces_numbers_by_excel_rules_only() {
    assert_eq!(
        eval("=FILTERXML(\"<r><a>1,000</a></r>\",\"//a\")"),
        LiteralValue::Number(1000.0)
    );
    assert_eq!(
        eval("=FILTERXML(\"<r><a>50%</a></r>\",\"//a\")"),
        LiteralValue::Number(0.5)
    );
    assert_eq!(
        eval("=FILTERXML(\"<r><a>1/2/2024</a></r>\",\"//a\")"),
        LiteralValue::Number(45293.0)
    );
    for spelling in ["inf", "-inf", "infinity", "NaN", "nan", "0x10", "1_000"] {
        assert_eq!(
            eval(&format!("=FILTERXML(\"<r><a>{spelling}</a></r>\",\"//a\")")),
            text(spelling),
            "{spelling} must stay text"
        );
    }
}

#[test]
fn permutationa_is_number_to_the_chosen_power() {
    assert_eq!(num(&eval("=PERMUTATIONA(3,2)")), 9.0);
    assert_eq!(num(&eval("=PERMUTATIONA(2.9,3.1)")), 8.0);
    assert_eq!(num(&eval("=PERMUTATIONA(0,0)")), 1.0);
    assert_eq!(num(&eval("=PERMUTATIONA(5,0)")), 1.0);
    assert_eq!(err_kind(&eval("=PERMUTATIONA(-1,2)")), ExcelErrorKind::Num);
    assert_eq!(err_kind(&eval("=PERMUTATIONA(2,-1)")), ExcelErrorKind::Num);
    assert_eq!(
        err_kind(&eval("=PERMUTATIONA(\"a\",2)")),
        ExcelErrorKind::Value
    );
}

#[test]
fn bahttext_spells_baht_and_satang() {
    assert_eq!(eval("=BAHTTEXT(1234)"), text("หนึ่งพันสองร้อยสามสิบสี่บาทถ้วน"));
    assert_eq!(eval("=BAHTTEXT(21.5)"), text("ยี่สิบเอ็ดบาทห้าสิบสตางค์"));
    assert_eq!(eval("=BAHTTEXT(0)"), text("ศูนย์บาทถ้วน"));
    assert_eq!(eval("=BAHTTEXT(-5)"), text("ลบห้าบาทถ้วน"));
    assert_eq!(err_kind(&eval("=BAHTTEXT(\"abc\")")), ExcelErrorKind::Value);
    // Satang round to two places and carry into the baht — never "one hundred satang".
    assert_eq!(eval("=BAHTTEXT(0.995)"), text("หนึ่งบาทถ้วน"));
    assert_eq!(eval("=BAHTTEXT(1.995)"), text("สองบาทถ้วน"));
    assert_eq!(
        eval("=BAHTTEXT(1234.567)"),
        text("หนึ่งพันสองร้อยสามสิบสี่บาทห้าสิบเจ็ดสตางค์")
    );
}

#[test]
fn dbcs_jis_widen_and_asc_narrows() {
    assert_eq!(eval("=DBCS(\"ABC123\")"), text("ＡＢＣ１２３"));
    assert_eq!(eval("=JIS(\"A B\")"), text("Ａ　Ｂ"));
    assert_eq!(eval("=DBCS(\"東京\")"), text("東京"));
    assert_eq!(eval("=ASC(DBCS(\"Hello, World!\"))"), text("Hello, World!"));
    assert_eq!(eval("=DBCS(A1)"), text("１０"));
}

#[test]
fn phonetic_returns_the_text_when_no_furigana_is_recorded() {
    assert_eq!(eval("=PHONETIC(B1)"), text("x"));
    assert_eq!(eval("=PHONETIC(\"東京\")"), text("東京"));
    assert_eq!(eval("=PHONETIC(A1)"), text("10"));
}

#[test]
fn n_reduces_an_array_to_its_top_left_element() {
    // A-24: `N({…})` used to return 0 for every array argument.
    assert_eq!(num(&eval("=N({7,8,9})")), 7.0);
    assert_eq!(num(&eval("=N({TRUE;2})")), 1.0);
    assert_eq!(num(&eval("=N({\"a\",1})")), 0.0);
    // A multi-cell range from the formula's own row intersects implicitly (E2 → A2).
    assert_eq!(num(&eval("=N(A1:A3)")), 20.0);
    assert_eq!(num(&eval("=N(A1)")), 10.0);
    assert_eq!(num(&eval("=N(\"5\")")), 0.0);
}

#[test]
fn registry_exposes_no_test_helpers() {
    // A-17: `COUNTING` / `ERRORFN` / `THROWNAME` are `#[cfg(test)]` fixtures; they must never be
    // reachable from a workbook.
    for helper in ["COUNTING", "ERRORFN", "THROWNAME"] {
        assert_eq!(
            err_kind(&eval(&format!("={helper}()"))),
            ExcelErrorKind::Name,
            "{helper} leaked into the public registry"
        );
        assert!(
            formualizer_eval::function_registry::get("", helper).is_none(),
            "{helper} is registered"
        );
    }
}
