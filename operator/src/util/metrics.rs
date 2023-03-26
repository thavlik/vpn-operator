use prometheus::{register_counter_vec, register_histogram_vec, CounterVec, HistogramVec};

/// Contains the metrics for a controller. Each controller will use
/// unique metric names, but they will use these same metric types.
pub struct ControllerMetrics {
    pub reconcile_counter: CounterVec,
    pub action_counter: CounterVec,
    pub read_histogram: HistogramVec,
    pub write_histogram: HistogramVec,
}

impl ControllerMetrics {
    /// Creates a new set of metrics for a controller. The tag is used
    /// to associate the metrics with a specific controller.
    pub fn new(tag: &str) -> Self {
        let prefix = metrics_prefix();
        let reconcile_counter = register_counter_vec!(
            &format!("{}_{}_reconcile_counter", prefix, tag),
            "Number of reconciliations by the controller.",
            &["name", "namespace"]
        )
        .unwrap();
        let action_counter = register_counter_vec!(
            &format!("{}_{}_action_counter", prefix, tag),
            "Number of actions taken by the controller.",
            &["name", "namespace", "action"]
        )
        .unwrap();
        let read_histogram = register_histogram_vec!(
            &format!("{}_{}_read_duration_seconds", prefix, tag),
            "Read phase latency of the controller.",
            &["name", "namespace", "action"]
        )
        .unwrap();
        let write_histogram = register_histogram_vec!(
            &format!("{}_{}_write_duration_seconds", prefix, tag),
            "Write phase latency of the controller.",
            &["name", "namespace", "action"]
        )
        .unwrap();
        ControllerMetrics {
            reconcile_counter,
            action_counter,
            read_histogram,
            write_histogram,
        }
    }
}

/// Returns the metrics prefix, which can be overridden with the
/// METRICS_PREFIX environment variable.
fn metrics_prefix() -> String {
    std::env::var("METRICS_PREFIX").unwrap_or_else(|_| "vpno".to_string())
}
