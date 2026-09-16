//! Bond / securities functions: ACCRINT, ACCRINTM, PRICE, YIELD, TBILLEQ, TBILLPRICE, TBILLYIELD,
//! COUPDAYBS, COUPDAYS, COUPDAYSNC, COUPNCD, COUPPCD, COUPNUM, DURATION, MDURATION, DISC, PRICEDISC,
//! YIELDDISC, INTRATE, RECEIVED, PRICEMAT, YIELDMAT, ODDFPRICE, ODDFYIELD, ODDLPRICE, ODDLYIELD.
//!
//! Day-count conventions are shared with the JS engine (`fast-formula-parser/formulas/functions/financial.js`):
//! `basis` 0 = US (NASD) 30/360 (last-of-February rules), 1 = actual/actual (Excel's YEARFRAC
//! algorithm), 2 = actual/360, 3 = actual/365, 4 = European 30/360; the coupon schedule steps back
//! from maturity with month-end pinning; odd periods are measured in quasi-coupon periods
//! (Σ A_i / NL_i) anchored on the odd coupon date, as Excel documents for ACCRINT and the ODD*
//! functions.

use crate::args::ArgSchema;
use crate::builtins::datetime::serial_to_date;
use crate::function::Function;
use crate::traits::{ArgumentHandle, CalcValue, FunctionContext};
use chrono::{Datelike, NaiveDate};
use formualizer_common::{ExcelError, LiteralValue};
use formualizer_macros::func_caps;

fn coerce_num(arg: &ArgumentHandle) -> Result<f64, ExcelError> {
    let v = arg.value()?.into_literal();
    coerce_literal_num(&v)
}

fn coerce_literal_num(v: &LiteralValue) -> Result<f64, ExcelError> {
    match v {
        LiteralValue::Number(f) => Ok(*f),
        LiteralValue::Int(i) => Ok(*i as f64),
        LiteralValue::Boolean(b) => Ok(if *b { 1.0 } else { 0.0 }),
        LiteralValue::Empty => Ok(0.0),
        LiteralValue::Error(e) => Err(e.clone()),
        _ => Err(ExcelError::new_value()),
    }
}

/// Optional numeric argument: absent, blank, or a skipped slot (`FN(a, b,)` — the parser emits an
/// empty text literal) yields `None` so the caller applies Excel's default.
fn opt_num(args: &[ArgumentHandle], index: usize) -> Result<Option<f64>, ExcelError> {
    match args.get(index) {
        None => Ok(None),
        Some(arg) => match arg.value()?.into_literal() {
            LiteralValue::Empty => Ok(None),
            LiteralValue::Text(t) if t.is_empty() => Ok(None),
            v => coerce_literal_num(&v).map(Some),
        },
    }
}

/// Day count basis calculation
/// Returns (num_days, year_basis) for the given basis type
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum DayCountBasis {
    UsNasd30360 = 0,   // US (NASD) 30/360
    ActualActual = 1,  // Actual/actual
    Actual360 = 2,     // Actual/360
    Actual365 = 3,     // Actual/365
    European30360 = 4, // European 30/360
}

impl DayCountBasis {
    pub(super) fn from_int(basis: i32) -> Result<Self, ExcelError> {
        match basis {
            0 => Ok(DayCountBasis::UsNasd30360),
            1 => Ok(DayCountBasis::ActualActual),
            2 => Ok(DayCountBasis::Actual360),
            3 => Ok(DayCountBasis::Actual365),
            4 => Ok(DayCountBasis::European30360),
            _ => Err(ExcelError::new_num()),
        }
    }
}

/// Check if a year is a leap year
fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

/// Check if a date is the last day of the month
fn is_last_day_of_month(date: &NaiveDate) -> bool {
    let next_day = *date + chrono::Duration::days(1);
    next_day.month() != date.month()
}

/// Calculate days between two dates using the specified basis
fn days_between(start: &NaiveDate, end: &NaiveDate, basis: DayCountBasis) -> i32 {
    match basis {
        DayCountBasis::UsNasd30360 => days_30_360_us(start, end),
        DayCountBasis::ActualActual | DayCountBasis::Actual360 | DayCountBasis::Actual365 => {
            (*end - *start).num_days() as i32
        }
        DayCountBasis::European30360 => days_30_360_eu(start, end),
    }
}

/// Calculate days using US (NASD) 30/360 method
fn days_30_360_us(start: &NaiveDate, end: &NaiveDate) -> i32 {
    let mut sd = start.day() as i32;
    let sm = start.month() as i32;
    let sy = start.year();

    let mut ed = end.day() as i32;
    let em = end.month() as i32;
    let ey = end.year();

    // Adjust for last day of February
    let start_is_last_feb = sm == 2 && is_last_day_of_month(start);
    let end_is_last_feb = em == 2 && is_last_day_of_month(end);

    if start_is_last_feb && end_is_last_feb {
        ed = 30;
    }
    if start_is_last_feb {
        sd = 30;
    }
    if ed == 31 && sd >= 30 {
        ed = 30;
    }
    if sd == 31 {
        sd = 30;
    }

    (ey - sy) * 360 + (em - sm) * 30 + (ed - sd)
}

/// Calculate days using European 30/360 method
fn days_30_360_eu(start: &NaiveDate, end: &NaiveDate) -> i32 {
    let mut sd = start.day() as i32;
    let sm = start.month() as i32;
    let sy = start.year();

    let mut ed = end.day() as i32;
    let em = end.month() as i32;
    let ey = end.year();

    if sd == 31 {
        sd = 30;
    }
    if ed == 31 {
        ed = 30;
    }

    (ey - sy) * 360 + (em - sm) * 30 + (ed - sd)
}

/// Excel's YEARFRAC basis-1 "one year or less" test (`s <= e` assumed): same calendar year, or
/// consecutive years with the end month/day not past the start month/day.
fn spans_at_most_one_year(s: &NaiveDate, e: &NaiveDate) -> bool {
    s.year() == e.year()
        || (e.year() == s.year() + 1
            && (s.month() > e.month() || (s.month() == e.month() && s.day() >= e.day())))
}

/// A real Feb 29 falls inside `[s, e]` (`s <= e` assumed).
fn feb29_between(s: &NaiveDate, e: &NaiveDate) -> bool {
    let mar1 = |y: i32| NaiveDate::from_ymd_opt(y, 3, 1).expect("mar 1");
    (is_leap_year(s.year()) && *s < mar1(s.year()) && *e >= mar1(s.year()))
        || (is_leap_year(e.year()) && *s < mar1(e.year()) && *e >= mar1(e.year()))
}

/// Excel's YEARFRAC arithmetic for `basis`.
///
/// Actual/actual follows Excel's documented algorithm (the one the host `YEARFRAC` implements): a
/// span of one year or less is divided by 366 when it lies in a single leap year or straddles a
/// Feb 29 (else 365); a longer span is divided by the average length of the calendar years it
/// touches. The discount / at-maturity securities and ACCRINTM are defined in terms of it.
pub(super) fn year_fraction(start: &NaiveDate, end: &NaiveDate, basis: DayCountBasis) -> f64 {
    if start == end {
        return 0.0;
    }

    let (s, e, sign) = if start <= end {
        (start, end, 1.0)
    } else {
        (end, start, -1.0)
    };

    let actual_days = (*e - *s).num_days() as f64;
    let frac = match basis {
        DayCountBasis::UsNasd30360 | DayCountBasis::European30360 => {
            days_between(s, e, basis) as f64 / 360.0
        }
        DayCountBasis::Actual360 => actual_days / 360.0,
        DayCountBasis::Actual365 => actual_days / 365.0,
        DayCountBasis::ActualActual => {
            let denominator = if spans_at_most_one_year(s, e) {
                if (s.year() == e.year() && is_leap_year(s.year())) || feb29_between(s, e) {
                    366.0
                } else {
                    365.0
                }
            } else {
                let total: i64 = (s.year()..=e.year())
                    .map(|y| if is_leap_year(y) { 366 } else { 365 })
                    .sum();
                total as f64 / (e.year() - s.year() + 1) as f64
            };
            actual_days / denominator
        }
    };

    sign * frac
}

fn days_in_month(year: i32, month: u32) -> u32 {
    let (ny, nm) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    NaiveDate::from_ymd_opt(ny, nm, 1)
        .unwrap()
        .signed_duration_since(NaiveDate::from_ymd_opt(year, month, 1).unwrap())
        .num_days() as u32
}

/// The coupon date `months_back` months before `maturity` on Excel's back-from-maturity
/// schedule: maturity's day-of-month is kept (clamped to each month's length) and a maturity on
/// the last day of its month pins every coupon date to month-end.
fn coupon_date_back(maturity: &NaiveDate, months_back: i32) -> NaiveDate {
    let eom = maturity.day() == days_in_month(maturity.year(), maturity.month());
    let total = maturity.year() * 12 + (maturity.month() as i32 - 1) - months_back;
    let year = total.div_euclid(12);
    let month = (total.rem_euclid(12) + 1) as u32;
    let dim = days_in_month(year, month);
    let day = if eom { dim } else { maturity.day().min(dim) };
    NaiveDate::from_ymd_opt(year, month, day).expect("coupon date within month length")
}

/// The coupon period containing `settlement`: the previous coupon date (`pcd`, at or before
/// settlement), the next one (`ncd`, strictly after) and `num`, the number of coupons still
/// payable after settlement up to and including maturity (Excel's COUPPCD / COUPNCD / COUPNUM).
struct CouponPeriod {
    pcd: NaiveDate,
    ncd: NaiveDate,
    num: i32,
}

fn coupon_period(settlement: &NaiveDate, maturity: &NaiveDate, frequency: i32) -> CouponPeriod {
    let step = 12 / frequency;
    let mut j = 1;
    while coupon_date_back(maturity, j * step) > *settlement {
        j += 1;
    }
    CouponPeriod {
        pcd: coupon_date_back(maturity, j * step),
        ncd: coupon_date_back(maturity, (j - 1) * step),
        num: j,
    }
}

/// COUPDAYS — days in the coupon period: actual for actual/actual, else the nominal year over
/// the frequency.
fn coupon_days(period: &CouponPeriod, frequency: i32, basis: DayCountBasis) -> f64 {
    match basis {
        DayCountBasis::ActualActual => (period.ncd - period.pcd).num_days() as f64,
        DayCountBasis::Actual365 => 365.0 / frequency as f64,
        _ => 360.0 / frequency as f64,
    }
}

