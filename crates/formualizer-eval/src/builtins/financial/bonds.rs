//! Bond pricing functions: ACCRINT, ACCRINTM, PRICE, YIELD, the COUP* coupon-schedule family,
//! DURATION, MDURATION, PRICEDISC, YIELDDISC, INTRATE, RECEIVED, PRICEMAT, YIELDMAT

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

/// Day count basis calculation
/// Returns (num_days, year_basis) for the given basis type
#[derive(Debug, Clone, Copy, PartialEq)]
enum DayCountBasis {
    UsNasd30360 = 0,   // US (NASD) 30/360
    ActualActual = 1,  // Actual/actual
    Actual360 = 2,     // Actual/360
    Actual365 = 3,     // Actual/365
    European30360 = 4, // European 30/360
}

impl DayCountBasis {
    fn from_int(basis: i32) -> Result<Self, ExcelError> {
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

/// Calculate the year fraction between two dates.
///
/// For Actual/Actual we mirror the prorated year-boundary behavior used by YEARFRAC
/// rather than averaging year lengths across the entire span. This keeps bond accrual
/// math aligned with the engine's date-part functions for cross-year ranges.
fn year_fraction(start: &NaiveDate, end: &NaiveDate, basis: DayCountBasis) -> f64 {
    if start == end {
        return 0.0;
    }

    let (s, e, sign) = if start <= end {
        (start, end, 1.0)
    } else {
        (end, start, -1.0)
    };

    let frac = match basis {
        DayCountBasis::UsNasd30360 | DayCountBasis::European30360 => {
            days_between(s, e, basis) as f64 / 360.0
        }
        DayCountBasis::Actual360 => ((*e - *s).num_days() as f64) / 360.0,
        DayCountBasis::Actual365 => ((*e - *s).num_days() as f64) / 365.0,
        DayCountBasis::ActualActual => {
            let actual_days = (*e - *s).num_days() as f64;
            if s.year() == e.year() {
                actual_days / if is_leap_year(s.year()) { 366.0 } else { 365.0 }
            } else {
                let start_year_end = NaiveDate::from_ymd_opt(s.year() + 1, 1, 1).unwrap();
                let end_year_start = NaiveDate::from_ymd_opt(e.year(), 1, 1).unwrap();

                let mut out = (start_year_end - *s).num_days() as f64
                    / if is_leap_year(s.year()) { 366.0 } else { 365.0 };
                for _year in (s.year() + 1)..e.year() {
                    out += 1.0;
                }
                out + (*e - end_year_start).num_days() as f64
                    / if is_leap_year(e.year()) { 366.0 } else { 365.0 }
            }
        }
    };

    sign * frac
}

/// Find the coupon date before settlement date
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

/// The coupon date strictly before `settlement` (ACCRINT's accrual start when `calc_method` is
/// FALSE).
fn coupon_date_before(settlement: &NaiveDate, maturity: &NaiveDate, frequency: i32) -> NaiveDate {
    let step = 12 / frequency;
    let mut j = 0;
    while coupon_date_back(maturity, j * step) >= *settlement {
        j += 1;
    }
    coupon_date_back(maturity, j * step)
}

/// Returns accrued interest for a coupon-bearing security.
///
/// `ACCRINT` calculates interest from either `issue` or the previous coupon date up to
/// `settlement`, depending on `calc_method`.
///
/// # Remarks
/// - Date inputs are spreadsheet serial dates; `settlement` must be after `issue`.
/// - `rate` is the annual coupon rate as a decimal (for example, `0.06` for 6%), and `par` is principal amount; both must be positive.
/// - `frequency` must be `1` (annual), `2` (semiannual), or `4` (quarterly).
/// - `basis` codes: `0=US(NASD)30/360`, `1=Actual/Actual`, `2=Actual/360`, `3=Actual/365`, `4=European30/360`.
/// - `calc_method`: non-zero accrues from `issue`; `0` accrues from the previous coupon date.
/// - Return value is in the same currency units as `par` and is positive for valid positive inputs.
///
/// # Examples
///
/// ```yaml,sandbox
/// title: "Accrue from issue date (default calc_method)"
/// formula: "=ACCRINT(DATE(2024,1,1), DATE(2024,7,1), DATE(2024,7,1), 0.06, 1000, 2, 0)"
/// expected: 30
/// ```
///
/// ```yaml,sandbox
/// title: "Accrue from previous coupon (calc_method = 0)"
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
///     a: "`calc_method=0` accrues from the previous coupon date; any non-zero value accrues from `issue`."
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

        let issue_serial = coerce_num(&args[0])?;
        let first_interest_serial = coerce_num(&args[1])?;
        let settlement_serial = coerce_num(&args[2])?;
        let rate = coerce_num(&args[3])?;
        let par = coerce_num(&args[4])?;
        let frequency = coerce_num(&args[5])?.trunc() as i32;
        let basis_int = if args.len() > 6 {
            coerce_num(&args[6])?.trunc() as i32
        } else {
            0
        };
        let calc_method = if args.len() > 7 {
            coerce_num(&args[7])?.trunc() as i32
        } else {
            1
        };

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

        let issue = serial_to_date(issue_serial)?;
        let first_interest = serial_to_date(first_interest_serial)?;
        let settlement = serial_to_date(settlement_serial)?;

        // settlement must be after issue
        if settlement <= issue {
            return Ok(CalcValue::Scalar(
                LiteralValue::Error(ExcelError::new_num()),
            ));
        }

        // Calculate accrued interest
        // If calc_method is TRUE (or 1), calculate from issue to settlement
        // If calc_method is FALSE (or 0), calculate from last coupon to settlement
        let accrued_interest = if calc_method != 0 {
            // Calculate from issue date to settlement date
            // ACCRINT = par * rate * year_fraction(issue, settlement)
            let yf = year_fraction(&issue, &settlement, basis);
            par * rate * yf
        } else {
            // Calculate from last coupon date to settlement
            let prev_coupon = coupon_date_before(&settlement, &first_interest, frequency);
            let start_date = if prev_coupon < issue {
                issue
            } else {
                prev_coupon
            };
            let yf = year_fraction(&start_date, &settlement, basis);
            par * rate * yf
        };

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
    let basis_int = if args.len() > basis_index {
        coerce_num(&args[basis_index])?.trunc() as i32
    } else {
        0
    };
    let basis = DayCountBasis::from_int(basis_int)?;
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn year_fraction_actual_actual_prorates_cross_year_ranges() {
        let start = NaiveDate::from_ymd_opt(2023, 12, 1).unwrap();
        let end = NaiveDate::from_ymd_opt(2024, 3, 1).unwrap();
        let expected = 31.0 / 365.0 + 60.0 / 366.0;
        let actual = year_fraction(&start, &end, DayCountBasis::ActualActual);
        assert!(
            (actual - expected).abs() < 1e-12,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn year_fraction_actual_actual_is_sign_symmetric() {
        let start = NaiveDate::from_ymd_opt(2023, 12, 1).unwrap();
        let end = NaiveDate::from_ymd_opt(2024, 3, 1).unwrap();
        let forward = year_fraction(&start, &end, DayCountBasis::ActualActual);
        let backward = year_fraction(&end, &start, DayCountBasis::ActualActual);
        assert!((forward + backward).abs() < 1e-12);
    }
}
