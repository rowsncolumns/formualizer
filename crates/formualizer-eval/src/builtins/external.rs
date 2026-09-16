//! Functions Excel evaluates through a live external service or a host add-in that this engine
//! does not have: OLAP cube queries (`CUBE*`), real-time data servers (`RTD`), the connected
//! `STOCKHISTORY` data type, XLM / DLL bridges (`CALL`, `REGISTER.ID`), the Euro Currency Tools
//! add-in (`EUROCONVERT`), the Copilot language functions (`TRANSLATE`, `DETECTLANGUAGE`) and
//! `WEBSERVICE` (no network access from the calc engine). `FILTERXML` needs no service and is
//! implemented for the XPath subset workbooks actually use (`//tag`, `//tag/@attr`).
//!
//! They are registered so a workbook that uses them degrades the way Excel does without the
//! service — the call is a recognised function that yields `#N/A` (catchable with `IFNA` /
//! `IFERROR`) — instead of `#NAME?`, which Excel reserves for unrecognised identifiers. Importers
//! keep the cached result Excel wrote, and exporters write the formula back unchanged, so a
//! workbook round-trips intact; only a recalculation on this engine surfaces the `#N/A`.

use crate::args::ArgSchema;
use crate::function::Function;
use crate::traits::{ArgumentHandle, CalcValue, FunctionContext};
use formualizer_common::{ExcelError, LiteralValue};
use formualizer_macros::func_caps;
use regex::Regex;

use super::utils::{ARG_ANY_ONE, ARG_ANY_TWO};

/// Declare one external-service stub: any number of arguments of any kind, always `#N/A`.
macro_rules! external_stub {
    ($(#[$doc:meta])* $ty:ident, $name:literal) => {
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
                Ok(CalcValue::Scalar(LiteralValue::Error(ExcelError::new_na())))
            }
        }
    };
}

