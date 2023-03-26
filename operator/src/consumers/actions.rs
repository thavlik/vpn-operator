use crate::util::{messages, patch::*, Error};
use k8s_openapi::api::core::v1::Secret;
use kube::{
    api::{ObjectMeta, Resource},
    Api, Client,
};
use std::collections::BTreeMap;
use vpn_types::*;

use crate::util::{PROVIDER_UID_LABEL, VERIFICATION_LABEL};

/// Updates the `MaskConsumer`'s phase to Pending, which indicates
/// the resource made its initial appearance to the operator.
pub async fn pending(client: Client, instance: &MaskConsumer) -> Result<(), Error> {
    patch_status(client, instance, |status| {
        status.message = Some(messages::PENDING.to_owned());
        status.phase = Some(MaskConsumerPhase::Pending);
    })
    .await?;
    Ok(())
}

/// Updates the `MaskConsumer`'s phase to Active.
pub async fn active(client: Client, instance: &MaskConsumer) -> Result<(), Error> {
    patch_status(client, instance, |status| {
        status.phase = Some(MaskConsumerPhase::Active);
        status.message = Some(messages::ACTIVE.to_owned());
    })
    .await?;
    Ok(())
}

/// Updates the `MaskConsumer`'s phase to Terminating.
pub async fn terminating(client: Client, instance: &MaskConsumer) -> Result<(), Error> {
    patch_status(client, instance, |status| {
        status.phase = Some(MaskConsumerPhase::Terminating);
        status.message = Some(messages::TERMINATING.to_owned());
    })
    .await?;
    Ok(())
}

/// Assign a MaskProvider to a MaskConsumer that is meant for verifying the service.
/// This will skip checks on the MaskProvider's status, only failing if there
/// are no empty slots available.
pub async fn assign_verify_provider(
    client: Client,
    name: &str,
    namespace: &str,
    instance: &MaskConsumer,
    provider_uid: &str,
) -> Result<bool, Error> {
    // Get the MaskProvider resource we are verifying. It must be in the same
    // namespace as the MaskConsumer and have the given uid.
    let provider_api: Api<MaskProvider> = Api::namespaced(client.clone(), namespace);
    let provider = provider_api
        .list(&Default::default())
        .await?
        .into_iter()
        .filter(|p| {
            p.metadata
                .uid
                .as_deref()
                .map_or(false, |uid| uid == provider_uid)
        })
        .next()
        .ok_or_else(|| {
            Error::UserInputError(format!(
                "MaskProvider with uid {} not found in namespace {}",
                provider_uid, namespace
            ))
        })?;
    // Only assign the MaskProvider that the MaskConsumer is meant to verify.
    if try_reserve_slot(client.clone(), name, namespace, instance, &provider).await? {
        // MaskProvider had an open slot and it was reserved.
        return Ok(true);
    }
    // See if we can prune any dangling slot reservations.
    if prune_provider(client.clone(), &provider).await? {
        // Slots were pruned so we should be able to reserve one now.
        if try_reserve_slot(client.clone(), name, namespace, instance, &provider).await? {
            return Ok(true);
        }
    }
    // Still unable to find a slot after pruning.
    patch_status(client, instance, |status| {
        status.phase = Some(MaskConsumerPhase::Waiting);
        status.message = Some(messages::WAITING.to_owned());
    })
    .await?;
    Ok(false)
}

/// Assigns a new MaskProvider to the MaskConsumer. Prunes and retries if necessary.
/// Returns true if a MaskProvider was assigned, false otherwise.
pub async fn assign_provider(
    client: Client,
    name: &str,
    namespace: &str,
    instance: &MaskConsumer,
) -> Result<bool, Error> {
    // This will be set to the MaskProvider's uid if the MaskConsumer is meant
    // for verification of the credentials. In this case, a slot will be assigned
    // regardless of the MaskProvider's phase. The only problem that may occur is
    // that all slots are already in use.
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
        // No valid MaskProviders at all. Reflect the error in the status.
        patch_status(client, instance, |status| {
            status.phase = Some(MaskConsumerPhase::ErrNoProviders);
            status.message = Some(messages::ERR_NO_PROVIDERS.to_owned());
        })
        .await?;

        // No reason to prune or retry.
        return Ok(false);
    }

    // For the first attempt, filter out the MaskProviders that have reached
    // their capacity. This way we can try not slamming the kube api server
    // with a bunch of requests that are likely to fail in the first place.
    // The status object may be stale, so if we fail the first attempt we
    // won't do this the second time.
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
        // Try a second time if we pruned or if we excluded any MaskProviders
        // during the first attempt due to possibly stale status objects.
        if assign_provider_base(client.clone(), name, namespace, instance, &new_providers).await? {
            return Ok(true);
        }
    }

    // Unable to find an empty slot with any MaskProvider.
    patch_status(client, instance, |status| {
        status.phase = Some(MaskConsumerPhase::Waiting);
        status.message = Some(messages::WAITING.to_owned());
    })
    .await?;

    // Signal to the caller that we failed to assign a MaskProvider.
    Ok(false)
}