/// COUPDAYBS — days from the previous coupon date to settlement on the given basis.
fn coupon_days_bs(period: &CouponPeriod, settlement: &NaiveDate, basis: DayCountBasis) -> f64 {
    days_between(&period.pcd, settlement, basis) as f64
}

/// COUPDAYSNC — days from settlement to the next coupon date. On the 30/360 bases Excel takes
/// the remainder of the nominal period (`COUPDAYS - COUPDAYBS`); otherwise the actual day span.
fn coupon_days_nc(
    period: &CouponPeriod,
    settlement: &NaiveDate,
    frequency: i32,
    basis: DayCountBasis,
) -> f64 {
    match basis {
        DayCountBasis::UsNasd30360 | DayCountBasis::European30360 => {
            coupon_days(period, frequency, basis) - coupon_days_bs(period, settlement, basis)
        }
        _ => (period.ncd - *settlement).num_days() as f64,
    }
}

/// Σ A_i / NL_i over the quasi-coupon periods (anchored at `anchor`, 12/frequency months apart)
/// that cover `[start, end]`: A_i is the part of the span inside period i counted under `basis`,
/// NL_i the period's normal length (the COUPDAYS rule). This is the odd-period arithmetic Excel
/// documents for ACCRINT and the ODD* functions; on the 30/360 bases it reduces to
/// `days_between / 360 × frequency`.
fn quasi_coupon_fraction(
    start: &NaiveDate,
    end: &NaiveDate,
    anchor: &NaiveDate,
    frequency: i32,
    basis: DayCountBasis,
) -> f64 {
    if end <= start {
        return 0.0;
    }
    let step = 12 / frequency;
    let quasi_date = |k: i32| coupon_date_back(anchor, -k * step);
    let mut k: i32 = 0;
    while quasi_date(k) > *start {
        k -= 1;
    }
    while quasi_date(k + 1) <= *start {
        k += 1;
    }
    let mut sum = 0.0;
    loop {
        let q0 = quasi_date(k);
        let q1 = quasi_date(k + 1);
        let from = if q0 > *start { q0 } else { *start };
        let to = if q1 < *end { q1 } else { *end };
        let normal_length = match basis {
            DayCountBasis::ActualActual => (q1 - q0).num_days() as f64,
            DayCountBasis::Actual365 => 365.0 / frequency as f64,
            _ => 360.0 / frequency as f64,
        };
        sum += days_between(&from, &to, basis) as f64 / normal_length;
        if *end <= q1 {
            return sum;
        }
        k += 1;
    }
}

/// Returns accrued interest for a coupon-bearing security.
///
/// `ACCRINT` accrues `par × rate / frequency × Σ A_i / NL_i` over the quasi-coupon periods
/// anchored on `first_interest` (Excel's odd-period arithmetic), from `issue` — or, when
/// `calc_method` is FALSE and settlement is past `first_interest`, from `first_interest`.
///
/// # Remarks
/// - Date inputs are spreadsheet serial dates; `settlement` must be after `issue`.
/// - `rate` is the annual coupon rate as a decimal (for example, `0.06` for 6%), and `par` is principal amount; both must be positive.
/// - `frequency` must be `1` (annual), `2` (semiannual), or `4` (quarterly).
/// - `basis` codes: `0=US(NASD)30/360`, `1=Actual/Actual`, `2=Actual/360`, `3=Actual/365`, `4=European30/360`.
/// - `calc_method` (default TRUE) only matters when `settlement > first_interest`: TRUE returns the total accrued from `issue`, FALSE the accrued interest from `first_interest`.
/// - Return value is in the same currency units as `par` and is positive for valid positive inputs.
///
/// # Examples
///
/// ```yaml,sandbox
/// title: "Microsoft reference: 60 days of a 10% semiannual coupon on 1000 (30/360)"
/// formula: "=ACCRINT(DATE(2008,3,1), DATE(2008,8,31), DATE(2008,5,1), 0.1, 1000, 2, 0)"
/// expected: 16.666666666666668
/// ```
///
/// ```yaml,sandbox
/// title: "calc_method FALSE accrues from first_interest once settlement passes it"
/// formula: "=ACCRINT(DATE(2024,1,1), DATE(2024,7,1), DATE(2024,10,1), 0.08, 1000, 2, 0, 0)"
/// expected: 20
/// ```
/// ```yaml,docs
/// related:
///   - ACCRINTM
///   - PRICE
///   - YIELD
/// faq:
///   - q: "When does `calc_method` change the result?"
///     a: "Only when `settlement` is later than `first_interest`: FALSE accrues from `first_interest`, TRUE (the default) accrues the total from `issue`."
///   - q: "Which inputs return `#NUM!`?"
///     a: "Invalid `basis`, non-positive `rate`/`par`, unsupported `frequency`, or `settlement <= issue` return `#NUM!`."
/// ```
#[derive(Debug)]
pub struct AccrintFn;

/// [formualizer-docgen:schema:start]
/// Name: ACCRINT
/// Type: AccrintFn
/// Min args: 6
/// Max args: variadic
/// Variadic: true
/// Signature: ACCRINT(arg1: number@scalar, arg2: number@scalar, arg3: number@scalar, arg4: number@scalar, arg5: number@scalar, arg6: number@scalar, arg7: number@scalar, arg8...: number@scalar)
/// Arg schema: arg1{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg2{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg3{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg4{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg5{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg6{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg7{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg8{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}
/// Caps: PURE
/// [formualizer-docgen:schema:end]
impl Function for AccrintFn {
    func_caps!(PURE);
    fn name(&self) -> &'static str {
        "ACCRINT"
    }
    fn min_args(&self) -> usize {
        6
    }
    fn variadic(&self) -> bool {
        true
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        use std::sync::LazyLock;
        static SCHEMA: LazyLock<Vec<ArgSchema>> = LazyLock::new(|| {
            vec![
                ArgSchema::number_lenient_scalar(), // issue
                ArgSchema::number_lenient_scalar(), // first_interest
                ArgSchema::number_lenient_scalar(), // settlement
                ArgSchema::number_lenient_scalar(), // rate
                ArgSchema::number_lenient_scalar(), // par
                ArgSchema::number_lenient_scalar(), // frequency
                ArgSchema::number_lenient_scalar(), // basis (optional)
                ArgSchema::number_lenient_scalar(), // calc_method (optional)
            ]
        });
        &SCHEMA[..]
    }
    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        _ctx: &dyn FunctionContext<'b>,
    ) -> Result<CalcValue<'b>, ExcelError> {
        // Check minimum required arguments
        if args.len() < 6 {
            return Ok(CalcValue::Scalar(LiteralValue::Error(
                ExcelError::new_value(),
            )));
        }

        let issue = serial_to_date(coerce_num(&args[0])?)?;
        let first_interest = serial_to_date(coerce_num(&args[1])?)?;
        let settlement = serial_to_date(coerce_num(&args[2])?)?;
        let rate = coerce_num(&args[3])?;
        let par = coerce_num(&args[4])?;
        let frequency = coerce_num(&args[5])?.trunc() as i32;
        let basis_int = opt_num(args, 6)?.map_or(0, |v| v.trunc() as i32);
        let calc_method = opt_num(args, 7)?.is_none_or(|v| v != 0.0);

        // Validate inputs
        if rate <= 0.0 || par <= 0.0 {
            return Ok(CalcValue::Scalar(
                LiteralValue::Error(ExcelError::new_num()),
            ));
        }
        if frequency != 1 && frequency != 2 && frequency != 4 {
            return Ok(CalcValue::Scalar(
                LiteralValue::Error(ExcelError::new_num()),
            ));
        }

        let basis = DayCountBasis::from_int(basis_int)?;

        // settlement must be after issue
        if settlement <= issue {
            return Ok(CalcValue::Scalar(
                LiteralValue::Error(ExcelError::new_num()),
            ));
        }

        // calc_method only matters once settlement passes first_interest: TRUE accrues the total
        // from issue, FALSE only from first_interest. Interest accrues per quasi-coupon period
        // anchored on first_interest (par × rate / frequency × Σ A_i / NL_i).
        let start = if !calc_method && settlement > first_interest {
            first_interest
        } else {
            issue
        };
        let accrued_interest = par * rate / frequency as f64
            * quasi_coupon_fraction(&start, &settlement, &first_interest, frequency, basis);

        Ok(CalcValue::Scalar(LiteralValue::Number(accrued_interest)))
    }
}

/// Returns accrued interest for a security that pays interest at maturity.
///
/// `ACCRINTM` accrues from `issue` to `settlement` with no periodic coupon schedule.
///
/// # Remarks
/// - Date inputs are spreadsheet serial dates and must satisfy `settlement > issue`.
/// - `rate` is an annual decimal rate (for example, `0.05` for 5%), and `par` must be positive.
/// - `basis` codes: `0=US(NASD)30/360`, `1=Actual/Actual`, `2=Actual/360`, `3=Actual/365`, `4=European30/360`.
/// - Return value is accrued interest amount in the same currency units as `par`.
/// - With positive `rate` and `par`, the result is positive.
///
/// # Examples
///
/// ```yaml,sandbox
/// title: "One full 30/360 year"
/// formula: "=ACCRINTM(DATE(2024,1,1), DATE(2025,1,1), 0.05, 1000, 0)"
/// expected: 50
/// ```
///
/// ```yaml,sandbox
/// title: "European 30/360 half-year accrual"
/// formula: "=ACCRINTM(DATE(2024,2,28), DATE(2024,8,28), 0.04, 1000, 4)"
/// expected: 20
/// ```
/// ```yaml,docs
/// related:
///   - ACCRINT
///   - PRICE
///   - YIELD
/// faq:
///   - q: "Does `ACCRINTM` use coupon frequency?"
///     a: "No. It accrues directly from `issue` to `settlement` with no periodic coupon schedule."
///   - q: "What causes `#NUM!`?"
///     a: "Invalid `basis`, non-positive `rate`/`par`, or `settlement <= issue`."
/// ```
#[derive(Debug)]
pub struct AccrintmFn;

