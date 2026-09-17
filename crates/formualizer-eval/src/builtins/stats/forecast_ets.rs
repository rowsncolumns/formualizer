//! `FORECAST.ETS` family — additive Holt-Winters ("AAA") exponential smoothing.
//!
//! A port of the JS engine's implementation (`fast-formula-parser/formulas/functions/distribution.js`)
//! so both engines produce the same numbers on the same inputs: a 5×5×5 grid search over the α/β/γ
//! smoothing parameters minimising the in-sample mean squared error, seasonality auto-detected by
//! comparing candidate periods 2..min(n/2, 12) (a candidate has to cut the non-seasonal error by
//! 10 %), and a confidence interval of `z · RMSE · √horizon`. Excel does not publish its fitter, so
//! the two engines agreeing with each other is the parity target; Excel's results match to roughly
//! 1 % on typical series. `data_completion` and `aggregation` are validated but not modelled: the
//! timeline is used as given (no gap filling, duplicates are not aggregated).
//! (rowsncolumns/spreadsheet#546 A-14)
//!
//! The fit keeps only its terminal state (level, trend, one seasonal cycle), so a forecast is O(1)
//! in the horizon: `FORECAST.ETS(1E15, …)` is a multiplication, not a `Vec` of 10¹⁵ steps — on
//! wasm32 that allocation was a `capacity overflow` panic that poisoned the whole engine
//! (rowsncolumns/spreadsheet#642 B-06). Two numeric pairs are enough to fit, like Excel, and the
//! Holt level starts one step before the series so a perfectly linear history forecasts its exact
//! continuation (#642 B-01).

use super::{coerce_num, scalar_like_value};
use crate::args::{ArgSchema, ShapeKind};
use crate::function::Function;
use crate::traits::{ArgumentHandle, CalcValue, FunctionContext};
use formualizer_common::{ExcelError, ExcelErrorKind, LiteralValue};
use formualizer_macros::func_caps;
use std::sync::LazyLock;

/// Excel caps `seasonality` at 8760 (hourly data over a year).
const MAX_SEASONALITY: i64 = 8760;
/// Auto-detection scans periods 2..=min(n/2, 12), Excel's default cap for monthly data.
const MAX_DETECTED_PERIOD: usize = 12;
const SMOOTHING_GRID: [f64; 5] = [0.1, 0.3, 0.5, 0.7, 0.9];

fn number_scalar() -> ArgSchema {
    ArgSchema::number_lenient_scalar()
}

fn optional_number_scalar() -> ArgSchema {
    let mut s = ArgSchema::number_lenient_scalar();
    s.required = false;
    s
}

fn series_range() -> ArgSchema {
    let mut s = ArgSchema::any();
    s.shape = ShapeKind::Range;
    s
}

static ARGS_FORECAST_ETS: LazyLock<Vec<ArgSchema>> = LazyLock::new(|| {
    vec![
        number_scalar(),
        series_range(),
        series_range(),
        optional_number_scalar(),
        optional_number_scalar(),
        optional_number_scalar(),
    ]
});

static ARGS_FORECAST_ETS_CONFINT: LazyLock<Vec<ArgSchema>> = LazyLock::new(|| {
    vec![
        number_scalar(),
        series_range(),
        series_range(),
        optional_number_scalar(),
        optional_number_scalar(),
        optional_number_scalar(),
        optional_number_scalar(),
    ]
});

static ARGS_FORECAST_ETS_SEASONALITY: LazyLock<Vec<ArgSchema>> = LazyLock::new(|| {
    vec![
        series_range(),
        series_range(),
        optional_number_scalar(),
        optional_number_scalar(),
    ]
});

static ARGS_FORECAST_ETS_STAT: LazyLock<Vec<ArgSchema>> = LazyLock::new(|| {
    vec![
        series_range(),
        series_range(),
        number_scalar(),
        optional_number_scalar(),
        optional_number_scalar(),
        optional_number_scalar(),
    ]
});

/* ─────────────────────────── argument helpers ─────────────────────────── */

fn num_error() -> ExcelError {
    ExcelError::new(ExcelErrorKind::Num)
}

fn na_error() -> ExcelError {
    ExcelError::new(ExcelErrorKind::Na)
}

/// Every cell of a range / array argument in row-major order — blanks included, so the two series
/// can be length-checked the way Excel does — or the single value of a scalar argument.
fn collect_cells(arg: &ArgumentHandle<'_, '_>) -> Result<Vec<LiteralValue>, ExcelError> {
    if let Some(arr) = arg.inline_array_literal()? {
        return Ok(arr.into_iter().flatten().collect());
    }
    if let Ok(view) = arg.range_view() {
        let mut out = Vec::new();
        view.for_each_cell(&mut |v| {
            out.push(v.clone());
            Ok(())
        })?;
        return Ok(out);
    }
    Ok(vec![scalar_like_value(arg)?])
}

