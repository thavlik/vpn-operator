use kube::{client::Client, ResourceExt};
use std::clone::Clone;
use tokio::spawn;
use vpn_types::*;

use super::util::*;

#[tokio::test]
async fn basic() -> Result<(), Error> {
    let client: Client = Client::try_default().await.unwrap();
    let (uid, namespace) = create_test_namespace(client.clone()).await?;
    let provider_label = format!("{}-{}", PROVIDER_NAME, uid);

    // Create the test MaskProvider and wait for it to be Ready.
    let provider_ready = {
        let client = client.clone();
        let namespace = namespace.clone();
        spawn(
            async move { wait_for_provider_phase(client, &namespace, MaskProviderPhase::Ready).await },
        )
    };
    let provider = create_test_provider(client.clone(), &namespace, &uid)
        .await
        .expect("failed to create provider");
    let provider_uid = provider.metadata.uid.as_deref().unwrap();
    provider_ready.await.unwrap()?;

    // Watch for a MaskProvider to be assigned to the Mask.
    let mask_secret = {
        let mask_secret_name = format!("{}-{}-{}", MASK_NAME, 0, provider_uid);
        let client = client.clone();
        let namespace = namespace.clone();
        spawn(async move { wait_for_secret(client, mask_secret_name, &namespace).await })
    };

    // Create the test Mask and wait for a provider to be assigned.
    let assigned_provider = {
        let client = client.clone();
        let namespace = namespace.clone();
        spawn(async move { wait_for_provider_assignment(client, &namespace, 0).await })
    };
    let mask = create_test_mask(client.clone(), &namespace, 0, &provider_label).await?;

    // The provider assigned should be the same as the one we created.
    let assigned_provider = assigned_provider.await.unwrap()?;
    assert_eq!(assigned_provider.name, provider.name_any());
    assert_eq!(assigned_provider.namespace, provider.namespace().unwrap());
    assert_eq!(&assigned_provider.uid, provider_uid);
    assert_eq!(
        assigned_provider.secret,
        format!("{}-{}", mask.name_any(), provider_uid)
    );

    // Ensure the Mask's credentials were correctly inherited
    // from the MaskProvider's secret. It should be an exact match.
    let mask_secret = mask_secret.await.unwrap()?;
    let provider_secret = get_provider_secret(client.clone(), &provider).await?;
    assert_eq!(provider_secret.data, mask_secret.data);

    // Garbage collect the test resources.
    cleanup(client, &namespace).await?;

    Ok(())
}