/// [formualizer-docgen:schema:start]
/// Name: ACCRINTM
/// Type: AccrintmFn
/// Min args: 4
/// Max args: variadic
/// Variadic: true
/// Signature: ACCRINTM(arg1: number@scalar, arg2: number@scalar, arg3: number@scalar, arg4: number@scalar, arg5...: number@scalar)
/// Arg schema: arg1{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg2{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg3{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg4{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg5{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}
/// Caps: PURE
/// [formualizer-docgen:schema:end]
impl Function for AccrintmFn {
    func_caps!(PURE);
    fn name(&self) -> &'static str {
        "ACCRINTM"
    }
    fn min_args(&self) -> usize {
        4
    }
    fn variadic(&self) -> bool {
        true
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        use std::sync::LazyLock;
        static SCHEMA: LazyLock<Vec<ArgSchema>> = LazyLock::new(|| {
            vec![
                ArgSchema::number_lenient_scalar(), // issue
                ArgSchema::number_lenient_scalar(), // settlement
                ArgSchema::number_lenient_scalar(), // rate
                ArgSchema::number_lenient_scalar(), // par
                ArgSchema::number_lenient_scalar(), // basis (optional)
            ]
        });
        &SCHEMA[..]
    }
    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        _ctx: &dyn FunctionContext<'b>,
    ) -> Result<CalcValue<'b>, ExcelError> {
        // Check minimum required arguments
        if args.len() < 4 {
            return Ok(CalcValue::Scalar(LiteralValue::Error(
                ExcelError::new_value(),
            )));
        }

        let issue_serial = coerce_num(&args[0])?;
        let settlement_serial = coerce_num(&args[1])?;
        let rate = coerce_num(&args[2])?;
        let par = coerce_num(&args[3])?;
        let basis_int = if args.len() > 4 {
            coerce_num(&args[4])?.trunc() as i32
        } else {
            0
        };

        // Validate inputs
        if rate <= 0.0 || par <= 0.0 {
            return Ok(CalcValue::Scalar(
                LiteralValue::Error(ExcelError::new_num()),
            ));
        }

        let basis = DayCountBasis::from_int(basis_int)?;

        let issue = serial_to_date(issue_serial)?;
        let settlement = serial_to_date(settlement_serial)?;

        // settlement must be after issue
        if settlement <= issue {
            return Ok(CalcValue::Scalar(
                LiteralValue::Error(ExcelError::new_num()),
            ));
        }

        // ACCRINTM = par * rate * year_fraction(issue, settlement)
        let yf = year_fraction(&issue, &settlement, basis);
        let accrued_interest = par * rate * yf;

        Ok(CalcValue::Scalar(LiteralValue::Number(accrued_interest)))
    }
}

/// Returns clean price per 100 face value for a coupon-paying security.
///
/// `PRICE` discounts remaining coupons and redemption to settlement and subtracts accrued
/// coupon interest according to the chosen day-count basis.
///
/// # Remarks
/// - Date inputs are spreadsheet serial dates and must satisfy `maturity > settlement`.
/// - `rate` (coupon) and `yld` (yield) are annual decimal rates; `redemption` is amount paid per 100 face value at maturity.
/// - `frequency` must be `1` (annual), `2` (semiannual), or `4` (quarterly).
/// - `basis` codes: `0=US(NASD)30/360`, `1=Actual/Actual`, `2=Actual/360`, `3=Actual/365`, `4=European30/360`.
/// - Return value is quoted per 100 face value; positive inputs usually produce a positive price.
/// - Coupon schedule is derived by stepping backward from `maturity`, with end-of-month adjustment behavior in month arithmetic.
///
/// # Examples
///
/// ```yaml,sandbox
/// title: "Single remaining coupon period"
/// formula: "=PRICE(DATE(2024,4,1), DATE(2024,7,1), 0.06, 0.05, 100, 2, 0)"
/// expected: 100.2283950617284
/// ```
///
/// ```yaml,sandbox
/// title: "Par bond when coupon rate equals yield"
/// formula: "=PRICE(DATE(2024,3,1), DATE(2026,3,1), 0.05, 0.05, 100, 2, 0)"
/// expected: 100
/// ```
/// ```yaml,docs
/// related:
///   - YIELD
///   - ACCRINT
///   - ACCRINTM
/// faq:
///   - q: "Why is `PRICE` quoted per 100 even if my bond face value differs?"
///     a: "This implementation follows Excel quoting conventions and returns clean price per 100 face value."
///   - q: "Which domain checks return `#NUM!`?"
///     a: "`maturity <= settlement`, invalid `basis`, unsupported `frequency`, negative `rate`/`yld`, or non-positive `redemption`."
/// ```
#[derive(Debug)]
pub struct PriceFn;

/// [formualizer-docgen:schema:start]
/// Name: PRICE
/// Type: PriceFn
/// Min args: 6
/// Max args: variadic
/// Variadic: true
/// Signature: PRICE(arg1: number@scalar, arg2: number@scalar, arg3: number@scalar, arg4: number@scalar, arg5: number@scalar, arg6: number@scalar, arg7...: number@scalar)
/// Arg schema: arg1{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg2{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg3{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg4{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg5{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg6{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg7{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}
/// Caps: PURE
/// [formualizer-docgen:schema:end]
impl Function for PriceFn {
    func_caps!(PURE);
    fn name(&self) -> &'static str {
        "PRICE"
    }
    fn min_args(&self) -> usize {
        6
    }
    fn variadic(&self) -> bool {
        true
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        use std::sync::LazyLock;
        static SCHEMA: LazyLock<Vec<ArgSchema>> = LazyLock::new(|| {
            vec![
                ArgSchema::number_lenient_scalar(), // settlement
                ArgSchema::number_lenient_scalar(), // maturity
                ArgSchema::number_lenient_scalar(), // rate (coupon rate)
                ArgSchema::number_lenient_scalar(), // yld (yield)
                ArgSchema::number_lenient_scalar(), // redemption
                ArgSchema::number_lenient_scalar(), // frequency
                ArgSchema::number_lenient_scalar(), // basis (optional)
            ]
        });
        &SCHEMA[..]
    }
    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        _ctx: &dyn FunctionContext<'b>,
    ) -> Result<CalcValue<'b>, ExcelError> {
        // Check minimum required arguments
        if args.len() < 6 {
            return Ok(CalcValue::Scalar(LiteralValue::Error(
                ExcelError::new_value(),
            )));
        }

        let settlement_serial = coerce_num(&args[0])?;
        let maturity_serial = coerce_num(&args[1])?;
        let rate = coerce_num(&args[2])?;
        let yld = coerce_num(&args[3])?;
        let redemption = coerce_num(&args[4])?;
        let frequency = coerce_num(&args[5])?.trunc() as i32;
        let basis_int = if args.len() > 6 {
            coerce_num(&args[6])?.trunc() as i32
        } else {
            0
        };

        // Validate inputs
        if rate < 0.0 || yld < 0.0 || redemption <= 0.0 {
            return Ok(CalcValue::Scalar(
                LiteralValue::Error(ExcelError::new_num()),
            ));
        }
        if frequency != 1 && frequency != 2 && frequency != 4 {
            return Ok(CalcValue::Scalar(
                LiteralValue::Error(ExcelError::new_num()),
            ));
        }

        let basis = DayCountBasis::from_int(basis_int)?;

        let settlement = serial_to_date(settlement_serial)?;
        let maturity = serial_to_date(maturity_serial)?;

        // maturity must be after settlement
        if maturity <= settlement {
            return Ok(CalcValue::Scalar(
                LiteralValue::Error(ExcelError::new_num()),
            ));
        }

        let price = calculate_price(
            &settlement,
            &maturity,
            rate,
            yld,
            redemption,
            frequency,
            basis,
        );
        Ok(CalcValue::Scalar(LiteralValue::Number(price)))
    }
}

/// Calculate bond price using standard bond pricing formula
/// Excel's PRICE formula. With `N` coupons left, `E` days in the coupon period, `A` days accrued
/// since the previous coupon and `DSC` days to the next one, each cash flow `k` (1-based) is
/// discounted by `(1 + yld/f)^(k - 1 + DSC/E)` and the accrued coupon `100·rate/f · A/E` is
/// subtracted; a single remaining coupon is discounted linearly.
fn calculate_price(
    settlement: &NaiveDate,
    maturity: &NaiveDate,
    rate: f64,
    yld: f64,
    redemption: f64,
    frequency: i32,
    basis: DayCountBasis,
) -> f64 {
    let period = coupon_period(settlement, maturity, frequency);
    let f = frequency as f64;
    let e = coupon_days(&period, frequency, basis);
    let a = coupon_days_bs(&period, settlement, basis);
    let dsc = coupon_days_nc(&period, settlement, frequency, basis);
    let n = period.num;
    let coupon = 100.0 * rate / f;
    let accrued = coupon * a / e;

    if n <= 1 {
        return (redemption + coupon) / (1.0 + dsc / e * yld / f) - accrued;
    }

    let base = 1.0 + yld / f;
    let frac = dsc / e;
    let mut pv = redemption / base.powf(n as f64 - 1.0 + frac);
    for k in 1..=n {
        pv += coupon / base.powf(k as f64 - 1.0 + frac);
    }
    pv - accrued
}

/// Excel's closed-form YIELD for a security with at most one coupon left.
fn yield_single_coupon(
    settlement: &NaiveDate,
    maturity: &NaiveDate,
    rate: f64,
    price: f64,
    redemption: f64,
    frequency: i32,
    basis: DayCountBasis,
) -> f64 {
    let period = coupon_period(settlement, maturity, frequency);
    let f = frequency as f64;
    let e = coupon_days(&period, frequency, basis);
    let a = coupon_days_bs(&period, settlement, basis);
    let dsr = coupon_days_nc(&period, settlement, frequency, basis);
    let coupon = rate / f;
    let paid = price / 100.0 + a / e * coupon;
    (redemption / 100.0 + coupon - paid) / paid * (f * e / dsr)
}