/// A series cell as a number: numbers count, blanks / text / logicals are skipped (with their pair),
/// an error propagates.
fn series_number(v: &LiteralValue) -> Result<Option<f64>, ExcelError> {
    match v {
        LiteralValue::Number(n) => Ok(Some(*n)),
        LiteralValue::Int(i) => Ok(Some(*i as f64)),
        LiteralValue::Error(e) => Err(e.clone()),
        _ => Ok(None),
    }
}

fn required_number(arg: &ArgumentHandle<'_, '_>) -> Result<f64, ExcelError> {
    match scalar_like_value(arg)? {
        LiteralValue::Error(e) => Err(e),
        v => coerce_num(&v).map_err(|_| ExcelError::new_value()),
    }
}

/// An optional numeric argument: absent or skipped (`FORECAST.ETS(x,y,t,,1)`) takes the default.
fn optional_number(
    args: &[ArgumentHandle<'_, '_>],
    idx: usize,
    default: f64,
) -> Result<f64, ExcelError> {
    match args.get(idx) {
        None => Ok(default),
        Some(a) if a.is_skipped() => Ok(default),
        Some(a) => match scalar_like_value(a)? {
            LiteralValue::Error(e) => Err(e),
            LiteralValue::Empty => Ok(default),
            v => coerce_num(&v).map_err(|_| ExcelError::new_value()),
        },
    }
}

/// `seasonality`: 1 (the default) auto-detects, 0 fits without a seasonal component, 2..=8760 is
/// the period; anything else is `#NUM!`. Fractions are truncated like Excel.
fn seasonality_arg(args: &[ArgumentHandle<'_, '_>], idx: usize) -> Result<i64, ExcelError> {
    let s = optional_number(args, idx, 1.0)?.floor();
    if s < 0.0 || s > MAX_SEASONALITY as f64 {
        return Err(num_error());
    }
    Ok(s as i64)
}

/// `data_completion` must be 0 or 1, `aggregation` 1..=7 (AVERAGE, COUNT, COUNTA, MAX, MEDIAN, MIN,
/// SUM); otherwise `#NUM!`. Neither changes the fit (see the module docs).
fn validate_completion_and_aggregation(
    args: &[ArgumentHandle<'_, '_>],
    completion_idx: usize,
) -> Result<(), ExcelError> {
    let completion = optional_number(args, completion_idx, 1.0)?.floor();
    if completion != 0.0 && completion != 1.0 {
        return Err(num_error());
    }
    let aggregation = optional_number(args, completion_idx + 1, 1.0)?.floor();
    if !(1.0..=7.0).contains(&aggregation) {
        return Err(num_error());
    }
    Ok(())
}

fn scalar(v: LiteralValue) -> Result<CalcValue<'static>, ExcelError> {
    Ok(CalcValue::Scalar(v))
}

fn error_scalar(e: ExcelError) -> Result<CalcValue<'static>, ExcelError> {
    scalar(LiteralValue::Error(e))
}

/* ─────────────────────────── the model ─────────────────────────── */

struct Series {
    /// Values ordered by timeline.
    y: Vec<f64>,
    last_x: f64,
    /// Median positive consecutive timeline gap.
    step: f64,
    /// Seasonal period; 1 is Holt's linear method (no seasonal component).
    period: usize,
}

/// Pair the values with the timeline, sort by timeline, infer the constant step and resolve the
/// seasonality argument.
fn prepare(
    values: &[LiteralValue],
    timeline: &[LiteralValue],
    seasonality: i64,
) -> Result<Series, ExcelError> {
    if values.len() != timeline.len() {
        return Err(na_error());
    }
    let mut pairs: Vec<(f64, f64)> = Vec::with_capacity(values.len());
    for (x, y) in timeline.iter().zip(values) {
        let (Some(x), Some(y)) = (series_number(x)?, series_number(y)?) else {
            continue;
        };
        pairs.push((x, y));
    }
    // Excel fits from two pairs (there is no four-pair floor); fewer is #N/A, and two pairs at the
    // same timeline point have no step (#NUM! below).
    if pairs.len() < 2 {
        return Err(na_error());
    }
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
    let y: Vec<f64> = pairs.iter().map(|p| p.1).collect();
    // The median positive gap: Excel requires a uniform timeline; the median tolerates a few
    // oddly spaced entries.
    let mut diffs: Vec<f64> = pairs
        .windows(2)
        .map(|w| w[1].0 - w[0].0)
        .filter(|d| *d > 0.0)
        .collect();
    if diffs.is_empty() {
        return Err(num_error());
    }
    diffs.sort_by(f64::total_cmp);
    let step = diffs[diffs.len() / 2];
    let period = match seasonality {
        0 => 1,
        1 => detect_seasonality(&y),
        s => s as usize,
    };
    Ok(Series {
        y,
        last_x: pairs[pairs.len() - 1].0,
        step,
        period,
    })
}

