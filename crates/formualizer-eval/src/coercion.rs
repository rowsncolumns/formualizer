use formualizer_common::{ExcelError, ExcelErrorKind, LiteralValue};

/// Centralized coercion and error policy utilities (Milestone 7).
/// These functions implement invariant, Excel-compatible coercions and
/// numeric sanitization. They should be used by the interpreter, builtins,
/// and evaluation pipelines (map/fold/window) instead of ad-hoc parsing.
/// Strict numeric coercion.
/// - Accepts Number/Int/Boolean/Empty/Date-like serial-bearing variants
/// - Rejects Text (returns #VALUE!)
pub fn to_number_strict(value: &LiteralValue) -> Result<f64, ExcelError> {
    match value {
        LiteralValue::Number(n) => Ok(*n),
        LiteralValue::Int(i) => Ok(*i as f64),
        LiteralValue::Boolean(b) => Ok(if *b { 1.0 } else { 0.0 }),
        LiteralValue::Empty => Ok(0.0),
        // Date/time/duration map to serials
        other if other.as_serial_number().is_some() => Ok(other.as_serial_number().unwrap()),
        LiteralValue::Error(e) => Err(e.clone()),
        _ => Err(ExcelError::new(ExcelErrorKind::Value)
            .with_message("Cannot convert to number (strict)")),
    }
}

/// Lenient numeric coercion.
/// - As strict, but also parses numeric text using ASCII/invariant rules
pub fn to_number_lenient(value: &LiteralValue) -> Result<f64, ExcelError> {
    to_number_lenient_with_locale(value, &crate::locale::Locale::invariant())
}

/// Context-aware lenient numeric coercion using locale.
pub fn to_number_lenient_with_locale(
    value: &LiteralValue,
    loc: &crate::locale::Locale,
) -> Result<f64, ExcelError> {
    match value {
        LiteralValue::Text(s) => parse_numeric_text(s, loc).ok_or_else(|| {
            ExcelError::new(ExcelErrorKind::Value)
                .with_message(format!("Cannot convert '{s}' to number"))
        }),
        _ => to_number_strict(value),
    }
}

/// Excel's text→number coercion used by arithmetic and `VALUE()`: numeric text
/// (`"1,000"`, `"$5"`, `"50%"`, `"(5)"`, `"1e3"`) via the locale, otherwise
/// en-US date/time text (`"1/2/2024"` → 45293, `"6:00 PM"` → 0.75,
/// `"2024-01-02 18:00"` → 45293.75). `None` when the text is not numeric.
pub fn parse_numeric_text(text: &str, loc: &crate::locale::Locale) -> Option<f64> {
    loc.parse_number_invariant(text)
        .or_else(|| parse_date_time_text(text))
}

/// Parse en-US date and/or time text to an Excel (1900 date system) serial.
pub fn parse_date_time_text(text: &str) -> Option<f64> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if let Some(fraction) = parse_time_text(text) {
        return Some(fraction);
    }
    if let Some(date) = parse_date_text(text) {
        return Some(crate::builtins::datetime::date_to_serial(&date));
    }
    // "<date> <time>": split at the last run of whitespace whose right-hand
    // side parses as a time
    let mut split_points = text
        .char_indices()
        .filter(|(_, c)| c.is_whitespace())
        .map(|(i, _)| i)
        .collect::<Vec<_>>();
    split_points.reverse();
    for i in split_points {
        let (date_part, time_part) = (text[..i].trim_end(), text[i..].trim_start());
        if let (Some(date), Some(fraction)) =
            (parse_date_text(date_part), parse_time_text(time_part))
        {
            return Some(crate::builtins::datetime::date_to_serial(&date) + fraction);
        }
    }
    None
}

/// Excel: two-digit years 00–29 → 2000–2029, 30–99 → 1930–1999.
fn expand_year(text: &str) -> Option<i32> {
    let year: i32 = text.parse().ok()?;
    Some(match text.len() {
        4 => year,
        2 if year < 30 => 2000 + year,
        2 => 1900 + year,
        _ => return None,
    })
}

fn month_from_name(name: &str) -> Option<u32> {
    const MONTHS: [&str; 12] = [
        "january",
        "february",
        "march",
        "april",
        "may",
        "june",
        "july",
        "august",
        "september",
        "october",
        "november",
        "december",
    ];
    let lower = name.to_ascii_lowercase();
    if lower.len() < 3 {
        return None;
    }
    MONTHS
        .iter()
        .position(|m| m.starts_with(&lower))
        .map(|i| i as u32 + 1)
}