// Attempts to reserve a slot with the MaskProvider. Returns true
// if a slot was reserved, false otherwise.
async fn try_reserve_slot(
    client: Client,
    name: &str,
    namespace: &str,
    instance: &MaskConsumer,
    provider: &MaskProvider,
) -> Result<bool, Error> {
    let owner_uid = instance.metadata.uid.as_deref().unwrap();
    let provider_name = provider.metadata.name.as_deref().unwrap();
    let provider_namespace = provider.metadata.namespace.as_deref().unwrap();
    let slots = list_inactive_slots(client.clone(), provider).await?;
    for slot in slots {
        // Try and take the slot.
        let reservation =
            match create_reservation(client.clone(), name, namespace, provider, slot, owner_uid)
                .await
            {
                // Slot was reserved successfully.
                Ok(reservation) => reservation,
                // Slot is already reserved.
                Err(kube::Error::Api(e)) if e.code == 409 => continue,
                // Unknown failure reserving slot.
                Err(e) => return Err(e.into()),
            };
        let msg = format!(
            "reserved slot {} for MaskProvider {}/{}",
            slot, provider_namespace, provider_name,
        );
        // Patch the MaskConsumer resource to assign the MaskProvider.
        let provider_uid = provider.metadata.uid.clone().unwrap();
        patch_status(client, instance, move |status| {
            let secret = format!("{}-{}", name, &provider_uid);
            status.provider = Some(AssignedProvider {
                name: provider_name.to_owned(),
                namespace: provider_namespace.to_owned(),
                uid: provider_uid,
                reservation: reservation.metadata.uid.clone().unwrap(),
                slot,
                secret,
            });
            status.message = Some(msg);
        })
        .await?;
        // Next reconciliation will create the credentials Secret,
        // after which the MaskConsumer's phase will become Active.
        return Ok(true);
    }
    // Failed to reserve a slot with the MaskProvider.
    Ok(false)
}