/// The terminal state of one Holt-Winters pass: everything a forecast at any horizon needs.
struct HoltWinters {
    fitted: Vec<f64>,
    /// Smoothed level after the last observation.
    level: f64,
    /// Smoothed per-step trend after the last observation.
    trend: f64,
    /// One cycle of seasonal indices (a single zero entry without a seasonal component).
    seasonal: Vec<f64>,
    /// Mean squared in-sample residual.
    sse: f64,
}

impl HoltWinters {
    /// The forecast `h` whole steps past the last observation: `level + h·trend + seasonal[(n + h - 1) mod m]`.
    /// O(1) in `h`, which stays an `f64` — the seasonal slot is reduced with `%` (exact for finite
    /// operands), so a horizon beyond `usize` on wasm32 neither overflows nor allocates.
    fn forecast_at(&self, h: f64) -> f64 {
        let m = self.seasonal.len();
        let season_comp = if m > 1 {
            let n = self.fitted.len() as f64;
            self.seasonal[((n + h - 1.0) % m as f64) as usize]
        } else {
            0.0
        };
        self.level + h * self.trend + season_comp
    }
}

/// One additive Holt-Winters pass with fixed smoothing parameters. `period == 1` collapses to Holt's
/// linear method. Seasonal indices initialise from the first cycle's deviations from its mean, the
/// trend from the average per-step change between the first two cycles. The initial level is the
/// level one step *before* the series (first value or first-cycle mean, minus the trend), so the
/// one-step-ahead prediction of the first observation is the observation itself and a perfectly
/// linear history has zero residual — an initial level *at* the first value predicted `y₀ + trend`
/// for `y₀`, and that spurious residual pulled the grid search off the exact continuation.
fn run_holt_winters(y: &[f64], period: usize, alpha: f64, beta: f64, gamma: f64) -> HoltWinters {
    let n = y.len();
    let m = period;
    let mut fitted = vec![0.0; n];
    let mut seasonal = vec![0.0; m.max(1)];
    let (mut level, mut trend);
    if m > 1 && n >= 2 * m {
        let mut sum1 = 0.0;
        for &v in &y[..m] {
            sum1 += v;
        }
        let mean1 = sum1 / m as f64;
        let mut trend_sum = 0.0;
        for i in 0..m {
            trend_sum += (y[i + m] - y[i]) / m as f64;
        }
        trend = trend_sum / m as f64;
        level = mean1 - trend;
        for i in 0..m {
            seasonal[i] = y[i] - mean1;
        }
    } else {
        trend = if n > 1 { y[1] - y[0] } else { 0.0 };
        level = y[0] - trend;
    }
    let mut sse = 0.0;
    let mut count = 0usize;
    for t in 0..n {
        let season_idx = if m > 1 { t % m } else { 0 };
        let season_comp = if m > 1 { seasonal[season_idx] } else { 0.0 };
        let y_hat = level + trend + season_comp;
        fitted[t] = y_hat;
        let resid = y[t] - y_hat;
        sse += resid * resid;
        count += 1;
        let prev_level = level;
        level = alpha * (y[t] - season_comp) + (1.0 - alpha) * (level + trend);
        trend = beta * (level - prev_level) + (1.0 - beta) * trend;
        if m > 1 {
            seasonal[season_idx] = gamma * (y[t] - level) + (1.0 - gamma) * season_comp;
        }
    }
    HoltWinters {
        fitted,
        level,
        trend,
        seasonal,
        sse: if count > 0 {
            sse / count as f64
        } else {
            f64::INFINITY
        },
    }
}

struct Fit {
    alpha: f64,
    beta: f64,
    gamma: f64,
    model: HoltWinters,
}