fn make_date(year: i32, month: u32, day: u32) -> Option<chrono::NaiveDate> {
    if !(1900..=9999).contains(&year) {
        return None;
    }
    chrono::NaiveDate::from_ymd_opt(year, month, day)
}

/// `1/2/2024`, `1/2/24`, `2024-01-02`, `2024/1/2`, `5-Jan-2024`, `5 Jan 2024`,
/// `Jan 5, 2024`, `January 5 2024`. Forms without a year are not accepted
/// (the engine has no ambient clock here).
fn parse_date_text(text: &str) -> Option<chrono::NaiveDate> {
    let parts: Vec<&str> = text
        .split(|c: char| c == '/' || c == '-' || c == ',' || c.is_whitespace())
        .filter(|p| !p.is_empty())
        .collect();
    if parts.len() != 3 {
        return None;
    }
    let is_digits = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
    let (a, b, c) = (parts[0], parts[1], parts[2]);
    if is_digits(a) && is_digits(b) && is_digits(c) {
        if a.len() == 4 {
            // y-m-d
            return make_date(a.parse().ok()?, b.parse().ok()?, c.parse().ok()?);
        }
        if a.len() <= 2 && b.len() <= 2 && (c.len() == 2 || c.len() == 4) {
            // m/d/y (en-US)
            return make_date(expand_year(c)?, a.parse().ok()?, b.parse().ok()?);
        }
        return None;
    }
    if is_digits(a) && is_digits(c) && a.len() <= 2 && (c.len() == 2 || c.len() == 4) {
        // d-mmm-y
        return make_date(expand_year(c)?, month_from_name(b)?, a.parse().ok()?);
    }
    if is_digits(b) && is_digits(c) && b.len() <= 2 && (c.len() == 2 || c.len() == 4) {
        // mmm d, y
        return make_date(expand_year(c)?, month_from_name(a)?, b.parse().ok()?);
    }
    None
}

/// `6:00 PM`, `18:30`, `6 pm`, `6:00:30` → fraction of a day. A bare number is
/// not a time.
fn parse_time_text(text: &str) -> Option<f64> {
    let lower = text.trim().to_ascii_lowercase();
    let (clock, meridiem) = if let Some(rest) = lower.strip_suffix("pm") {
        (rest.trim_end(), Some(true))
    } else if let Some(rest) = lower.strip_suffix("am") {
        (rest.trim_end(), Some(false))
    } else {
        (lower.as_str(), None)
    };
    let fields: Vec<&str> = clock.split(':').collect();
    if fields.is_empty() || fields.len() > 3 || (fields.len() == 1 && meridiem.is_none()) {
        return None;
    }
    let num = |s: &str| -> Option<u32> {
        (!s.is_empty() && s.len() <= 2 && s.bytes().all(|b| b.is_ascii_digit()))
            .then(|| s.parse().ok())
            .flatten()
    };
    let mut hour = num(fields[0])?;
    let minute = fields.get(1).map(|f| num(f)).unwrap_or(Some(0))?;
    let second = fields.get(2).map(|f| num(f)).unwrap_or(Some(0))?;
    if minute > 59 || second > 59 {
        return None;
    }
    match meridiem {
        Some(pm) => {
            if !(1..=12).contains(&hour) {
                return None;
            }
            hour = hour % 12 + if pm { 12 } else { 0 };
        }
        None if hour > 23 => return None,
        None => {}
    }
    Some(f64::from(hour * 3600 + minute * 60 + second) / 86400.0)
}

/// Logical coercion.
/// - Accepts Boolean
/// - Numbers: nonzero → true, zero → false
/// - Text: "TRUE"/"FALSE" (ASCII case-insensitive)
pub fn to_logical(value: &LiteralValue) -> Result<bool, ExcelError> {
    match value {
        LiteralValue::Boolean(b) => Ok(*b),
        LiteralValue::Number(n) => Ok(*n != 0.0),
        LiteralValue::Int(i) => Ok(*i != 0),
        LiteralValue::Text(s) => match s.to_ascii_lowercase().as_str() {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err(ExcelError::new(ExcelErrorKind::Value)
                .with_message("Cannot convert text to logical")),
        },
        LiteralValue::Empty => Ok(false),
        LiteralValue::Error(e) => Err(e.clone()),
        _ => Err(ExcelError::new(ExcelErrorKind::Value).with_message("Cannot convert to logical")),
    }
}

