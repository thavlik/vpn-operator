use kube::{
    api::{Patch, Resource},
    core::NamespaceResourceScope,
    Api, Client, Error,
};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};
use std::{clone::Clone, fmt::Debug};

/// Name of the kubernetes resource finalizer field.
pub const FINALIZER_NAME: &str = "vpn.beebs.dev/finalizer";

/// Adds a finalizer record into a `T` kind of resource. If the finalizer already exists,
/// this action has no effect.
///
/// # Arguments:
/// - `client` - Kubernetes client to modify the `MaskReservation` resource with.
/// - `name` - Name of the `MaskReservation` resource to modify. Existence is not verified
/// - `namespace` - Namespace where the `MaskReservation` resource with given `name` resides.
///
/// Note: Does not check for resource's existence for simplicity.
pub async fn add<T: Clone + Resource + Serialize + DeserializeOwned + Debug>(
    client: Client,
    name: &str,
    namespace: &str,
) -> Result<T, Error>
where
    <T as Resource>::DynamicType: Default,
    T: Resource<Scope = NamespaceResourceScope>,
{
    let api: Api<T> = Api::namespaced(client, namespace);
    let finalizer: Value = json!({
        "metadata": {
            "finalizers": [FINALIZER_NAME]
        }
    });
    let patch: Patch<&Value> = Patch::Merge(&finalizer);
    Ok(api.patch(name, &Default::default(), &patch).await?)
}

/// Removes all finalizers from `T` resource. If there are no finalizers already, this
/// action has no effect.
///
/// # Arguments:
/// - `client` - Kubernetes client to modify the `MaskReservation` resource with.
/// - `name` - Name of the `MaskReservation` resource to modify. Existence is not verified
/// - `namespace` - Namespace where the `MaskReservation` resource with given `name` resides.
///
/// Note: Does not check for resource's existence for simplicity.
pub async fn delete<T: Clone + Resource + Serialize + DeserializeOwned + Debug>(
    client: Client,
    name: &str,
    namespace: &str,
) -> Result<T, Error>
where
    <T as Resource>::DynamicType: Default,
    T: Resource<Scope = NamespaceResourceScope>,
{
    let api: Api<T> = Api::namespaced(client, namespace);
    let finalizer: Value = json!({
        "metadata": {
            "finalizers": null
        }
    });
    let patch: Patch<&Value> = Patch::Merge(&finalizer);
    Ok(api.patch(name, &Default::default(), &patch).await?)
}
