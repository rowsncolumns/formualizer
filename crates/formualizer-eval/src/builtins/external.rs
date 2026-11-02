//! Functions Excel evaluates through a live external service, a data provider or a host capability
//! this engine does not have: OLAP cube queries (`CUBE*`), real-time data servers (`RTD`), the
//! connected `STOCKHISTORY`, XLM / DLL bridges (`CALL`, `REGISTER.ID`), the translation functions
//! (`TRANSLATE`, `DETECTLANGUAGE`), `WEBSERVICE`, the file imports (`IMPORTTEXT`, `IMPORTCSV`),
//! Python in Excel (`PY`) and linked data type fields (`FIELDVALUE`, `A1.Price` = `_FV`). The calc
//! engine makes no network calls.
//!
//! Each is registered so a workbook that uses it degrades the way Excel does without the provider:
//! a recognised function returning the error Excel documents for that situation (`#CONNECT!`,
//! `#BLOCKED!`, `#FIELD!`, `#VALUE!`, …) instead of `#NAME?`. Importers keep the cached result
//! Excel wrote and exporters write the formula back unchanged, so a workbook round-trips intact;
//! only a recalculation on this engine surfaces the error. `EUROCONVERT` (fixed EU rates) and
//! `FILTERXML` (the XPath subset workbooks use: `//tag`, `//tag/@attr`) need no provider and are
//! implemented.

use crate::args::ArgSchema;
use crate::function::Function;
use crate::traits::{ArgumentHandle, CalcValue, FunctionContext};
use formualizer_common::{ExcelError, ExcelErrorKind, LiteralValue};
use formualizer_macros::func_caps;
use regex::Regex;

use super::utils::{ARG_ANY_ONE, ARG_ANY_TWO};

/// Declare one external-service stub: any number of arguments of any kind, always the given
/// Excel error.
macro_rules! external_stub {
    ($(#[$doc:meta])* $ty:ident, $name:literal, $code:literal) => {
        $(#[$doc])*
        #[derive(Debug)]
        pub struct $ty;
        impl Function for $ty {
            func_caps!(PURE);
            fn name(&self) -> &'static str {
                $name
            }
            fn min_args(&self) -> usize {
                0
            }
            fn variadic(&self) -> bool {
                true
            }
            fn arg_schema(&self) -> &'static [ArgSchema] {
                &ARG_ANY_ONE[..]
            }
            fn eval<'a, 'b, 'c>(
                &self,
                _args: &'c [ArgumentHandle<'a, 'b>],
                _ctx: &dyn FunctionContext<'b>,
            ) -> Result<CalcValue<'b>, ExcelError> {
                Ok(CalcValue::Scalar(LiteralValue::Error(provider_error($code))))
            }
        }
    };
}

/// The error a provider function returns without its provider.
pub fn provider_error(code: &str) -> ExcelError {
    ExcelError::new(ExcelErrorKind::parse(code))
}

external_stub!(
    /// Returns a key performance indicator property from an OLAP cube.
    ///
    /// # Remarks
    /// - Without a workbook connection to an OLAP server Excel returns `#NAME?` ("If the connection name is not a valid workbook connection … returns a #NAME? error value"); this engine has no OLAP connections.
    /// - <https://support.microsoft.com/en-us/office/cubekpimember-function-744608bf-2c62-42cd-b67a-a56109f4b03b>
    ///
    /// ```yaml,sandbox
    /// title: "No provider"
    /// formula: '=CUBEKPIMEMBER("Sales","[KPI]",1)'
    /// expected: "#NAME?"
    /// ```
    CubeKpiMemberFn,
    "CUBEKPIMEMBER",
    "#NAME?"
);

external_stub!(
    /// Returns a member or tuple from an OLAP cube.
    ///
    /// # Remarks
    /// - Without a workbook connection to an OLAP server Excel returns `#NAME?` ("If the connection name is not a valid workbook connection … returns a #NAME? error value"); this engine has no OLAP connections.
    /// - <https://support.microsoft.com/en-us/office/cubemember-function-0f6a15b9-2c18-4819-ae89-e1b5c8b398ad>
    ///
    /// ```yaml,sandbox
    /// title: "No provider"
    /// formula: '=CUBEMEMBER("Sales","[Time].[2024]")'
    /// expected: "#NAME?"
    /// ```
    CubeMemberFn,
    "CUBEMEMBER",
    "#NAME?"
);