/// Macaulay duration (years) of a coupon bond: the present-value-weighted average time to each
/// cash flow, with the same fractional first period `DSC/E` as PRICE.
fn macaulay_duration(
    settlement: &NaiveDate,
    maturity: &NaiveDate,
    coupon_rate: f64,
    yld: f64,
    frequency: i32,
    basis: DayCountBasis,
) -> f64 {
    let period = coupon_period(settlement, maturity, frequency);
    let f = frequency as f64;
    let e = coupon_days(&period, frequency, basis);
    let dsc = coupon_days_nc(&period, settlement, frequency, basis);
    let frac = dsc / e;
    let n = period.num;
    let coupon = 100.0 * coupon_rate / f;
    let base = 1.0 + yld / f;

    let mut weighted = 0.0;
    let mut pv = 0.0;
    for k in 1..=n {
        let t = k as f64 - 1.0 + frac;
        let cash = if k == n { coupon + 100.0 } else { coupon };
        let discounted = cash / base.powf(t);
        weighted += t * discounted;
        pv += discounted;
    }
    weighted / pv / f
}

/// Returns annual yield for a coupon-paying security from its market price.
///
/// `YIELD` solves for the annual rate that makes `PRICE(...)` match the input `pr`.
///
/// # Remarks
/// - Date inputs are spreadsheet serial dates and must satisfy `maturity > settlement`.
/// - `rate` is coupon rate (annual decimal), `pr` is price per 100 face value, and `redemption` is redemption per 100; `pr` and `redemption` must be positive.
/// - `frequency` must be `1` (annual), `2` (semiannual), or `4` (quarterly).
/// - `basis` codes: `0=US(NASD)30/360`, `1=Actual/Actual`, `2=Actual/360`, `3=Actual/365`, `4=European30/360`.
/// - Result is an annualized decimal yield (for example, `0.05` means 5%).
/// - This implementation uses Newton-Raphson iteration; if it cannot converge, it returns `#NUM!`.
///
/// # Examples
///
/// ```yaml,sandbox
/// title: "Par price implies coupon-rate yield"
/// formula: "=YIELD(DATE(2024,3,1), DATE(2026,3,1), 0.05, 100, 100, 2, 0)"
/// expected: 0.05
/// ```
///
/// ```yaml,sandbox
/// title: "Yield recovered from a discounted price"
/// formula: "=YIELD(DATE(2024,2,15), DATE(2027,2,15), 0.05, 97.2914042780609, 100, 2, 0)"
/// expected: 0.06
/// ```
/// ```yaml,docs
/// related:
///   - PRICE
///   - ACCRINT
///   - ACCRINTM
/// faq:
///   - q: "What does the returned `YIELD` represent?"
///     a: "It is an annualized decimal yield (for example, `0.06` means 6% per year)."
///   - q: "When does `YIELD` return `#NUM!` besides invalid inputs?"
///     a: "The Newton-Raphson solve can fail to converge or hit an unstable derivative; in those cases it returns `#NUM!`."
/// ```
#[derive(Debug)]
pub struct YieldFn;

/// [formualizer-docgen:schema:start]
/// Name: YIELD
/// Type: YieldFn
/// Min args: 6
/// Max args: variadic
/// Variadic: true
/// Signature: YIELD(arg1: number@scalar, arg2: number@scalar, arg3: number@scalar, arg4: number@scalar, arg5: number@scalar, arg6: number@scalar, arg7...: number@scalar)
/// Arg schema: arg1{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg2{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg3{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg4{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg5{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg6{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg7{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}
/// Caps: PURE
/// [formualizer-docgen:schema:end]
impl Function for YieldFn {
    func_caps!(PURE);
    fn name(&self) -> &'static str {
        "YIELD"
    }
    fn min_args(&self) -> usize {
        6
    }
    fn variadic(&self) -> bool {
        true
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        use std::sync::LazyLock;
        static SCHEMA: LazyLock<Vec<ArgSchema>> = LazyLock::new(|| {
            vec![
                ArgSchema::number_lenient_scalar(), // settlement
                ArgSchema::number_lenient_scalar(), // maturity
                ArgSchema::number_lenient_scalar(), // rate (coupon rate)
                ArgSchema::number_lenient_scalar(), // pr (price)
                ArgSchema::number_lenient_scalar(), // redemption
                ArgSchema::number_lenient_scalar(), // frequency
                ArgSchema::number_lenient_scalar(), // basis (optional)
            ]
        });
        &SCHEMA[..]
    }
    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        _ctx: &dyn FunctionContext<'b>,
    ) -> Result<CalcValue<'b>, ExcelError> {
        // Check minimum required arguments
        if args.len() < 6 {
            return Ok(CalcValue::Scalar(LiteralValue::Error(
                ExcelError::new_value(),
            )));
        }

        let settlement_serial = coerce_num(&args[0])?;
        let maturity_serial = coerce_num(&args[1])?;
        let rate = coerce_num(&args[2])?;
        let pr = coerce_num(&args[3])?;
        let redemption = coerce_num(&args[4])?;
        let frequency = coerce_num(&args[5])?.trunc() as i32;
        let basis_int = if args.len() > 6 {
            coerce_num(&args[6])?.trunc() as i32
        } else {
            0
        };

        // Validate inputs
        if rate < 0.0 || pr <= 0.0 || redemption <= 0.0 {
            return Ok(CalcValue::Scalar(
                LiteralValue::Error(ExcelError::new_num()),
            ));
        }
        if frequency != 1 && frequency != 2 && frequency != 4 {
            return Ok(CalcValue::Scalar(
                LiteralValue::Error(ExcelError::new_num()),
            ));
        }

        let basis = DayCountBasis::from_int(basis_int)?;

        let settlement = serial_to_date(settlement_serial)?;
        let maturity = serial_to_date(maturity_serial)?;

        // maturity must be after settlement
        if maturity <= settlement {
            return Ok(CalcValue::Scalar(
                LiteralValue::Error(ExcelError::new_num()),
            ));
        }

        if coupon_period(&settlement, &maturity, frequency).num <= 1 {
            let y = yield_single_coupon(
                &settlement,
                &maturity,
                rate,
                pr,
                redemption,
                frequency,
                basis,
            );
            return Ok(CalcValue::Scalar(LiteralValue::Number(y)));
        }

        // Use Newton-Raphson to find yield where price = target price
        let yld = calculate_yield(
            &settlement,
            &maturity,
            rate,
            pr,
            redemption,
            frequency,
            basis,
        );

        match yld {
            Some(y) => Ok(CalcValue::Scalar(LiteralValue::Number(y))),
            None => Ok(CalcValue::Scalar(
                LiteralValue::Error(ExcelError::new_num()),
            )),
        }
    }
}

/// Calculate yield using Newton-Raphson iteration
fn calculate_yield(
    settlement: &NaiveDate,
    maturity: &NaiveDate,
    rate: f64,
    target_price: f64,
    redemption: f64,
    frequency: i32,
    basis: DayCountBasis,
) -> Option<f64> {
    const MAX_ITER: i32 = 100;
    const EPSILON: f64 = 1e-10;

    // Initial guess based on coupon rate
    let mut yld = rate;
    if yld == 0.0 {
        yld = 0.05; // Default guess if rate is 0
    }

    for _ in 0..MAX_ITER {
        let price = calculate_price(
            settlement, maturity, rate, yld, redemption, frequency, basis,
        );
        let diff = price - target_price;

        if diff.abs() < EPSILON {
            return Some(yld);
        }

        // Calculate derivative numerically
        let delta = 0.0001;
        let price_up = calculate_price(
            settlement,
            maturity,
            rate,
            yld + delta,
            redemption,
            frequency,
            basis,
        );
        let derivative = (price_up - price) / delta;

        if derivative.abs() < EPSILON {
            return None;
        }

        let new_yld = yld - diff / derivative;

        // Prevent yield from going too negative
        if new_yld < -0.99 {
            yld = -0.99;
        } else {
            yld = new_yld;
        }

        // Prevent yield from going too high
        if yld > 10.0 {
            yld = 10.0;
        }
    }

    // If close enough after max iterations, return the result
    let final_price = calculate_price(
        settlement, maturity, rate, yld, redemption, frequency, basis,
    );
    if (final_price - target_price).abs() < 0.01 {
        Some(yld)
    } else {
        None
    }
}

/// Returns bond-equivalent yield for a US Treasury bill.
///
/// `TBILLEQ` converts a T-bill discount rate into an annualized bond-equivalent yield
/// so it can be compared with coupon-bearing securities.
///
/// # Remarks
/// - Date inputs are spreadsheet serial dates; `maturity` must be after `settlement`.
/// - The T-bill must mature within one year of settlement (DSM <= 365).
/// - `discount` is the T-bill discount rate as a decimal (e.g. `0.09` for 9%).
/// - For bills with DSM <= 182: `yield = 365 * discount / (360 - discount * DSM)`.
/// - For bills with DSM > 182 a quadratic coupon-equivalent formula is used.
/// - Returns `#NUM!` for invalid dates, non-positive discount, or DSM out of range.
///
/// # Examples
/// ```excel
/// =TBILLEQ(DATE(2024,1,1), DATE(2024,4,1), 0.038)
/// ```
///
/// ```yaml,sandbox
/// title: "Short bill bond-equivalent yield"
/// formula: '=TBILLEQ(DATE(2024,1,1), DATE(2024,4,1), 0.038)'
/// expected: 0.03890144779577161
/// ```
///
/// ```yaml,docs
/// related:
///   - TBILLPRICE
///   - TBILLYIELD
///   - YIELD
/// faq:
///   - q: "Why does TBILLEQ differ from the discount rate?"
///     a: "TBILLEQ annualizes the bill using a bond-equivalent convention so it can be compared with coupon-bearing yields."
/// ```
#[derive(Debug)]
pub struct TbilleqFn;

