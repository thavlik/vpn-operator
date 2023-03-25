use crate::util::{messages, patch::*, Error};
use kube::{
    api::{ObjectMeta, Resource},
    Api, Client,
};
use vpn_types::*;

/// Updates the `Mask`'s phase to Pending, which indicates
/// the resource made its initial appearance to the operator.
pub async fn pending(client: Client, instance: &Mask) -> Result<(), Error> {
    patch_status(client, instance, |status| {
        status.message = Some(messages::PENDING.to_owned());
        status.phase = Some(MaskPhase::Pending);
    })
    .await?;
    Ok(())
}

/// Updates the `Mask`'s phase to Waiting, which indicates
/// the `MaskConsumer` is waiting for a provider to be available.
pub async fn waiting(client: Client, instance: &Mask) -> Result<(), Error> {
    patch_status(client, instance, |status| {
        status.phase = Some(MaskPhase::Waiting);
        status.message = Some(messages::WAITING.to_owned());
    })
    .await?;
    Ok(())
}

/// Updates the Mask's phase to Active, signifying that everything
/// is fully reconciled and the VPN credentials are ready to be used.
pub async fn active(client: Client, instance: &Mask) -> Result<(), Error> {
    patch_status(client, instance, |status| {
        status.phase = Some(MaskPhase::Active);
        status.message = Some(messages::ACTIVE.to_owned());
    })
    .await?;
    Ok(())
}

/// Updates the `Mask`'s phase to ErrNoProviders, which indicates
/// that the `MaskConsumer` controller was unable to find any providers
/// when attempting to assign this `Mask` a `MaskProvider`.
pub async fn err_no_providers(client: Client, instance: &Mask) -> Result<(), Error> {
    patch_status(client, instance, |status| {
        status.phase = Some(MaskPhase::ErrNoProviders);
        status.message = Some(messages::ERR_NO_PROVIDERS.to_owned());
    })
    .await?;
    Ok(())
}

/// Creates the child MaskConsumer for the Mask, which manages provider assignment.
pub async fn create_consumer(
    client: Client,
    name: &str,
    namespace: &str,
    instance: &Mask,
) -> Result<(), Error> {
    let consumer = MaskConsumer {
        metadata: ObjectMeta {
            name: Some(name.to_owned()),
            namespace: Some(namespace.to_owned()),
            // Use an owner ref so it'll be deleted with the Mask.
            owner_references: Some(vec![instance.controller_owner_ref(&()).unwrap()]),
            // Inherit labels from the Mask.
            labels: instance.metadata.labels.clone(),
            ..Default::default()
        },
        spec: MaskConsumerSpec {
            // Use the desired providers, if specified.
            providers: instance.spec.providers.clone(),
            ..Default::default()
        },
        ..Default::default()
    };
    Api::<MaskConsumer>::namespaced(client, namespace)
        .create(&Default::default(), &consumer)
        .await?;
    Ok(())
}
