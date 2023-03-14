use const_format::concatcp;
use hyper::{
    header::CONTENT_TYPE,
    service::{make_service_fn, service_fn},
    Body, Request, Response, Server,
};
use lazy_static::lazy_static;
use prometheus::{
    labels, opts, register_counter, register_counter_vec, register_gauge, register_histogram_vec,
};
use prometheus::{Counter, CounterVec, Encoder, Gauge, HistogramVec, TextEncoder};

/// The prefix to add to all the prometheus metrics keys.
const METRICS_PREFIX: &str = "vpno_";
const MASK_METRICS_PREFIX: &str = concatcp!(METRICS_PREFIX, "provider_");
const PROVIDER_METRICS_PREFIX: &str = concatcp!(METRICS_PREFIX, "mask_");

lazy_static! {
    pub static ref MASK_RECONCILE_COUNTER: CounterVec = register_counter_vec!(
        concatcp!(MASK_METRICS_PREFIX, "reconcile_counter"),
        "Number of reconciliations by the mask controller.",
        &["name", "namespace"]
    )
    .unwrap();
    static ref MASK_ACTION_COUNTER: CounterVec = register_counter_vec!(
        concatcp!(MASK_METRICS_PREFIX, "action_counter"),
        "Number of actions taken by the mask controller.",
        &["name", "namespace", "action"]
    )
    .unwrap();
    static ref MASK_READ_HISTOGRAM: HistogramVec = register_histogram_vec!(
        concatcp!(MASK_METRICS_PREFIX, "read_duration_seconds"),
        "Amount of time taken by the read phase of the Mask controller.",
        &["name", "namespace", "action"]
    )
    .unwrap();
    static ref MASK_WRITE_HISTOGRAM: HistogramVec = register_histogram_vec!(
        concatcp!(MASK_METRICS_PREFIX, "write_duration_seconds"),
        "Amount of time taken by the write phase of the Mask controller.",
        &["name", "namespace", "action"]
    )
    .unwrap();
    pub static ref PROVIDER_RECONCILE_COUNTER: CounterVec = register_counter_vec!(
        concatcp!(PROVIDER_METRICS_PREFIX, "reconcile_counter"),
        "Number of reconciliations by the provider controller.",
        &["name", "namespace"]
    )
    .unwrap();
    static ref PROVIDER_ACTION_COUNTER: CounterVec = register_counter_vec!(
        concatcp!(PROVIDER_METRICS_PREFIX, "action_counter"),
        "Number of actions taken by the provider controller.",
        &["name", "namespace", "action"]
    )
    .unwrap();
    static ref PROVIDER_READ_HISTOGRAM: HistogramVec = register_histogram_vec!(
        concatcp!(PROVIDER_METRICS_PREFIX, "read_duration_seconds"),
        "Amount of time taken by the read phase of the Provider controller.",
        &["name", "namespace", "action"]
    )
    .unwrap();
    static ref PROVIDER_WRITE_HISTOGRAM: HistogramVec = register_histogram_vec!(
        concatcp!(PROVIDER_METRICS_PREFIX, "write_duration_seconds"),
        "Amount of time taken by the write phase of the Provider controller.",
        &["name", "namespace", "action"]
    )
    .unwrap();
    static ref HTTP_COUNTER: Counter = register_counter!(opts!(
        concatcp!(METRICS_PREFIX, "http_requests_total"),
        "Number of HTTP requests made to the metrics server.",
        labels! {"handler" => "all",}
    ))
    .unwrap();
    static ref HTTP_BODY_GAUGE: Gauge = register_gauge!(opts!(
        concatcp!(METRICS_PREFIX, "http_response_size_bytes"),
        "Metrics server HTTP response sizes in bytes.",
        labels! {"handler" => "all",}
    ))
    .unwrap();
    static ref HTTP_REQ_HISTOGRAM: HistogramVec = register_histogram_vec!(
        concatcp!(METRICS_PREFIX, "http_request_duration_seconds"),
        "Metrics server HTTP request latencies in seconds.",
        &["handler"]
    )
    .unwrap();
}

/// Handler to serve the prometheus metrics to the request.
async fn serve_req(_req: Request<Body>) -> Result<Response<Body>, hyper::Error> {
    let encoder = TextEncoder::new();
    HTTP_COUNTER.inc();
    let timer = HTTP_REQ_HISTOGRAM.with_label_values(&["all"]).start_timer();
    let metric_families = prometheus::gather();
    let mut buffer = vec![];
    encoder.encode(&metric_families, &mut buffer).unwrap();
    HTTP_BODY_GAUGE.set(buffer.len() as f64);
    let response = Response::builder()
        .status(200)
        .header(CONTENT_TYPE, encoder.format_type())
        .body(Body::from(buffer))
        .unwrap();
    timer.observe_duration();
    Ok(response)
}

/// Runs the prometheus metrics server on the given port.
pub async fn run_server(port: u16) {
    let addr = ([0, 0, 0, 0], port).into();
    println!("Metrics server listening on http://{}", addr);

    let serve_future = Server::bind(&addr).serve(make_service_fn(|_| async {
        Ok::<_, hyper::Error>(service_fn(serve_req))
    }));

    if let Err(err) = serve_future.await {
        panic!("metrics server error: {}", err);
    }

    panic!("metrics server exited");
}