external_stub!(
    /// Returns a key performance indicator property from an OLAP cube.
    ///
    /// # Remarks
    /// - Requires a live OLAP connection, which this engine does not have; the call evaluates
    ///   to `#N/A` (the cached result survives import / export unchanged).
    ///
    /// ```yaml,sandbox
    /// title: "No cube connection"
    /// formula: '=CUBEKPIMEMBER("Sales","[KPI]",1)'
    /// expected: "#N/A"
    /// ```
    CubeKpiMemberFn,
    "CUBEKPIMEMBER"
);
external_stub!(
    /// Returns a member or tuple from an OLAP cube.
    ///
    /// # Remarks
    /// - Requires a live OLAP connection, which this engine does not have; the call evaluates
    ///   to `#N/A` (the cached result survives import / export unchanged).
    ///
    /// ```yaml,sandbox
    /// title: "No cube connection"
    /// formula: '=CUBEMEMBER("Sales","[Time].[2024]")'
    /// expected: "#N/A"
    /// ```
    CubeMemberFn,
    "CUBEMEMBER"
);
external_stub!(
    /// Returns the value of a member property from an OLAP cube.
    ///
    /// # Remarks
    /// - Requires a live OLAP connection, which this engine does not have; the call evaluates
    ///   to `#N/A` (the cached result survives import / export unchanged).
    ///
    /// ```yaml,sandbox
    /// title: "No cube connection"
    /// formula: '=CUBEMEMBERPROPERTY("Sales","[Store].[1]","Name")'
    /// expected: "#N/A"
    /// ```
    CubeMemberPropertyFn,
    "CUBEMEMBERPROPERTY"
);
external_stub!(
    /// Returns the nth, or ranked, member of an OLAP set.
    ///
    /// # Remarks
    /// - Requires a live OLAP connection, which this engine does not have; the call evaluates
    ///   to `#N/A` (the cached result survives import / export unchanged).
    ///
    /// ```yaml,sandbox
    /// title: "No cube connection"
    /// formula: '=CUBERANKEDMEMBER("Sales",A1,1)'
    /// expected: "#N/A"
    /// ```
    CubeRankedMemberFn,
    "CUBERANKEDMEMBER"
);
external_stub!(
    /// Defines a calculated set of members or tuples from an OLAP cube.
    ///
    /// # Remarks
    /// - Requires a live OLAP connection, which this engine does not have; the call evaluates
    ///   to `#N/A` (the cached result survives import / export unchanged).
    ///
    /// ```yaml,sandbox
    /// title: "No cube connection"
    /// formula: '=CUBESET("Sales","[Product].Children","Products")'
    /// expected: "#N/A"
    /// ```
    CubeSetFn,
    "CUBESET"
);
external_stub!(
    /// Returns the number of items in an OLAP set.
    ///
    /// # Remarks
    /// - Requires a live OLAP connection, which this engine does not have; the call evaluates
    ///   to `#N/A` (the cached result survives import / export unchanged).
    ///
    /// ```yaml,sandbox
    /// title: "No cube connection"
    /// formula: '=CUBESETCOUNT(A1)'
    /// expected: "#N/A"
    /// ```
    CubeSetCountFn,
    "CUBESETCOUNT"
);
external_stub!(
    /// Returns an aggregated value from an OLAP cube.
    ///
    /// # Remarks
    /// - Requires a live OLAP connection, which this engine does not have; the call evaluates
    ///   to `#N/A` (the cached result survives import / export unchanged).
    ///
    /// ```yaml,sandbox
    /// title: "No cube connection"
    /// formula: '=CUBEVALUE("Sales","[Measures].[Amount]")'
    /// expected: "#N/A"
    /// ```
    CubeValueFn,
    "CUBEVALUE"
);
external_stub!(
    /// Retrieves real-time data from a COM automation server.
    ///
    /// # Remarks
    /// - RTD servers are a Windows COM facility; without one the call evaluates to `#N/A`.
    ///
    /// ```yaml,sandbox
    /// title: "No RTD server"
    /// formula: '=RTD("prog.id",,"topic")'
    /// expected: "#N/A"
    /// ```
    RtdFn,
    "RTD"
);
external_stub!(
    /// Retrieves historical data about a financial instrument.
    ///
    /// # Remarks
    /// - Needs the connected Stocks data service; without it the call evaluates to `#N/A`.
    ///
    /// ```yaml,sandbox
    /// title: "No data service"
    /// formula: '=STOCKHISTORY("MSFT","1/1/2024")'
    /// expected: "#N/A"
    /// ```
    StockHistoryFn,
    "STOCKHISTORY"
);
external_stub!(
    /// Calls a procedure in a dynamic link library or code resource.
    ///
    /// # Remarks
    /// - Native code cannot be invoked from the calc engine; the call evaluates to `#N/A`.
    ///
    /// ```yaml,sandbox
    /// title: "No native bridge"
    /// formula: '=CALL("Kernel32","GetTickCount","J")'
    /// expected: "#N/A"
    /// ```
    CallFn,
    "CALL"
);
external_stub!(
    /// Returns the register ID of a previously registered DLL or code resource.
    ///
    /// # Remarks
    /// - Native code cannot be registered from the calc engine; the call evaluates to `#N/A`.
    ///
    /// ```yaml,sandbox
    /// title: "No native bridge"
    /// formula: '=REGISTER.ID("Kernel32","GetTickCount","J")'
    /// expected: "#N/A"
    /// ```
    RegisterIdFn,
    "REGISTER.ID"
);
external_stub!(
    /// Converts a number between euro member currencies (Euro Currency Tools add-in).
    ///
    /// # Remarks
    /// - The add-in's conversion tables are not available; the call evaluates to `#N/A`.
    ///
    /// ```yaml,sandbox
    /// title: "Add-in not loaded"
    /// formula: '=EUROCONVERT(100,"DEM","EUR")'
    /// expected: "#N/A"
    /// ```
    EuroConvertFn,
    "EUROCONVERT"
);
external_stub!(
    /// Translates text from one language to another (Copilot language service).
    ///
    /// # Remarks
    /// - Needs the connected translation service; without it the call evaluates to `#N/A`.
    ///
    /// ```yaml,sandbox
    /// title: "No translation service"
    /// formula: '=TRANSLATE("hello","en","fr")'
    /// expected: "#N/A"
    /// ```
    TranslateFn,
    "TRANSLATE"
);
external_stub!(
    /// Identifies the language of a text (Copilot language service).
    ///
    /// # Remarks
    /// - Needs the connected language service; without it the call evaluates to `#N/A`.
    ///
    /// ```yaml,sandbox
    /// title: "No language service"
    /// formula: '=DETECTLANGUAGE("hello")'
    /// expected: "#N/A"
    /// ```
    DetectLanguageFn,
    "DETECTLANGUAGE"
);
external_stub!(
    /// Returns data from a web service.
    ///
    /// # Remarks
    /// - The calc engine has no network access; the call evaluates to `#N/A`.
    ///
    /// ```yaml,sandbox
    /// title: "No network access"
    /// formula: '=WEBSERVICE("https://example.com/api")'
    /// expected: "#N/A"
    /// ```
    WebServiceFn,
    "WEBSERVICE"
);

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
