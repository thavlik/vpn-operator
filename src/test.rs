use vpn_types::*;
use futures::{StreamExt, TryStreamExt};
use k8s_openapi::api::core::v1::Secret;
use kube::{
    api::{ListParams, ObjectMeta},
    client::Client,
    core::WatchEvent,
    Api, ResourceExt,
};

/// All errors possible to occur during testing.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Any error originating from the `kube-rs` crate
    #[error("Kubernetes reported error: {source}")]
    KubeError {
        #[from]
        source: kube::Error,
    },
    #[error("Generic error: {0}")]
    Generic(String),
}

fn get_test_provider(namespace: &str) -> Provider {
    Provider {
        metadata: ObjectMeta {
            name: Some("test-provider".to_string()),
            namespace: Some(namespace.to_owned()),
            ..Default::default()
        },
        spec: ProviderSpec {
            max_clients: 2,
            secret: "test-secret".to_owned(),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn get_test_mask(name: &str, namespace: &str) -> Mask {
    Mask {
        metadata: ObjectMeta {
            name: Some(name.to_owned()),
            namespace: Some(namespace.to_owned()),
            ..Default::default()
        },
        ..Default::default()
    }
}

async fn create_test_provider(client: Client, namespace: &str) -> Result<Provider, Error> {
    let provider = get_test_provider(namespace);
    let provider_api: Api<Provider> = Api::namespaced(client, namespace);
    Ok(provider_api.create(&Default::default(), &provider).await?)
}

async fn create_test_mask(client: Client, name: &str, namespace: &str) -> Result<Mask, Error> {
    let mask = get_test_mask(name, namespace);
    let mask_api: Api<Mask> = Api::namespaced(client, namespace);
    Ok(mask_api.create(&Default::default(), &mask).await?)
}

async fn wait_for_provider_assignment(
    client: Client,
    name: &str,
    namespace: &str,
) -> Result<AssignedProvider, Error> {
    let mask_api: Api<Mask> = Api::namespaced(client, namespace);
    let lp = ListParams::default()
        .fields(&format!("metadata.name={}", name))
        .timeout(20);
    let mut stream = mask_api.watch(&lp, "0").await?.boxed();
    while let Some(event) = stream.try_next().await? {
        match event {
            WatchEvent::Added(m) | WatchEvent::Modified(m) => match m.status {
                Some(ref status) if status.provider.is_some() => {
                    return Ok(status.provider.as_ref().unwrap().clone())
                }
                _ => continue,
            },
            _ => {}
        }
    }
    Err(Error::Generic(format!(
        "Provider not assigned to Mask before timeout"
    )))
}

async fn delete_provider(client: Client, provider: &Provider) -> Result<(), Error> {
    let provider_api: Api<Provider> = Api::namespaced(client, &provider.namespace().unwrap());
    provider_api
        .delete(&provider.name_any(), &Default::default())
        .await?;
    Ok(())
}

async fn delete_mask(client: Client, mask: &Mask) -> Result<(), Error> {
    let mask_api: Api<Mask> = Api::namespaced(client, &mask.namespace().unwrap());
    mask_api
        .delete(&mask.name_any(), &Default::default())
        .await?;
    Ok(())
}

async fn get_provider_secret(client: Client, provider: &Provider) -> Result<Secret, Error> {
    let secret_api: Api<Secret> = Api::namespaced(client, &provider.namespace().unwrap());
    Ok(secret_api.get(&provider.spec.secret).await?)
}

async fn get_mask_secret(client: Client, mask: &Mask) -> Result<Secret, Error> {
    let secret_api: Api<Secret> = Api::namespaced(client, &mask.namespace().unwrap());
    let provider = mask.status.as_ref().unwrap().provider.as_ref().unwrap();
    Ok(secret_api.get(&provider.secret).await?)
}

#[tokio::test]
async fn basic() -> Result<(), Error> {
    let client: Client = Client::try_default().await.unwrap();
    let namespace = "default";
    let mask_name = "test-mask";

    // Create the test Provider.
    let provider = create_test_provider(client.clone(), &namespace).await?;

    // Watch for a Provider to be assigned to the Mask.
    let assigned_provider = tokio::spawn(wait_for_provider_assignment(
        client.clone(),
        mask_name,
        namespace,
    ));

    // Create the test Mask.
    let mask = create_test_mask(client.clone(), mask_name, &namespace).await?;

    // The provider assigned should be the same as the one we created.
    let assigned_provider = assigned_provider.await.unwrap()?;
    assert_eq!(assigned_provider.name, provider.name_any());
    assert_eq!(assigned_provider.namespace, provider.namespace().unwrap());
    assert_eq!(
        assigned_provider.secret,
        format!("{}-{}", mask.name_any(), provider.name_any())
    );

    // Ensure the Mask's credentials were correctly inherited
    // from the Provider's secret. It should be an exact match.
    let provider_secret = get_provider_secret(client.clone(), &provider).await?;
    let mask_secret = get_mask_secret(client.clone(), &mask).await?;
    assert_eq!(provider_secret.data, mask_secret.data);

    // Garbage collect the test resources.
    delete_mask(client.clone(), &mask).await?;
    delete_provider(client.clone(), &provider).await?;

    Ok(())
}
