use lazy_static::lazy_static;
use prometheus::{
    core::{MetricVec, MetricVecBuilder},
    register_gauge, register_gauge_vec, register_histogram, register_histogram_vec,
    register_int_counter, register_int_counter_vec, register_int_gauge, Gauge, GaugeVec, Histogram,
    HistogramVec, IntCounter, IntCounterVec, IntGauge,
};

lazy_static! {
    pub static ref METRICS: Metrics = Metrics::new();
}

pub struct Metrics {
    pub total_debt_grt: Gauge,
    pub total_balance_grt: Gauge,
    pub total_adjustment_grt: Gauge,
    pub receiver_count: IntGauge,
    pub loop_duration: Histogram,
    pub deposit: ResponseMetrics,
    // Per-receiver metrics
    pub debt_grt: GaugeVec,
    pub balance_grt: GaugeVec,
    pub adjustment_grt: GaugeVec,
}

impl Metrics {
    fn new() -> Self {
        Self {
            total_debt_grt: register_gauge!(
                "escrow_total_debt_grt",
                "total outstanding debt across all receivers in GRT"
            )
            .unwrap(),
            total_balance_grt: register_gauge!(
                "escrow_total_balance_grt",
                "total escrow balance across all receivers in GRT"
            )
            .unwrap(),
            total_adjustment_grt: register_gauge!(
                "escrow_total_adjustment_grt",
                "total GRT deposited in the last cycle"
            )
            .unwrap(),
            receiver_count: register_int_gauge!(
                "escrow_receiver_count",
                "number of receivers being tracked"
            )
            .unwrap(),
            loop_duration: register_histogram!(
                "escrow_loop_duration_seconds",
                "duration of each polling cycle in seconds"
            )
            .unwrap(),
            deposit: ResponseMetrics::new("escrow_deposit", "escrow deposit transaction"),
            debt_grt: register_gauge_vec!(
                "escrow_debt_grt",
                "outstanding debt per receiver in GRT",
                &["receiver"]
            )
            .unwrap(),
            balance_grt: register_gauge_vec!(
                "escrow_balance_grt",
                "escrow balance per receiver in GRT",
                &["receiver"]
            )
            .unwrap(),
            adjustment_grt: register_gauge_vec!(
                "escrow_adjustment_grt",
                "last adjustment per receiver in GRT",
                &["receiver"]
            )
            .unwrap(),
        }
    }
}

#[derive(Clone)]
pub struct ResponseMetrics {
    pub ok: IntCounter,
    pub err: IntCounter,
    pub duration: Histogram,
}

impl ResponseMetrics {
    pub fn new(prefix: &str, description: &str) -> Self {
        let metrics = Self {
            ok: register_int_counter!(
                &format!("{prefix}_ok"),
                &format!("{description} success count"),
            )
            .unwrap(),
            err: register_int_counter!(
                &format!("{prefix}_err"),
                &format!("{description} error count"),
            )
            .unwrap(),
            duration: register_histogram!(
                &format!("{prefix}_duration"),
                &format!("{description} duration"),
            )
            .unwrap(),
        };
        metrics.ok.inc();
        metrics.err.inc();
        metrics
    }
}

#[derive(Clone)]
pub struct ResponseMetricVecs {
    pub ok: IntCounterVec,
    pub err: IntCounterVec,
    pub duration: HistogramVec,
}

impl ResponseMetricVecs {
    pub fn new(prefix: &str, description: &str, labels: &[&str]) -> Self {
        Self {
            ok: register_int_counter_vec!(
                &format!("{prefix}_ok"),
                &format!("{description} success count"),
                labels,
            )
            .unwrap(),
            err: register_int_counter_vec!(
                &format!("{prefix}_err"),
                &format!("{description} error count"),
                labels,
            )
            .unwrap(),
            duration: register_histogram_vec!(
                &format!("{prefix}_duration"),
                &format!("{description} duration"),
                labels,
            )
            .unwrap(),
        }
    }

    pub fn check<T, E>(&self, label_values: &[&str], result: &Result<T, E>) {
        match &result {
            Ok(_) => with_metric(&self.ok, label_values, |c| c.inc()),
            Err(_) => with_metric(&self.err, label_values, |c| c.inc()),
        };
    }
}

pub fn with_metric<T, F, B>(metric_vec: &MetricVec<B>, label_values: &[&str], f: F) -> Option<T>
where
    B: MetricVecBuilder,
    F: Fn(B::M) -> T,
{
    metric_vec
        .get_metric_with_label_values(label_values)
        .ok()
        .map(f)
}
