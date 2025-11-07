use kube::{client::Client, Api};
use vpn_types::*;

use crate::util::Error;

/// Returns the `MaskConsumer` resource that is managing provider assignment for the `Mask`.
pub async fn get_consumer(client: Client, instance: &Mask) -> Result<Option<MaskConsumer>, Error> {
    let mask_name = instance.metadata.name.as_deref().unwrap();
    let mask_namespace = instance.metadata.namespace.as_deref().unwrap();
    let mask_uid = instance.metadata.uid.as_deref().unwrap();
    let mc_api: Api<MaskConsumer> = Api::namespaced(client, mask_namespace);
    Ok(match mc_api.get(mask_name).await {
        // Ensure the MaskConsumer has an owner reference to the Mask.
        Ok(mc)
            if mc
                .metadata
                .owner_references
                .as_ref()
                .map_or(false, |o| o.iter().any(|r| r.uid == mask_uid)) =>
        {
            // The MaskConsumer exists and the owner UID matches.
            Some(mc)
        }
        // Owner ref doesn't match. This could happen if the MaskConsumer is
        // deleted and then quickly recreated. Everything should eventually
        // become consistent, so just return None for now.
        Ok(_) => None,
        // MaskConsumer doesn't exist yet.
        Err(kube::Error::Api(ae)) if ae.code == 404 => None,
        // Some other error occurred.
        Err(e) => return Err(e.into()),
    })
}