/// Grid-search the smoothing parameters for the lowest in-sample error (first minimum wins, grid
/// order α, β, γ). Without a seasonal component γ is 0.
fn fit_holt_winters(y: &[f64], period: usize) -> Fit {
    let gammas: &[f64] = if period > 1 { &SMOOTHING_GRID } else { &[0.0] };
    let mut best: Option<Fit> = None;
    for &alpha in &SMOOTHING_GRID {
        for &beta in &SMOOTHING_GRID {
            for &gamma in gammas {
                let model = run_holt_winters(y, period, alpha, beta, gamma);
                if best.as_ref().is_none_or(|b| model.sse < b.model.sse) {
                    best = Some(Fit {
                        alpha,
                        beta,
                        gamma,
                        model,
                    });
                }
            }
        }
    }
    best.expect("the smoothing grid is non-empty")
}

/// The candidate period whose canonical-parameter fit beats the non-seasonal baseline by at least
/// 10 % (each candidate needs two full cycles); 1 when none does.
fn detect_seasonality(y: &[f64]) -> usize {
    let n = y.len();
    if n < 4 {
        return 1;
    }
    let max_period = (n / 2).min(MAX_DETECTED_PERIOD);
    let baseline = run_holt_winters(y, 1, 0.5, 0.1, 0.0).sse;
    let mut best_period = 1;
    let mut best_sse = baseline;
    for m in 2..=max_period {
        if n < 2 * m {
            continue;
        }
        let mut m_sse = f64::INFINITY;
        for alpha in [0.3, 0.7] {
            for beta in [0.1, 0.3] {
                for gamma in [0.3, 0.7] {
                    let sse = run_holt_winters(y, m, alpha, beta, gamma).sse;
                    if sse < m_sse {
                        m_sse = sse;
                    }
                }
            }
        }
        if m_sse < best_sse * 0.9 {
            best_sse = m_sse;
            best_period = m;
        }
    }
    best_period
}

/// JavaScript's `Math.round` (half rounds up) for the forecast horizon.
fn js_round(v: f64) -> f64 {
    (v + 0.5).floor()
}

/* ─────────────────────────── normal quantile (jStat port) ─────────────────────────── */

/// `erf` by the Chebyshev fit jStat uses (Numerical Recipes `erfccheb`), so `FORECAST.ETS.CONFINT`
/// scales its interval by the same z as the JS engine.
fn erf(x: f64) -> f64 {
    const COF: [f64; 28] = [
        -1.3026537197817094,
        6.4196979235649026e-1,
        1.9476473204185836e-2,
        -9.561514786808631e-3,
        -9.46595344482036e-4,
        3.66839497852761e-4,
        4.2523324806907e-5,
        -2.0278578112534e-5,
        -1.624290004647e-6,
        1.303655835580e-6,
        1.5626441722e-8,
        -8.5238095915e-8,
        6.529054439e-9,
        5.059343495e-9,
        -9.91364156e-10,
        -2.27365122e-10,
        9.6467911e-11,
        2.394038e-12,
        -6.886027e-12,
        8.94487e-13,
        3.13092e-13,
        -1.12708e-13,
        3.81e-16,
        7.106e-15,
        -1.523e-15,
        -9.4e-17,
        1.21e-16,
        -2.8e-17,
    ];
    let (x, negative) = if x < 0.0 { (-x, true) } else { (x, false) };
    let t = 2.0 / (2.0 + x);
    let ty = 4.0 * t - 2.0;
    let mut d = 0.0;
    let mut dd = 0.0;
    for j in (1..COF.len()).rev() {
        let tmp = d;
        d = ty * d - dd + COF[j];
        dd = tmp;
    }
    let res = t * (-x * x + 0.5 * (COF[0] + ty * d) - dd).exp();
    if negative { res - 1.0 } else { 1.0 - res }
}

/// Inverse complementary error function: a rational first guess refined by two Newton steps.
fn erfc_inv(p: f64) -> f64 {
    if p >= 2.0 {
        return -100.0;
    }
    if p <= 0.0 {
        return 100.0;
    }
    let pp = if p < 1.0 { p } else { 2.0 - p };
    let t = (-2.0 * (pp / 2.0).ln()).sqrt();
    let mut x = -0.70711 * ((2.30753 + t * 0.27061) / (1.0 + t * (0.99229 + t * 0.04481)) - t);
    for _ in 0..2 {
        let err = (1.0 - erf(x)) - pp;
        x += err / (1.12837916709551257 * (-x * x).exp() - x * err);
    }
    if p < 1.0 { x } else { -x }
}

/// Standard normal quantile, `Φ⁻¹(p)`.
fn std_normal_quantile(p: f64) -> f64 {
    -1.41421356237309505 * erfc_inv(2.0 * p)
}

/* ─────────────────────────── FORECAST.ETS ─────────────────────────── */