/// [formualizer-docgen:schema:start]
/// Name: TBILLEQ
/// Type: TbilleqFn
/// Min args: 3
/// Max args: 3
/// Variadic: false
/// Signature: TBILLEQ(arg1: number@scalar, arg2: number@scalar, arg3: number@scalar)
/// Arg schema: arg1{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg2{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg3{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}
/// Caps: PURE
/// [formualizer-docgen:schema:end]
impl Function for TbilleqFn {
    func_caps!(PURE);
    fn name(&self) -> &'static str {
        "TBILLEQ"
    }
    fn min_args(&self) -> usize {
        3
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        use std::sync::LazyLock;
        static SCHEMA: LazyLock<Vec<ArgSchema>> = LazyLock::new(|| {
            vec![
                ArgSchema::number_lenient_scalar(), // settlement
                ArgSchema::number_lenient_scalar(), // maturity
                ArgSchema::number_lenient_scalar(), // discount
            ]
        });
        &SCHEMA[..]
    }
    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        _ctx: &dyn FunctionContext<'b>,
    ) -> Result<CalcValue<'b>, ExcelError> {
        let settlement_serial = coerce_num(&args[0])?;
        let maturity_serial = coerce_num(&args[1])?;
        let discount = coerce_num(&args[2])?;

        if discount <= 0.0 {
            return Ok(CalcValue::Scalar(
                LiteralValue::Error(ExcelError::new_num()),
            ));
        }

        let settlement = serial_to_date(settlement_serial)?;
        let maturity = serial_to_date(maturity_serial)?;

        if maturity <= settlement {
            return Ok(CalcValue::Scalar(
                LiteralValue::Error(ExcelError::new_num()),
            ));
        }

        let dsm = (maturity - settlement).num_days() as f64;
        if dsm > 365.0 || dsm <= 0.0 {
            return Ok(CalcValue::Scalar(
                LiteralValue::Error(ExcelError::new_num()),
            ));
        }

        let result = if dsm <= 182.0 {
            365.0 * discount / (360.0 - discount * dsm)
        } else {
            // Coupon-equivalent yield for long-dated T-bills (> 182 days)
            // Microsoft formula: (-2a + 2*sqrt(a^2 - (2a-1)*(1 - 1/p))) / (2a-1)
            // where a = DSM/365 and p = price as fraction of par
            let price_frac = 1.0 - discount * dsm / 360.0;
            let a = dsm / 365.0;
            let inner = a * a - (2.0 * a - 1.0) * (1.0 - 1.0 / price_frac);
            if inner < 0.0 {
                return Ok(CalcValue::Scalar(
                    LiteralValue::Error(ExcelError::new_num()),
                ));
            }
            (-2.0 * a + 2.0 * inner.sqrt()) / (2.0 * a - 1.0)
        };

        Ok(CalcValue::Scalar(LiteralValue::Number(result)))
    }
}

/// Returns price per $100 face value for a US Treasury bill.
///
/// `TBILLPRICE` computes the dollar price from a discount rate using
/// `price = 100 * (1 - discount * DSM / 360)`.
///
/// # Remarks
/// - Date inputs are spreadsheet serial dates; `maturity` must be after `settlement`.
/// - The T-bill must mature within one year of settlement (DSM <= 365).
/// - `discount` is a decimal discount rate; must be positive.
/// - Returns `#NUM!` for invalid dates, non-positive discount, or DSM out of range.
///
/// # Examples
/// ```excel
/// =TBILLPRICE(DATE(2024,1,1), DATE(2024,4,1), 0.038)
/// ```
///
/// ```yaml,sandbox
/// title: "Price a 91-day T-bill"
/// formula: '=TBILLPRICE(DATE(2024,1,1), DATE(2024,4,1), 0.038)'
/// expected: 99.03944444444444
/// ```
///
/// ```yaml,docs
/// related:
///   - TBILLEQ
///   - TBILLYIELD
///   - PRICE
/// faq:
///   - q: "What does TBILLPRICE quote?"
///     a: "The result is the dollar price per 100 of face value, following Excel's Treasury-bill convention."
/// ```
#[derive(Debug)]
pub struct TbillpriceFn;

/// [formualizer-docgen:schema:start]
/// Name: TBILLPRICE
/// Type: TbillpriceFn
/// Min args: 3
/// Max args: 3
/// Variadic: false
/// Signature: TBILLPRICE(arg1: number@scalar, arg2: number@scalar, arg3: number@scalar)
/// Arg schema: arg1{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg2{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg3{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}
/// Caps: PURE
/// [formualizer-docgen:schema:end]
impl Function for TbillpriceFn {
    func_caps!(PURE);
    fn name(&self) -> &'static str {
        "TBILLPRICE"
    }
    fn min_args(&self) -> usize {
        3
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        use std::sync::LazyLock;
        static SCHEMA: LazyLock<Vec<ArgSchema>> = LazyLock::new(|| {
            vec![
                ArgSchema::number_lenient_scalar(), // settlement
                ArgSchema::number_lenient_scalar(), // maturity
                ArgSchema::number_lenient_scalar(), // discount
            ]
        });
        &SCHEMA[..]
    }
    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        _ctx: &dyn FunctionContext<'b>,
    ) -> Result<CalcValue<'b>, ExcelError> {
        let settlement_serial = coerce_num(&args[0])?;
        let maturity_serial = coerce_num(&args[1])?;
        let discount = coerce_num(&args[2])?;

        if discount <= 0.0 {
            return Ok(CalcValue::Scalar(
                LiteralValue::Error(ExcelError::new_num()),
            ));
        }

        let settlement = serial_to_date(settlement_serial)?;
        let maturity = serial_to_date(maturity_serial)?;

        if maturity <= settlement {
            return Ok(CalcValue::Scalar(
                LiteralValue::Error(ExcelError::new_num()),
            ));
        }

        let dsm = (maturity - settlement).num_days() as f64;
        if dsm > 365.0 || dsm <= 0.0 {
            return Ok(CalcValue::Scalar(
                LiteralValue::Error(ExcelError::new_num()),
            ));
        }

        let price = 100.0 * (1.0 - discount * dsm / 360.0);
        Ok(CalcValue::Scalar(LiteralValue::Number(price)))
    }
}

/// Returns the yield for a US Treasury bill.
///
/// `TBILLYIELD` computes the discount-rate yield from a T-bill's price using
/// `yield = (100 - price) / price * (360 / DSM)`.
///
/// # Remarks
/// - Date inputs are spreadsheet serial dates; `maturity` must be after `settlement`.
/// - The T-bill must mature within one year of settlement (DSM <= 365).
/// - `price` is the dollar price per $100 face value; must be positive.
/// - Returns `#NUM!` for invalid dates, non-positive price, or DSM out of range.
///
/// # Examples
/// ```excel
/// =TBILLYIELD(DATE(2024,1,1), DATE(2024,4,1), 98.5)
/// ```
///
/// ```yaml,sandbox
/// title: "Yield from quoted T-bill price"
/// formula: '=TBILLYIELD(DATE(2024,1,1), DATE(2024,4,1), 98.5)'
/// expected: 0.060244324203715074
/// ```
///
/// ```yaml,docs
/// related:
///   - TBILLPRICE
///   - TBILLEQ
///   - YIELD
/// faq:
///   - q: "Is TBILLYIELD the same as bond-equivalent yield?"
///     a: "No. TBILLYIELD gives the bill's discount-rate yield, while TBILLEQ converts that pricing into a bond-equivalent yield."
/// ```
#[derive(Debug)]
pub struct TbillyieldFn;

/// [formualizer-docgen:schema:start]
/// Name: TBILLYIELD
/// Type: TbillyieldFn
/// Min args: 3
/// Max args: 3
/// Variadic: false
/// Signature: TBILLYIELD(arg1: number@scalar, arg2: number@scalar, arg3: number@scalar)
/// Arg schema: arg1{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg2{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg3{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}
/// Caps: PURE
/// [formualizer-docgen:schema:end]
impl Function for TbillyieldFn {
    func_caps!(PURE);
    fn name(&self) -> &'static str {
        "TBILLYIELD"
    }
    fn min_args(&self) -> usize {
        3
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        use std::sync::LazyLock;
        static SCHEMA: LazyLock<Vec<ArgSchema>> = LazyLock::new(|| {
            vec![
                ArgSchema::number_lenient_scalar(), // settlement
                ArgSchema::number_lenient_scalar(), // maturity
                ArgSchema::number_lenient_scalar(), // price
            ]
        });
        &SCHEMA[..]
    }
    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        _ctx: &dyn FunctionContext<'b>,
    ) -> Result<CalcValue<'b>, ExcelError> {
        let settlement_serial = coerce_num(&args[0])?;
        let maturity_serial = coerce_num(&args[1])?;
        let price = coerce_num(&args[2])?;

        if price <= 0.0 {
            return Ok(CalcValue::Scalar(
                LiteralValue::Error(ExcelError::new_num()),
            ));
        }

        let settlement = serial_to_date(settlement_serial)?;
        let maturity = serial_to_date(maturity_serial)?;

        if maturity <= settlement {
            return Ok(CalcValue::Scalar(
                LiteralValue::Error(ExcelError::new_num()),
            ));
        }

        let dsm = (maturity - settlement).num_days() as f64;
        if dsm > 365.0 || dsm <= 0.0 {
            return Ok(CalcValue::Scalar(
                LiteralValue::Error(ExcelError::new_num()),
            ));
        }

        let yld = (100.0 - price) / price * (360.0 / dsm);
        Ok(CalcValue::Scalar(LiteralValue::Number(yld)))
    }
}

/// Shared argument decoding for the securities functions: `(settlement, maturity)` serial dates
/// (settlement must precede maturity) and an optional `basis` at `basis_index` (default 0).
fn security_dates(
    args: &[ArgumentHandle<'_, '_>],
    basis_index: usize,
) -> Result<(NaiveDate, NaiveDate, DayCountBasis), ExcelError> {
    let settlement = serial_to_date(coerce_num(&args[0])?.trunc())?;
    let maturity = serial_to_date(coerce_num(&args[1])?.trunc())?;
    let basis =
        DayCountBasis::from_int(opt_num(args, basis_index)?.map_or(0, |v| v.trunc() as i32))?;
    if settlement >= maturity {
        return Err(ExcelError::new_num());
    }
    Ok((settlement, maturity, basis))
}

fn coupon_frequency(arg: &ArgumentHandle<'_, '_>) -> Result<i32, ExcelError> {
    let frequency = coerce_num(arg)?.trunc() as i32;
    if frequency != 1 && frequency != 2 && frequency != 4 {
        return Err(ExcelError::new_num());
    }
    Ok(frequency)
}

fn num_or_error(v: Result<f64, ExcelError>) -> Result<CalcValue<'static>, ExcelError> {
    Ok(CalcValue::Scalar(match v {
        Ok(n) => LiteralValue::Number(n),
        Err(e) => LiteralValue::Error(e),
    }))
}