external_stub!(
    /// Returns the value of a member property from an OLAP cube.
    ///
    /// # Remarks
    /// - Without a workbook connection to an OLAP server Excel returns `#NAME?` ("If the connection name is not a valid workbook connection … returns a #NAME? error value"); this engine has no OLAP connections.
    /// - <https://support.microsoft.com/en-us/office/cubememberproperty-function-001e57d6-b35a-49e5-abcd-05ff599e8951>
    ///
    /// ```yaml,sandbox
    /// title: "No provider"
    /// formula: '=CUBEMEMBERPROPERTY("Sales","[Store].[1]","Name")'
    /// expected: "#NAME?"
    /// ```
    CubeMemberPropertyFn,
    "CUBEMEMBERPROPERTY",
    "#NAME?"
);

external_stub!(
    /// Returns the nth, or ranked, member of an OLAP set.
    ///
    /// # Remarks
    /// - Without a workbook connection to an OLAP server Excel returns `#NAME?` ("If the connection name is not a valid workbook connection … returns a #NAME? error value"); this engine has no OLAP connections.
    /// - <https://support.microsoft.com/en-us/office/cuberankedmember-function-07efecde-e669-4075-b4bf-6b40df2dc4b3>
    ///
    /// ```yaml,sandbox
    /// title: "No provider"
    /// formula: '=CUBERANKEDMEMBER("Sales","[Set]",1)'
    /// expected: "#NAME?"
    /// ```
    CubeRankedMemberFn,
    "CUBERANKEDMEMBER",
    "#NAME?"
);

external_stub!(
    /// Defines a calculated set of members or tuples from an OLAP cube.
    ///
    /// # Remarks
    /// - Without a workbook connection to an OLAP server Excel returns `#NAME?` ("If the connection name is not a valid workbook connection … returns a #NAME? error value"); this engine has no OLAP connections.
    /// - <https://support.microsoft.com/en-us/office/cubeset-function-5b2146bd-62d6-4d04-9d8f-670e993ee1d9>
    ///
    /// ```yaml,sandbox
    /// title: "No provider"
    /// formula: '=CUBESET("Sales","[Product].Children","Products")'
    /// expected: "#NAME?"
    /// ```
    CubeSetFn,
    "CUBESET",
    "#NAME?"
);

external_stub!(
    /// Returns the number of items in an OLAP set.
    ///
    /// # Remarks
    /// - Without a workbook connection to an OLAP server Excel returns `#NAME?` ("If the connection name is not a valid workbook connection … returns a #NAME? error value"); this engine has no OLAP connections.
    /// - <https://support.microsoft.com/en-us/office/cubesetcount-function-c4c2a438-c1ff-4061-80fe-982f2d705286>
    ///
    /// ```yaml,sandbox
    /// title: "No provider"
    /// formula: '=CUBESETCOUNT(A1)'
    /// expected: "#NAME?"
    /// ```
    CubeSetCountFn,
    "CUBESETCOUNT",
    "#NAME?"
);

external_stub!(
    /// Returns an aggregated value from an OLAP cube.
    ///
    /// # Remarks
    /// - Without a workbook connection to an OLAP server Excel returns `#NAME?` ("If the connection name is not a valid workbook connection … returns a #NAME? error value"); this engine has no OLAP connections.
    /// - <https://support.microsoft.com/en-us/office/cubevalue-function-8733da24-26d1-4e34-9b3a-84a8f00dcbe0>
    ///
    /// ```yaml,sandbox
    /// title: "No provider"
    /// formula: '=CUBEVALUE("Sales","[Measures].[Amount]")'
    /// expected: "#NAME?"
    /// ```
    CubeValueFn,
    "CUBEVALUE",
    "#NAME?"
);