/// Forecasts a value at a future timeline point with additive Holt-Winters exponential smoothing.
///
/// `FORECAST.ETS(target_date, values, timeline, [seasonality], [data_completion], [aggregation])`
/// fits level, trend and (when a period is detected or given) seasonal components to the history
/// and extrapolates them to `target_date`.
///
/// # Remarks
/// - `values` and `timeline` must have the same size and at least two numeric pairs; otherwise
///   `#N/A`. Pairs with a non-numeric value or date are skipped; the timeline need not be sorted.
/// - `seasonality`: `1` (default) detects the period automatically, `0` fits without seasonality,
///   `2`..`8760` forces the period. Anything else is `#NUM!`.
/// - `target_date` before the last timeline point is `#NUM!`; a target on the last point returns
///   the last value. Any finite target after it is extrapolated in constant time — a perfectly
///   linear history forecasts its exact continuation, however far out.
/// - The smoothing parameters come from a grid search minimising the in-sample error, so results
///   match the JS engine exactly and Excel to within about 1 %.
///
/// # Examples
///
/// ```yaml,sandbox
/// title: "Next value of a linear trend"
/// formula: "=ROUND(FORECAST.ETS(11,{10;20;30;40;50;60;70;80;90;100},{1;2;3;4;5;6;7;8;9;10}),2)"
/// expected: 110
/// ```
///
/// ```yaml,sandbox
/// title: "A target on the last timeline point is the last value"
/// formula: "=FORECAST.ETS(10,{10;20;30;40;50;60;70;80;90;100},{1;2;3;4;5;6;7;8;9;10})"
/// expected: 100
/// ```
#[derive(Debug)]
pub struct ForecastEtsFn;

/// [formualizer-docgen:schema:start]
/// Name: FORECAST.ETS
/// Type: ForecastEtsFn
/// Min args: 3
/// Max args: 6
/// Variadic: false
/// Signature: FORECAST.ETS(arg1: number@scalar, arg2: any@range, arg3: any@range, arg4?: number@scalar, arg5?: number@scalar, arg6?: number@scalar)
/// Arg schema: arg1{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg2{kinds=any,required=true,shape=range,by_ref=false,coercion=None,max=None,repeating=None,default=false}; arg3{kinds=any,required=true,shape=range,by_ref=false,coercion=None,max=None,repeating=None,default=false}; arg4{kinds=number,required=false,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg5{kinds=number,required=false,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg6{kinds=number,required=false,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}
/// Caps: PURE
/// [formualizer-docgen:schema:end]
impl Function for ForecastEtsFn {
    func_caps!(PURE);
    fn name(&self) -> &'static str {
        "FORECAST.ETS"
    }
    fn min_args(&self) -> usize {
        3
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        &ARGS_FORECAST_ETS[..]
    }
    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        _ctx: &dyn FunctionContext<'b>,
    ) -> Result<CalcValue<'b>, ExcelError> {
        let target = match required_number(&args[0]) {
            Ok(n) => n,
            Err(e) => return error_scalar(e),
        };
        let values = collect_cells(&args[1])?;
        let timeline = collect_cells(&args[2])?;
        let seasonality = match seasonality_arg(args, 3) {
            Ok(s) => s,
            Err(e) => return error_scalar(e),
        };
        if let Err(e) = validate_completion_and_aggregation(args, 4) {
            return error_scalar(e);
        }
        let series = match prepare(&values, &timeline, seasonality) {
            Ok(s) => s,
            Err(e) => return error_scalar(e),
        };
        if target < series.last_x {
            return error_scalar(num_error());
        }
        let horizon = js_round((target - series.last_x) / series.step);
        if !horizon.is_finite() {
            return error_scalar(num_error());
        }
        if horizon < 1.0 {
            return scalar(LiteralValue::Number(series.y[series.y.len() - 1]));
        }
        let fit = fit_holt_winters(&series.y, series.period);
        let forecast = fit.model.forecast_at(horizon);
        if !forecast.is_finite() {
            return error_scalar(num_error());
        }
        scalar(LiteralValue::Number(forecast))
    }
}

/* ─────────────────────────── FORECAST.ETS.CONFINT ─────────────────────────── */

/// Confidence interval half-width for a `FORECAST.ETS` forecast.
///
/// `FORECAST.ETS.CONFINT(target_date, values, timeline, [confidence_level], [seasonality],
/// [data_completion], [aggregation])` returns `z · RMSE · √steps`, where RMSE is the in-sample error
/// of the fitted model and `z` the two-sided normal quantile of `confidence_level` (default 0.95).
///
/// # Remarks
/// - `confidence_level` must be strictly between 0 and 1; otherwise `#NUM!`.
/// - The series and `seasonality` rules are those of `FORECAST.ETS`.
///
/// # Examples
///
/// ```yaml,sandbox
/// title: "Interval widens with the forecast horizon"
/// formula: "=FORECAST.ETS.CONFINT(13,{10;20;30;40;50;60;70;80;90;100},{1;2;3;4;5;6;7;8;9;10})>FORECAST.ETS.CONFINT(11,{10;20;30;40;50;60;70;80;90;100},{1;2;3;4;5;6;7;8;9;10})"
/// expected: true
/// ```
#[derive(Debug)]
pub struct ForecastEtsConfintFn;

