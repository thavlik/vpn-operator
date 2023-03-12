use futures::{StreamExt, TryStreamExt};
use k8s_openapi::api::core::v1::{Namespace, Secret};
use kube::{
    api::{ListParams, ObjectMeta, Resource},
    client::Client,
    core::{NamespaceResourceScope, WatchEvent},
    Api, CustomResourceExt, ResourceExt,
};
use serde::{de::DeserializeOwned, Serialize};
use std::{clone::Clone, fmt::Debug};
use tokio::spawn;
use vpn_types::*;

const MAX_SLOTS: usize = 1;
const PROVIDER_NAME: &str = "test-provider";
const MASK_NAME: &str = "test-mask";
const NAMESPACE_PREFIX: &str = "vpn-test-";

/// All errors possible to occur during testing.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Any error originating from the `kube-rs` crate
    #[error("Kubernetes reported error: {source}")]
    KubeError {
        #[from]
        source: kube::Error,
    },
    #[error("Error: {0}")]
    Other(String),
}

/// Returns the test Provider's credentials Secret resource.
fn get_test_provider_secret(provider: &Provider) -> Secret {
    Secret {
        metadata: ObjectMeta {
            name: Some(PROVIDER_NAME.to_owned()),
            namespace: Some(provider.metadata.namespace.clone().unwrap()),
            owner_references: Some(vec![provider.controller_owner_ref(&()).unwrap()]),
            ..Default::default()
        },
        string_data: Some(
            vec![
                // These values correspond to glueten environment variables.
                ("VPN_NAME".to_owned(), "my-vpn-provider-name".to_owned()),
                ("VPN_USERNAME".to_owned(), "test-username".to_owned()),
                ("VPN_PASSWORD".to_owned(), "test-password".to_owned()),
            ]
            .into_iter()
            .collect(),
        ),
        ..Default::default()
    }
}