external_stub!(
    /// Retrieves real-time data from a COM automation server.
    ///
    /// # Remarks
    /// - Without an installed real-time data server Excel shows `#N/A`; RTD servers are a Windows COM facility this engine does not have.
    /// - <https://support.microsoft.com/en-us/office/rtd-function-e0cc001a-56f0-470a-9b19-9455dc0eb593>
    ///
    /// ```yaml,sandbox
    /// title: "No provider"
    /// formula: '=RTD("prog.id",,"topic")'
    /// expected: "#N/A"
    /// ```
    RtdFn,
    "RTD",
    "#N/A"
);

external_stub!(
    /// Retrieves historical data about a financial instrument.
    ///
    /// # Remarks
    /// - Needs a Microsoft data service the engine cannot reach (no network calls from the calc engine); Excel reports an unreachable service as `#CONNECT!`.
    /// - <https://support.microsoft.com/en-us/office/stockhistory-function-1ac8b5b3-5f62-4d94-8ab8-7504ec7239a8>
    ///
    /// ```yaml,sandbox
    /// title: "No provider"
    /// formula: '=STOCKHISTORY("MSFT","1/1/2024")'
    /// expected: "#CONNECT!"
    /// ```
    StockHistoryFn,
    "STOCKHISTORY",
    "#CONNECT!"
);

external_stub!(
    /// Calls a procedure in a dynamic link library or code resource (an Excel 4.0 macro function).
    ///
    /// # Remarks
    /// - Needs a capability this engine blocks (local files, native code, a Python runtime); Excel reports a blocked capability as `#BLOCKED!`.
    ///
    /// ```yaml,sandbox
    /// title: "No provider"
    /// formula: '=CALL("Kernel32","GetTickCount","J")'
    /// expected: "#BLOCKED!"
    /// ```
    CallFn,
    "CALL",
    "#BLOCKED!"
);

external_stub!(
    /// Returns the register ID of a DLL or code resource (an Excel 4.0 macro function).
    ///
    /// # Remarks
    /// - Needs a capability this engine blocks (local files, native code, a Python runtime); Excel reports a blocked capability as `#BLOCKED!`.
    ///
    /// ```yaml,sandbox
    /// title: "No provider"
    /// formula: '=REGISTER.ID("Kernel32","GetTickCount","J")'
    /// expected: "#BLOCKED!"
    /// ```
    RegisterIdFn,
    "REGISTER.ID",
    "#BLOCKED!"
);

external_stub!(
    /// Translates text from one language to another (Microsoft Translator service).
    ///
    /// # Remarks
    /// - Needs a Microsoft data service the engine cannot reach (no network calls from the calc engine); Excel reports an unreachable service as `#CONNECT!`.
    ///
    /// ```yaml,sandbox
    /// title: "No provider"
    /// formula: '=TRANSLATE("hello","en","fr")'
    /// expected: "#CONNECT!"
    /// ```
    TranslateFn,
    "TRANSLATE",
    "#CONNECT!"
);

external_stub!(
    /// Identifies the language of a text (Microsoft Translator service).
    ///
    /// # Remarks
    /// - Needs a Microsoft data service the engine cannot reach (no network calls from the calc engine); Excel reports an unreachable service as `#CONNECT!`.
    ///
    /// ```yaml,sandbox
    /// title: "No provider"
    /// formula: '=DETECTLANGUAGE("hello")'
    /// expected: "#CONNECT!"
    /// ```
    DetectLanguageFn,
    "DETECTLANGUAGE",
    "#CONNECT!"
);

external_stub!(
    /// Returns data from a web service.
    ///
    /// # Remarks
    /// - The calc engine makes no network calls; Excel returns `#VALUE!` when WEBSERVICE cannot return the data.
    /// - <https://support.microsoft.com/en-us/office/webservice-function-0546a35a-ecc6-4739-aed7-c0b7ce1562c4>
    ///
    /// ```yaml,sandbox
    /// title: "No provider"
    /// formula: '=WEBSERVICE("https://example.com/api")'
    /// expected: "#VALUE!"
    /// ```
    WebServiceFn,
    "WEBSERVICE",
    "#VALUE!"
);

