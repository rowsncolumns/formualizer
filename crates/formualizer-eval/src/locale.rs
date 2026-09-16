/// Locale contract for the engine.
///
/// Milestone 0 intentionally uses an invariant locale:
///
/// - Numeric parsing is ASCII/invariant (en-US) only: `.` decimal separator, `,` thousands
///   separators, `$` currency, accounting parentheses and trailing percent suffixes
///   (`"$1,000"` -> 1000, `"(5)"` -> -5, `"90%"` -> 0.9).
/// - Strings are case-folded with ASCII-only rules (`to_ascii_lowercase`).
///
/// This means locale-dependent inputs like `"1.234,56"` are *not* interpreted as numbers.
/// Callers should surface `#VALUE!` for locale-dependent numeric coercions (e.g. `VALUE()`)
/// rather than silently producing a wrong number.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct Locale;

impl Locale {
    pub const fn invariant() -> Self {
        Locale
    }

    /// Parse a number using invariant rules (ASCII, dot decimal separator).
    ///
    /// Also supports percent-suffixed numeric text (e.g. "90%" -> 0.9),
    /// matching spreadsheet numeric-coercion behavior in numeric contexts.
    pub fn parse_number_invariant(&self, s: &str) -> Option<f64> {
        // Excel's text→number rules (invariant / en-US): surrounding whitespace,
        // accounting parentheses, a leading sign, a `$` currency symbol (before
        // or after the sign), `,` thousands separators in the integer part, an
        // exponent, and one or more trailing `%`. Anything else is not a number.
        let mut body = s.trim();
        if body.is_empty() {
            return None;
        }
        let mut negative = false;
        if let Some(inner) = body.strip_prefix('(').and_then(|b| b.strip_suffix(')')) {
            negative = true;
            body = inner.trim();
        }
        let mut take_sign = |body: &mut &str| {
            if let Some(rest) = body.strip_prefix('-') {
                negative = !negative;
                *body = rest.trim_start();
            } else if let Some(rest) = body.strip_prefix('+') {
                *body = rest.trim_start();
            }
        };
        take_sign(&mut body);
        if let Some(rest) = body.strip_prefix('$') {
            body = rest.trim_start();
            take_sign(&mut body);
        }
        let mut percent_divisor = 1.0;
        while let Some(rest) = body.strip_suffix('%') {
            percent_divisor *= 100.0;
            body = rest.trim_end();
        }
        if !is_invariant_numeric_body(body) {
            return None;
        }
        let digits: String = body.chars().filter(|c| *c != ',').collect();
        let n = digits.parse::<f64>().ok()?;
        if !n.is_finite() {
            return None;
        }
        Some(if negative { -n } else { n } / percent_divisor)
    }

    /// Case folding for comparisons; invariant = ASCII lower.
    pub fn fold_case_invariant(&self, s: &str) -> String {
        s.to_ascii_lowercase()
    }
}

/// `123`, `1,234.5`, `.5`, `5.`, `1e3`, `1,000e-2` — digits with optional `,`
/// groups in the integer part, an optional fraction and an optional exponent.
/// Rejects an empty integer part with commas, `,,`, a trailing `,` and a `,`
/// after the decimal point (`1,000,.5`).
fn is_invariant_numeric_body(body: &str) -> bool {
    let (mantissa, exponent) = match body.find(['e', 'E']) {
        Some(i) => (&body[..i], Some(&body[i + 1..])),
        None => (body, None),
    };
    if let Some(exp) = exponent {
        let exp = exp.strip_prefix(['+', '-']).unwrap_or(exp);
        if exp.is_empty() || !exp.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
    }
    let (int_part, frac_part) = match mantissa.find('.') {
        Some(i) => (&mantissa[..i], Some(&mantissa[i + 1..])),
        None => (mantissa, None),
    };
    if let Some(frac) = frac_part {
        if !frac.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
    }
    let has_int_digits = int_part.bytes().any(|b| b.is_ascii_digit());
    let has_frac_digits = frac_part.is_some_and(|f| !f.is_empty());
    if !has_int_digits && !has_frac_digits {
        return false;
    }
    if int_part.is_empty() {
        return true;
    }
    int_part.bytes().all(|b| b.is_ascii_digit() || b == b',')
        && int_part.as_bytes()[0].is_ascii_digit()
        && !int_part.ends_with(',')
        && !int_part.contains(",,")
}

#[cfg(test)]
mod tests {
    use super::Locale;

    #[test]
    fn parse_number_invariant_supports_percent_suffix() {
        let loc = Locale::invariant();
        assert_eq!(loc.parse_number_invariant("90%"), Some(0.9));
        assert_eq!(loc.parse_number_invariant(" 90.5% "), Some(0.905));
        assert_eq!(loc.parse_number_invariant("90 %"), Some(0.9));
    }

    #[test]
    fn parse_number_invariant_accepts_excel_numeric_text() {
        let loc = Locale::invariant();
        assert_eq!(loc.parse_number_invariant("1,000"), Some(1000.0));
        assert_eq!(loc.parse_number_invariant("$1,000.50"), Some(1000.5));
        assert_eq!(loc.parse_number_invariant("-$5"), Some(-5.0));
        assert_eq!(loc.parse_number_invariant("$-5"), Some(-5.0));
        assert_eq!(loc.parse_number_invariant("(5)"), Some(-5.0));
        assert_eq!(loc.parse_number_invariant("(12.5%)"), Some(-0.125));
        assert_eq!(loc.parse_number_invariant("1e3"), Some(1000.0));
        assert_eq!(loc.parse_number_invariant(".5"), Some(0.5));
        assert_eq!(loc.parse_number_invariant("5."), Some(5.0));
        assert_eq!(loc.parse_number_invariant(" 5 "), Some(5.0));
        assert_eq!(loc.parse_number_invariant("+7"), Some(7.0));
    }

    #[test]
    fn parse_number_invariant_rejects_non_numeric_text() {
        let loc = Locale::invariant();
        for s in [
            "", "abc", "TRUE", "1,,000", "1,000,", ",100", "0x10", "1/2/2024", "$", "()", "inf",
            "NaN", "1e", "1.2.3",
        ] {
            assert_eq!(loc.parse_number_invariant(s), None, "{s:?}");
        }
    }

    #[test]
    fn parse_number_invariant_rejects_invalid_percent_text() {
        let loc = Locale::invariant();
        assert_eq!(loc.parse_number_invariant("abc%"), None);
        assert_eq!(loc.parse_number_invariant("%"), None);
        assert_eq!(loc.parse_number_invariant("90% trailing"), None);
    }
}
