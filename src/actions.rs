use k8s_openapi::api::core::v1::{ConfigMap, Secret};
use kube::api::{DeleteParams, ObjectMeta, Patch, PatchParams, PostParams, Resource};
use kube::{Api, Client, Error, ResourceExt};
use std::collections::BTreeMap;

use crate::crd::{AssignedProvider, Mask, MaskPhase, MaskStatus, Provider};

/// Friendly name for the controller.
pub const MANAGER_NAME: &str = "vpn-operator";

/// Delete the resources associated with the Mask's reservation
/// of a Provider and nullifies the Mask's provider status object.
pub async fn unassign_provider(
    client: Client,
    name: &str,
    namespace: &str,
    instance: &Mask,
) -> Result<(), Error> {
    if instance.status.as_ref().unwrap().provider.is_none() {
        // No provider is currently assigned.
        return Ok(());
    }

    // Delete the credentials Secret.
    delete_secret(client.clone(), namespace, instance).await?;

    // Delete the reservation ConfigMap so it can be reused.
    // This doesn't always happen, in which case there will
    // be no available slots for any providers. This scenario
    // will require a pruning operator to resolve.
    delete_reservation(client.clone(), name, namespace, instance).await?;

    // Patch the Mask resource to remove the provider. We do
    // this last because the previous operations require so.
    patch_status(client.clone(), name, namespace, instance, |status| {
        status.provider = None;
        status.phase = Some(MaskPhase::Pending);
    })
    .await?;

    // Successfully unassigned the provider.
    Ok(())
}

async fn list_providers(client: Client) -> Result<Vec<Provider>, Error> {
    let api: Api<Provider> = Api::all(client);
    let providers = api.list(&Default::default()).await?;
    Ok(providers.items)
}

/// Assigns a new Provider to the Mask.
async fn assign_provider_base(
    client: Client,
    name: &str,
    namespace: &str,
    instance: &Mask,
) -> Result<bool, Error> {
    let providers = list_providers(client.clone()).await?;
    for provider in &providers {
        let provider_name = provider.name_any();
        let provider_namespace = provider.namespace().unwrap();
        let max_clients = provider.spec.max_clients;
        for id in 0..max_clients {
            let reservation_name = format!("{}-{}", &provider_name, id);

            // Try and take the slot.
            match create_reservation(
                client.clone(),
                name,
                namespace,
                provider,
                &reservation_name,
                &provider_namespace,
            )
            .await
            {
                // Slot reserved.
                Ok(_) => {}
                // Slot is already reserved.
                Err(Error::Api(e)) if e.code == 409 => continue,
                // Unknown failure reserving slot.
                Err(e) => return Err(e),
            }

            // Patch the Mask resource to add the provider.
            patch_status(client.clone(), name, namespace, instance, move |status| {
                let secret = format!("{}-{}", name, &provider_name);
                status.provider = Some(AssignedProvider {
                    name: provider_name,
                    namespace: provider_namespace,
                    id,
                    secret,
                });
                status.phase = Some(MaskPhase::Active);
            })
            .await?;

            // We will create the credentials secret next time we requeue
            // because the status object isn't updated locally.
            return Ok(true);
        }
    }

    Ok(false)
}

pub async fn assign_provider(
    client: Client,
    name: &str,
    namespace: &str,
    instance: &Mask,
) -> Result<bool, Error> {
    // Try to assign a provider.
    if assign_provider_base(client.clone(), name, namespace, instance).await? {
        return Ok(true);
    }
    // Remove any dangling reservations.
    if prune(client.clone()).await? {
        // One or more dangling reservations were removed,
        // so retrying should succeed.
        if assign_provider_base(client.clone(), name, namespace, instance).await? {
            return Ok(true);
        }
    }
    // Unable to find a Provider. Reflect the error in the status.
    patch_status(client, name, namespace, instance, |status| {
        status.phase = Some(MaskPhase::ErrNoProvidersAvailable);
    })
    .await?;
    Ok(false)
}