external_stub!(
    /// Imports a text file (CSV, TSV, TXT) as an array.
    ///
    /// # Remarks
    /// - Needs a capability this engine blocks (local files, native code, a Python runtime); Excel reports a blocked capability as `#BLOCKED!`.
    ///
    /// ```yaml,sandbox
    /// title: "No provider"
    /// formula: '=IMPORTTEXT("C:\\data\\sales.txt")'
    /// expected: "#BLOCKED!"
    /// ```
    ImportTextFn,
    "IMPORTTEXT",
    "#BLOCKED!"
);

external_stub!(
    /// Imports a CSV file as an array.
    ///
    /// # Remarks
    /// - Needs a capability this engine blocks (local files, native code, a Python runtime); Excel reports a blocked capability as `#BLOCKED!`.
    ///
    /// ```yaml,sandbox
    /// title: "No provider"
    /// formula: '=IMPORTCSV("C:\\data\\sales.csv")'
    /// expected: "#BLOCKED!"
    /// ```
    ImportCsvFn,
    "IMPORTCSV",
    "#BLOCKED!"
);

external_stub!(
    /// Runs Python code (Python in Excel).
    ///
    /// # Remarks
    /// - Needs a capability this engine blocks (local files, native code, a Python runtime); Excel reports a blocked capability as `#BLOCKED!`.
    ///
    /// ```yaml,sandbox
    /// title: "No provider"
    /// formula: '=PY("1+1",0)'
    /// expected: "#BLOCKED!"
    /// ```
    PyFn,
    "PY",
    "#BLOCKED!"
);

external_stub!(
    /// Returns a field of a linked data type value (Stocks, Geography, …); `A1.Price` is the same call.
    ///
    /// # Remarks
    /// - Athena has no linked data types, so no value has a field to read; Excel returns `#FIELD!` for a field the value does not have.
    /// - <https://support.microsoft.com/en-us/office/fieldvalue-function-f8b7b6c0-fb47-4f1c-a4c9-3b9fe9bd7d0f>
    ///
    /// ```yaml,sandbox
    /// title: "No provider"
    /// formula: '=FIELDVALUE(A1,"Price")'
    /// expected: "#FIELD!"
    /// ```
    FieldValueFn,
    "FIELDVALUE",
    "#FIELD!"
);

external_stub!(
    /// The dot-field operator `A1.Price` (stored in a workbook file as `_FV(A1,"Price")`).
    ///
    /// # Remarks
    /// - Athena has no linked data types, so no value has a field to read; Excel returns `#FIELD!` for a field the value does not have.
    ///
    /// ```yaml,sandbox
    /// title: "No provider"
    /// formula: '=_FV(A1,"Price")'
    /// expected: "#FIELD!"
    /// ```
    FieldAccessFn,
    "_FV",
    "#FIELD!"
);

/// The fixed conversion rates the euro replaced (units of the national currency per euro) and the
/// decimals the Euro Currency Tools add-in calculates each currency to.
const EURO_RATES: &[(&str, f64, i32)] = &[
    ("EUR", 1.0, 2),
    ("BEF", 40.3399, 0),
    ("LUF", 40.3399, 0),
    ("DEM", 1.95583, 2),
    ("ESP", 166.386, 0),
    ("FRF", 6.55957, 2),
    ("IEP", 0.787564, 2),
    ("ITL", 1936.27, 0),
    ("NLG", 2.20371, 2),
    ("ATS", 13.7603, 2),
    ("PTE", 200.482, 0),
    ("FIM", 5.94573, 2),
    ("GRD", 340.75, 0),
    ("SIT", 239.64, 2),
];

fn round_to(x: f64, decimals: i32) -> f64 {
    let f = 10f64.powi(decimals);
    significant_15((x * f).round() / f)
}

/// Excel's 15 significant digits: drops the binary noise of a multiply / divide
/// (0.152 × 1.95583 = 0.29728615999999997 → 0.29728616).
fn significant_15(x: f64) -> f64 {
    format!("{x:.14e}").parse().unwrap_or(x)
}

#[derive(Debug)]
pub struct EuroConvertFn;

