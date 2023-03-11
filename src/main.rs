#![feature(provide_any)]
#![feature(error_generic_member_access)]

use kube::client::Client;
pub use vpn_types::*;

mod masks;
mod providers;
mod util;

#[cfg(feature = "metrics")]
mod metrics;

#[cfg(test)]
mod test;

/// Gets the prometheus metrics server port from the environment.
#[cfg(feature = "metrics")]
fn metrics_port() -> Option<u16> {
    std::env::var("METRICS_PORT").ok().map(|s| {
        s.parse()
            .expect("failed to parse metrics port environment variable")
    })
}

/// Runs the controllers and the prometheus metrics server.
#[cfg(feature = "metrics")]
async fn run_with_metrics(client: Client, port: u16) {
    let result = tokio::join!(
        tokio::spawn(async move {
            run_controllers(client).await;
        }),
        tokio::spawn(async move {
            metrics::run_server(port).await;
        }),
    );
    result.0.unwrap();
    result.1.unwrap();
}

/// Runs the controllers, without metrics.
async fn run_controllers(client: Client) {
    let providers_client = client.clone();
    let masks_client = client.clone();
    let result = tokio::join!(
        tokio::spawn(async move {
            providers::run(providers_client)
                .await
                .expect("providers controller exit prematurely");
        }),
        tokio::spawn(async move {
            masks::run(masks_client)
                .await
                .expect("masks controller exit prematurely");
        }),
    );
    result.0.unwrap();
    result.1.unwrap();
}

/// General entrypoint when compiled with the prometheus metrics feature.
#[cfg(feature = "metrics")]
async fn run(client: Client) {
    match metrics_port() {
        // Only run the metrics server if the port is
        // specified in the environment.
        Some(port) => run_with_metrics(client, port).await,
        // Use the default entrypoint.
        None => run_controllers(client).await,
    }
}

/// General entrypoint when compiled without the prometheus metrics feature.
#[cfg(not(feature = "metrics"))]
async fn run(client: Client) {
    run_controllers(client).await;
}

#[tokio::main]
async fn main() {
    // First, a Kubernetes client must be obtained using the `kube` crate
    // The client will later be moved to the custom controller
    let client: Client = Client::try_default()
        .await
        .expect("Expected a valid KUBECONFIG environment variable.");

    // Run the configured entrypoint. It depends on whether the
    // operator was compiled with prometheus metrics enabled or not.
    run(client).await;

    // This is an unreachable branch. The controllers and metrics
    // servers should never exit without a panic.
    panic!("exited prematurely");
}