/// [formualizer-docgen:schema:start]
/// Name: FORECAST.ETS.CONFINT
/// Type: ForecastEtsConfintFn
/// Min args: 3
/// Max args: 7
/// Variadic: false
/// Signature: FORECAST.ETS.CONFINT(arg1: number@scalar, arg2: any@range, arg3: any@range, arg4?: number@scalar, arg5?: number@scalar, arg6?: number@scalar, arg7?: number@scalar)
/// Arg schema: arg1{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg2{kinds=any,required=true,shape=range,by_ref=false,coercion=None,max=None,repeating=None,default=false}; arg3{kinds=any,required=true,shape=range,by_ref=false,coercion=None,max=None,repeating=None,default=false}; arg4{kinds=number,required=false,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg5{kinds=number,required=false,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg6{kinds=number,required=false,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg7{kinds=number,required=false,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}
/// Caps: PURE
/// [formualizer-docgen:schema:end]
impl Function for ForecastEtsConfintFn {
    func_caps!(PURE);
    fn name(&self) -> &'static str {
        "FORECAST.ETS.CONFINT"
    }
    fn min_args(&self) -> usize {
        3
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        &ARGS_FORECAST_ETS_CONFINT[..]
    }
    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        _ctx: &dyn FunctionContext<'b>,
    ) -> Result<CalcValue<'b>, ExcelError> {
        let target = match required_number(&args[0]) {
            Ok(n) => n,
            Err(e) => return error_scalar(e),
        };
        let values = collect_cells(&args[1])?;
        let timeline = collect_cells(&args[2])?;
        let confidence = match optional_number(args, 3, 0.95) {
            Ok(c) => c,
            Err(e) => return error_scalar(e),
        };
        if confidence <= 0.0 || confidence >= 1.0 {
            return error_scalar(num_error());
        }
        let seasonality = match seasonality_arg(args, 4) {
            Ok(s) => s,
            Err(e) => return error_scalar(e),
        };
        if let Err(e) = validate_completion_and_aggregation(args, 5) {
            return error_scalar(e);
        }
        let series = match prepare(&values, &timeline, seasonality) {
            Ok(s) => s,
            Err(e) => return error_scalar(e),
        };
        if target < series.last_x {
            return error_scalar(num_error());
        }
        let horizon = js_round((target - series.last_x) / series.step).max(1.0);
        if !horizon.is_finite() {
            return error_scalar(num_error());
        }
        let fit = fit_holt_winters(&series.y, series.period);
        let mut sse = 0.0;
        for (y, fitted) in series.y.iter().zip(&fit.model.fitted) {
            sse += (y - fitted).powi(2);
        }
        let rmse = if series.y.is_empty() {
            0.0
        } else {
            (sse / series.y.len() as f64).sqrt()
        };
        let z = std_normal_quantile(1.0 - (1.0 - confidence) / 2.0);
        let half_width = z * rmse * horizon.sqrt();
        if !half_width.is_finite() {
            return error_scalar(num_error());
        }
        scalar(LiteralValue::Number(half_width))
    }
}

/* ─────────────────────────── FORECAST.ETS.SEASONALITY ─────────────────────────── */

/// The seasonal period `FORECAST.ETS` detects in a series.
///
/// `FORECAST.ETS.SEASONALITY(values, timeline, [data_completion], [aggregation])` returns the number
/// of timeline steps in one seasonal cycle, or `1` when no seasonal pattern beats a non-seasonal
/// fit.
///
/// # Examples
///
/// ```yaml,sandbox
/// title: "A repeating four-step pattern"
/// formula: "=FORECAST.ETS.SEASONALITY({10;30;20;40;10;30;20;40;10;30;20;40},{1;2;3;4;5;6;7;8;9;10;11;12})"
/// expected: 4
/// ```
///
/// ```yaml,sandbox
/// title: "A pure trend has no seasonality"
/// formula: "=FORECAST.ETS.SEASONALITY({10;20;30;40;50;60;70;80},{1;2;3;4;5;6;7;8})"
/// expected: 1
/// ```
#[derive(Debug)]
pub struct ForecastEtsSeasonalityFn;