/// Converts a number to euros, from euros to a euro member currency, or between two member
/// currencies through the euro (triangulation), at the fixed EU conversion rates.
///
/// `EUROCONVERT(number, source, target, [full_precision], [triangulation_precision])`
///
/// # Remarks
/// - `source` / `target` are ISO codes: EUR, BEF, LUF, DEM, ESP, FRF, IEP, ITL, NLG, ATS, PTE, FIM,
///   GRD, SIT. An unknown code is `#VALUE!`; equal codes return `number` unchanged.
/// - `full_precision` FALSE (the default) rounds the result to the target currency's calculation
///   decimals; TRUE keeps every digit.
/// - `triangulation_precision` (an integer ≥ 3) rounds the intermediate euro value to that many
///   decimals.
/// - <https://support.microsoft.com/en-us/office/euroconvert-function-79c8fd67-c665-450c-bb6c-15fc92f8345c>
///
/// ```yaml,sandbox
/// title: "Deutsche marks to euros"
/// formula: '=EUROCONVERT(1.2,"DEM","EUR")'
/// expected: 0.61
/// ```
///
/// ```yaml,sandbox
/// title: "Francs to marks through a 3-decimal euro"
/// formula: '=EUROCONVERT(1,"FRF","DEM",TRUE,3)'
/// expected: 0.29728616
/// ```
impl Function for EuroConvertFn {
    func_caps!(PURE);
    fn name(&self) -> &'static str {
        "EUROCONVERT"
    }
    fn min_args(&self) -> usize {
        3
    }
    fn variadic(&self) -> bool {
        true
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        &ARG_ANY_ONE[..]
    }
    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        _ctx: &dyn FunctionContext<'b>,
    ) -> Result<CalcValue<'b>, ExcelError> {
        let value_err = || {
            Ok(CalcValue::Scalar(LiteralValue::Error(
                ExcelError::new_value(),
            )))
        };
        if args.len() < 3 || args.len() > 5 {
            return value_err();
        }
        let scalar_of = |arg: &ArgumentHandle<'_, '_>| -> Result<LiteralValue, ExcelError> {
            if arg.is_skipped() {
                return Ok(LiteralValue::Empty);
            }
            Ok(match arg.value()? {
                CalcValue::Scalar(v) => v,
                CalcValue::Range(rv) => rv.get_cell(0, 0),
                CalcValue::Callable(_) => return Err(ExcelError::new_value()),
            })
        };
        let mut vals = Vec::with_capacity(args.len());
        for arg in args {
            match scalar_of(arg) {
                Ok(LiteralValue::Error(e)) | Err(e) => {
                    return Ok(CalcValue::Scalar(LiteralValue::Error(e)));
                }
                Ok(v) => vals.push(v),
            }
        }
        let number = match &vals[0] {
            LiteralValue::Int(i) => *i as f64,
            LiteralValue::Number(n) => *n,
            LiteralValue::Boolean(b) => *b as i64 as f64,
            LiteralValue::Empty => 0.0,
            v => match crate::builtins::utils::coerce_num_for(&args[0], v) {
                Ok(n) => n,
                Err(_) => return value_err(),
            },
        };
        let currency = |v: &LiteralValue| match v {
            LiteralValue::Text(code) => EURO_RATES
                .iter()
                .find(|(c, _, _)| c.eq_ignore_ascii_case(code.trim()))
                .copied(),
            _ => None,
        };
        let (Some(source), Some(target)) = (currency(&vals[1]), currency(&vals[2])) else {
            return value_err();
        };
        let full_precision = match vals.get(3) {
            None | Some(LiteralValue::Empty) => false,
            Some(LiteralValue::Boolean(b)) => *b,
            Some(LiteralValue::Int(i)) => *i != 0,
            Some(LiteralValue::Number(n)) => *n != 0.0,
            Some(_) => return value_err(),
        };
        let triangulation = match vals.get(4) {
            None | Some(LiteralValue::Empty) => None,
            Some(LiteralValue::Int(i)) if *i >= 3 => Some(*i as i32),
            Some(LiteralValue::Number(n)) if n.trunc() >= 3.0 => Some(n.trunc() as i32),
            Some(_) => return value_err(),
        };
        if source.0 == target.0 {
            return Ok(CalcValue::Scalar(LiteralValue::Number(number)));
        }
        let mut euros = number / source.1;
        if let Some(decimals) = triangulation
            && source.0 != "EUR"
        {
            euros = round_to(euros, decimals);
        }
        let mut result = euros * target.1;
        result = if full_precision {
            significant_15(result)
        } else {
            round_to(result, target.2)
        };
        Ok(CalcValue::Scalar(LiteralValue::Number(result)))
    }
}

