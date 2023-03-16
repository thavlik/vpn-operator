use crate::util::{patch::*, Error, PROVIDER_NAME_LABEL, PROVIDER_UID_LABEL, VERIFICATION_LABEL};
use k8s_openapi::api::core::v1::{ConfigMap, Secret};
use kube::{
    api::{DeleteParams, ObjectMeta, PostParams, Resource},
    Api, Client,
};
use std::collections::BTreeMap;
use vpn_types::*;

/// Updates the Provider's phase to Pending, which indicates
/// the resource made its initial appearance to the operator.
pub async fn pending(client: Client, instance: &Mask) -> Result<(), Error> {
    patch_status(client, instance, |status| {
        status.message = Some("Resource first appeared to the controller.".to_owned());
        status.phase = Some(MaskPhase::Pending);
    })
    .await?;
    Ok(())
}

/// Delete the resources associated with the Mask's reservation
/// of a Provider and nullifies the Mask's provider status object.
pub async fn unassign_provider(
    client: Client,
    name: &str,
    namespace: &str,
    instance: &Mask,
) -> Result<(), Error> {
    if instance
        .status
        .as_ref()
        .map_or(true, |s| s.provider.is_none())
    {
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
    patch_status(client, instance, |status| {
        status.provider = None;
        status.phase = Some(MaskPhase::Pending);
        status.message = Some("Provider was unassigned.".to_owned());
    })
    .await?;

    Ok(())
}

/// Lists all Provider resources, cluster-wide, that are in the Active phase.
/// An optional filter can specified, in which case only Providers with a
/// matching vpn.beebs.dev/provider label will be returned.
async fn list_active_providers(
    client: Client,
    filter_labels: Option<&Vec<String>>,
    mask_namespace: &str,
) -> Result<Vec<Provider>, Error> {
    let api: Api<Provider> = Api::all(client);
    let mut providers: Vec<Provider> = api
        .list(&Default::default())
        .await?
        .into_iter()
        .filter(|p| p.metadata.deletion_timestamp.is_none())
        .filter(|p| {
            // Filter out Providers that have namespace preferences.
            // If the Provider has no namespace preferences, it is
            // assumed to be available in all namespaces.
            p.spec
                .namespaces
                .as_ref()
                .map_or(true, |ns| ns.iter().any(|n| n == mask_namespace))
        })
        .filter(|p| {
            // Ignore Providers that aren't in the Ready or Active phases.
            p.status
                .as_ref()
                .map_or(None, |s| s.phase)
                .map_or(false, |p| {
                    p == ProviderPhase::Ready || p == ProviderPhase::Active
                })
        })
        .collect();
    if let Some(ref filter_labels) = filter_labels {
        // The Mask is asking for a specific Provider.
        // Only return Providers with a matching label.
        providers = providers
            .into_iter()
            .filter(|p| {
                p.metadata.labels.as_ref().map_or(false, |l| {
                    l.get(PROVIDER_NAME_LABEL).as_ref().map_or(false, |v| {
                        v.split(",").any(|p| filter_labels.iter().any(|l| l == p))
                    })
                })
            })
            .collect();
    }
    Ok(providers)
}

// Attempts to reserve a slot with the Provider. Returns true
// if a slot was reserved, false otherwise.
async fn try_reserve_slot(
    client: Client,
    name: &str,
    namespace: &str,
    instance: &Mask,
    provider: &Provider,
) -> Result<bool, Error> {
    let owner_uid = instance.metadata.uid.as_deref().unwrap();
    let provider_name = provider.metadata.name.as_deref().unwrap();
    let provider_namespace = provider.metadata.namespace.as_deref().unwrap();
    let slots = list_inactive_slots(client.clone(), provider).await?;
    for slot in slots {
        // Try and take the slot.
        match create_reservation(
            client.clone(),
            name,
            namespace,
            provider,
            slot,
            owner_uid.to_owned(),
        )
        .await
        {
            // Slot was reserved successfully.
            Ok(_) => {}
            // Slot is already reserved.
            Err(kube::Error::Api(e)) if e.code == 409 => continue,
            // Unknown failure reserving slot.
            Err(e) => return Err(e.into()),
        }
        let msg = format!(
            "reserved slot {} for Provider {}/{}",
            slot, provider_namespace, provider_name,
        );
        println!("Mask {}/{} {}", namespace, name, msg);
        // Patch the Mask resource to assign the Provider.
        let provider_uid = provider.metadata.uid.clone().unwrap();
        patch_status(client, instance, move |status| {
            let secret = format!("{}-{}", name, &provider_uid);
            status.provider = Some(AssignedProvider {
                name: provider_name.to_owned(),
                namespace: provider_namespace.to_owned(),
                uid: provider_uid,
                slot,
                secret,
            });
            status.message = Some(msg);
        })
        .await?;
        // Next reconciliation will create the credentials Secret,
        // after which the Mask's phase will be updated to Active.
        return Ok(true);
    }
    // Failed to reserve a slot with the Provider.
    Ok(false)
}

/// Assigns a new Provider to the Mask. Returns true
/// if a Provider was assigned, false otherwise.
async fn assign_provider_base(
    client: Client,
    name: &str,
    namespace: &str,
    instance: &Mask,
    providers: &Vec<Provider>,
) -> Result<bool, Error> {
    for provider in providers {
        if try_reserve_slot(client.clone(), name, namespace, instance, provider).await? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Assign a Provider to a Mask that is meant for verifying the service.
/// This will skip checks on the Provider's status, only failing if there
/// are no empty slots available.
pub async fn assign_verify_provider(
    client: Client,
    name: &str,
    namespace: &str,
    instance: &Mask,
    provider_uid: &str,
) -> Result<bool, Error> {
    // Get the Provider resource we are verifying.
    let provider_api: Api<Provider> = Api::namespaced(client.clone(), namespace);
    let provider = provider_api
        .list(&Default::default())
        .await?
        .into_iter()
        .filter(|p| p.metadata.uid.as_deref() == Some(provider_uid))
        .next()
        .ok_or_else(|| {
            Error::UserInputError(format!(
                "Provider with uid {} not found in namespace {}",
                provider_uid, namespace
            ))
        })?;
    // Only assign the Provider that the Mask is meant to verify.
    if try_reserve_slot(client.clone(), name, namespace, instance, &provider).await? {
        // Provider had an open slot and it was reserved.
        return Ok(true);
    }
    // Prune and try again.
    if prune_provider(client.clone(), &provider).await? {
        if try_reserve_slot(client.clone(), name, namespace, instance, &provider).await? {
            return Ok(true);
        }
    }
    patch_status(client, instance, |status| {
        status.phase = Some(MaskPhase::Waiting);
        status.message = Some("Waiting on a slot from the Provider.".to_owned());
    })
    .await?;
    return Ok(false);
}

/// Assigns a new Provider to the Mask. Prunes and retries if necessary.
/// Returns true if a Provider was assigned, false otherwise.
pub async fn assign_provider(
    client: Client,
    name: &str,
    namespace: &str,
    instance: &Mask,
) -> Result<bool, Error> {
    // This will be set to the Provider's uid if the Mask is meant
    // for verification of the credentials. In this case, a slot will
    // be assigned regardless of the Provider's phase. The only problem
    // that may occur is that all slots are already in use.
    if let Some(provider_uid) = instance
        .metadata
        .labels
        .as_ref()
        .map_or(None, |l| l.get(VERIFICATION_LABEL).map(|v| v.as_str()))
    {
        return assign_verify_provider(client, name, namespace, instance, provider_uid).await;
    }

    // See if there are any providers available.
    let providers =
        list_active_providers(client.clone(), instance.spec.providers.as_ref(), namespace).await?;
    if providers.is_empty() {
        // No valid Providers at all. Reflect the error in the status.
        patch_status(client, instance, |status| {
            status.phase = Some(MaskPhase::ErrNoProviders);
            status.message = Some("No VPN providers available.".to_owned());
        })
        .await?;

        // No reason to prune or retry.
        return Ok(false);
    }

    // For the first attempt, filter out the Providers that have reached
    // their capacity. This way we can try not slamming the kube api server
    // with a bunch of requests that are likely to fail in the first place.
    let providers = providers
        .into_iter()
        .filter(|p| {
            p.status.as_ref().map_or(true, |s| {
                s.active_slots.map_or(true, |a| a < p.spec.max_slots)
            })
        })
        .collect();

    // Try to assign a provider for the first time.
    if assign_provider_base(client.clone(), name, namespace, instance, &providers).await? {
        return Ok(true);
    }

    // Remove dangling reservations and try again.
    let pruned = prune(client.clone()).await?;
    let new_providers =
        list_active_providers(client.clone(), instance.spec.providers.as_ref(), namespace).await?;
    if pruned || providers.len() != new_providers.len() {
        // Try a second time if we pruned or if we excluded any Providers
        // during the first attempt due to a potentially stale status object.
        if assign_provider_base(client.clone(), name, namespace, instance, &new_providers).await? {
            return Ok(true);
        }
    }

    // Unable to find an empty slot with any Provider.
    patch_status(client, instance, |status| {
        status.phase = Some(MaskPhase::Waiting);
        status.message = Some("Waiting on a slot from a Provider.".to_owned());
    })
    .await?;

    // Signal to the caller that we failed to assign a Provider.
    Ok(false)
}

/// Returns true if the reservation needs to be garbage collected.
async fn check_prune(
    client: Client,
    namespace: &str,
    provider: &Provider,
    slot: usize,
    reservation_name: &str,
) -> Result<bool, Error> {
    // Start by getting the reservation ConfigMap.
    let cm_api: Api<ConfigMap> = Api::namespaced(client.clone(), namespace);
    let data = match cm_api.get(&reservation_name).await {
        // Reservation exists, make sure it's not dangling.
        Ok(cm) => match cm.data {
            Some(data) => data,
            // Malformed reservation is dangling, so delete it.
            None => return Ok(true),
        },
        // Reservation doesn't exist, so it can't be dangling.
        Err(kube::Error::Api(e)) if e.code == 404 => return Ok(false),
        // Error getting reservation ConfigMap.
        Err(e) => return Err(e.into()),
    };

    // Extract the Mask owner properties from the ConfigMap.
    let (owner_name, owner_namespace, owner_uid) =
        match (data.get("name"), data.get("namespace"), data.get("uid")) {
            // Well-formed reservation with a reference to the owner Mask.
            (Some(name), Some(namespace), Some(uid)) => (name, namespace, uid),
            // Malformed reservation is dangling, so delete it.
            _ => return Ok(true),
        };

    // Ensure the Mask still exists and is using the reservation.
    let mask_api: Api<Mask> = Api::namespaced(client, owner_namespace);
    match mask_api.get(owner_name).await {
        // Ensure the UID matches and the Mask is still using the reservation.
        Ok(mask) => Ok(mask.metadata.uid.as_ref().unwrap() != owner_uid
            || !mask_uses_reservation(&mask, provider, slot)),
        // Owner Mask no longer exists. Garbage collect it.
        Err(kube::Error::Api(e)) if e.code == 404 => Ok(true),
        // Error getting Mask resource.
        Err(e) => return Err(e.into()),
    }
}

/// Prunes dangling slots for a given Provider.
async fn prune_provider(client: Client, provider: &Provider) -> Result<bool, Error> {
    let mut deleted = false;
    let name = provider.metadata.name.as_deref().unwrap();
    let namespace = provider.metadata.namespace.as_deref().unwrap();
    let cm_api: Api<ConfigMap> = Api::namespaced(client.clone(), namespace);
    for slot in 0..provider.spec.max_slots {
        let reservation_name = format!("{}-{}", name, slot);
        if !check_prune(client.clone(), namespace, provider, slot, &reservation_name).await? {
            continue;
        }
        cm_api
            .delete(&reservation_name, &DeleteParams::default())
            .await?;
        deleted = true;
    }
    Ok(deleted)
}

/// Deletes dangling reservations that are no longer owned by a Mask.
async fn prune(client: Client) -> Result<bool, Error> {
    let mut deleted = false;
    let provider_api: Api<Provider> = Api::all(client.clone());
    let providers = provider_api.list(&Default::default()).await?;
    for provider in &providers {
        if prune_provider(client.clone(), provider).await? {
            deleted = true;
        }
    }
    Ok(deleted)
}

/// Returns true if the Mask resource is assigned the given Provider
/// and is reserving a slot with the given ID.
fn mask_uses_reservation(instance: &Mask, provider: &Provider, slot: usize) -> bool {
    match instance.status.as_ref().unwrap().provider {
        None => false,
        Some(ref assigned) => {
            provider.metadata.name.as_deref() == Some(&assigned.name)
                && provider.metadata.namespace.as_deref() == Some(&assigned.namespace)
                && assigned.slot == slot
        }
    }
}

/// Returns a list of inactive slot numbers for the Provider.
pub async fn list_inactive_slots(client: Client, provider: &Provider) -> Result<Vec<usize>, Error> {
    let active_slots = list_active_slots(client, provider).await?;
    Ok((0..provider.spec.max_slots)
        .filter(|slot| !active_slots.contains(slot))
        .collect())
}

/// Returns a list of active slot numbers for the Provider.
pub async fn list_active_slots(client: Client, provider: &Provider) -> Result<Vec<usize>, Error> {
    let provider_uid = provider.metadata.uid.as_deref().unwrap();
    let cm_api: Api<ConfigMap> = Api::namespaced(
        client.clone(),
        provider.metadata.namespace.as_deref().unwrap(),
    );
    Ok(cm_api
        .list(&Default::default())
        .await?
        .into_iter()
        .map(|cm| cm.metadata)
        .filter(|meta| {
            // Filter out any ConfigMaps that don't belong to the Provider.
            meta.owner_references
                .as_ref()
                .map_or(false, |orefs| orefs.iter().any(|o| o.uid == provider_uid))
        })
        .filter_map(|meta| {
            // Extract the slot numbers and ignore any that are malformed.
            meta.name
                .as_ref()
                .unwrap()
                .split('-')
                .last()
                .map(|slot| slot.parse::<usize>().ok())
                .flatten()
        })
        .collect())
}

/// Creates the ConfigMap reserving a slot with the provider.
pub async fn create_reservation(
    client: Client,
    name: &str,
    namespace: &str,
    provider: &Provider,
    slot: usize,
    owner_uid: String,
) -> Result<(), kube::Error> {
    let cm_api: Api<ConfigMap> = Api::namespaced(client.clone(), namespace);
    let cm = ConfigMap {
        metadata: ObjectMeta {
            name: Some(format!(
                "{}-{}",
                provider.metadata.name.as_deref().unwrap(),
                slot
            )),
            namespace: provider.metadata.namespace.clone(),
            // Set the Provider as the owner reference so the
            // ConfigMap will be deleted with the Provider.
            // This is important when a Provider is deleted
            // and recreated quickly, as otherwise there may
            // be some dangling reservations from the previous
            // Provider resource. This ensure they are all
            // no matter how quickly it is recreated.
            owner_references: Some(vec![provider.controller_owner_ref(&()).unwrap()]),
            ..Default::default()
        },
        data: Some({
            let mut data = BTreeMap::new();
            data.insert("name".to_owned(), name.to_owned());
            data.insert("namespace".to_owned(), namespace.to_owned());
            data.insert("uid".to_owned(), owner_uid);
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
pub async fn create_secret(client: Client, namespace: &str, instance: &Mask) -> Result<(), Error> {
    let provider = instance.status.as_ref().unwrap().provider.as_ref().unwrap();
    let provider_secret =
        get_provider_secret(client.clone(), &provider.name, &provider.namespace).await?;
    let oref = instance.controller_owner_ref(&()).unwrap();
    let secret = Secret {
        metadata: ObjectMeta {
            name: Some(provider.secret.clone()),
            namespace: Some(namespace.to_owned()),
            // Delete the Secret when the Mask is deleted.
            owner_references: Some(vec![oref]),
            labels: Some({
                let mut labels = BTreeMap::new();
                labels.insert(PROVIDER_UID_LABEL.to_owned(), provider.uid.clone());
                labels
            }),
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
    match api.delete(&provider.secret, &DeleteParams::default()).await {
        // Secret was deleted.
        Ok(_) => Ok(()),
        // Secret does not exist.
        Err(kube::Error::Api(e)) if e.code == 404 => Ok(()),
        // Error deleting Secret.
        Err(e) => Err(e.into()),
    }
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
    let name = format!("{}-{}", &provider.name, provider.slot);
    match api.delete(&name, &DeleteParams::default()).await {
        // Reservation was deleted.
        Ok(_) => Ok(()),
        // Reservation does not exist; could have been deleted asynchronously.
        Err(kube::Error::Api(e)) if e.code == 404 => Ok(()),
        // Error deleting reservation.
        Err(e) => Err(e.into()),
    }
}

/// Updates the Mask's phase to Ready.
pub async fn ready(client: Client, instance: &Mask) -> Result<(), Error> {
    patch_status(client, instance, |status| {
        status.phase = Some(MaskPhase::Ready);
        status.message = Some("Mask is ready to use.".to_owned())
    })
    .await?;
    Ok(())
}

/// Updates the Mask's phase to Active.
pub async fn active(client: Client, instance: &Mask, pod_name: &str) -> Result<(), Error> {
    patch_status(client, instance, |status| {
        status.phase = Some(MaskPhase::Active);
        status.message = Some(format!("Mask is in use by Pod {}", pod_name));
    })
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
    match api.get(name).await {
        Ok(pod) => Ok(Some(pod)),
        Err(kube::Error::Api(ae)) if ae.code == 404 => Ok(None),
        Err(e) => Err(e.into()),
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
    let reservation_name = format!("{}-{}", &provider.name, provider.slot);
    let mask_uid = instance.metadata.uid.as_deref().unwrap();
    let provider_uid = &provider.uid;
    match get_reservation(client, &reservation_name, &provider.namespace).await? {
        Some(cm) => match cm.data {
            Some(data) => {
                // Make sure the reservation ConfigMap is owned by assigned provider uid.
                if !cm
                    .metadata
                    .owner_references
                    .as_ref()
                    .map_or(false, |ors| ors.iter().any(|or| &or.uid == provider_uid))
                {
                    // Reservation ConfigMap is not owned by the assigned provider.
                    return Ok(false);
                }
                // Check if the reservation ConfigMap is owned by the Mask.
                match (data.get("name"), data.get("namespace"), data.get("uid")) {
                    // Extract owner Mask reference.
                    (Some(cm_name), Some(cm_namespace), Some(cm_uid)) => {
                        // The reservation ConfigMap is owned by the Mask
                        // if all of these values match.
                        Ok(cm_name == name && cm_namespace == namespace && cm_uid == mask_uid)
                    }
                    // Invalid ConfigMap.
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