/// [formualizer-docgen:schema:start]
/// Name: FORECAST.ETS.SEASONALITY
/// Type: ForecastEtsSeasonalityFn
/// Min args: 2
/// Max args: 4
/// Variadic: false
/// Signature: FORECAST.ETS.SEASONALITY(arg1: any@range, arg2: any@range, arg3?: number@scalar, arg4?: number@scalar)
/// Arg schema: arg1{kinds=any,required=true,shape=range,by_ref=false,coercion=None,max=None,repeating=None,default=false}; arg2{kinds=any,required=true,shape=range,by_ref=false,coercion=None,max=None,repeating=None,default=false}; arg3{kinds=number,required=false,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg4{kinds=number,required=false,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}
/// Caps: PURE
/// [formualizer-docgen:schema:end]
impl Function for ForecastEtsSeasonalityFn {
    func_caps!(PURE);
    fn name(&self) -> &'static str {
        "FORECAST.ETS.SEASONALITY"
    }
    fn min_args(&self) -> usize {
        2
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        &ARGS_FORECAST_ETS_SEASONALITY[..]
    }
    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        _ctx: &dyn FunctionContext<'b>,
    ) -> Result<CalcValue<'b>, ExcelError> {
        let values = collect_cells(&args[0])?;
        let timeline = collect_cells(&args[1])?;
        if let Err(e) = validate_completion_and_aggregation(args, 2) {
            return error_scalar(e);
        }
        match prepare(&values, &timeline, 1) {
            Ok(series) => scalar(LiteralValue::Int(series.period as i64)),
            Err(e) => error_scalar(e),
        }
    }
}

/* ─────────────────────────── FORECAST.ETS.STAT ─────────────────────────── */

/// A statistic of the model `FORECAST.ETS` fits to a series.
///
/// `FORECAST.ETS.STAT(values, timeline, statistic_type, [seasonality], [data_completion],
/// [aggregation])` where `statistic_type` is 1 alpha, 2 beta, 3 gamma (the smoothing parameters),
/// 4 MASE, 5 SMAPE, 6 MAE, 7 RMSE (in-sample accuracy) or 8 the detected timeline step.
///
/// # Remarks
/// - `statistic_type` outside 1..8 is `#NUM!`.
/// - The series and `seasonality` rules are those of `FORECAST.ETS`.
///
/// # Examples
///
/// ```yaml,sandbox
/// title: "Detected step of a monthly-numbered timeline"
/// formula: "=FORECAST.ETS.STAT({10;20;30;40;50;60;70;80},{1;2;3;4;5;6;7;8},8)"
/// expected: 1
/// ```
///
/// ```yaml,sandbox
/// title: "Gamma is 0 without a seasonal component"
/// formula: "=FORECAST.ETS.STAT({10;20;30;40;50;60;70;80},{1;2;3;4;5;6;7;8},3,0)"
/// expected: 0
/// ```
#[derive(Debug)]
pub struct ForecastEtsStatFn;

