use futures::{StreamExt, TryStreamExt};
use k8s_openapi::api::core::v1::Secret;
use kube::{
    api::{ListParams, ObjectMeta, Resource},
    client::Client,
    core::WatchEvent,
    Api, ResourceExt,
};
use std::collections::BTreeMap;
use vpn_types::*;

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

fn get_test_provider(namespace: &str) -> Provider {
    Provider {
        metadata: ObjectMeta {
            name: Some("test-provider".to_string()),
            namespace: Some(namespace.to_owned()),
            ..Default::default()
        },
        spec: ProviderSpec {
            max_slots: 2,
            secret: "test-provider".to_owned(),
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

async fn create_test_provider_secret(
    client: Client,
    namespace: &str,
    provider: &Provider,
) -> Result<Secret, Error> {
    let secret = get_test_provider_secret(namespace, &provider);
    let secret_api: Api<Secret> = Api::namespaced(client, namespace);
    Ok(secret_api.create(&Default::default(), &secret).await?)
}

async fn create_test_provider(client: Client, namespace: &str) -> Result<Provider, Error> {
    let provider = get_test_provider(namespace);
    let provider_api: Api<Provider> = Api::namespaced(client.clone(), namespace);
    let provider = provider_api.create(&Default::default(), &provider).await?;
    create_test_provider_secret(client, namespace, &provider).await?;
    Ok(provider)
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
                    println!("provider assigned: {:?}", &status.provider);
                    return Ok(status.provider.clone().unwrap());
                }
                _ => continue,
            },
            _ => {}
        }
    }
    Err(Error::Other(
        "Provider not assigned to Mask before timeout".to_owned(),
    ))
}

async fn delete_provider(client: Client, name: &str, namespace: &str) -> Result<(), Error> {
    let provider_api: Api<Provider> = Api::namespaced(client, namespace);
    match provider_api.delete(name, &Default::default()).await {
        Ok(_) => Ok(()),
        Err(kube::Error::Api(ae)) if ae.code == 404 => Ok(()),
        Err(e) => Err(e.into()),
    }
}

async fn delete_mask(client: Client, name: &str, namespace: &str) -> Result<(), Error> {
    let mask_api: Api<Mask> = Api::namespaced(client, namespace);
    match mask_api.delete(name, &Default::default()).await {
        Ok(_) => Ok(()),
        Err(kube::Error::Api(ae)) if ae.code == 404 => Ok(()),
        Err(e) => Err(e.into()),
    }
}

async fn get_provider_secret(client: Client, provider: &Provider) -> Result<Secret, Error> {
    let secret_api: Api<Secret> =
        Api::namespaced(client, provider.metadata.namespace.as_deref().unwrap());
    Ok(secret_api.get(&provider.spec.secret).await?)
}

async fn wait_for_provider_secret(
    client: Client,
    name: &str,
    namespace: &str,
) -> Result<Secret, Error> {
    let provider_api: Api<Provider> = Api::namespaced(client.clone(), namespace);
    let provider = match provider_api.get(name).await {
        Ok(p) => p,
        Err(e) => return Err(e.into()),
    };
    let secret_api: Api<Secret> = Api::namespaced(client, namespace);
    let lp = ListParams::default()
        .fields(&format!("metadata.name={}", &provider.spec.secret))
        .timeout(20);
    let mut stream = secret_api.watch(&lp, "0").await?.boxed();
    while let Some(event) = stream.try_next().await? {
        match event {
            WatchEvent::Added(m) | WatchEvent::Modified(m) => return Ok(m),
            _ => {}
        }
    }
    // See if we missed update events and we can get it now.
    secret_api.get(&provider.spec.secret).await
}

async fn get_mask_secret(client: Client, name: &str, namespace: &str) -> Result<Secret, Error> {
    let mask_api: Api<Mask> = Api::namespaced(client.clone(), namespace);
    let mask: Mask = match mask_api.get(name).await {
        Ok(m) => m,
        Err(e) => return Err(e.into()),
    };
    let secret_api: Api<Secret> = Api::namespaced(client, namespace);
    let provider = mask.status.as_ref().unwrap().provider.as_ref().unwrap();
    Ok(secret_api.get(&provider.secret).await?)
}

#[tokio::test]
async fn basic() -> Result<(), Error> {
    let client: Client = Client::try_default().await.unwrap();
    let namespace = "default";
    let provider_name = "test-provider";
    let mask_name = "test-mask";

    // Starting out, mask sure the test resources don't exist.
    delete_mask(client.clone(), mask_name, namespace).await?;
    delete_provider(client.clone(), provider_name, namespace).await?;

    // Create the test Provider.
    let provider = create_test_provider(client.clone(), namespace).await?;
    let provider_uid = provider.meta().uid.as_deref().unwrap();

    // Watch for a Provider to be assigned to the Mask.
    let assigned_provider = tokio::spawn(wait_for_provider_assignment(
        client.clone(),
        mask_name,
        namespace,
    ));
    let mask_secret = tokio::spawn(wait_for_provider_secret(
        client.clone(),
        provider_name,
        namespace,
    ));

    // Create the test Mask.
    let mask = create_test_mask(client.clone(), mask_name, namespace).await?;

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
    let secret_api: Api<Secret> = Api::namespaced(client.clone(), namespace);
    let provider_secret = secret_api.get(&provider.spec.secret).await?;
    assert_eq!(provider_secret.data, mask_secret.data);

    // Garbage collect the test resources.
    delete_mask(client.clone(), mask_name, namespace).await?;
    delete_provider(client.clone(), provider_name, namespace).await?;

    // TODO: ensure the credentials Secret and reservation ConfigMap
    // were garbage collected with their owner resources.

    Ok(())
}