static SECURITY_SCHEMA_4: std::sync::LazyLock<Vec<ArgSchema>> = std::sync::LazyLock::new(|| {
    vec![
        ArgSchema::number_lenient_scalar(),
        ArgSchema::number_lenient_scalar(),
        ArgSchema::number_lenient_scalar(),
        ArgSchema::number_lenient_scalar(),
    ]
});
static SECURITY_SCHEMA_5: std::sync::LazyLock<Vec<ArgSchema>> = std::sync::LazyLock::new(|| {
    vec![
        ArgSchema::number_lenient_scalar(),
        ArgSchema::number_lenient_scalar(),
        ArgSchema::number_lenient_scalar(),
        ArgSchema::number_lenient_scalar(),
        ArgSchema::number_lenient_scalar(),
    ]
});
static SECURITY_SCHEMA_6: std::sync::LazyLock<Vec<ArgSchema>> = std::sync::LazyLock::new(|| {
    vec![
        ArgSchema::number_lenient_scalar(),
        ArgSchema::number_lenient_scalar(),
        ArgSchema::number_lenient_scalar(),
        ArgSchema::number_lenient_scalar(),
        ArgSchema::number_lenient_scalar(),
        ArgSchema::number_lenient_scalar(),
    ]
});

#[derive(Clone, Copy)]
enum CouponMetric {
    DaysBs,
    Days,
    DaysNc,
    Ncd,
    Pcd,
    Num,
}

/// `COUP*(settlement, maturity, frequency, [basis])` — one coupon-schedule metric.
fn eval_coupon_metric(
    args: &[ArgumentHandle<'_, '_>],
    metric: CouponMetric,
) -> Result<CalcValue<'static>, ExcelError> {
    num_or_error((|| {
        let (settlement, maturity, basis) = security_dates(args, 3)?;
        let frequency = coupon_frequency(&args[2])?;
        let period = coupon_period(&settlement, &maturity, frequency);
        Ok(match metric {
            CouponMetric::DaysBs => coupon_days_bs(&period, &settlement, basis),
            CouponMetric::Days => coupon_days(&period, frequency, basis),
            CouponMetric::DaysNc => coupon_days_nc(&period, &settlement, frequency, basis),
            CouponMetric::Ncd => crate::builtins::datetime::date_to_serial(&period.ncd),
            CouponMetric::Pcd => crate::builtins::datetime::date_to_serial(&period.pcd),
            CouponMetric::Num => period.num as f64,
        })
    })())
}

macro_rules! coupon_metric_fn {
    ($ty:ident, $name:literal, $metric:expr, $doc:literal) => {
        #[doc = $doc]
        #[derive(Debug)]
        pub struct $ty;
        impl Function for $ty {
            func_caps!(PURE);
            fn name(&self) -> &'static str {
                $name
            }
            fn min_args(&self) -> usize {
                3
            }
            fn variadic(&self) -> bool {
                true
            }
            fn arg_schema(&self) -> &'static [ArgSchema] {
                &SECURITY_SCHEMA_4[..]
            }
            fn eval<'a, 'b, 'c>(
                &self,
                args: &'c [ArgumentHandle<'a, 'b>],
                _ctx: &dyn FunctionContext<'b>,
            ) -> Result<CalcValue<'b>, ExcelError> {
                eval_coupon_metric(args, $metric)
            }
        }
    };
}

coupon_metric_fn!(
    CoupdaybsFn,
    "COUPDAYBS",
    CouponMetric::DaysBs,
    "`COUPDAYBS(settlement, maturity, frequency, [basis])` — days from the start of the coupon period to settlement."
);
coupon_metric_fn!(
    CoupdaysFn,
    "COUPDAYS",
    CouponMetric::Days,
    "`COUPDAYS(settlement, maturity, frequency, [basis])` — days in the coupon period containing settlement."
);
coupon_metric_fn!(
    CoupdaysncFn,
    "COUPDAYSNC",
    CouponMetric::DaysNc,
    "`COUPDAYSNC(settlement, maturity, frequency, [basis])` — days from settlement to the next coupon date."
);
coupon_metric_fn!(
    CoupncdFn,
    "COUPNCD",
    CouponMetric::Ncd,
    "`COUPNCD(settlement, maturity, frequency, [basis])` — the next coupon date after settlement, as a serial date."
);
coupon_metric_fn!(
    CouppcdFn,
    "COUPPCD",
    CouponMetric::Pcd,
    "`COUPPCD(settlement, maturity, frequency, [basis])` — the coupon date at or before settlement, as a serial date."
);
coupon_metric_fn!(
    CoupnumFn,
    "COUPNUM",
    CouponMetric::Num,
    "`COUPNUM(settlement, maturity, frequency, [basis])` — the number of coupons payable between settlement and maturity."
);

/// `DURATION(settlement, maturity, coupon, yld, frequency, [basis])` — Macaulay duration in years.
/// Excel: `DURATION(DATE(2008,1,1),DATE(2016,1,1),0.08,0.09,2,1)` = 5.993775.
#[derive(Debug)]
pub struct DurationFn;
/// `MDURATION(settlement, maturity, coupon, yld, frequency, [basis])` — modified duration,
/// `DURATION / (1 + yld / frequency)`. Excel: 5.73567 for the DURATION example.
#[derive(Debug)]
pub struct MdurationFn;

fn eval_duration(
    args: &[ArgumentHandle<'_, '_>],
    modified: bool,
) -> Result<CalcValue<'static>, ExcelError> {
    num_or_error((|| {
        let (settlement, maturity, basis) = security_dates(args, 5)?;
        let coupon = coerce_num(&args[2])?;
        let yld = coerce_num(&args[3])?;
        let frequency = coupon_frequency(&args[4])?;
        if coupon < 0.0 || yld < 0.0 {
            return Err(ExcelError::new_num());
        }
        let duration = macaulay_duration(&settlement, &maturity, coupon, yld, frequency, basis);
        Ok(if modified {
            duration / (1.0 + yld / frequency as f64)
        } else {
            duration
        })
    })())
}

macro_rules! duration_fn {
    ($ty:ident, $name:literal, $modified:expr) => {
        impl Function for $ty {
            func_caps!(PURE);
            fn name(&self) -> &'static str {
                $name
            }
            fn min_args(&self) -> usize {
                5
            }
            fn variadic(&self) -> bool {
                true
            }
            fn arg_schema(&self) -> &'static [ArgSchema] {
                &SECURITY_SCHEMA_6[..]
            }
            fn eval<'a, 'b, 'c>(
                &self,
                args: &'c [ArgumentHandle<'a, 'b>],
                _ctx: &dyn FunctionContext<'b>,
            ) -> Result<CalcValue<'b>, ExcelError> {
                eval_duration(args, $modified)
            }
        }
    };
}
duration_fn!(DurationFn, "DURATION", false);
duration_fn!(MdurationFn, "MDURATION", true);

#[derive(Clone, Copy)]
enum DiscountMetric {
    Disc,
    PriceDisc,
    YieldDisc,
    IntRate,
    Received,
}

/// The four discounted-security functions share the shape
/// `(settlement, maturity, amount, amount, [basis])` and a single `YEARFRAC(settlement, maturity)`.
fn eval_discount_metric(
    args: &[ArgumentHandle<'_, '_>],
    metric: DiscountMetric,
) -> Result<CalcValue<'static>, ExcelError> {
    num_or_error((|| {
        let (settlement, maturity, basis) = security_dates(args, 4)?;
        let x = coerce_num(&args[2])?;
        let y = coerce_num(&args[3])?;
        if x <= 0.0 || y <= 0.0 {
            return Err(ExcelError::new_num());
        }
        let yf = year_fraction(&settlement, &maturity, basis);
        if yf <= 0.0 {
            return Err(ExcelError::new_num());
        }
        Ok(match metric {
            // DISC(settlement, maturity, pr, redemption)
            DiscountMetric::Disc => (y - x) / y / yf,
            // PRICEDISC(settlement, maturity, discount, redemption)
            DiscountMetric::PriceDisc => y - x * y * yf,
            // YIELDDISC(settlement, maturity, pr, redemption)
            DiscountMetric::YieldDisc => (y / x - 1.0) / yf,
            // INTRATE(settlement, maturity, investment, redemption)
            DiscountMetric::IntRate => (y / x - 1.0) / yf,
            // RECEIVED(settlement, maturity, investment, discount)
            DiscountMetric::Received => x / (1.0 - y * yf),
        })
    })())
}

macro_rules! discount_metric_fn {
    ($ty:ident, $name:literal, $metric:expr, $doc:literal) => {
        #[doc = $doc]
        #[derive(Debug)]
        pub struct $ty;
        impl Function for $ty {
            func_caps!(PURE);
            fn name(&self) -> &'static str {
                $name
            }
            fn min_args(&self) -> usize {
                4
            }
            fn variadic(&self) -> bool {
                true
            }
            fn arg_schema(&self) -> &'static [ArgSchema] {
                &SECURITY_SCHEMA_5[..]
            }
            fn eval<'a, 'b, 'c>(
                &self,
                args: &'c [ArgumentHandle<'a, 'b>],
                _ctx: &dyn FunctionContext<'b>,
            ) -> Result<CalcValue<'b>, ExcelError> {
                eval_discount_metric(args, $metric)
            }
        }
    };
}

discount_metric_fn!(
    DiscFn,
    "DISC",
    DiscountMetric::Disc,
    "`DISC(settlement, maturity, pr, redemption, [basis])` — discount rate of a security: `(redemption − pr) / redemption / YEARFRAC(settlement, maturity, basis)`. Excel: `DISC(DATE(2007,1,25),DATE(2007,6,15),97.975,100,1)` = 0.052420213."
);
discount_metric_fn!(
    PricediscFn,
    "PRICEDISC",
    DiscountMetric::PriceDisc,
    "`PRICEDISC(settlement, maturity, discount, redemption, [basis])` — price per 100 of a discounted security. Excel: `PRICEDISC(DATE(2008,2,16),DATE(2008,3,1),0.0525,100,2)` = 99.795833."
);
discount_metric_fn!(
    YielddiscFn,
    "YIELDDISC",
    DiscountMetric::YieldDisc,
    "`YIELDDISC(settlement, maturity, pr, redemption, [basis])` — annual yield of a discounted security."
);
discount_metric_fn!(
    IntrateFn,
    "INTRATE",
    DiscountMetric::IntRate,
    "`INTRATE(settlement, maturity, investment, redemption, [basis])` — interest rate of a fully invested security."
);
discount_metric_fn!(
    ReceivedFn,
    "RECEIVED",
    DiscountMetric::Received,
    "`RECEIVED(settlement, maturity, investment, discount, [basis])` — amount received at maturity for a fully invested security."
);

