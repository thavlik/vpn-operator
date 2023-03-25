use futures::{StreamExt, TryStreamExt};
use k8s_openapi::api::core::v1::{Namespace, Secret};
use kube::{
    api::{ListParams, ObjectMeta, Resource},
    client::Client,
    core::{NamespaceResourceScope, WatchEvent},
    Api, CustomResourceExt,
};
use serde::{de::DeserializeOwned, Serialize};
use std::{clone::Clone, fmt::Debug};
use vpn_types::*;

/// Maximum number of slots for the real VPN provider.
pub const MAX_SLOTS: usize = 1;

/// Base name of the test MaskProvider resource. The actual name will include
/// a randomly generated UUID to distinguish it from other test providers.
pub const PROVIDER_NAME: &str = "test-provider";

/// Base name of the test Mask resource. If multiple Masks are used in a test,
/// they will be named `test-mask-0`, `test-mask-1`, etc.
pub const MASK_NAME: &str = "test-mask";

/// Prefix of all test namespaces.
pub const NAMESPACE_PREFIX: &str = "vpn-test-";

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

/// Returns the Secret resource that contains actual VPN credentials
/// when testing against external services. If the environment variables
/// SECRET_NAME or SECRET_NAMESPACE are not set, this will return None,
/// and the providers will skip verification.
pub async fn get_actual_provider_secret(client: Client) -> Result<Option<Secret>, Error> {
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
pub async fn get_test_provider_secret(
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
pub async fn get_test_provider(
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
pub fn get_test_mask(namespace: &str, slot: usize, provider_label: &str) -> Mask {
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
pub async fn create_test_provider_secret(
    client: Client,
    namespace: &str,
    provider: &MaskProvider,
) -> Result<Secret, Error> {
    let secret = get_test_provider_secret(client.clone(), &provider).await?;
    let secret_api: Api<Secret> = Api::namespaced(client, namespace);
    Ok(secret_api.create(&Default::default(), &secret).await?)
}

/// Creates the test MaskProvider and its secret.
pub async fn create_test_provider(
    client: Client,
    namespace: &str,
    uid: &str,
) -> Result<MaskProvider, Error> {
    let name = format!("{}-{}", PROVIDER_NAME, uid);
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
pub async fn create_test_mask(
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
pub async fn wait_for_provider_phase(
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
pub async fn wait_for_provider_assignment(
    client: Client,
    namespace: &str,
    slot: usize,
) -> Result<AssignedProvider, Error> {
    let name = format!("{}-{}", MASK_NAME, slot);
    let mc_api: Api<MaskConsumer> = Api::namespaced(client, namespace);
    let lp = ListParams::default()
        .fields(&format!("metadata.name={}", name))
        .timeout(120);
    let mut stream = mc_api.watch(&lp, "0").await?.boxed();
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
    if let Some(provider) = mc_api.get(&name).await?.status.map_or(None, |s| s.provider) {
        return Ok(provider);
    }
    Err(Error::Other(format!(
        "MaskProvider not assigned to MaskConsumer {} before timeout",
        name,
    )))
}

/// Waits for the Mask resource to observe the phase.
pub async fn wait_for_mask_phase(
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
pub async fn get_provider_secret(client: Client, provider: &MaskProvider) -> Result<Secret, Error> {
    let secret_api: Api<Secret> =
        Api::namespaced(client, provider.metadata.namespace.as_deref().unwrap());
    Ok(secret_api.get(&provider.spec.secret).await?)
}

/// Waits for a Secret resource to appear.
pub async fn wait_for_secret(
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
pub async fn create_test_namespace(client: Client) -> Result<(String, String), Error> {
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

/// Deletes the namespace with the given name. Used to delete test namespaces.
pub async fn delete_namespace(client: Client, name: &str) -> Result<(), Error> {
    let namespace_api: Api<Namespace> = Api::all(client);
    namespace_api.delete(name, &Default::default()).await?;
    Ok(())
}

/// Deletes all of the test resources.
pub async fn cleanup(client: Client, namespace: &str) -> Result<(), Error> {
    delete_namespace(client, namespace).await
}

/// Deletes the test MaskProvider.
pub async fn delete_test_provider(
    client: Client,
    namespace: &str,
    name: &str,
) -> Result<(), Error> {
    assert!(delete_wait::<MaskProvider>(client.clone(), name, namespace).await?);
    Ok(())
}

/// Deletes the test Mask at the given slot.
pub async fn delete_test_mask(client: Client, namespace: &str, slot: usize) -> Result<(), Error> {
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
pub async fn delete_wait<
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
