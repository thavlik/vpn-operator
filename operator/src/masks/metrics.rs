use crate::metrics::METRICS_PREFIX;
use const_format::concatcp;
use lazy_static::lazy_static;
use prometheus::{register_counter_vec, register_histogram_vec, CounterVec, HistogramVec};

const MASK_METRICS_PREFIX: &str = concatcp!(METRICS_PREFIX, "mask_");

lazy_static! {
    pub static ref MASK_RECONCILE_COUNTER: CounterVec = register_counter_vec!(
        concatcp!(MASK_METRICS_PREFIX, "reconcile_counter"),
        "Number of reconciliations by the mask controller.",
        &["name", "namespace"]
    )
    .unwrap();
    pub static ref MASK_ACTION_COUNTER: CounterVec = register_counter_vec!(
        concatcp!(MASK_METRICS_PREFIX, "action_counter"),
        "Number of actions taken by the mask controller.",
        &["name", "namespace", "action"]
    )
    .unwrap();
    pub static ref MASK_READ_HISTOGRAM: HistogramVec = register_histogram_vec!(
        concatcp!(MASK_METRICS_PREFIX, "read_duration_seconds"),
        "Amount of time taken by the read phase of the Mask controller.",
        &["name", "namespace", "action"]
    )
    .unwrap();
    pub static ref MASK_WRITE_HISTOGRAM: HistogramVec = register_histogram_vec!(
        concatcp!(MASK_METRICS_PREFIX, "write_duration_seconds"),
        "Amount of time taken by the write phase of the Mask controller.",
        &["name", "namespace", "action"]
    )
    .unwrap();
}