/// `PRICEMAT(settlement, maturity, issue, rate, yld, [basis])` — price per 100 of a security that
/// pays interest at maturity. Excel: `PRICEMAT(DATE(2008,2,15),DATE(2008,4,13),DATE(2007,11,11),0.061,0.061,0)`
/// = 99.98449888.
#[derive(Debug)]
pub struct PricematFn;
/// `YIELDMAT(settlement, maturity, issue, rate, pr, [basis])` — annual yield of a security that
/// pays interest at maturity.
#[derive(Debug)]
pub struct YieldmatFn;

fn eval_maturity_security(
    args: &[ArgumentHandle<'_, '_>],
    price_mode: bool,
) -> Result<CalcValue<'static>, ExcelError> {
    num_or_error((|| {
        let (settlement, maturity, basis) = security_dates(args, 5)?;
        let issue = serial_to_date(coerce_num(&args[2])?.trunc())?;
        let rate = coerce_num(&args[3])?;
        let other = coerce_num(&args[4])?;
        if rate < 0.0 || (price_mode && other < 0.0) || (!price_mode && other <= 0.0) {
            return Err(ExcelError::new_num());
        }
        let issue_to_maturity = year_fraction(&issue, &maturity, basis);
        let issue_to_settlement = year_fraction(&issue, &settlement, basis);
        let settlement_to_maturity = year_fraction(&settlement, &maturity, basis);
        Ok(if price_mode {
            let yld = other;
            ((1.0 + issue_to_maturity * rate) / (1.0 + settlement_to_maturity * yld)
                - issue_to_settlement * rate)
                * 100.0
        } else {
            let price = other;
            ((1.0 + issue_to_maturity * rate) / (price / 100.0 + issue_to_settlement * rate) - 1.0)
                / settlement_to_maturity
        })
    })())
}

macro_rules! maturity_security_fn {
    ($ty:ident, $name:literal, $price_mode:expr) => {
        impl Function for $ty {
            func_caps!(PURE);
            fn name(&self) -> &'static str {
                $name
            }
            fn min_args(&self) -> usize {
                5
            }
            fn variadic(&self) -> bool {
                true
            }
            fn arg_schema(&self) -> &'static [ArgSchema] {
                &SECURITY_SCHEMA_6[..]
            }
            fn eval<'a, 'b, 'c>(
                &self,
                args: &'c [ArgumentHandle<'a, 'b>],
                _ctx: &dyn FunctionContext<'b>,
            ) -> Result<CalcValue<'b>, ExcelError> {
                eval_maturity_security(args, $price_mode)
            }
        }
    };
}
maturity_security_fn!(PricematFn, "PRICEMAT", true);
maturity_security_fn!(YieldmatFn, "YIELDMAT", false);

// ── odd-coupon bonds ─────────────────────────────────────────────────────────────────────────────

/// Solve `price_at(yield) = target` for a price function that decreases in yield: bracket the
/// root, then bisect (robust for any basis / odd-period variant). `#NUM!` when no bracket exists.
fn solve_yield(
    price_at: impl Fn(f64) -> f64,
    target: f64,
    frequency: i32,
) -> Result<f64, ExcelError> {
    let mut lo = -0.999 * frequency as f64;
    let mut hi = 1.0;
    let at_lo = price_at(lo);
    if at_lo.is_nan() || at_lo < target {
        return Err(ExcelError::new_num());
    }
    while price_at(hi) > target {
        hi *= 2.0;
        if hi > 1e9 {
            return Err(ExcelError::new_num());
        }
    }
    for _ in 0..300 {
        let mid = (lo + hi) / 2.0;
        let p = price_at(mid);
        if p == target || hi - lo < 1e-15 {
            return Ok(mid);
        }
        if p > target {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    Ok((lo + hi) / 2.0)
}

/// Excel's ODDFPRICE: odd (short or long) first coupon period.
#[allow(clippy::too_many_arguments)]
fn odd_first_price(
    settlement: &NaiveDate,
    maturity: &NaiveDate,
    issue: &NaiveDate,
    first_coupon: &NaiveDate,
    rate: f64,
    yld: f64,
    redemption: f64,
    frequency: i32,
    basis: DayCountBasis,
) -> f64 {
    let period = coupon_period(settlement, maturity, frequency);
    let mut a = days_between(issue, settlement, basis) as f64;
    let mut ds = days_between(settlement, first_coupon, basis) as f64;
    let mut df = days_between(issue, first_coupon, basis) as f64;
    let mut e = coupon_days(&period, frequency, basis);
    let mut n = period.num;
    if ds > e {
        // Odd long first period: settlement is more than one quasi-coupon period before the
        // first coupon. Count the coupons from the first coupon date and measure the odd period
        // in quasi-coupon periods (A_i / NL_i sums) instead of a single E.
        match basis {
            DayCountBasis::UsNasd30360 | DayCountBasis::European30360 => {
                n = 1 + (days_between(first_coupon, maturity, basis) as f64 / e).ceil() as i32;
            }
            _ => {
                let step = 12 / frequency;
                n = 0;
                let mut k = 1;
                loop {
                    let previous = coupon_date_back(first_coupon, -(k - 1) * step);
                    let next = coupon_date_back(first_coupon, -k * step);
                    if next >= *maturity {
                        let length = match basis {
                            DayCountBasis::ActualActual => (next - previous).num_days() as f64,
                            DayCountBasis::Actual365 => 365.0 / frequency as f64,
                            _ => 360.0 / frequency as f64,
                        };
                        n += ((*maturity - previous).num_days() as f64 / length).ceil() as i32 + 1;
                        break;
                    }
                    n += 1;
                    k += 1;
                }
                a = quasi_coupon_fraction(issue, settlement, first_coupon, frequency, basis);
                ds =
                    quasi_coupon_fraction(settlement, first_coupon, first_coupon, frequency, basis);
                df = quasi_coupon_fraction(issue, first_coupon, first_coupon, frequency, basis);
                e = 1.0;
            }
        }
    }
    let coupon = 100.0 * rate / frequency as f64;
    let f = 1.0 + yld / frequency as f64;
    let mut price =
        redemption / f.powf((n - 1) as f64 + ds / e) + coupon * (df / e) / f.powf(ds / e);
    for k in 2..=n {
        price += coupon / f.powf((k - 1) as f64 + ds / e);
    }
    price - coupon * (a / e)
}

/// (DC_i, DSC_i, A_i) quasi-coupon sums of an odd last period anchored on `last_interest`.
fn odd_last_fractions(
    settlement: &NaiveDate,
    maturity: &NaiveDate,
    last_interest: &NaiveDate,
    frequency: i32,
    basis: DayCountBasis,
) -> (f64, f64, f64) {
    (
        quasi_coupon_fraction(last_interest, maturity, last_interest, frequency, basis),
        quasi_coupon_fraction(settlement, maturity, last_interest, frequency, basis),
        quasi_coupon_fraction(last_interest, settlement, last_interest, frequency, basis),
    )
}

/// `ODDF*(settlement, maturity, issue, first_coupon, rate, x, redemption, frequency, [basis])`;
/// Excel requires `maturity > first_coupon > settlement > issue`.
fn odd_first_args(
    args: &[ArgumentHandle<'_, '_>],
) -> Result<
    (
        NaiveDate,
        NaiveDate,
        NaiveDate,
        NaiveDate,
        i32,
        DayCountBasis,
    ),
    ExcelError,
> {
    let (settlement, maturity, basis) = security_dates(args, 8)?;
    let issue = serial_to_date(coerce_num(&args[2])?.trunc())?;
    let first_coupon = serial_to_date(coerce_num(&args[3])?.trunc())?;
    let frequency = coupon_frequency(&args[7])?;
    if !(maturity > first_coupon && first_coupon > settlement && settlement > issue) {
        return Err(ExcelError::new_num());
    }
    Ok((settlement, maturity, issue, first_coupon, frequency, basis))
}

/// `ODDL*(settlement, maturity, last_interest, rate, x, redemption, frequency, [basis])`; Excel
/// requires `maturity > settlement > last_interest`.
fn odd_last_args(
    args: &[ArgumentHandle<'_, '_>],
) -> Result<(NaiveDate, NaiveDate, NaiveDate, i32, DayCountBasis), ExcelError> {
    let (settlement, maturity, basis) = security_dates(args, 7)?;
    let last_interest = serial_to_date(coerce_num(&args[2])?.trunc())?;
    let frequency = coupon_frequency(&args[6])?;
    if settlement <= last_interest {
        return Err(ExcelError::new_num());
    }
    Ok((settlement, maturity, last_interest, frequency, basis))
}

#[derive(Clone, Copy)]
enum OddMetric {
    FirstPrice,
    FirstYield,
    LastPrice,
    LastYield,
}

fn eval_odd_metric(
    args: &[ArgumentHandle<'_, '_>],
    metric: OddMetric,
) -> Result<CalcValue<'static>, ExcelError> {
    num_or_error((|| match metric {
        OddMetric::FirstPrice | OddMetric::FirstYield => {
            let (settlement, maturity, issue, first_coupon, frequency, basis) =
                odd_first_args(args)?;
            let rate = coerce_num(&args[4])?;
            let x = coerce_num(&args[5])?;
            let redemption = coerce_num(&args[6])?;
            if rate < 0.0 || redemption <= 0.0 {
                return Err(ExcelError::new_num());
            }
            let price_at = |yld: f64| {
                odd_first_price(
                    &settlement,
                    &maturity,
                    &issue,
                    &first_coupon,
                    rate,
                    yld,
                    redemption,
                    frequency,
                    basis,
                )
            };
            if matches!(metric, OddMetric::FirstPrice) {
                if x < 0.0 {
                    return Err(ExcelError::new_num());
                }
                Ok(price_at(x))
            } else {
                if x <= 0.0 {
                    return Err(ExcelError::new_num());
                }
                solve_yield(price_at, x, frequency)
            }
        }
        OddMetric::LastPrice | OddMetric::LastYield => {
            let (settlement, maturity, last_interest, frequency, basis) = odd_last_args(args)?;
            let rate = coerce_num(&args[3])?;
            let x = coerce_num(&args[4])?;
            let redemption = coerce_num(&args[5])?;
            if rate < 0.0 || redemption <= 0.0 {
                return Err(ExcelError::new_num());
            }
            let (dci, dsci, ai) =
                odd_last_fractions(&settlement, &maturity, &last_interest, frequency, basis);
            let coupon = 100.0 * rate / frequency as f64;
            if matches!(metric, OddMetric::LastPrice) {
                if x < 0.0 {
                    return Err(ExcelError::new_num());
                }
                Ok((redemption + dci * coupon) / (1.0 + dsci * x / frequency as f64) - ai * coupon)
            } else {
                if x <= 0.0 {
                    return Err(ExcelError::new_num());
                }
                let dirty = x + ai * coupon;
                Ok((redemption + dci * coupon - dirty) / dirty * (frequency as f64 / dsci))
            }
        }
    })())
}

static SECURITY_SCHEMA_8: std::sync::LazyLock<Vec<ArgSchema>> =
    std::sync::LazyLock::new(|| (0..8).map(|_| ArgSchema::number_lenient_scalar()).collect());
static SECURITY_SCHEMA_9: std::sync::LazyLock<Vec<ArgSchema>> =
    std::sync::LazyLock::new(|| (0..9).map(|_| ArgSchema::number_lenient_scalar()).collect());

macro_rules! odd_coupon_fn {
    ($ty:ident, $name:literal, $metric:expr, $min:expr, $schema:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Debug)]
        pub struct $ty;
        impl Function for $ty {
            func_caps!(PURE);
            fn name(&self) -> &'static str {
                $name
            }
            fn min_args(&self) -> usize {
                $min
            }
            fn variadic(&self) -> bool {
                true
            }
            fn arg_schema(&self) -> &'static [ArgSchema] {
                &$schema[..]
            }
            fn eval<'a, 'b, 'c>(
                &self,
                args: &'c [ArgumentHandle<'a, 'b>],
                _ctx: &dyn FunctionContext<'b>,
            ) -> Result<CalcValue<'b>, ExcelError> {
                if args.len() < $min {
                    return Ok(CalcValue::Scalar(LiteralValue::Error(
                        ExcelError::new_value(),
                    )));
                }
                eval_odd_metric(args, $metric)
            }
        }
    };
}