/// Assigns a new MaskProvider to the Mask. Returns true
/// if a MaskProvider was assigned, false otherwise.
async fn assign_provider_base(
    client: Client,
    name: &str,
    namespace: &str,
    instance: &MaskConsumer,
    providers: &Vec<MaskProvider>,
) -> Result<bool, Error> {
    for provider in providers {
        if try_reserve_slot(client.clone(), name, namespace, instance, provider).await? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Lists all MaskProvider resources, cluster-wide, that are in the Active phase.
/// An optional filter can specified, in which case only MaskProviders with a
/// matching tags will be returned.
async fn list_active_providers(
    client: Client,
    filter_tags: Option<&Vec<String>>,
    mask_namespace: &str,
) -> Result<Vec<MaskProvider>, Error> {
    let api: Api<MaskProvider> = Api::all(client);
    let mut providers: Vec<MaskProvider> = api
        .list(&Default::default())
        .await?
        .into_iter()
        .filter(|p| p.metadata.deletion_timestamp.is_none())
        .filter(|p| {
            // Filter out MaskProviders that have namespace preferences.
            // If the MaskProvider has no namespace preferences, it will
            // be made available to all namespaces.
            p.spec
                .namespaces
                .as_ref()
                .map_or(true, |ns| ns.iter().any(|n| n == mask_namespace))
        })
        .filter(|p| {
            // Ignore MaskProviders that aren't in the Ready or Active phases.
            p.status
                .as_ref()
                .map_or(None, |s| s.phase)
                .map_or(false, |p| {
                    p == MaskProviderPhase::Ready || p == MaskProviderPhase::Active
                })
        })
        .collect();
    if let Some(ref filter_tags) = filter_tags {
        // The Mask is asking for one or more specific MaskProviders.
        // Only return MaskProviders with matching tags.
        providers = providers
            .into_iter()
            .filter(|p| {
                p.spec.tags.as_ref().map_or(false, |t| {
                    t.iter().any(|v| filter_tags.iter().any(|l| l == v))
                })
            })
            .collect();
    }
    Ok(providers)
}

/// Prunes dangling slots for a given `MaskProvider`.
async fn prune_provider(client: Client, provider: &MaskProvider) -> Result<bool, Error> {
    let mut pruned = false;
    let name = provider.metadata.name.as_deref().unwrap();
    let namespace = provider.metadata.namespace.as_deref().unwrap();
    let mr_api: Api<MaskReservation> = Api::namespaced(client.clone(), namespace);
    for slot in 0..provider.spec.max_slots {
        let reservation_name = format!("{}-{}", name, slot);
        if !check_prune(client.clone(), namespace, provider, slot, &reservation_name).await? {
            continue;
        }
        mr_api
            .delete(&reservation_name, &Default::default())
            .await?;
        pruned = true;
    }
    Ok(pruned)
}

/// Deletes dangling reservations that no longer have associated MaskConsumers.
/// These shouldn't occur under normal operation as the finalizers should prevent
/// the MaskReservation resources from being deleted before their MaskConsumers.
async fn prune(client: Client) -> Result<bool, Error> {
    let mut pruned = false;
    let provider_api: Api<MaskProvider> = Api::all(client.clone());
    let providers = provider_api.list(&Default::default()).await?;
    for provider in &providers {
        if prune_provider(client.clone(), provider).await? {
            pruned = true;
        }
    }
    Ok(pruned)
}

/// Deletes the `MaskConsumer`. This should be invoked whenever the
/// referenced `MaskReservation` no longer exists in order to properly
/// garbage collect the slots for a `MaskProvider`.
pub async fn delete(client: Client, name: &str, namespace: &str) -> Result<(), Error> {
    let mr_api: Api<MaskConsumer> = Api::namespaced(client, namespace);
    mr_api.delete(name, &Default::default()).await?;
    Ok(())
}

/// Returns true if the slot needs to be garbage collected. Under normal operation
/// this function should always return false as MaskReservations should only be
/// deleted after their associated MaskConsumers.
async fn check_prune(
    client: Client,
    namespace: &str,
    provider: &MaskProvider,
    slot: usize,
    reservation_name: &str,
) -> Result<bool, Error> {
    let provider_uid = provider.metadata.uid.as_deref().unwrap();
    // Start by getting the slot's MaskReservation.
    let mr_api: Api<MaskReservation> = Api::namespaced(client.clone(), namespace);
    let reservation = match mr_api.get(&reservation_name).await {
        // Don't garbage collect slots unless they belong to the MaskProvider.
        Ok(reservation)
            if reservation
                .metadata
                .owner_references
                .as_ref()
                .map_or(false, |o| o.iter().any(|r| r.uid == provider_uid)) =>
        {
            // This MaskReservation belongs to the MaskProvider.
            reservation
        }
        // MaskReservation does not belong to the MaskProvider.
        // This could happen when the MaskProvider is deleted
        // and quickly recreated.
        Ok(_) => return Ok(false),
        // Reservation doesn't exist, so it can't be dangling.
        Err(kube::Error::Api(e)) if e.code == 404 => return Ok(false),
        // Error getting the reservation.
        Err(e) => return Err(e.into()),
    };
    // Ensure the MaskConsumer still exists and is using this MaskReservation.
    let mask_api: Api<MaskConsumer> = Api::namespaced(client, &reservation.spec.namespace);
    match mask_api.get(&reservation.spec.name).await {
        // Ensure the UID matches and the MaskConsumer is still using the reservation.
        Ok(consumer) => Ok(
            consumer.metadata.uid.as_deref() != Some(&reservation.spec.uid)
                || !consumer_uses_reservation(&consumer, provider, slot),
        ),
        // Associated MaskConsumer no longer exists. Garbage collect it.
        Err(kube::Error::Api(e)) if e.code == 404 => Ok(true),
        // Error getting MaskConsumer resource.
        Err(e) => return Err(e.into()),
    }
}

/// Returns true if the MaskConsumer resource is assigned the given MaskProvider
/// and is reserving a slot with the given ID.
fn consumer_uses_reservation(
    instance: &MaskConsumer,
    provider: &MaskProvider,
    slot: usize,
) -> bool {
    instance
        .status
        .as_ref()
        .unwrap()
        .provider
        .as_ref()
        .map_or(false, |assigned| {
            provider.metadata.name.as_deref() == Some(&assigned.name)
                && provider.metadata.namespace.as_deref() == Some(&assigned.namespace)
                && assigned.slot == slot
        })
}

/// Attempts to create a `MaskReservation` that reserves a slot with the provider.
pub async fn create_reservation(
    client: Client,
    name: &str,
    namespace: &str,
    provider: &MaskProvider,
    slot: usize,
    owner_uid: &str,
) -> Result<MaskReservation, kube::Error> {
    let mr_api: Api<MaskReservation> = Api::namespaced(client, namespace);
    let mr = MaskReservation {
        metadata: ObjectMeta {
            name: Some(format!(
                "{}-{}",
                provider.metadata.name.as_deref().unwrap(),
                slot
            )),
            namespace: provider.metadata.namespace.clone(),
            // Set the MaskProvider as the owner reference so the
            // reservation will be deleted with the MaskProvider.
            // This is important when a MaskProvider is deleted
            // and recreated quickly, as otherwise there may
            // be some dangling reservations from the previous
            // MaskProvider resource. This ensure they are all
            // no matter how quickly it is recreated.
            owner_references: Some(vec![provider.controller_owner_ref(&()).unwrap()]),
            ..Default::default()
        },
        spec: MaskReservationSpec {
            name: name.to_owned(),
            namespace: namespace.to_owned(),
            uid: owner_uid.to_owned(),
        },
        ..Default::default()
    };
    Ok(mr_api.create(&Default::default(), &mr).await?)
}

/// Returns a list of inactive slot numbers for the `MaskProvider`.
pub async fn list_inactive_slots(
    client: Client,
    provider: &MaskProvider,
) -> Result<Vec<usize>, Error> {
    let active_slots = list_active_slots(client, provider).await?;
    Ok((0..provider.spec.max_slots)
        .filter(|slot| !active_slots.contains(slot))
        .collect())
}

/// Returns a list of active slot numbers for the `MaskProvider`.
pub async fn list_active_slots(
    client: Client,
    provider: &MaskProvider,
) -> Result<Vec<usize>, Error> {
    let provider_uid = provider.metadata.uid.as_deref().unwrap();
    let mr_api: Api<MaskReservation> = Api::namespaced(
        client.clone(),
        provider.metadata.namespace.as_deref().unwrap(),
    );
    Ok(mr_api
        .list(&Default::default())
        .await?
        .into_iter()
        .map(|cm| cm.metadata)
        .filter(|meta| {
            // Filter out MaskReservations that don't belong to the MaskProvider.
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

/// Returns the MaskProvider's secret resource, which contains the
/// environment variables for connecting to a VPN server.
async fn get_provider_secret(client: Client, name: &str, namespace: &str) -> Result<Secret, Error> {
    // Get the MaskProvider resource.
    let provider_api: Api<MaskProvider> = Api::namespaced(client.clone(), namespace);
    let provider = provider_api.get(name).await?;
    // Get the referenced Secret.
    let secret_api: Api<Secret> = Api::namespaced(client, namespace);
    Ok(secret_api.get(&provider.spec.secret).await?)
}

/// Creates the secret for the Mask to use. It is a copy of the MaskProvider's secret.
pub async fn create_secret(
    client: Client,
    namespace: &str,
    instance: &MaskConsumer,
) -> Result<(), Error> {
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
        // Inherit all of the data from the MaskProvider's secret.
        data: provider_secret.data,
        ..Default::default()
    };
    let api: Api<Secret> = Api::namespaced(client, namespace);
    api.create(&Default::default(), &secret).await?;
    Ok(())
}