/// [formualizer-docgen:schema:start]
/// Name: FORECAST.ETS.STAT
/// Type: ForecastEtsStatFn
/// Min args: 3
/// Max args: 6
/// Variadic: false
/// Signature: FORECAST.ETS.STAT(arg1: any@range, arg2: any@range, arg3: number@scalar, arg4?: number@scalar, arg5?: number@scalar, arg6?: number@scalar)
/// Arg schema: arg1{kinds=any,required=true,shape=range,by_ref=false,coercion=None,max=None,repeating=None,default=false}; arg2{kinds=any,required=true,shape=range,by_ref=false,coercion=None,max=None,repeating=None,default=false}; arg3{kinds=number,required=true,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg4{kinds=number,required=false,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg5{kinds=number,required=false,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}; arg6{kinds=number,required=false,shape=scalar,by_ref=false,coercion=NumberLenientText,max=None,repeating=None,default=false}
/// Caps: PURE
/// [formualizer-docgen:schema:end]
impl Function for ForecastEtsStatFn {
    func_caps!(PURE);
    fn name(&self) -> &'static str {
        "FORECAST.ETS.STAT"
    }
    fn min_args(&self) -> usize {
        3
    }
    fn arg_schema(&self) -> &'static [ArgSchema] {
        &ARGS_FORECAST_ETS_STAT[..]
    }
    fn eval<'a, 'b, 'c>(
        &self,
        args: &'c [ArgumentHandle<'a, 'b>],
        _ctx: &dyn FunctionContext<'b>,
    ) -> Result<CalcValue<'b>, ExcelError> {
        let values = collect_cells(&args[0])?;
        let timeline = collect_cells(&args[1])?;
        let stat_type = match required_number(&args[2]) {
            Ok(n) => n.floor(),
            Err(e) => return error_scalar(e),
        };
        if !(1.0..=8.0).contains(&stat_type) {
            return error_scalar(num_error());
        }
        let seasonality = match seasonality_arg(args, 3) {
            Ok(s) => s,
            Err(e) => return error_scalar(e),
        };
        if let Err(e) = validate_completion_and_aggregation(args, 4) {
            return error_scalar(e);
        }
        let series = match prepare(&values, &timeline, seasonality) {
            Ok(s) => s,
            Err(e) => return error_scalar(e),
        };
        let y = &series.y;
        let fit = fit_holt_winters(y, series.period);
        let fitted = &fit.model.fitted;
        let value = match stat_type as i64 {
            1 => fit.alpha,
            2 => fit.beta,
            3 => fit.gamma,
            4 => {
                // MASE: mean absolute error scaled by the naive one-step forecast's.
                let mae = y
                    .iter()
                    .zip(fitted)
                    .map(|(a, f)| (a - f).abs())
                    .sum::<f64>()
                    / y.len() as f64;
                let naive_mae = if y.len() > 1 {
                    y.windows(2).map(|w| (w[1] - w[0]).abs()).sum::<f64>() / (y.len() - 1) as f64
                } else {
                    1.0
                };
                if naive_mae == 0.0 {
                    0.0
                } else {
                    mae / naive_mae
                }
            }
            5 => {
                // SMAPE, skipping points where both actual and fitted are 0.
                let mut sum = 0.0;
                let mut count = 0usize;
                for (a, f) in y.iter().zip(fitted) {
                    let denom = (a.abs() + f.abs()) / 2.0;
                    if denom == 0.0 {
                        continue;
                    }
                    sum += (a - f).abs() / denom;
                    count += 1;
                }
                if count > 0 { sum / count as f64 } else { 0.0 }
            }
            6 => {
                y.iter()
                    .zip(fitted)
                    .map(|(a, f)| (a - f).abs())
                    .sum::<f64>()
                    / y.len() as f64
            }
            7 => (y
                .iter()
                .zip(fitted)
                .map(|(a, f)| (a - f).powi(2))
                .sum::<f64>()
                / y.len() as f64)
                .sqrt(),
            _ => {
                // Detected step: the median gap between consecutive sorted timeline points
                // (every numeric point, paired or not).
                let mut xs: Vec<f64> = Vec::with_capacity(timeline.len());
                for x in &timeline {
                    if let Some(x) = series_number(x)? {
                        xs.push(x);
                    }
                }
                xs.sort_by(f64::total_cmp);
                let mut diffs: Vec<f64> = xs.windows(2).map(|w| w[1] - w[0]).collect();
                if diffs.is_empty() {
                    0.0
                } else {
                    diffs.sort_by(f64::total_cmp);
                    diffs[diffs.len() / 2]
                }
            }
        };
        scalar(LiteralValue::Number(value))
    }
}

pub fn register_builtins() {
    use std::sync::Arc;
    crate::function_registry::register_builtin(Arc::new(ForecastEtsFn));
    crate::function_registry::register_builtin(Arc::new(ForecastEtsConfintFn));
    crate::function_registry::register_builtin(Arc::new(ForecastEtsSeasonalityFn));
    crate::function_registry::register_builtin(Arc::new(ForecastEtsStatFn));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_quantile_matches_reference_values() {
        assert!((std_normal_quantile(0.975) - 1.959963984540054).abs() < 1e-9);
        assert!((std_normal_quantile(0.95) - 1.6448536269514722).abs() < 1e-9);
        assert!((std_normal_quantile(0.5)).abs() < 1e-12);
        assert!((std_normal_quantile(0.025) + 1.959963984540054).abs() < 1e-9);
    }

    #[test]
    fn detects_a_period_four_cycle_and_no_seasonality_on_a_trend() {
        let seasonal = [
            10.0, 30.0, 20.0, 40.0, 10.0, 30.0, 20.0, 40.0, 10.0, 30.0, 20.0, 40.0,
        ];
        assert_eq!(detect_seasonality(&seasonal), 4);
        let trend: Vec<f64> = (1..=8).map(|i| i as f64 * 10.0).collect();
        assert_eq!(detect_seasonality(&trend), 1);
    }

    #[test]
    fn js_round_rounds_half_up() {
        assert_eq!(js_round(2.5), 3.0);
        assert_eq!(js_round(2.4999), 2.0);
        assert_eq!(js_round(0.5), 1.0);
    }
}
