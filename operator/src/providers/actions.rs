use crate::util::{deep_merge, messages, patch::*, Error, MANAGER_NAME, VERIFICATION_LABEL};
use const_format::concatcp;
use k8s_openapi::{
    api::core::v1::{
        Capabilities, Container, EnvVar, EnvVarSource, Pod, PodSpec, Secret, SecretKeySelector,
        SecurityContext, Volume, VolumeMount,
    },
    apimachinery::pkg::apis::meta::v1::Time,
};
use kube::{
    api::{Api, ObjectMeta, Resource},
    Client,
};
use lazy_static::lazy_static;
use serde_json::Value;
use std::collections::BTreeMap;
use vpn_types::*;

/// Image to use for the curl container. This is used to
/// retrieve the initial/unmasked IP address for the pod
/// during initialization.
pub const CURL_IMAGE: &str = "curlimages/curl:7.88.1";

/// The IP service to use for getting the public IP address.
pub const IP_SERVICE: &str = "https://api.ipify.org";

/// Name of the shared volume, used to share files between
/// containers and detect when the VPN connected. Containers
/// should mount this volume at `SHARED_PATH` and access
/// the initial ip file at `IP_FILE_PATH` to know when the
/// VPN finishes connecting.
pub const SHARED_VOLUME_NAME: &str = "shared";

/// Shared directory path.
pub const SHARED_PATH: &str = "/shared";

/// The file containing the unmasked IP address of the pod.
/// This is written by an init container so the executor
/// knows when the VPN is connected.
pub const IP_FILE_PATH: &str = concatcp!(SHARED_PATH, "/ip");

/// VPN sidecar image. Efforts were made to use a stock
/// image with no modifications, as to maximize the
/// modular paradigm of using sidecars.
pub const DEFAULT_VPN_IMAGE: &str = "qmcgaw/gluetun:v3.32.0";

/// The name of the probe container within the verify pod.
pub const PROBE_CONTAINER_NAME: &str = "probe";

/// The name of the probe container within the verify pod.
pub const VPN_CONTAINER_NAME: &str = "vpn";

