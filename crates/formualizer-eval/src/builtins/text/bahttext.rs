//! BAHTTEXT — a number spelled out in Thai as baht and satang.

use super::super::utils::{ARG_NUM_LENIENT_ONE, coerce_num};
use crate::args::ArgSchema;
use crate::function::Function;
use crate::traits::{ArgumentHandle, CalcValue, FunctionContext};
use formualizer_common::{ExcelError, LiteralValue};
use formualizer_macros::func_caps;

const DIGITS: [&str; 10] = [
    "",
    "หนึ่ง",
    "สอง",
    "สาม",
    "สี่",
    "ห้า",
    "หก",
    "เจ็ด",
    "แปด",
    "เก้า",
];
const PLACES: [&str; 7] = ["", "สิบ", "ร้อย", "พัน", "หมื่น", "แสน", "ล้าน"];

/// Spell a run of decimal digits (most significant first); groups of more than six digits
/// recurse on the leading part and join with "ล้าน" (million).
fn digits_to_words(digits: &[u8]) -> String {
    const MAX_LEN: usize = 7;
    if digits.len() > MAX_LEN {
        let split = digits.len() - MAX_LEN + 1;
        return format!(
            "{}ล้าน{}",
            digits_to_words(&digits[..split]),
            digits_to_words(&digits[split..])
        );
    }
    let mut out = String::new();
    let len = digits.len();
    for (i, &d) in digits.iter().enumerate() {
        if d > 0 {
            out.push_str(DIGITS[d as usize]);
            out.push_str(PLACES[len - i - 1]);
        }
    }
    out
}

/// Thai reading rules: "one-ten" is "ten", "two-ten" is "yee-sip", trailing "ten-one" is "sip-et".
fn grammar_fix(s: &str) -> String {
    s.replace("หนึ่งสิบ", "สิบ")
        .replace("สองสิบ", "ยี่สิบ")
        .replace("สิบหนึ่ง", "สิบเอ็ด")
}

fn number_digits(n: u64) -> Vec<u8> {
    n.to_string().bytes().map(|b| b - b'0').collect()
}

/// The Thai text for `num` (what Excel's `BAHTTEXT` renders).
pub(crate) fn bahttext(num: f64) -> String {
    const ZERO: &str = "ศูนย์บาทถ้วน";
    if !num.is_finite() || num == 0.0 || num.abs() > 9_007_199_254_740_991.0 {
        return ZERO.to_string();
    }
    let positive = num.abs();
    let baht_int = positive.floor();
    // Satang round to the nearest whole (432.214567 → 21); 0.995 rounds up to 100 like JS toFixed.
    let satang_int = ((positive - baht_int) * 100.0).round();
    let baht = grammar_fix(&digits_to_words(&number_digits(baht_int as u64)));
    let satang = grammar_fix(&digits_to_words(&number_digits(satang_int as u64)));
    let body = match (baht.is_empty(), satang.is_empty()) {
        (true, true) => ZERO.to_string(),
        (false, true) => format!("{baht}บาทถ้วน"),
        (true, false) => format!("{satang}สตางค์"),
        (false, false) => format!("{baht}บาท{satang}สตางค์"),
    };
    if num < 0.0 {
        format!("ลบ{body}")
    } else {
        body
    }
}

/// Converts a number to Thai text and appends the baht / satang suffixes.
///
/// # Remarks
/// - The integer part is read as baht ("บาท"), the fractional part (rounded to two places) as
///   satang ("สตางค์"); a whole amount ends in "ถ้วน".
/// - Negative numbers are prefixed with "ลบ"; zero reads "ศูนย์บาทถ้วน".
/// - Non-numeric text returns `#VALUE!`; errors propagate unchanged.
///
/// ```yaml,sandbox
/// title: "Whole baht"
/// formula: '=BAHTTEXT(1234)'
/// expected: "หนึ่งพันสองร้อยสามสิบสี่บาทถ้วน"
/// ```
///
/// ```yaml,sandbox
/// title: "Baht and satang"
/// formula: '=BAHTTEXT(21.5)'
/// expected: "ยี่สิบเอ็ดบาทห้าสิบสตางค์"
/// ```
#[derive(Debug)]
pub struct BahtTextFn;
/// [formualizer-docgen:schema:start]
/// Name: BAHTTEXT
/// Type: BahtTextFn
/// Min args: 1
/// Max args: 1
/// Variadic: false
/// Signature: BAHTTEXT(arg1: number@scalar)
/// Arg schema: arg1{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}
/// Caps: PURE, ELEMENTWISE
/// [formualizer-docgen:schema:end]
impl Function for BahtTextFn {
    func_caps!(PURE, ELEMENTWISE);
    fn name(&self) -> &'static str {
        "BAHTTEXT"
    }
    fn min_args(&self) -> usize {
        1
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        &ARG_NUM_LENIENT_ONE[..]
    }
    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        _: &dyn FunctionContext<'b>,
    ) -> Result<CalcValue<'b>, ExcelError> {
        let n = match args[0].value()?.into_literal() {
            LiteralValue::Error(e) => return Ok(CalcValue::Scalar(LiteralValue::Error(e))),
            other => match coerce_num(&other) {
                Ok(n) => n,
                Err(e) => return Ok(CalcValue::Scalar(LiteralValue::Error(e))),
            },
        };
        Ok(CalcValue::Scalar(LiteralValue::Text(bahttext(n))))
    }
}

pub fn register_builtins() {
    use std::sync::Arc;
    crate::function_registry::register_builtin(Arc::new(BahtTextFn));
}

#[cfg(test)]
mod tests {
    use super::bahttext;

    #[test]
    fn spells_baht_and_satang() {
        assert_eq!(bahttext(0.0), "ศูนย์บาทถ้วน");
        assert_eq!(bahttext(1.0), "หนึ่งบาทถ้วน");
        assert_eq!(bahttext(11.0), "สิบเอ็ดบาทถ้วน");
        assert_eq!(bahttext(21.5), "ยี่สิบเอ็ดบาทห้าสิบสตางค์");
        assert_eq!(bahttext(0.25), "ยี่สิบห้าสตางค์");
        assert_eq!(bahttext(1234.0), "หนึ่งพันสองร้อยสามสิบสี่บาทถ้วน");
        assert_eq!(bahttext(1_000_000.0), "หนึ่งล้านบาทถ้วน");
        assert_eq!(
            bahttext(12_345_678.0),
            "สิบสองล้านสามแสนสี่หมื่นห้าพันหกร้อยเจ็ดสิบแปดบาทถ้วน"
        );
        assert_eq!(bahttext(-5.0), "ลบห้าบาทถ้วน");
    }
}
