use crate::metrics::METRICS_PREFIX;
use const_format::concatcp;
use lazy_static::lazy_static;
use prometheus::{register_counter_vec, register_histogram_vec, CounterVec, HistogramVec};

const RESERVATIONS_METRICS_PREFIX: &str = concatcp!(METRICS_PREFIX, "reservations_");

lazy_static! {
    pub static ref RESERVATIONS_RECONCILE_COUNTER: CounterVec = register_counter_vec!(
        concatcp!(RESERVATIONS_METRICS_PREFIX, "reconcile_counter"),
        "Number of reconciliations by the MaskReservation controller.",
        &["name", "namespace"]
    )
    .unwrap();
    pub static ref RESERVATIONS_ACTION_COUNTER: CounterVec = register_counter_vec!(
        concatcp!(RESERVATIONS_METRICS_PREFIX, "action_counter"),
        "Number of actions taken by the MaskReservation controller.",
        &["name", "namespace", "action"]
    )
    .unwrap();
    pub static ref RESERVATIONS_READ_HISTOGRAM: HistogramVec = register_histogram_vec!(
        concatcp!(RESERVATIONS_METRICS_PREFIX, "read_duration_seconds"),
        "Amount of time taken by the read phase of the MaskReservation controller.",
        &["name", "namespace", "action"]
    )
    .unwrap();
    pub static ref RESERVATIONS_WRITE_HISTOGRAM: HistogramVec = register_histogram_vec!(
        concatcp!(RESERVATIONS_METRICS_PREFIX, "write_duration_seconds"),
        "Amount of time taken by the write phase of the MaskReservation controller.",
        &["name", "namespace", "action"]
    )
    .unwrap();
}
