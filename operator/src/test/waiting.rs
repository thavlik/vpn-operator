use kube::{client::Client, Api, ResourceExt};
use std::clone::Clone;
use tokio::spawn;
use vpn_types::*;

use super::util::*;

#[tokio::test]
async fn waiting() -> Result<(), Error> {
    let client: Client = Client::try_default().await.unwrap();
    let (uid, namespace) = create_test_namespace(client.clone()).await?;

    // Create the test MaskProvider.
    let provider = create_test_provider(client.clone(), &namespace, &uid)
        .await
        .expect("failed to create test provider");
    let provider_name = provider.metadata.name.as_deref().unwrap();
    let provider_uid = provider.metadata.uid.as_deref().unwrap();

    // Watch for a MaskProvider to be assigned to the Mask.
    let mask0_secret_name = format!("{}-{}-{}", MASK_NAME, 0, provider_uid);
    let mask0_secret = {
        let client = client.clone();
        let namespace = namespace.clone();
        spawn(async move { wait_for_secret(client, mask0_secret_name.clone(), &namespace).await })
    };

    // Create the first test Mask and wait for a provider to be assigned.
    let assigned_provider = {
        let client = client.clone();
        let namespace = namespace.clone();
        spawn(async move { wait_for_provider_assignment(client, &namespace, 0).await })
    };
    let mask0 = create_test_mask(client.clone(), &namespace, 0, provider_name).await?;

    // The provider assigned should be the same as the one we created.
    let assigned_provider = assigned_provider
        .await
        .unwrap()
        .expect("failed to wait for provider assignment");
    assert_eq!(assigned_provider.name, provider.name_any());
    assert_eq!(assigned_provider.namespace, provider.namespace().unwrap());
    assert_eq!(
        &assigned_provider.uid,
        provider.metadata.uid.as_deref().unwrap()
    );
    assert_eq!(
        assigned_provider.secret,
        format!("{}-{}", mask0.name_any(), provider_uid)
    );

    // Ensure the Mask's credentials were correctly inherited
    // from the MaskProvider's secret. It should be an exact match.
    let mask0_secret = mask0_secret.await.unwrap()?;
    let provider_secret = get_provider_secret(client.clone(), &provider).await?;
    assert_eq!(provider_secret.data, mask0_secret.data);

    // Try and create a second Mask and ensure it doesn't go Ready.
    let mask1_wait = {
        let client = client.clone();
        let namespace = namespace.clone();
        spawn(async move { wait_for_mask_phase(client, &namespace, 1, MaskPhase::Waiting).await })
    };
    let mask1 = create_test_mask(client.clone(), &namespace, 1, provider_name).await?;

    // Ensure the waiting status was observed.
    mask1_wait.await.unwrap()?;

    // Delete the first Mask and ensure the second Mask is assigned to the MaskProvider.
    let assigned_provider = {
        let client = client.clone();
        let namespace = namespace.clone();
        spawn(async move { wait_for_provider_assignment(client, &namespace, 1).await })
    };
    delete_test_mask(client.clone(), &namespace, 0).await?;

    // Ensure the test provider was assigned to the second Mask.
    let assigned_provider = assigned_provider
        .await
        .unwrap()
        .expect("failed to wait for provider assignment");
    assert_eq!(assigned_provider.name, provider.name_any());
    assert_eq!(assigned_provider.namespace, provider.namespace().unwrap());
    assert_eq!(
        &assigned_provider.uid,
        provider.metadata.uid.as_deref().unwrap()
    );
    assert_eq!(
        assigned_provider.secret,
        format!("{}-{}", mask1.name_any(), provider_uid)
    );

    // Delete the Provider and ensure the Mask has ErrNoProviders phase.
    let mask1_wait = {
        let client = client.clone();
        let namespace = namespace.clone();
        spawn(
            async move { wait_for_mask_phase(client, &namespace, 1, MaskPhase::ErrNoProviders).await },
        )
    };
    delete_test_provider(client.clone(), &namespace, &provider_name).await?;

    // Ensure the ErrNoProviders phase was observed.
    mask1_wait.await.unwrap()?;

    // Sanity check: ensure all MaskConsumers in the namespace have ErrNoProviders.
    assert!(Api::<MaskConsumer>::namespaced(client.clone(), &namespace)
        .list(&Default::default())
        .await?
        .into_iter()
        .filter_map(|mc| mc.status)
        .filter_map(|s| s.phase)
        .all(|p| p == MaskConsumerPhase::ErrNoProviders));

    // Sanity check: ensure there are no MaskReservations in the namespace.
    assert_eq!(
        Api::<MaskReservation>::namespaced(client.clone(), &namespace)
            .list(&Default::default())
            .await?
            .items
            .len(),
        0
    );

    // Garbage collect the test resources.
    cleanup(client, &namespace).await?;

    Ok(())
}
