use kube::client::Client;
use std::clone::Clone;
use tokio::spawn;
use vpn_types::*;

use super::util::*;

#[tokio::test]
async fn err_no_providers() -> Result<(), Error> {
    let client: Client = Client::try_default().await.unwrap();
    let (uid, namespace) = create_test_namespace(client.clone()).await?;
    let provider_label = format!("{}-{}", PROVIDER_NAME, uid);

    // Watch for the error message in the Mask's status.
    let fail = {
        let client = client.clone();
        let namespace = namespace.clone();
        spawn(
            async move { wait_for_mask_phase(client, &namespace, 0, MaskPhase::ErrNoProviders).await },
        )
    };

    // Create a Mask without first creating the MaskProvider.
    create_test_mask(client.clone(), &namespace, 0, &provider_label).await?;

    // Ensure the error state is observed.
    fail.await.unwrap()?;

    // Garbage collect the test resources.
    cleanup(client, &namespace).await?;

    Ok(())
}
