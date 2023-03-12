#![feature(provide_any)]
#![feature(error_generic_member_access)]

use clap::{Parser, Subcommand};
use kube::client::Client;
pub use vpn_types::*;

mod masks;
mod providers;
mod util;

#[cfg(feature = "metrics")]
mod metrics;

#[cfg(test)]
mod test;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
#[command(propagate_version = true)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    ManageProviders,
    ManageMasks,
}

/// Gets the prometheus metrics server port from the environment.
#[cfg(feature = "metrics")]
fn metrics_port() -> Option<u16> {
    std::env::var("METRICS_PORT").ok().map(|s| {
        s.parse()
            .expect("failed to parse metrics port environment variable")
    })
}

/// Runs the controller and the prometheus metrics server.
#[cfg(feature = "metrics")]
async fn run_with_metrics(client: Client, port: u16) {
    tokio::join!(run_controller(client), metrics::run_server(port));
}

/// Runs just the controller by itself.
async fn run_controller(client: Client) {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::ManageProviders) => providers::run(client)
            .await
            .unwrap(),
        Some(Command::ManageMasks) => masks::run(client)
            .await
            .unwrap(),
        None => {
            println!("Please choose a subcommand.");
            std::process::exit(1);
        }
    }
    panic!("exited prematurely");
}

/// General entrypoint when compiled with the prometheus metrics feature.
#[cfg(feature = "metrics")]
async fn run(client: Client) {
    match metrics_port() {
        // Only run the metrics server if the port is
        // specified in the environment.
        Some(port) => run_with_metrics(client, port).await,
        // Use the default entrypoint.
        None => run_controller(client).await,
    }
}

/// General entrypoint when compiled without the prometheus metrics feature.
#[cfg(not(feature = "metrics"))]
async fn run(client: Client) {
    run_controller(client).await;
}

#[tokio::main]
async fn main() {
    // Set the panic hook to exit the process with a non-zero exit code
    // when a panic occurs on any thread.
    let default_panic = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default_panic(info);
        std::process::exit(1);
    }));

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
