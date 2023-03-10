#![feature(provide_any)]
#![feature(error_generic_member_access)]

use kube::client::Client;
pub use vpn_types::*;

mod masks;
mod providers;
mod util;

#[cfg(test)]
mod test;

#[tokio::main]
async fn main() {
    // First, a Kubernetes client must be obtained using the `kube` crate
    // The client will later be moved to the custom controller
    let kubernetes_client: Client = Client::try_default()
        .await
        .expect("Expected a valid KUBECONFIG environment variable.");
    let providers_client = kubernetes_client.clone();
    let masks_client = kubernetes_client.clone();
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
    panic!("exited prematurely");
}
