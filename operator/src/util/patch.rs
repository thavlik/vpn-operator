use super::MANAGER_NAME;
use kube::{
    api::{Patch, PatchParams, Resource},
    core::NamespaceResourceScope,
    Api, Client, Error,
};
use serde::{de::DeserializeOwned, Serialize};
use std::{clone::Clone, fmt::Debug};
use vpn_types::*;

pub trait Object<S: Status> {
    /// Returns a mutable reference to the status object, initializing
    /// it with the default value if it does not exist.
    fn mut_status(&mut self) -> &mut S;
}

pub trait Status {
    /// Sets the last updated timestamp to the given value.
    fn set_last_updated(&mut self, last_updated: String);
}

impl Object<MaskStatus> for Mask {
    fn mut_status(&mut self) -> &mut MaskStatus {
        if self.status.is_some() {
            return self.status.as_mut().unwrap();
        }
        self.status = Some(Default::default());
        self.status.as_mut().unwrap()
    }
}

impl Status for MaskStatus {
    fn set_last_updated(&mut self, last_updated: String) {
        self.last_updated = Some(last_updated);
    }
}

impl Object<MaskProviderStatus> for MaskProvider {
    fn mut_status(&mut self) -> &mut MaskProviderStatus {
        if self.status.is_some() {
            return self.status.as_mut().unwrap();
        }
        self.status = Some(Default::default());
        self.status.as_mut().unwrap()
    }
}

impl Status for MaskProviderStatus {
    fn set_last_updated(&mut self, last_updated: String) {
        self.last_updated = Some(last_updated);
    }
}

impl Object<MaskReservationStatus> for MaskReservation {
    fn mut_status(&mut self) -> &mut MaskReservationStatus {
        if self.status.is_some() {
            return self.status.as_mut().unwrap();
        }
        self.status = Some(Default::default());
        self.status.as_mut().unwrap()
    }
}

impl Status for MaskReservationStatus {
    fn set_last_updated(&mut self, last_updated: String) {
        self.last_updated = Some(last_updated);
    }
}

impl Object<MaskConsumerStatus> for MaskConsumer {
    fn mut_status(&mut self) -> &mut MaskConsumerStatus {
        if self.status.is_some() {
            return self.status.as_mut().unwrap();
        }
        self.status = Some(Default::default());
        self.status.as_mut().unwrap()
    }
}

impl Status for MaskConsumerStatus {
    fn set_last_updated(&mut self, last_updated: String) {
        self.last_updated = Some(last_updated);
    }
}

/// Patch the resource's status object with the provided function.
/// The function is passed a mutable reference to the status object,
/// which is to be mutated in-place. Move closures are supported.
pub async fn patch_status<
    S: Status,
    T: Clone + Resource + Object<S> + Serialize + DeserializeOwned + Debug,
>(
    client: Client,
    instance: &T,
    f: impl FnOnce(&mut S),
) -> Result<T, Error>
where
    <T as Resource>::DynamicType: Default,
    T: Resource<Scope = NamespaceResourceScope>,
{
    let patch = Patch::Json::<T>({
        let mut modified = instance.clone();
        let status = modified.mut_status();
        f(status);
        status.set_last_updated(chrono::Utc::now().to_rfc3339());
        json_patch::diff(
            &serde_json::to_value(instance).unwrap(),
            &serde_json::to_value(&modified).unwrap(),
        )
    });
    let name = instance.meta().name.as_deref().unwrap();
    let namespace = instance.meta().namespace.as_deref().unwrap();
    let api: Api<T> = Api::namespaced(client, namespace);
    Ok(api
        .patch_status(name, &PatchParams::apply(MANAGER_NAME), &patch)
        .await?)
}