/// Deletes dangling reservations that are no longer owned by a Mask.
async fn prune(client: Client) -> Result<bool, Error> {
    let mut deleted = false;
    let providers = list_providers(client.clone()).await?;
    for provider in &providers {
        let name = provider.name_any();
        let namespace = provider.namespace().unwrap();
        let cm_api: Api<ConfigMap> = Api::namespaced(client.clone(), &namespace);
        let max_clients = provider.spec.max_clients;
        for id in 0..max_clients {
            let reservation_name = format!("{}-{}", &name, id);
            let cm = match cm_api.get(&reservation_name).await {
                // Reservation exists, make sure it's not dangling.
                Ok(cm) => cm,
                // Reservation doesn't exist, so it's not dangling.
                Err(kube::Error::Api(e)) if e.code == 404 => continue,
                // Error getting reservation ConfigMap.
                Err(e) => return Err(e),
            };
            let data = match cm.data {
                Some(data) => data,
                // Malformed reservation is dangling, so delete it.
                None => {
                    cm_api
                        .delete(&reservation_name, &DeleteParams::default())
                        .await?;
                    deleted = true;
                    continue;
                }
            };
            let (mask_name, mask_namespace) = match (data.get("name"), data.get("namespace")) {
                // Well-formed reservation with a reference to the owner Mask.
                (Some(name), Some(namespace)) => (name, namespace),
                // Malformed reservation is dangling, so delete it.
                _ => {
                    cm_api
                        .delete(&reservation_name, &DeleteParams::default())
                        .await?;
                    deleted = true;
                    continue;
                }
            };
            let mask_api: Api<Mask> = Api::namespaced(client.clone(), mask_namespace);
            match mask_api.get(mask_name).await {
                // Mask exists, so the reservation is not dangling.
                Ok(_) => continue,
                // Mask doesn't exist, so the reservation is dangling.
                Err(kube::Error::Api(e)) if e.code == 404 => {
                    cm_api
                        .delete(&reservation_name, &DeleteParams::default())
                        .await?;
                    deleted = true;
                    continue;
                }
                // Error getting Mask resource.
                Err(e) => return Err(e),
            }
        }
    }
    Ok(deleted)
}

/// Creates the ConfigMap reserving a connection with the provider.
pub async fn create_reservation(
    client: Client,
    name: &str,
    namespace: &str,
    provider: &Provider,
    reservation_name: &str,
    reservation_namespace: &str,
) -> Result<(), Error> {
    let cm_api: Api<ConfigMap> = Api::namespaced(client.clone(), namespace);
    let cm = ConfigMap {
        metadata: ObjectMeta {
            name: Some(reservation_name.to_owned()),
            namespace: Some(reservation_namespace.to_owned()),
            owner_references: Some(vec![provider.controller_owner_ref(&()).unwrap()]),
            ..Default::default()
        },
        data: Some({
            let mut data = BTreeMap::new();
            data.insert("name".to_owned(), name.to_owned());
            data.insert("namespace".to_owned(), namespace.to_owned());
            data
        }),
        ..Default::default()
    };
    cm_api.create(&PostParams::default(), &cm).await?;
    Ok(())
}

/// Returns the Provider's secret resource, which contains the
/// environment variables for connecting to a VPN server.
pub async fn get_provider_secret(
    client: Client,
    name: &str,
    namespace: &str,
) -> Result<Secret, Error> {
    // Get the Provider resource.
    let provider_api: Api<Provider> = Api::namespaced(client.clone(), namespace);
    let provider = provider_api.get(name).await?;
    // Get the referenced Secret.
    let secret_api: Api<Secret> = Api::namespaced(client, namespace);
    Ok(secret_api.get(&provider.spec.secret).await?)
}

/// Creates the secret for the Mask to use. It is a copy of the Provider's secret.
pub async fn create_secret(
    client: Client,
    name: &str,
    namespace: &str,
    instance: &Mask,
) -> Result<(), Error> {
    let provider = instance
        .status
        .as_ref()
        .unwrap()
        .provider
        .as_ref()
        .expect("provider is not assigned");
    let provider_secret =
        get_provider_secret(client.clone(), &provider.name, &provider.namespace).await?;
    let oref = instance.controller_owner_ref(&()).unwrap();
    let secret = Secret {
        metadata: ObjectMeta {
            name: Some(format!("{}-{}", name, &provider.name)),
            namespace: Some(namespace.to_owned()),
            // Delete the Secret when the Mask is deleted.
            owner_references: Some(vec![oref]),
            ..Default::default()
        },
        // Inherit all of the data from the Provider's secret.
        data: provider_secret.data,
        ..Default::default()
    };
    let api: Api<Secret> = Api::namespaced(client, namespace);
    api.create(&PostParams::default(), &secret).await?;
    Ok(())
}