odd_coupon_fn!(
    OddfpriceFn,
    "ODDFPRICE",
    OddMetric::FirstPrice,
    8,
    SECURITY_SCHEMA_9,
    "`ODDFPRICE(settlement, maturity, issue, first_coupon, rate, yld, redemption, frequency, [basis])` — price per 100 of a security with an odd (short or long) first period; requires `maturity > first_coupon > settlement > issue`. Excel: `ODDFPRICE(DATE(2008,11,11),DATE(2021,3,1),DATE(2008,10,15),DATE(2009,3,1),0.0785,0.0625,100,2,1)` = 113.5977."
);
odd_coupon_fn!(
    OddfyieldFn,
    "ODDFYIELD",
    OddMetric::FirstYield,
    8,
    SECURITY_SCHEMA_9,
    "`ODDFYIELD(settlement, maturity, issue, first_coupon, rate, pr, redemption, frequency, [basis])` — yield of a security with an odd first period (inverse of ODDFPRICE). Excel: `ODDFYIELD(DATE(2008,11,11),DATE(2021,3,1),DATE(2008,10,15),DATE(2009,3,1),0.0575,84.5,100,2,0)` = 0.07725."
);
odd_coupon_fn!(
    OddlpriceFn,
    "ODDLPRICE",
    OddMetric::LastPrice,
    7,
    SECURITY_SCHEMA_8,
    "`ODDLPRICE(settlement, maturity, last_interest, rate, yld, redemption, frequency, [basis])` — price per 100 of a security with an odd (short or long) last period, measured in quasi-coupon periods anchored on `last_interest`. Excel: `ODDLPRICE(DATE(2008,2,7),DATE(2008,6,15),DATE(2007,10,15),0.0375,0.0405,100,2,0)` = 99.87829."
);
odd_coupon_fn!(
    OddlyieldFn,
    "ODDLYIELD",
    OddMetric::LastYield,
    7,
    SECURITY_SCHEMA_8,
    "`ODDLYIELD(settlement, maturity, last_interest, rate, pr, redemption, frequency, [basis])` — yield of a security with an odd last period (inverse of ODDLPRICE). Excel: `ODDLYIELD(DATE(2008,4,20),DATE(2008,6,15),DATE(2007,12,24),0.0375,99.875,100,2,0)` = 0.04519."
);

pub fn register_builtins() {
    use std::sync::Arc;
    crate::function_registry::register_builtin(Arc::new(AccrintFn));
    crate::function_registry::register_builtin(Arc::new(AccrintmFn));
    crate::function_registry::register_builtin(Arc::new(PriceFn));
    crate::function_registry::register_builtin(Arc::new(YieldFn));
    crate::function_registry::register_builtin(Arc::new(TbilleqFn));
    crate::function_registry::register_builtin(Arc::new(TbillpriceFn));
    crate::function_registry::register_builtin(Arc::new(TbillyieldFn));
    crate::function_registry::register_builtin(Arc::new(CoupdaybsFn));
    crate::function_registry::register_builtin(Arc::new(CoupdaysFn));
    crate::function_registry::register_builtin(Arc::new(CoupdaysncFn));
    crate::function_registry::register_builtin(Arc::new(CoupncdFn));
    crate::function_registry::register_builtin(Arc::new(CouppcdFn));
    crate::function_registry::register_builtin(Arc::new(CoupnumFn));
    crate::function_registry::register_builtin(Arc::new(DurationFn));
    crate::function_registry::register_builtin(Arc::new(MdurationFn));
    crate::function_registry::register_builtin(Arc::new(PricediscFn));
    crate::function_registry::register_builtin(Arc::new(YielddiscFn));
    crate::function_registry::register_builtin(Arc::new(IntrateFn));
    crate::function_registry::register_builtin(Arc::new(ReceivedFn));
    crate::function_registry::register_builtin(Arc::new(PricematFn));
    crate::function_registry::register_builtin(Arc::new(YieldmatFn));
    crate::function_registry::register_builtin(Arc::new(DiscFn));
    crate::function_registry::register_builtin(Arc::new(OddfpriceFn));
    crate::function_registry::register_builtin(Arc::new(OddfyieldFn));
    crate::function_registry::register_builtin(Arc::new(OddlpriceFn));
    crate::function_registry::register_builtin(Arc::new(OddlyieldFn));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn year_fraction_actual_actual_follows_excel_yearfrac() {
        // ≤ 1 year straddling Feb 29 2024 → 366-day year.
        let actual = year_fraction(&d(2023, 12, 1), &d(2024, 3, 1), DayCountBasis::ActualActual);
        assert!((actual - 91.0 / 366.0).abs() < 1e-12, "got {actual}");
        // ≤ 1 year across a year boundary without a Feb 29 → 365.
        let actual = year_fraction(&d(2022, 12, 1), &d(2023, 3, 1), DayCountBasis::ActualActual);
        assert!((actual - 90.0 / 365.0).abs() < 1e-12, "got {actual}");
        // > 1 year → average length of the calendar years touched (2008 is leap).
        let actual = year_fraction(&d(2008, 1, 1), &d(2010, 1, 1), DayCountBasis::ActualActual);
        assert!(
            (actual - 731.0 / (1096.0 / 3.0)).abs() < 1e-12,
            "got {actual}"
        );
    }

    #[test]
    fn year_fraction_actual_actual_is_sign_symmetric() {
        let forward = year_fraction(&d(2023, 12, 1), &d(2024, 3, 1), DayCountBasis::ActualActual);
        let backward = year_fraction(&d(2024, 3, 1), &d(2023, 12, 1), DayCountBasis::ActualActual);
        assert!((forward + backward).abs() < 1e-12);
    }

    #[test]
    fn quasi_coupon_fraction_splits_the_span_at_quasi_coupon_dates() {
        // Anchor 31-Aug-2008 (month-end) → quasi dates 29-Feb-2008, 31-Aug-2008; 1-Mar → 1-May is
        // 61 of the 184 actual days of that period.
        let f = quasi_coupon_fraction(
            &d(2008, 3, 1),
            &d(2008, 5, 1),
            &d(2008, 8, 31),
            2,
            DayCountBasis::ActualActual,
        );
        assert!((f - 61.0 / 184.0).abs() < 1e-12, "got {f}");
        // Spanning two periods: a full first period + 60/180 of the second (30/360).
        let f = quasi_coupon_fraction(
            &d(2007, 10, 15),
            &d(2008, 6, 15),
            &d(2007, 10, 15),
            2,
            DayCountBasis::UsNasd30360,
        );
        assert!((f - 4.0 / 3.0).abs() < 1e-12, "got {f}");
        // Starting before the anchor's period walks backwards to find the containing period.
        let f = quasi_coupon_fraction(
            &d(2006, 10, 15),
            &d(2007, 10, 15),
            &d(2007, 10, 15),
            2,
            DayCountBasis::UsNasd30360,
        );
        assert!((f - 2.0).abs() < 1e-12, "got {f}");
        assert_eq!(
            quasi_coupon_fraction(
                &d(2008, 5, 1),
                &d(2008, 5, 1),
                &d(2008, 8, 31),
                2,
                DayCountBasis::ActualActual
            ),
            0.0
        );
    }

    #[test]
    fn solve_yield_inverts_calculate_price() {
        let target = calculate_price(
            &d(2020, 1, 1),
            &d(2030, 1, 1),
            0.05,
            0.06,
            100.0,
            2,
            DayCountBasis::ActualActual,
        );
        let y = solve_yield(
            |y| {
                calculate_price(
                    &d(2020, 1, 1),
                    &d(2030, 1, 1),
                    0.05,
                    y,
                    100.0,
                    2,
                    DayCountBasis::ActualActual,
                )
            },
            target,
            2,
        )
        .unwrap();
        assert!((y - 0.06).abs() < 1e-10, "got {y}");
        assert!(solve_yield(|_| 50.0, 1000.0, 2).is_err());
    }
}
