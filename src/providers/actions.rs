use crate::util::Error;
use k8s_openapi::api::core::v1::Secret;
use kube::{
    api::{Api, DeleteParams, ListParams},
    Client,
};
use vpn_types::*;

/// Updates the Provider's phase to Pending, which indicates
/// the resource made its initial appearance to the operator.
pub async fn pending(client: Client, instance: &Provider) -> Result<(), Error> {
    patch_status(client, instance, |status| {
        status.message = Some("the resource first appeared to the controller".to_owned());
        status.phase = Some(ProviderPhase::Pending);
    })
    .await?;
    Ok(())
}

/// Updates the Provider's phase to Active, which indicates
/// the VPN provider is ready to use.
pub async fn active(client: Client, instance: &Provider, active_slots: usize) -> Result<(), Error> {
    patch_status(client, instance, |status| {
        status.message = Some("the VPN provider resource is ready to use".to_owned());
        status.phase = Some(ProviderPhase::Active);
        status.active_slots = Some(active_slots);
    })
    .await?;
    Ok(())
}

/// Updates the Provider's phase to ErrSecretNotFound, which indicates
/// the VPN provider is ready to use.
pub async fn secret_missing(
    client: Client,
    instance: &Provider,
    secret_name: &str,
) -> Result<(), Error> {
    patch_status(client, instance, |status| {
        status.message = Some(format!("secret '{}' does not exist", secret_name));
        status.phase = Some(ProviderPhase::ErrSecretNotFound);
    })
    .await?;
    Ok(())
}

async fn list_masks(client: Client) -> Result<Vec<Mask>, Error> {
    let api: Api<Mask> = Api::all(client);
    Ok(api
        .list(&ListParams::default())
        .await?
        .items
        .into_iter()
        .filter(|p| p.metadata.deletion_timestamp.is_none())
        .collect())
}

/// Returns true if the Mask resource is assigned this Provider.
fn mask_uses_provider(name: &str, namespace: &str, uid: &str, mask: &Mask) -> bool {
    match mask.status.as_ref().map(|status| &status.provider) {
        // Mask is assigned a Provider.
        Some(Some(provider)) => {
            // Check if the assigned Provider matches the given one.
            provider.name == name && provider.namespace == namespace && provider.uid == uid
        }
        // Mask is not assigned a Provider.
        _ => false,
    }
}

pub async fn unassign_all(
    client: Client,
    name: &str,
    namespace: &str,
    instance: &Provider,
) -> Result<(), Error> {
    // List all Mask resources.
    let uid = instance.metadata.uid.as_deref().unwrap();
    for mask in list_masks(client.clone())
        .await?
        .into_iter()
        .filter(|mask| mask_uses_provider(name, namespace, uid, &mask))
    {
        // Unassign this provider in the Mask status object.
        // Reconciliation will trigger a new assignment.
        patch_status(client.clone(), &mask, |status| {
            status.provider = None;
            status.message = Some("Provider was unassigned upon its deletion".to_owned());
            status.phase = Some(MaskPhase::Pending);
        })
        .await?;

        // Garbage collect the Secret that was created for this Mask.
        delete_mask_secret(client.clone(), &mask, instance).await?;
    }

    Ok(())
}

/// Delete the credentials env Secret for the Mask.
pub async fn delete_mask_secret(
    client: Client,
    mask: &Mask,
    provider: &Provider,
) -> Result<(), Error> {
    // Because the Secret's name is based on the uid, we don't have
    // to check its labels to make sure it belongs to the Provider.
    let secret_name = format!(
        "{}-{}",
        mask.metadata.name.as_deref().unwrap(),
        provider.metadata.uid.as_deref().unwrap(),
    );
    let api: Api<Secret> = Api::namespaced(client, mask.metadata.namespace.as_deref().unwrap());
    match api.delete(&secret_name, &DeleteParams::default()).await {
        // Secret was deleted.
        Ok(_) => Ok(()),
        // Secret does not exist. This could happen if it was
        // deleted by the Mask controller.
        Err(kube::Error::Api(e)) if e.code == 404 => Ok(()),
        // Error deleting Secret.
        Err(e) => Err(e.into()),
    }
}
