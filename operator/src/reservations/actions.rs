use crate::util::{messages, patch::*, Error};
use kube::{Api, Client};
use vpn_types::*;

/// Updates the `MaskReservation`'s phase to Pending, which indicates
/// the resource made its initial appearance to the operator.
pub async fn pending(client: Client, instance: &MaskReservation) -> Result<(), Error> {
    patch_status(client, instance, |status| {
        status.message = Some(messages::PENDING.to_owned());
        status.phase = Some(MaskReservationPhase::Pending);
    })
    .await?;
    Ok(())
}

/// Updates the `MaskReservation`'s phase to Active.
pub async fn active(client: Client, instance: &MaskReservation) -> Result<(), Error> {
    patch_status(client, instance, |status| {
        status.phase = Some(MaskReservationPhase::Active);
        status.message = Some("MaskReservation is in use by the MaskConsumer.".to_owned());
    })
    .await?;
    Ok(())
}

/// Updates the `MaskReservation`'s phase to Terminating.
pub async fn terminating(client: Client, instance: &MaskReservation) -> Result<(), Error> {
    patch_status(client, instance, |status| {
        status.phase = Some(MaskReservationPhase::Terminating);
        status.message = Some("Resource deletion is pending garbage collection.".to_owned());
    })
    .await?;
    Ok(())
}

/// Deletes the `MaskReservation`. This should be invoked whenever the
/// referenced `MaskConsumer` no longer exists in order to properly garbage
/// collect the slots for a `MaskProvider`.
pub async fn delete(client: Client, name: &str, namespace: &str) -> Result<(), Error> {
    let mr_api: Api<MaskReservation> = Api::namespaced(client, namespace);
    mr_api.delete(name, &Default::default()).await?;
    Ok(())
}

/// Deletes the [`MaskConsumer`] referenced by the given [`MaskReservation`].
/// Returns true if the [`MaskConsumer`] does not exist, false if it does exist
/// and was deleted.
pub async fn delete_consumer(client: Client, instance: &MaskReservation) -> Result<bool, Error> {
    // Retrieve the MaskConsumer referenced by this MaskReservation.
    let mc_api: Api<MaskConsumer> = Api::namespaced(client, &instance.spec.namespace);
    let _mc = match mc_api.get(&instance.spec.name).await {
        // Ensure the `MaskConsumer` has the same UID as referenced in the spec.
        Ok(mc)
            if mc
                .metadata
                .uid
                .as_deref()
                .map_or(false, |uid| instance.spec.uid == uid) =>
        {
            // The referenced MaskConsumer is still around. We will need to
            // delete it and requeue to ensure it is deleted before removing
            // the finalizer.
            mc
        }
        // There is a MaskConsumer with the referenced name, but it doesn't
        // have the same UID so it was probably deleted and recreated quickly.
        Ok(_) => return Ok(true),
        // MaskConsumer is no longer around.
        Err(kube::Error::Api(ae)) if ae.code == 404 => return Ok(true),
        // Some other error occurred.
        Err(e) => return Err(e.into()),
    };

    // Delete the `MaskConsumer`. Its deletion logic is trivial and should be
    // removed by the Kubernetes cluster as soon as its child resources are gone.
    mc_api
        .delete(&instance.spec.name, &Default::default())
        .await?;

    // Requeue to ensure the `MaskConsumer` is deleted.
    Ok(false)
}