/// The script used by the probe container to check if the
/// VPN is connected. Requires the environment variables.
const PROBE_SCRIPT: &str = "#!/bin/sh
INITIAL_IP=$(cat $IP_FILE_PATH) # created by init container
echo \"Unmasked IP address is $INITIAL_IP\"
INITIAL_WAIT=6s
echo \"Waiting for $INITIAL_WAIT to allow the VPN container time to connect...\"
sleep $INITIAL_WAIT
TIMEOUT=5 # IP service request timeout (seconds)
IP=$(curl -m $TIMEOUT -s $IP_SERVICE)
ITER=0
# Continue probing the IP service if it fails while the
# VPN is connecting or returns the initial IP address.
while [ $? -ne 0 ] || [ \"$IP\" = \"$INITIAL_IP\" ]; do
    echo \"Current IP address is $IP, sleeping for $SLEEP_TIME\"
    sleep $SLEEP_TIME
    IP=$(curl -m $TIMEOUT -s $IP_SERVICE)
    # exponential backoff
    TIMEOUT=$((TIMEOUT + ITER))
    SLEEP_TIME=$((SLEEP_TIME + ITER))
    ITER=$((ITER + 1))
done
echo \"VPN connected. Masked IP address: $IP\"";

lazy_static! {
    static ref SHARED_VOLUME_MOUNT: VolumeMount = VolumeMount {
        name: SHARED_VOLUME_NAME.to_owned(),
        mount_path: SHARED_PATH.to_owned(),
        ..Default::default()
    };
    static ref DEFAULT_INIT_CONTAINER: Container = Container {
        name: "init".to_owned(),
        image: Some(CURL_IMAGE.to_owned()),
        image_pull_policy: Some("IfNotPresent".to_owned()),
        command: Some(
            vec!["curl", "-o", IP_FILE_PATH, "-s", IP_SERVICE]
                .into_iter()
                .map(String::from)
                .collect()
        ),
        volume_mounts: Some(vec![SHARED_VOLUME_MOUNT.clone()]),
        ..Default::default()
    };
    static ref DEFAULT_VPN_CONTAINER: Container = Container {
        name: VPN_CONTAINER_NAME.to_owned(),
        image: Some(DEFAULT_VPN_IMAGE.to_owned()),
        image_pull_policy: Some("IfNotPresent".to_owned()),
        security_context: Some(SecurityContext {
            capabilities: Some(Capabilities {
                add: Some(vec!["NET_ADMIN".to_owned()]),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    };
    static ref DEFAULT_PROBE_CONTAINER: Container = Container {
        name: PROBE_CONTAINER_NAME.to_owned(),
        image: Some(CURL_IMAGE.to_owned()),
        image_pull_policy: Some("IfNotPresent".to_owned()),
        command: Some(
            vec!["sh", "-c", "echo \"$PROBE_SCRIPT\" | sh -"]
                .into_iter()
                .map(String::from)
                .collect()
        ),
        env: Some(vec![
            EnvVar {
                name: "PROBE_SCRIPT".to_owned(),
                value: Some(PROBE_SCRIPT.to_owned()),
                ..Default::default()
            },
            EnvVar {
                name: "IP_SERVICE".to_owned(),
                value: Some(IP_SERVICE.to_owned()),
                ..Default::default()
            },
            EnvVar {
                name: "IP_FILE_PATH".to_owned(),
                value: Some(IP_FILE_PATH.to_owned()),
                ..Default::default()
            },
            EnvVar {
                name: "SLEEP_TIME".to_owned(),
                value: Some("10s".to_owned()),
                ..Default::default()
            },
        ]),
        volume_mounts: Some(vec![SHARED_VOLUME_MOUNT.clone()]),
        ..Default::default()
    };
}

/// Updates the MaskProvider's phase to Pending, which indicates
/// the resource made its initial appearance to the operator.
pub async fn pending(client: Client, instance: &MaskProvider) -> Result<(), Error> {
    patch_status(client, instance, |status| {
        status.message = Some(messages::PENDING.to_owned());
        status.phase = Some(MaskProviderPhase::Pending);
    })
    .await?;
    Ok(())
}

/// Updates the MaskProvider's phase to Ready, which indicates
/// the VPN provider is ready to use.
pub async fn ready(client: Client, instance: &MaskProvider) -> Result<(), Error> {
    patch_status(client, instance, |status| {
        status.message = Some("VPN service is ready to use.".to_owned());
        status.phase = Some(MaskProviderPhase::Ready);
        status.active_slots = Some(0);
    })
    .await?;
    Ok(())
}

/// Updates the MaskProvider's phase to Active, which indicates
/// the VPN provider is in use by one or more pods.
pub async fn active(
    client: Client,
    instance: &MaskProvider,
    active_slots: usize,
) -> Result<(), Error> {
    patch_status(client, instance, |status| {
        status.message = Some(format!("VPN service is in use by {} Masks.", active_slots));
        status.phase = Some(MaskProviderPhase::Active);
        status.active_slots = Some(active_slots);
    })
    .await?;
    Ok(())
}

/// Updates the `MaskProvider`'s phase to Terminating.
pub async fn terminating(client: Client, instance: &MaskProvider) -> Result<(), Error> {
    patch_status(client, instance, |status| {
        status.phase = Some(MaskProviderPhase::Terminating);
        status.message = Some(messages::TERMINATING.to_owned());
    })
    .await?;
    Ok(())
}

/// Updates the MaskProvider's phase to ErrSecretNotFound, which indicates
/// the VPN provider is ready to use.
pub async fn secret_missing(
    client: Client,
    instance: &MaskProvider,
    secret_name: &str,
) -> Result<(), Error> {
    patch_status(client, instance, |status| {
        status.message = Some(format!("Secret '{}' does not exist.", secret_name));
        status.phase = Some(MaskProviderPhase::ErrSecretNotFound);
    })
    .await?;
    Ok(())
}

/// Update the status object to show the verification is in progress.
pub async fn verify_progress(
    client: Client,
    instance: &MaskProvider,
    _start_time: Option<Time>,
    message: String,
) -> Result<(), Error> {
    patch_status(client, instance, |status| {
        status.message = Some(message);
        status.phase = Some(MaskProviderPhase::Verifying);
    })
    .await?;
    Ok(())
}

/// Update the status object to show an error message was
/// encountered during verification.
pub async fn verify_failed(
    client: Client,
    instance: &MaskProvider,
    message: String,
) -> Result<(), Error> {
    patch_status(client, instance, |status| {
        status.message = Some(message);
        status.phase = Some(MaskProviderPhase::ErrVerifyFailed);
    })
    .await?;
    Ok(())
}

/// Merges the container spec with the given overrides.
fn merge_containers(container: Container, overrides: Value) -> Result<Container, Error> {
    let mut val = serde_json::to_value(&container)?;
    deep_merge(&mut val, overrides);
    Ok(serde_json::from_value(val)?)
}

/// Creates the container spec for the init container that
/// retrieves the unmasked public IP address and writes it
/// to the shared volume. This is done on startup so that
/// the executor will truly know when it's okay to start
/// downloading the video and/or thumbnail.
fn get_init_container(overrides: Option<&Value>) -> Result<Container, Error> {
    let container = DEFAULT_INIT_CONTAINER.clone();
    match overrides {
        Some(overrides) => merge_containers(container, overrides.clone()),
        None => Ok(container),
    }
}

/// Returns the container the probes the external IP address
/// and exits with code zero when it changes or exits nonzero
/// if it fails to change before the timeout.
fn get_probe_container(overrides: Option<&Value>) -> Result<Container, Error> {
    let container = DEFAULT_PROBE_CONTAINER.clone();
    match overrides {
        Some(overrides) => merge_containers(container, overrides.clone()),
        None => Ok(container),
    }
}

/// Returns the container that connects to the VPN.
fn get_vpn_container(secret: &Secret, overrides: Option<&Value>) -> Result<Container, Error> {
    let secret_name = secret.metadata.name.as_deref().unwrap();
    let mut container = DEFAULT_VPN_CONTAINER.clone();
    container.env = secret.data.as_ref().map(|data| {
        data.iter()
            .map(|(key, _)| EnvVar {
                name: key.clone(),
                value_from: Some(EnvVarSource {
                    secret_key_ref: Some(SecretKeySelector {
                        name: Some(secret_name.to_owned()),
                        key: key.clone(),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .collect()
    });
    match overrides {
        Some(overrides) => merge_containers(container, overrides.clone()),
        None => Ok(container),
    }
}

/// Returns the name of the Mask resource used to reserve
/// a slot for verification.
pub fn get_verify_mask_name(name: &str) -> String {
    format!("{}-verify", name)
}

/// Labels for the verification `Mask` resource, used to force
/// the controller to assign a `MaskProvider` with a specific uid
/// to the child `MaskConsumer`.
fn verify_mask_labels(instance: &MaskProvider) -> BTreeMap<String, String> {
    let mut labels: BTreeMap<String, String> = BTreeMap::new();
    // Add a label to the Mask so that we can easily find it.
    labels.insert("app".to_owned(), MANAGER_NAME.to_owned());
    // Indicate the Mask is meant for this MaskProvider's verification.
    // This will force assign the Mask regardless of the MaskProvider's
    // phase and the only error possible will be ErrNoSlots.
    labels.insert(
        VERIFICATION_LABEL.to_owned(),
        instance.metadata.uid.clone().unwrap(),
    );
    labels
}

/// Returns the Mask resource used to cloak the verification Pod.
fn verify_mask(name: &str, namespace: &str, instance: &MaskProvider) -> Mask {
    Mask {
        metadata: ObjectMeta {
            name: Some(get_verify_mask_name(name)),
            namespace: Some(namespace.to_owned()),
            labels: Some(verify_mask_labels(instance)),
            owner_references: Some(vec![instance.controller_owner_ref(&()).unwrap()]),
            ..Default::default()
        },
        spec: MaskSpec {
            // Note: `providers` is omitted because we use the labels to constrain
            // the controller to assign a specific MaskProvider.
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Returns a Pod resource that verifies the VPN credentials work.
fn verify_pod(
    name: &str,
    namespace: &str,
    instance: &MaskProvider,
    secret: &Secret,
    consumer: &MaskConsumer,
) -> Result<Pod, Error> {
    let overrides = instance
        .spec
        .verify
        .as_ref()
        .map_or(None, |v| v.overrides.as_ref());
    let container_overrides = overrides.map_or(None, |o| o.containers.as_ref());

    // Assemble the container specs with the overrides.
    let init_container = get_init_container(container_overrides.map_or(None, |c| c.init.as_ref()))?;
    let vpn_container =
        get_vpn_container(secret, container_overrides.map_or(None, |c| c.vpn.as_ref()))?;
    let probe_container =
        get_probe_container(container_overrides.map_or(None, |c| c.probe.as_ref()))?;

    // Assemble the containers into a pod.
    let pod = Pod {
        metadata: ObjectMeta {
            name: Some(name.to_owned()),
            namespace: Some(namespace.to_owned()),
            labels: Some({
                // Add a label to the pod so that we can easily find it.
                let mut labels: BTreeMap<String, String> = BTreeMap::new();
                labels.insert("app".to_owned(), MANAGER_NAME.to_owned());
                labels
            }),
            // Setting the MaskConsumer as the owner will allow the
            // pod to be properly garbage collected when the provider
            // is unassigned from the Mask.
            owner_references: Some(vec![consumer.controller_owner_ref(&()).unwrap()]),
            ..Default::default()
        },
        spec: Some(PodSpec {
            restart_policy: Some("Never".to_owned()),
            init_containers: Some(vec![init_container]),
            containers: vec![vpn_container, probe_container],
            volumes: Some(vec![Volume {
                name: SHARED_VOLUME_NAME.to_owned(),
                empty_dir: Some(Default::default()),
                ..Default::default()
            }]),
            ..Default::default()
        }),
        ..Default::default()
    };

    // Apply overrides to the pod if necessary.
    match overrides.map_or(None, |o| o.pod.as_ref()) {
        // Merge the overriden values into the resource.
        Some(pod_template) => {
            let mut val = serde_json::to_value(&pod)?;
            deep_merge(&mut val, pod_template.clone());
            Ok(serde_json::from_value(val)?)
        }
        // No pod override requested.
        _ => Ok(pod),
    }
}

/// Signals that the VPN credentials are verified.
pub async fn verified(client: Client, instance: &MaskProvider) -> Result<(), Error> {
    patch_status(client, instance, |status| {
        status.last_verified = Some(chrono::Utc::now().to_rfc3339());
        status.phase = Some(MaskProviderPhase::Verified);
        status.message = Some("VPN credentials verified as authentic.".to_owned())
    })
    .await?;
    Ok(())
}

/// Creates a Mask for the verification pod.
pub async fn create_verify_mask(
    client: Client,
    name: &str,
    namespace: &str,
    instance: &MaskProvider,
) -> Result<Mask, Error> {
    let mask_api: Api<Mask> = Api::namespaced(client, namespace);
    let mask = verify_mask(name, namespace, instance);
    Ok(mask_api.create(&Default::default(), &mask).await?)
}

/// Creates a pod that verifies the VPN credentials work.
pub async fn create_verify_pod(
    client: Client,
    name: &str,
    namespace: &str,
    instance: &MaskProvider,
    consumer: &MaskConsumer,
) -> Result<Pod, Error> {
    // Extract the assigned provider from the status object.
    let assigned_provider = consumer
        .status
        .as_ref()
        .map_or(None, |s| s.provider.as_ref())
        .ok_or_else(|| {
            // This shouldn't happen under normal conditions because
            // this action shouldn't be called unless the the consumer
            // has already been assigned a provider.
            Error::UserInputError("MaskConsumer is not assigned to a MaskProvider".to_owned())
        })?;

    // Sanity check: make sure the the assigned provider matches the one we're verifying.
    if instance.metadata.uid.as_deref() != Some(&assigned_provider.uid) {
        return Err(Error::UserInputError(format!(
            "MaskConsumer is assigned to a different MaskProvider. Got {}, expected {}.",
            assigned_provider.uid,
            instance.metadata.uid.as_deref().unwrap(),
        )));
    }

    // Get the VPN credentials secret so we know which keys
    // to inject into the VPN container's environment. The secret
    // has a unique name so there's no need to check its UID.
    let secret_api: Api<Secret> = Api::namespaced(client.clone(), namespace);
    let secret = secret_api.get(&assigned_provider.secret).await?;

    // Create the pod, honoring overrides in the MaskProvider spec.
    let pod = verify_pod(name, namespace, instance, &secret, consumer)?;
    let pod_api: Api<Pod> = Api::namespaced(client, namespace);
    Ok(pod_api.create(&Default::default(), &pod).await?)
}

/// Deletes the verification Pod.
pub async fn delete_verify_pod(client: Client, name: &str, namespace: &str) -> Result<(), Error> {
    let api: Api<Pod> = Api::namespaced(client, namespace);
    match api.delete(name, &Default::default()).await {
        // Pod was deleted.
        Ok(_) => Ok(()),
        // Pod does not exist.
        Err(kube::Error::Api(e)) if e.code == 404 => Ok(()),
        // Error deleting Pod.
        Err(e) => Err(e.into()),
    }
}

/// Deletes the verification Mask.
pub async fn delete_verify_mask(client: Client, name: &str, namespace: &str) -> Result<(), Error> {
    let api: Api<Mask> = Api::namespaced(client, namespace);
    let name = get_verify_mask_name(name);
    match api.delete(&name, &Default::default()).await {
        // Pod was deleted.
        Ok(_) => Ok(()),
        // Pod does not exist.
        Err(kube::Error::Api(e)) if e.code == 404 => Ok(()),
        // Error deleting Pod.
        Err(e) => Err(e.into()),
    }
}
