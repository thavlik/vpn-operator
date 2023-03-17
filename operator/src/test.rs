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

async fn get_actual_provider_secret(client: Client) -> Result<Option<Secret>, Error> {
    let name = match std::env::var("SECRET_NAME") {
        Ok(name) => name,
        Err(_) => return Ok(None),
    };
    let namespace = match std::env::var("SECRET_NAMESPACE") {
        Ok(namespace) => namespace,
        Err(_) => "default".to_owned(),
    };
    let secret_api: Api<Secret> = Api::namespaced(client, &namespace);
    Ok(Some(secret_api.get(&name).await?))
}

/// Returns the test MaskProvider's credentials Secret resource.
/// If the environment specified a real secret, it will be used.
/// This will also enable verification. Otherwise, mock credentials
/// will be used and verificationwill be disabled.
async fn get_test_provider_secret(
    client: Client,
    provider: &MaskProvider,
) -> Result<Secret, Error> {
    // Use the default test credentials, which bypass verification.
    let env_secret = get_actual_provider_secret(client).await?;
    Ok(Secret {
        metadata: ObjectMeta {
            name: Some(provider.metadata.name.clone().unwrap()),
            namespace: Some(provider.metadata.namespace.clone().unwrap()),
            owner_references: Some(vec![provider.controller_owner_ref(&()).unwrap()]),
            ..Default::default()
        },
        string_data: {
            // See if the test environment is using real VPN credentials.
            if env_secret.is_none() {
                // Use mock VPN credentials.
                Some(
                    vec![
                        // These values correspond to gluetun environment variables.
                        ("VPN_NAME".to_owned(), "my-vpn-provider-name".to_owned()),
                        ("VPN_USERNAME".to_owned(), "test-username".to_owned()),
                        ("VPN_PASSWORD".to_owned(), "test-password".to_owned()),
                    ]
                    .into_iter()
                    .collect(),
                )
            } else {
                // We're using real VPN credentials, so we need to populate
                // the data field directly.
                None
            }
        },
        data: {
            if let Some(env_secret) = env_secret {
                // Inherit the data from the actual VPN secret.
                env_secret.data
            } else {
                // Use the mock data above.
                None
            }
        },
        ..Default::default()
    })
}

/// Returns the test MaskProvider resource. If we are using mock credentials,
/// verification will be disabled. Otherwise, verification will be enabled.
async fn get_test_provider(
    client: Client,
    name: &str,
    namespace: &str,
) -> Result<MaskProvider, Error> {
    Ok(MaskProvider {
        metadata: ObjectMeta {
            name: Some(name.to_owned()),
            namespace: Some(namespace.to_owned()),
            ..Default::default()
        },
        spec: MaskProviderSpec {
            // Maximum number of active connections.
            max_slots: MAX_SLOTS,
            // Same of Secret containing the env credentials.
            secret: name.to_owned(),
            // Only assign this MaskProvider to Masks in the same namespace.
            namespaces: Some(vec![namespace.to_owned()]),
            // Allow this MaskProvider to be assigned to Masks requesting this tag.
            tags: Some(vec![name.to_owned()]),
            // We currently need to skip verification for testing.
            verify: Some(MaskProviderVerifySpec {
                // Skip verification if we are using the mock credentials.
                skip: Some(get_actual_provider_secret(client).await?.is_none()),
                timeout: Some("50s".to_owned()),
                ..Default::default()
            }),
            ..Default::default()
        },
        ..Default::default()
    })
}

/// Returns a test Mask resource with the given slot as the name suffix.
fn get_test_mask(namespace: &str, slot: usize, provider_label: &str) -> Mask {
    Mask {
        metadata: ObjectMeta {
            name: Some(format!("{}-{}", MASK_NAME, slot)),
            namespace: Some(namespace.to_owned()),
            ..Default::default()
        },
        spec: MaskSpec {
            // Only use the MaskProvider created by this specific test.
            providers: Some(vec![provider_label.to_owned()]),
        },
        ..Default::default()
    }
}

/// Create the test MaskProvider's credentials Secret resource.
async fn create_test_provider_secret(
    client: Client,
    namespace: &str,
    provider: &MaskProvider,
) -> Result<Secret, Error> {
    let secret = get_test_provider_secret(client.clone(), &provider).await?;
    let secret_api: Api<Secret> = Api::namespaced(client, namespace);
    Ok(secret_api.create(&Default::default(), &secret).await?)
}

/// Creates the test MaskProvider and its secret.
async fn create_test_provider(
    client: Client,
    namespace: &str,
    uid: &str,
) -> Result<MaskProvider, Error> {
    let name = format!("{}-{}", PROVIDER_NAME, uid);
    //let provider = create_wait(
    //    client.clone(),
    //    &name,
    //    namespace,
    //    get_test_provider(client.clone(), &name, namespace).await?,
    //)
    //.await?;
    let api: Api<MaskProvider> = Api::namespaced(client.clone(), namespace);
    let provider = api
        .create(
            &Default::default(),
            &get_test_provider(client.clone(), &name, namespace).await?,
        )
        .await?;
    println!(
        "Created MaskProvider with uid {}",
        provider.metadata.uid.as_deref().unwrap()
    );
    create_test_provider_secret(client, namespace, &provider).await?;
    Ok(provider)
}

/// Creates a test Mask with the given slot as the name suffix.
async fn create_test_mask(
    client: Client,
    namespace: &str,
    slot: usize,
    provider_label: &str,
) -> Result<Mask, Error> {
    let api: Api<Mask> = Api::namespaced(client, namespace);
    Ok(api
        .create(
            &Default::default(),
            &get_test_mask(namespace, slot, provider_label),
        )
        .await?)
}