/// Invariant textification for comparisons/concatenation.
pub fn to_text_invariant(value: &LiteralValue) -> String {
    match value {
        LiteralValue::Text(s) => s.clone(),
        LiteralValue::Number(n) => n.to_string(),
        LiteralValue::Int(i) => i.to_string(),
        LiteralValue::Boolean(b) => if *b { "TRUE" } else { "FALSE" }.into(),
        LiteralValue::Error(e) => e.to_string(),
        LiteralValue::Empty => "".into(),
        // Dates/times/durations are stored as serial numbers in spreadsheet engines.
        // Use invariant numeric serialization so downstream consumers (e.g., criteria strings
        // like ">="&A1) parse consistently.
        LiteralValue::Date(_)
        | LiteralValue::DateTime(_)
        | LiteralValue::Time(_)
        | LiteralValue::Duration(_) => value.as_serial_number().unwrap_or(0.0).to_string(),
        other => format!("{other:?}"),
    }
}

/// Numeric sanitization: NaN/Inf → #NUM!
pub fn sanitize_numeric(n: f64) -> Result<f64, ExcelError> {
    if n.is_nan() || n.is_infinite() {
        return Err(ExcelError::new_num());
    }
    Ok(n)
}

/// Coerce to Excel serial (date/time/duration) or error.
pub fn to_datetime_serial(value: &LiteralValue) -> Result<f64, ExcelError> {
    match value.as_serial_number() {
        Some(n) => Ok(n),
        None => Err(ExcelError::new(ExcelErrorKind::Value)
            .with_message("Cannot convert to date/time serial")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_lenient_parses_text_and_booleans() {
        assert_eq!(
            to_number_lenient(&LiteralValue::Text(" 42 ".into())).unwrap(),
            42.0
        );
        assert_eq!(
            to_number_lenient(&LiteralValue::Boolean(true)).unwrap(),
            1.0
        );
        assert_eq!(to_number_lenient(&LiteralValue::Empty).unwrap(), 0.0);
    }

    #[test]
    fn number_lenient_parses_percent_text() {
        assert_eq!(
            to_number_lenient(&LiteralValue::Text("90%".into())).unwrap(),
            0.9
        );
        assert_eq!(
            to_number_lenient(&LiteralValue::Text(" 90.5% ".into())).unwrap(),
            0.905
        );
        assert!(to_number_lenient(&LiteralValue::Text("abc%".into())).is_err());
    }

    #[test]
    fn date_time_text_parses_to_serials() {
        assert_eq!(parse_date_time_text("1/2/2024"), Some(45293.0));
        assert_eq!(parse_date_time_text("2024-01-02"), Some(45293.0));
        assert_eq!(parse_date_time_text("1/2/24"), Some(45293.0));
        assert_eq!(parse_date_time_text("5-Jan-2024"), Some(45296.0));
        assert_eq!(parse_date_time_text("Jan 5, 2024"), Some(45296.0));
        assert_eq!(parse_date_time_text("6:00 PM"), Some(0.75));
        assert_eq!(parse_date_time_text("6 pm"), Some(0.75));
        assert_eq!(parse_date_time_text("18:30"), Some(18.5 / 24.0));
        assert_eq!(parse_date_time_text("1/2/2024 6:00 PM"), Some(45293.75));
        for s in ["2/30/2024", "25:00", "6", "13 PM", "1/2", "abc", ""] {
            assert_eq!(parse_date_time_text(s), None, "{s:?}");
        }
        assert_eq!(
            to_number_lenient(&LiteralValue::Text("$1,000".into())).unwrap(),
            1000.0
        );
    }

    #[test]
    fn number_strict_rejects_text() {
        assert!(to_number_strict(&LiteralValue::Text("1".into())).is_err());
    }

    #[test]
    fn logical_from_number_and_text() {
        assert!(to_logical(&LiteralValue::Int(5)).unwrap());
        assert!(!to_logical(&LiteralValue::Number(0.0)).unwrap());
        assert!(to_logical(&LiteralValue::Text("TRUE".into())).unwrap());
        assert!(to_logical(&LiteralValue::Text("true".into())).unwrap());
        assert!(to_logical(&LiteralValue::Text(" True ".into())).is_err());
    }

    #[test]
    fn sanitize_numeric_nan_inf() {
        assert!(sanitize_numeric(f64::NAN).is_err());
        assert!(sanitize_numeric(f64::INFINITY).is_err());
        assert_eq!(sanitize_numeric(1.5).unwrap(), 1.5);
    }
}