fn arg_text(arg: &ArgumentHandle<'_, '_>) -> Result<String, ExcelError> {
    let v = match arg.value()? {
        CalcValue::Scalar(v) => v,
        CalcValue::Range(rv) => rv.get_cell(0, 0),
        CalcValue::Callable(_) => return Err(ExcelError::new_value()),
    };
    match v {
        LiteralValue::Text(s) => Ok(s),
        LiteralValue::Empty => Ok(String::new()),
        LiteralValue::Error(e) => Err(e),
        LiteralValue::Boolean(b) => Ok(if b { "TRUE" } else { "FALSE" }.to_string()),
        LiteralValue::Int(i) => Ok(i.to_string()),
        LiteralValue::Number(n) => Ok(formualizer_common::number_to_excel_text(n)),
        other => Ok(other.to_string()),
    }
}

/// The XPath subset `FILTERXML` supports: `//tag` (element text) and `//tag/@attr` (attribute
/// value). Returns `None` when `xpath` is outside that subset.
fn filter_xml(xml: &str, xpath: &str) -> Option<Vec<String>> {
    static XPATH: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r"^//([A-Za-z0-9_:-]+)(?:/@([A-Za-z0-9_:-]+))?$").unwrap()
    });
    let caps = XPATH.captures(xpath)?;
    let tag = regex::escape(&caps[1]);
    let mut out = Vec::new();
    if let Some(attr) = caps.get(2) {
        let attr = regex::escape(attr.as_str());
        let tag_re = Regex::new(&format!(r"(?i)<{tag}\b([^>]*)/?>")).ok()?;
        let attr_re = Regex::new(&format!(r#"(?i){attr}\s*=\s*"([^"]*)""#)).ok()?;
        for m in tag_re.captures_iter(xml) {
            if let Some(a) = attr_re.captures(m.get(1).map_or("", |g| g.as_str())) {
                out.push(a[1].to_string());
            }
        }
    } else {
        let tag_re = Regex::new(&format!(r"(?is)<{tag}\b[^>]*>(.*?)</{tag}>")).ok()?;
        for m in tag_re.captures_iter(xml) {
            out.push(m[1].trim().to_string());
        }
    }
    Some(out)
}

/// Returns specific data from XML content by using the specified XPath.
///
/// # Remarks
/// - Supports the `//tag` and `//tag/@attr` XPath forms; every match becomes one row of the
///   result, so several matches spill down a column.
/// - No match, or an XPath outside the supported subset, returns `#VALUE!`.
///
/// ```yaml,sandbox
/// title: "Element text"
/// formula: '=FILTERXML("<r><a>1</a><a>2</a></r>","//a")'
/// expected: [[1], [2]]
/// ```
///
/// ```yaml,sandbox
/// title: "Attribute value"
/// formula: '=FILTERXML("<r><a id=""x""/></r>","//a/@id")'
/// expected: "x"
/// ```
#[derive(Debug)]
pub struct FilterXmlFn;
/// [formualizer-docgen:schema:start]
/// Name: FILTERXML
/// Type: FilterXmlFn
/// Min args: 2
/// Max args: 2
/// Variadic: false
/// Signature: FILTERXML(arg1: any@scalar, arg2: any@scalar)
/// Arg schema: arg1{kinds=any,required=true,shape=scalar,by_ref=false,coercion=None,max=None,repeating=None,default=false}; arg2{kinds=any,required=true,shape=scalar,by_ref=false,coercion=None,max=None,repeating=None,default=false}
/// Caps: PURE, MAY_SPILL
/// [formualizer-docgen:schema:end]
impl Function for FilterXmlFn {
    func_caps!(PURE, MAY_SPILL);
    fn name(&self) -> &'static str {
        "FILTERXML"
    }
    fn min_args(&self) -> usize {
        2
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        &ARG_ANY_TWO[..]
    }
    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        ctx: &dyn FunctionContext<'b>,
    ) -> Result<CalcValue<'b>, ExcelError> {
        let err = |e: ExcelError| Ok(CalcValue::Scalar(LiteralValue::Error(e)));
        let xml = match arg_text(&args[0]) {
            Ok(s) => s,
            Err(e) => return err(e),
        };
        let xpath = match arg_text(&args[1]) {
            Ok(s) => s,
            Err(e) => return err(e),
        };
        let matches = match filter_xml(&xml, &xpath) {
            Some(m) if !m.is_empty() => m,
            _ => return err(ExcelError::new_value()),
        };
        // Matches are text; Excel returns the number when the text is numeric by its own
        // text→number rules (what `VALUE` accepts: `"1,000"`, `"5%"`, `"1/2/2024"`), so spellings
        // Excel does not read as numbers (`"inf"`, `"NaN"`, `"0x10"`) stay text.
        let locale = ctx.locale();
        let cell = |s: String| match crate::coercion::parse_numeric_text(&s, &locale) {
            Some(n) => LiteralValue::Number(n),
            None => LiteralValue::Text(s),
        };
        if matches.len() == 1 {
            let only = matches.into_iter().next().unwrap_or_default();
            return Ok(CalcValue::Scalar(cell(only)));
        }
        Ok(CalcValue::Scalar(LiteralValue::Array(
            matches.into_iter().map(|s| vec![cell(s)]).collect(),
        )))
    }
}