/// Delete the credentials env Secret for the Mask.
pub async fn delete_secret(client: Client, namespace: &str, instance: &Mask) -> Result<(), Error> {
    let provider = match instance.status.as_ref().unwrap().provider {
        Some(ref provider) => provider,
        None => return Ok(()),
    };
    let api: Api<Secret> = Api::namespaced(client, namespace);
    api.delete(&provider.secret, &DeleteParams::default())
        .await?;
    Ok(())
}

/// Deletes the reservation ConfigMap for the Mask if it still belongs to it.
pub async fn delete_reservation(
    client: Client,
    name: &str,
    namespace: &str,
    instance: &Mask,
) -> Result<(), Error> {
    // Check if the ConfigMap still belongs to the Mask.
    if !owns_reservation(client.clone(), name, namespace, instance).await? {
        // It's owned by a different Mask or it doesn't exist.
        return Ok(());
    }
    // Delete the reservation ConfigMap.
    let provider = instance.status.as_ref().unwrap().provider.as_ref().unwrap();
    let api: Api<ConfigMap> = Api::namespaced(client, &provider.namespace);
    let name = format!("{}-{}", &provider.name, provider.id);
    api.delete(&name, &DeleteParams::default()).await?;
    Ok(())
}

/// Updates the Mask's phase to Active.
pub async fn set_active(
    client: Client,
    name: &str,
    namespace: &str,
    instance: &Mask,
) -> Result<(), Error> {
    patch_status(client.clone(), name, namespace, instance, |status| {
        status.phase = Some(MaskPhase::Active);
    })
    .await?;
    Ok(())
}

/// Patch the Mask's status object with the provided function.
/// The function is passed a mutable reference to the status object,
/// which is to be mutated in-place. Move closures are supported.
async fn patch_status(
    client: Client,
    name: &str,
    namespace: &str,
    instance: &Mask,
    f: impl FnOnce(&mut MaskStatus),
) -> Result<(), Error> {
    let patch = Patch::Apply({
        let mut instance: Mask = instance.clone();
        let status: &mut MaskStatus = match instance.status.as_mut() {
            Some(status) => status,
            None => {
                // Create the status object.
                instance.status = Some(MaskStatus::default());
                instance.status.as_mut().unwrap()
            }
        };
        f(status);
        let now = chrono::Utc::now().to_rfc3339();
        status.last_updated = Some(now);
        instance
    });
    let api: Api<Mask> = Api::namespaced(client, namespace);
    api.patch(name, &PatchParams::apply(MANAGER_NAME), &patch)
        .await?;
    Ok(())
}

/// Gets the ConfigMap that reserves a connection with the Provider.
/// This is mechanism used to prevent multiple Masks from using the
/// same connection.
pub async fn get_reservation(
    client: Client,
    name: &str,
    namespace: &str,
) -> Result<Option<ConfigMap>, Error> {
    let api: Api<ConfigMap> = Api::namespaced(client, namespace);
    match api.get(&name).await {
        Ok(pod) => Ok(Some(pod)),
        Err(e) => match &e {
            kube::Error::Api(ae) => match ae.code {
                // If the resource does not exist, return None
                404 => Ok(None),
                // If the resource exists but we can't access it, return an error
                _ => Err(e.into()),
            },
            _ => Err(e.into()),
        },
    }
}

/// Returns true if the Mask owns its reservation ConfigMap.
/// The reservation ConfigMap has the fields 'name' and 'namespace'
/// which correspond to the owner Mask. If the ConfigMap does not
/// exist, or if the fields do not match, this function returns false.
pub async fn owns_reservation(
    client: Client,
    name: &str,
    namespace: &str,
    instance: &Mask,
) -> Result<bool, Error> {
    let provider = match instance.status.as_ref().unwrap().provider {
        Some(ref provider) => provider,
        None => return Ok(false),
    };
    let reservation_name = format!("{}-{}", &provider.name, provider.id);
    match get_reservation(client, &reservation_name, &provider.namespace).await? {
        Some(cm) => match cm.data {
            Some(data) => {
                match (data.get("name"), data.get("namespace")) {
                    // Extract owner Mask name & namespace.
                    (Some(cm_name), Some(cm_namespace)) => {
                        Ok(cm_name == name && cm_namespace == namespace)
                    }
                    // Invalid ConfigMap, reassign.
                    _ => Ok(false),
                }
            }
            // Invalid ConfigMap. This may be because it's been created manually.
            // Reassigning will hopefully fix it, and pruning will remove it.
            None => Ok(false),
        },
        // Reservation ConfigMap does not exist anymore.
        None => Ok(false),
    }
}