/// Returns the test Provider resource.
fn get_test_provider(namespace: &str) -> Provider {
    Provider {
        metadata: ObjectMeta {
            name: Some(PROVIDER_NAME.to_string()),
            namespace: Some(namespace.to_owned()),
            ..Default::default()
        },
        spec: ProviderSpec {
            max_slots: MAX_SLOTS,
            secret: PROVIDER_NAME.to_owned(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Returns a test Mask resource with the given slot as the name suffix.
fn get_test_mask(namespace: &str, slot: usize) -> Mask {
    Mask {
        metadata: ObjectMeta {
            name: Some(format!("{}-{}", MASK_NAME, slot)),
            namespace: Some(namespace.to_owned()),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Create the test Provider's credentials Secret resource.
async fn create_test_provider_secret(client: Client, provider: &Provider) -> Result<Secret, Error> {
    let secret = get_test_provider_secret(&provider);
    let secret_api: Api<Secret> =
        Api::namespaced(client, provider.metadata.namespace.as_deref().unwrap());
    Ok(secret_api.create(&Default::default(), &secret).await?)
}

/// Creates the test Provider and its secret.
async fn create_test_provider(client: Client, namespace: &str) -> Result<Provider, Error> {
    let provider = create_wait(
        client.clone(),
        PROVIDER_NAME,
        namespace,
        get_test_provider(namespace),
    )
    .await?;
    println!(
        "Created Provider with uid {}",
        provider.metadata.uid.as_deref().unwrap()
    );
    create_test_provider_secret(client, &provider).await?;
    Ok(provider)
}

/// Creates a test Mask with the given slot as the name suffix.
async fn create_test_mask(client: Client, namespace: &str, slot: usize) -> Result<Mask, Error> {
    let mask_name = format!("{}-{}", MASK_NAME, slot);
    Ok(create_wait(
        client,
        &mask_name,
        namespace,
        get_test_mask(namespace, slot),
    )
    .await?)
}

/// Waits for the test Provider to be assigned to the test Mask.
async fn wait_for_provider_assignment(
    client: Client,
    namespace: &str,
    slot: usize,
) -> Result<AssignedProvider, Error> {
    let name = format!("{}-{}", MASK_NAME, slot);
    let mask_api: Api<Mask> = Api::namespaced(client, namespace);
    let lp = ListParams::default()
        .fields(&format!("metadata.name={}", name))
        .timeout(20);
    let mut stream = mask_api.watch(&lp, "0").await?.boxed();
    while let Some(event) = stream.try_next().await? {
        match event {
            WatchEvent::Added(m) | WatchEvent::Modified(m) => match m.status {
                Some(ref status) if status.provider.is_some() => {
                    return Ok(status.provider.clone().unwrap());
                }
                _ => continue,
            },
            _ => continue,
        }
    }
    // Check if it's assigned now and we missed it.
    let mask = mask_api.get(&name).await?;
    if let Some(ref status) = mask.status {
        if let Some(ref provider) = status.provider {
            return Ok(provider.clone());
        }
    }
    Err(Error::Other(format!(
        "Provider not assigned to Mask {} before timeout",
        name,
    )))
}

/// Waits for the Mask resourece to observe phase Waiting.
async fn wait_for_waiting(client: Client, namespace: &str, slot: usize) -> Result<(), Error> {
    let name = format!("{}-{}", MASK_NAME, slot);
    let mask_api: Api<Mask> = Api::namespaced(client, namespace);
    let lp = ListParams::default()
        .fields(&format!("metadata.name={}", &name))
        .timeout(20);
    let mut stream = mask_api.watch(&lp, "0").await?.boxed();
    while let Some(event) = stream.try_next().await? {
        match event {
            WatchEvent::Added(m) | WatchEvent::Modified(m) => {
                match m.status.as_ref().map(|status| status.phase) {
                    Some(Some(MaskPhase::Waiting)) => {
                        return Ok(());
                    }
                    _ => continue,
                }
            }
            _ => continue,
        }
    }
    // See if we missed it.
    let mask = mask_api.get(&name).await?;
    if let Some(ref status) = mask.status {
        if let Some(MaskPhase::Waiting) = status.phase {
            return Ok(());
        }
    }
    Err(Error::Other(format!(
        "Waiting not observed for Mask {} before timeout",
        name,
    )))
}

/// Returns the test Provider's credentials Secret resource.
async fn get_provider_secret(client: Client, provider: &Provider) -> Result<Secret, Error> {
    let secret_api: Api<Secret> =
        Api::namespaced(client, provider.metadata.namespace.as_deref().unwrap());
    Ok(secret_api.get(&provider.spec.secret).await?)
}

/// Waits for a Secret resource to appear.
async fn wait_for_secret(
    client: Client,
    secret_name: String,
    namespace: &str,
) -> Result<Secret, Error> {
    let secret_api: Api<Secret> = Api::namespaced(client, namespace);
    let lp = ListParams::default()
        .fields(&format!("metadata.name={}", &secret_name))
        .timeout(20);
    let mut stream = secret_api.watch(&lp, "0").await?.boxed();
    while let Some(event) = stream.try_next().await? {
        match event {
            WatchEvent::Added(m) | WatchEvent::Modified(m) => return Ok(m),
            _ => continue,
        }
    }
    // See if we missed update events and can get it now.
    Ok(secret_api.get(&secret_name).await?)
}

async fn create_test_namespace(client: Client) -> Result<String, Error> {
    let name = format!("{}{}", NAMESPACE_PREFIX, uuid::Uuid::new_v4());
    let namespace_api: Api<Namespace> = Api::all(client);
    let namespace = Namespace {
        metadata: ObjectMeta {
            name: Some(name.clone()),
            ..Default::default()
        },
        ..Default::default()
    };
    namespace_api
        .create(&Default::default(), &namespace)
        .await?;
    Ok(name)
}

async fn delete_namespace(client: Client, name: &str) -> Result<(), Error> {
    let namespace_api: Api<Namespace> = Api::all(client);
    namespace_api.delete(name, &Default::default()).await?;
    Ok(())
}

/// Deletes all of the test resources.
async fn cleanup(client: Client, namespace: &str) -> Result<(), Error> {
    delete_namespace(client, namespace).await
}

#[tokio::test]
async fn basic() -> Result<(), Error> {
    let client: Client = Client::try_default().await.unwrap();
    let namespace = create_test_namespace(client.clone()).await?;

    // Create the test Provider.
    let provider = create_test_provider(client.clone(), &namespace)
        .await
        .expect("failed to create provider");
    let provider_uid = provider.metadata.uid.as_deref().unwrap();

    // Watch for a Provider to be assigned to the Mask.
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
    let mask = create_test_mask(client.clone(), &namespace, 0).await?;

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
    // from the Provider's secret. It should be an exact match.
    let mask_secret = mask_secret.await.unwrap()?;
    let provider_secret = get_provider_secret(client.clone(), &provider).await?;
    assert_eq!(provider_secret.data, mask_secret.data);

    // Garbage collect the test resources.
    cleanup(client.clone(), &namespace).await?;

    Ok(())
}

#[tokio::test]
async fn waiting() -> Result<(), Error> {
    let client: Client = Client::try_default().await.unwrap();
    let namespace = create_test_namespace(client.clone()).await?;

    // Create the test Provider.
    let provider = create_test_provider(client.clone(), &namespace)
        .await
        .expect("failed to create test provider");
    let provider_uid = provider.metadata.uid.as_deref().unwrap();

    // Watch for a Provider to be assigned to the Mask.
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
    let mask0 = create_test_mask(client.clone(), &namespace, 0).await?;

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
    // from the Provider's secret. It should be an exact match.
    let mask0_secret = mask0_secret.await.unwrap()?;
    let provider_secret = get_provider_secret(client.clone(), &provider).await?;
    assert_eq!(provider_secret.data, mask0_secret.data);

    // Try and create a second Mask and ensure it fails.
    let mask1_fail = {
        let client = client.clone();
        let namespace = namespace.clone();
        spawn(async move { wait_for_waiting(client, &namespace, 1).await })
    };
    let mask1 = create_test_mask(client.clone(), &namespace, 1).await?;

    // Ensure the error state was observed.
    mask1_fail.await.unwrap()?;

    // Delete the first Mask and ensure the second Mask is assigned to the Provider.
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

    // Garbage collect the test resources.
    cleanup(client.clone(), &namespace).await?;

    Ok(())
}

/// Deletes the test Mask at the given slot.
async fn delete_test_mask(client: Client, namespace: &str, slot: usize) -> Result<(), Error> {
    assert!(
        delete_wait::<Mask>(
            client.clone(),
            &format!("{}-{}", MASK_NAME, slot),
            namespace
        )
        .await?
    );
    Ok(())
}

/// Waits for the resource to be deleted.
async fn delete_wait<
    T: Clone + Resource + CustomResourceExt + Serialize + DeserializeOwned + Debug,
>(
    client: Client,
    name: &str,
    namespace: &str,
) -> Result<bool, Error>
where
    <T as Resource>::DynamicType: Default,
    T: Resource<Scope = NamespaceResourceScope>,
{
    let api: Api<T> = Api::namespaced(client, namespace);
    match api.get(name).await {
        // Resource is still around. Try and delete it.
        Ok(_) => {}
        // The resource has already been deleted.
        Err(kube::Error::Api(ae)) if ae.code == 404 => {
            println!("{}/{} does not exist", namespace, name);
            return Ok(true);
        }
        // Some other error.
        Err(e) => return Err(e.into()),
    }
    println!("Watch delete events for {}/{}", namespace, name);
    let lp = ListParams::default()
        .fields(&format!("metadata.name={}", name))
        .timeout(8);
    let mut stream = api.watch(&lp, "0").await?.boxed();
    // Now that we're watching for the delete event,
    // try and remove the resource.
    println!("Deleting resource {}/{}", namespace, name);
    match api.delete(name, &Default::default()).await {
        // Wait for the delete event.
        Ok(_) => {}
        // Resource has already been deleted.
        Err(kube::Error::Api(ae)) if ae.code == 404 => return Ok(true),
        // Unknown error.
        Err(e) => return Err(e.into()),
    }
    println!("Waiting on delete event for {}/{}", namespace, name);
    while let Some(event) = stream.try_next().await? {
        match event {
            // Delete event detected.
            WatchEvent::Deleted(_) => {
                // As one last sanity check, let's make sure the resource
                // is actually gone.
                match api.get(name).await {
                    // Resource still exists. Continue watching.
                    Ok(_) => {
                        println!(
                            "Warning: Delete event for {}/{} detected, but resource still exists.",
                            namespace, name
                        );
                        continue;
                    }
                    // Resource no longer exists.
                    Err(kube::Error::Api(ae)) if ae.code == 404 => return Ok(true),
                    // Some other error.
                    Err(e) => return Err(e.into()),
                }
            }
            _ => continue,
        }
    }
    // We may have missed the deletion event. Check if it exists.
    println!(
        "Delete events timed out. Checking if {}/{} still exists...",
        namespace, name
    );
    match api.get(name).await {
        // Resource still exists.
        Ok(_) => Ok(false),
        // Resource no longer exists and we missed the WatchEvent.
        Err(kube::Error::Api(ae)) if ae.code == 404 => Ok(true),
        // Some other error.
        Err(e) => Err(e.into()),
    }
}

async fn create_wait<
    T: Clone + Resource + CustomResourceExt + Serialize + DeserializeOwned + Debug,
>(
    client: Client,
    name: &str,
    namespace: &str,
    resource: T,
) -> Result<T, Error>
where
    <T as Resource>::DynamicType: Default,
    T: Resource<Scope = NamespaceResourceScope>,
{
    let api: Api<T> = Api::namespaced(client, namespace);
    let start = std::time::SystemTime::now();
    let timeout = std::time::Duration::from_secs(12);
    loop {
        match api.create(&Default::default(), &resource).await {
            Ok(mask) => return Ok(mask),
            Err(kube::Error::Api(ae)) if ae.code == 409 => {
                if start.elapsed().unwrap() > timeout {
                    panic!(
                        "Timed out waiting for {} {}/{} to be deleted",
                        T::crd().metadata.name.as_deref().unwrap(),
                        namespace,
                        name
                    );
                }
                // Try and delete it again.
                match api.delete(name, &Default::default()).await {
                    Ok(_) => {}
                    Err(kube::Error::Api(ae)) if ae.code == 404 => {}
                    Err(e) => return Err(e.into()),
                }
                // Sleep and try again
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
            Err(e) => return Err(e.into()),
        }
    }
}
