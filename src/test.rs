use futures::{StreamExt, TryStreamExt};
use k8s_openapi::api::core::v1::Secret;
use kube::{
    api::{ListParams, ObjectMeta, Resource},
    client::Client,
    core::WatchEvent,
    Api, ResourceExt,
};
use vpn_types::*;

const MAX_SLOTS: usize = 2;
const NAMESPACE: &str = "default";
const PROVIDER_NAME: &str = "test-provider";
const MASK_NAME: &str = "test-mask";

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
fn get_test_provider_secret(namespace: &str, provider: &Provider) -> Secret {
    Secret {
        metadata: ObjectMeta {
            name: Some("test-provider".to_string()),
            namespace: Some(namespace.to_owned()),
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
            name: Some("test-provider".to_string()),
            namespace: Some(namespace.to_owned()),
            ..Default::default()
        },
        spec: ProviderSpec {
            max_slots: MAX_SLOTS,
            secret: "test-provider".to_owned(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Returns a test Mask resource with the given slot as the name suffix.
fn get_test_mask(name: &str, namespace: &str, slot: usize) -> Mask {
    Mask {
        metadata: ObjectMeta {
            name: Some(format!("{}-{}", name, slot)),
            namespace: Some(namespace.to_owned()),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Create the test Provider's credentials Secret resource.
async fn create_test_provider_secret(
    client: Client,
    namespace: &str,
    provider: &Provider,
) -> Result<Secret, Error> {
    let secret = get_test_provider_secret(namespace, &provider);
    let secret_api: Api<Secret> = Api::namespaced(client, namespace);
    Ok(secret_api.create(&Default::default(), &secret).await?)
}

/// Creates the test Provider and its secret.
async fn create_test_provider(client: Client, namespace: &str) -> Result<Provider, Error> {
    let provider = get_test_provider(namespace);
    let provider_api: Api<Provider> = Api::namespaced(client.clone(), namespace);
    let provider = provider_api.create(&Default::default(), &provider).await?;
    create_test_provider_secret(client, namespace, &provider).await?;
    Ok(provider)
}

/// Creates a test Mask with the given slot as the name suffix.
async fn create_test_mask(client: Client, name: &str, namespace: &str, slot: usize) -> Result<Mask, Error> {
    let mask = get_test_mask(name, namespace, slot);
    let mask_api: Api<Mask> = Api::namespaced(client, namespace);
    Ok(mask_api.create(&Default::default(), &mask).await?)
}

/// Waits for the test Provider to be assigned to the test Mask.
async fn wait_for_provider_assignment(
    client: Client,
    name: &str,
    namespace: &str,
    slot: usize,
) -> Result<AssignedProvider, Error> {
    let name = format!("{}-{}", name, slot);
    let mask_api: Api<Mask> = Api::namespaced(client, namespace);
    let lp = ListParams::default()
        .fields(&format!("metadata.name={}", name))
        .timeout(20);
    let mut stream = mask_api.watch(&lp, "0").await?.boxed();
    while let Some(event) = stream.try_next().await? {
        match event {
            WatchEvent::Added(m) | WatchEvent::Modified(m) => match m.status {
                Some(ref status) if status.provider.is_some() => {
                    println!("provider assigned: {:?}", &status.provider);
                    return Ok(status.provider.clone().unwrap());
                }
                _ => continue,
            },
            _ => {}
        }
    }
    Err(Error::Other(
        format!(
            "Provider not assigned to Mask {} before timeout",
            name,
        ),
    ))
}

/// Deletes the test Provider.
async fn delete_provider(client: Client, name: &str, namespace: &str) -> Result<(), Error> {
    let provider_api: Api<Provider> = Api::namespaced(client, namespace);
    match provider_api.delete(name, &Default::default()).await {
        Ok(_) => Ok(()),
        Err(kube::Error::Api(ae)) if ae.code == 404 => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Deletes the test Mask.
async fn delete_mask(client: Client, name: &str, namespace: &str, slot: usize) -> Result<(), Error> {
    let name = format!("{}-{}", name, slot);
    let mask_api: Api<Mask> = Api::namespaced(client, namespace);
    match mask_api.delete(&name, &Default::default()).await {
        Ok(_) => Ok(()),
        Err(kube::Error::Api(ae)) if ae.code == 404 => Ok(()),
        Err(e) => Err(e.into()),
    }
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
            _ => {}
        }
    }
    // See if we missed update events and can get it now.
    Ok(secret_api.get(&secret_name).await?)
}

/// Deletes all of the test resources.
async fn cleanup(client: Client) -> Result<(), Error> {
    let result = tokio::join!(
        delete_mask(client.clone(), MASK_NAME, NAMESPACE, 0),
        delete_mask(client.clone(), MASK_NAME, NAMESPACE, 1),
        delete_mask(client.clone(), MASK_NAME, NAMESPACE, 2),
        delete_provider(client, PROVIDER_NAME, NAMESPACE),
    );
    result.0?;
    result.1?;
    result.2?;
    result.3?;
    Ok(())
}

#[tokio::test]
async fn basic() -> Result<(), Error> {
    let client: Client = Client::try_default().await.unwrap();

    // Starting out, mask sure the test resources don't exist.
    cleanup(client.clone()).await?;

    // Create the test Provider.
    let provider = create_test_provider(client.clone(), NAMESPACE).await?;
    let provider_uid = provider.meta().uid.as_deref().unwrap();

    // Watch for a Provider to be assigned to the Mask.
    let assigned_provider = tokio::spawn(wait_for_provider_assignment(
        client.clone(),
        MASK_NAME,
        NAMESPACE,
        0,
    ));
    let mask_secret_name = format!("{}-{}-{}", MASK_NAME, 0, provider_uid);
    let mask_secret = tokio::spawn(wait_for_secret(
        client.clone(),
        mask_secret_name.clone(),
        NAMESPACE,
    ));

    // Create the test Mask.
    let mask = create_test_mask(client.clone(), MASK_NAME, NAMESPACE, 0).await?;

    // The provider assigned should be the same as the one we created.
    let assigned_provider = assigned_provider.await.unwrap()?;
    assert_eq!(assigned_provider.name, provider.name_any());
    assert_eq!(assigned_provider.namespace, provider.namespace().unwrap());
    assert_eq!(
        &assigned_provider.uid,
        provider.metadata.uid.as_deref().unwrap()
    );
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
    cleanup(client.clone()).await?;

    // TODO: ensure the credentials Secret and reservation ConfigMap
    // were garbage collected with their owner resources.

    Ok(())
}