pub fn register_builtins() {
    use std::sync::Arc;
    crate::function_registry::register_builtin(Arc::new(FilterXmlFn));
    crate::function_registry::register_builtin(Arc::new(CubeKpiMemberFn));
    crate::function_registry::register_builtin(Arc::new(CubeMemberFn));
    crate::function_registry::register_builtin(Arc::new(CubeMemberPropertyFn));
    crate::function_registry::register_builtin(Arc::new(CubeRankedMemberFn));
    crate::function_registry::register_builtin(Arc::new(CubeSetFn));
    crate::function_registry::register_builtin(Arc::new(CubeSetCountFn));
    crate::function_registry::register_builtin(Arc::new(CubeValueFn));
    crate::function_registry::register_builtin(Arc::new(RtdFn));
    crate::function_registry::register_builtin(Arc::new(StockHistoryFn));
    crate::function_registry::register_builtin(Arc::new(CallFn));
    crate::function_registry::register_builtin(Arc::new(RegisterIdFn));
    crate::function_registry::register_builtin(Arc::new(EuroConvertFn));
    crate::function_registry::register_builtin(Arc::new(ImportTextFn));
    crate::function_registry::register_builtin(Arc::new(ImportCsvFn));
    crate::function_registry::register_builtin(Arc::new(PyFn));
    crate::function_registry::register_builtin(Arc::new(FieldValueFn));
    crate::function_registry::register_builtin(Arc::new(FieldAccessFn));
    crate::function_registry::register_builtin(Arc::new(TranslateFn));
    crate::function_registry::register_builtin(Arc::new(DetectLanguageFn));
    crate::function_registry::register_builtin(Arc::new(WebServiceFn));
}

#[cfg(test)]
mod tests {
    use super::filter_xml;

    #[test]
    fn filter_xml_subset() {
        assert_eq!(
            filter_xml(r#"<r><a id="x"/></r>"#, "//a/@id"),
            Some(vec!["x".to_string()])
        );
        assert_eq!(
            filter_xml("<r><a>1</a><a> 2 </a></r>", "//a"),
            Some(vec!["1".to_string(), "2".to_string()])
        );
        assert_eq!(filter_xml("<r/>", "/r[1]/a"), None);
    }
}