/// Waits for the test MaskProvider to observe a certain phase.
async fn wait_for_provider_phase(
    client: Client,
    namespace: &str,
    phase: MaskProviderPhase,
) -> Result<(), Error> {
    let provider_api: Api<MaskProvider> = Api::namespaced(client, namespace);
    let lp = ListParams::default().timeout(120);
    let mut stream = provider_api.watch(&lp, "0").await?.boxed();
    while let Some(event) = stream.try_next().await? {
        match event {
            WatchEvent::Added(m) | WatchEvent::Modified(m) => {
                if m.status.as_ref().map_or(false, |s| s.phase == Some(phase)) {
                    return Ok(());
                }
            }
            _ => {}
        }
    }
    // See if we missed it.
    if provider_api
        .list(&Default::default())
        .await?
        .into_iter()
        .any(|provider| {
            provider
                .status
                .as_ref()
                .map_or(false, |s| s.phase == Some(phase))
        })
    {
        return Ok(());
    }
    Err(Error::Other(format!(
        "MaskProvider not {} before timeout",
        phase
    )))
}

/// Waits for the test MaskProvider to be assigned to the test Mask.
async fn wait_for_provider_assignment(
    client: Client,
    namespace: &str,
    slot: usize,
) -> Result<AssignedProvider, Error> {
    let name = format!("{}-{}", MASK_NAME, slot);
    let mask_api: Api<Mask> = Api::namespaced(client, namespace);
    let lp = ListParams::default()
        .fields(&format!("metadata.name={}", name))
        .timeout(120);
    let mut stream = mask_api.watch(&lp, "0").await?.boxed();
    while let Some(event) = stream.try_next().await? {
        match event {
            WatchEvent::Added(m) | WatchEvent::Modified(m) => {
                match m.status.map_or(None, |s| s.provider) {
                    Some(provider) => return Ok(provider),
                    _ => continue,
                }
            }
            _ => continue,
        }
    }
    // Check if it's assigned now and we missed it.
    if let Some(provider) = mask_api
        .get(&name)
        .await?
        .status
        .map_or(None, |s| s.provider)
    {
        return Ok(provider);
    }
    Err(Error::Other(format!(
        "MaskProvider not assigned to Mask {} before timeout",
        name,
    )))
}

/// Waits for the Mask resource to observe the phase.
async fn wait_for_mask_phase(
    client: Client,
    namespace: &str,
    slot: usize,
    phase: MaskPhase,
) -> Result<(), Error> {
    let name = format!("{}-{}", MASK_NAME, slot);
    let mask_api: Api<Mask> = Api::namespaced(client, namespace);
    let lp = ListParams::default()
        .fields(&format!("metadata.name={}", &name))
        .timeout(120);
    let mut stream = mask_api.watch(&lp, "0").await?.boxed();
    while let Some(event) = stream.try_next().await? {
        match event {
            WatchEvent::Added(m) | WatchEvent::Modified(m) => {
                if m.status.as_ref().map_or(false, |s| s.phase == Some(phase)) {
                    return Ok(());
                }
            }
            _ => continue,
        }
    }
    // See if we missed it.
    if mask_api
        .get(&name)
        .await?
        .status
        .as_ref()
        .map_or(false, |s| s.phase == Some(phase))
    {
        return Ok(());
    }
    Err(Error::Other(format!(
        "{} not observed for Mask {} before timeout",
        phase, name,
    )))
}

/// Returns the test MaskProvider's credentials Secret resource.
async fn get_provider_secret(client: Client, provider: &MaskProvider) -> Result<Secret, Error> {
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
        .timeout(120);
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

/// Creates a random test namespace and returns a tuple
/// containing the test's UUID and the namespace name.
async fn create_test_namespace(client: Client) -> Result<(String, String), Error> {
    let uid = uuid::Uuid::new_v4()
        .to_string()
        .split('-')
        .next()
        .unwrap()
        .to_string();
    let name = format!("{}{}", NAMESPACE_PREFIX, uid);
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
    Ok((uid.to_owned(), name))
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
    let (uid, namespace) = create_test_namespace(client.clone()).await?;
    let provider_label = format!("{}-{}", PROVIDER_NAME, uid);

    // Create the test MaskProvider.
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

    // Garbage collect the test resources.
    cleanup(client, &namespace).await?;

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

//async fn create_wait<
//    T: Clone + Resource + CustomResourceExt + Serialize + DeserializeOwned + Debug,
//>(
//    client: Client,
//    name: &str,
//    namespace: &str,
//    resource: T,
//) -> Result<T, Error>
//where
//    <T as Resource>::DynamicType: Default,
//    T: Resource<Scope = NamespaceResourceScope>,
//{
//    let api: Api<T> = Api::namespaced(client, namespace);
//    let start = std::time::SystemTime::now();
//    let timeout = std::time::Duration::from_secs(12);
//    loop {
//        match api.create(&Default::default(), &resource).await {
//            Ok(mask) => return Ok(mask),
//            Err(kube::Error::Api(ae)) if ae.code == 409 => {
//                if start.elapsed().unwrap() > timeout {
//                    panic!(
//                        "Timed out waiting for {} {}/{} to be deleted",
//                        T::crd().metadata.name.as_deref().unwrap(),
//                        namespace,
//                        name
//                    );
//                }
//                // Try and delete it again.
//                match api.delete(name, &Default::default()).await {
//                    Ok(_) => {}
//                    Err(kube::Error::Api(ae)) if ae.code == 404 => {}
//                    Err(e) => return Err(e.into()),
//                }
//                // Sleep and try again
//                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
//            }
//            Err(e) => return Err(e.into()),
//        }
//    }
//}
