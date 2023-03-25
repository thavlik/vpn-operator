use crate::util::FINALIZER_NAME;
use kube::api::{Patch, PatchParams};
use kube::{Api, Client, Error};
use serde_json::{json, Value};
use vpn_types::*;

/// Adds a finalizer record into an `MaskConsumer` kind of resource. If the finalizer already exists,
/// this action has no effect.
///
/// # Arguments:
/// - `client` - Kubernetes client to modify the `MaskConsumer` resource with.
/// - `name` - Name of the `MaskConsumer` resource to modify. Existence is not verified
/// - `namespace` - Namespace where the `MaskConsumer` resource with given `name` resides.
///
/// Note: Does not check for resource's existence for simplicity.
pub async fn add(client: Client, name: &str, namespace: &str) -> Result<MaskConsumer, Error> {
    let api: Api<MaskConsumer> = Api::namespaced(client, namespace);
    let finalizer: Value = json!({
        "metadata": {
            "finalizers": [FINALIZER_NAME]
        }
    });

    let patch: Patch<&Value> = Patch::Merge(&finalizer);
    Ok(api.patch(name, &PatchParams::default(), &patch).await?)
}

/// Removes all finalizers from an `MaskConsumer` resource. If there are no finalizers already, this
/// action has no effect.
///
/// # Arguments:
/// - `client` - Kubernetes client to modify the `MaskConsumer` resource with.
/// - `name` - Name of the `MaskConsumer` resource to modify. Existence is not verified
/// - `namespace` - Namespace where the `MaskConsumer` resource with given `name` resides.
///
/// Note: Does not check for resource's existence for simplicity.
pub async fn delete(client: Client, name: &str, namespace: &str) -> Result<MaskConsumer, Error> {
    let api: Api<MaskConsumer> = Api::namespaced(client, namespace);
    let finalizer: Value = json!({
        "metadata": {
            "finalizers": null
        }
    });

    let patch: Patch<&Value> = Patch::Merge(&finalizer);
    Ok(api.patch(name, &PatchParams::default(), &patch).await?)
}
