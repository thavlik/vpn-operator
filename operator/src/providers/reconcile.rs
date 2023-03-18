use chrono::Utc;
use futures::stream::StreamExt;
use k8s_openapi::api::core::v1::{ConfigMap, Pod, PodStatus, Secret};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;
use kube::{
    api::ListParams, client::Client, runtime::controller::Action, runtime::Controller, Api,
    Resource, ResourceExt,
};
use lazy_static::lazy_static;
use std::sync::Arc;
use tokio::time::Duration;
use vpn_types::*;

use super::{
    actions::{self, get_verify_mask_name, PROBE_CONTAINER_NAME, VPN_CONTAINER_NAME},
    finalizer,
};
use crate::util::{Error, FINALIZER_NAME, PROBE_INTERVAL};

#[cfg(feature = "metrics")]
use super::metrics;

/// Entrypoint for the `MaskProvider` controller.
pub async fn run(client: Client) -> Result<(), Error> {
    println!("Starting MaskProvider controller...");

    // Preparation of resources used by the `kube_runtime::Controller`
    let crd_api: Api<MaskProvider> = Api::all(client.clone());
    let context: Arc<ContextData> = Arc::new(ContextData::new(client.clone()));

    // The controller comes from the `kube_runtime` crate and manages the reconciliation process.
    // It requires the following information:
    // - `kube::Api<T>` this controller "owns". In this case, `T = MaskProvider`, as this controller owns the `MaskProvider` resource,
    // - `kube::api::ListParams` to select the `MaskProvider` resources with. Can be used for MaskProvider filtering `MaskProvider` resources before reconciliation,
    // - `reconcile` function with reconciliation logic to be called each time a resource of `MaskProvider` kind is created/updated/deleted,
    // - `on_error` function to call whenever reconciliation fails.
    Controller::new(crd_api, ListParams::default())
        .owns(Api::<ConfigMap>::all(client.clone()), ListParams::default())
        .owns(Api::<Mask>::all(client), ListParams::default())
        .run(reconcile, on_error, context)
        .for_each(|_reconciliation_result| async move {
            //match reconciliation_result {
            //    Ok(_MaskProvider_resource) => {
            //        //println!(
            //        //    "Reconciliation successful. Resource: {:?}",
            //        //    MaskProvider_resource
            //        //);
            //    }
            //    Err(reconciliation_err) => {
            //        eprintln!("Reconciliation error: {:?}", reconciliation_err)
            //    }
            //}
        })
        .await;
    Ok(())
}

/// Context injected with each `reconcile` and `on_error` method invocation.
struct ContextData {
    /// Kubernetes client to make Kubernetes API requests with. Required for K8S resource management.
    client: Client,
}

impl ContextData {
    /// Constructs a new instance of ContextData.
    ///
    /// # Arguments:
    /// - `client`: A Kubernetes client to make Kubernetes REST API requests with. Resources
    /// will be created and deleted with this client.
    pub fn new(client: Client) -> Self {
        ContextData { client }
    }
}

/// Action to be taken upon an `MaskProvider` resource during reconciliation
#[derive(Debug, PartialEq)]
enum MaskProviderAction {
    /// Set the `MaskProvider` resource status.phase to Pending.
    Pending,

    /// Adds the finalizer to the `MaskProvider` resource.
    AddFinalizer,

    /// Cleans up all subresources across all namespaces.
    Delete,

    /// Set the `MaskProvider` resource status.phase to ErrSecretNotFound.
    SecretNotFound(String),

    /// Create a Mask to reserve a slot for verification.
    CreateVerifyMask,

    /// Create a gluetun pod and verify that the external IP changes.
    CreateVerifyPod(Mask),

    /// Set the status to Verifying.
    Verifying {
        message: String,
        start_time: Option<Time>,
    },

    /// Set the status to Verified.
    Verified,

    /// Set the status to ErrVerifyFailed.
    VerifyFailed(String),

    /// Set the `MaskProvider` resource status.phase to Ready.
    Ready,

    /// Set the `MaskProvider` resource status.phase to Active.
    Active { active_slots: usize },

    /// This `MaskProvider` resource is in desired state and requires no actions to be taken
    NoOp,
}

impl MaskProviderAction {
    fn to_str(&self) -> &str {
        match self {
            MaskProviderAction::Pending => "Pending",
            MaskProviderAction::AddFinalizer => "AddFinalizer",
            MaskProviderAction::Delete => "Delete",
            MaskProviderAction::SecretNotFound(_) => "SecretNotFound",
            MaskProviderAction::CreateVerifyMask => "CreateVerifyMask",
            MaskProviderAction::CreateVerifyPod(_) => "CreateVerifyPod",
            MaskProviderAction::Verifying { .. } => "Verifying",
            MaskProviderAction::Verified => "Verified",
            MaskProviderAction::VerifyFailed(_) => "VerifyFailed",
            MaskProviderAction::Ready => "Ready",
            MaskProviderAction::Active { .. } => "Active",
            MaskProviderAction::NoOp => "NoOp",
        }
    }
}

/// Reconciliation function for the `MaskProvider` resource.
async fn reconcile(
    instance: Arc<MaskProvider>,
    context: Arc<ContextData>,
) -> Result<Action, Error> {
    // The `Client` is shared -> a clone from the reference is obtained
    let client: Client = context.client.clone();

    // The resource of `MaskProvider` kind is required to have a namespace set. However, it is not guaranteed
    // the resource will have a `namespace` set. Therefore, the `namespace` field on object's metadata
    // is optional and Rust forces the programmer to check for it's existence first.
    let namespace: String = match instance.namespace() {
        None => {
            // If there is no namespace to deploy to defined, reconciliation ends with an error immediately.
            return Err(Error::UserInputError(
                "Expected MaskProvider resource to be namespaced. Can't deploy to an unknown namespace."
                    .to_owned(),
            ));
        }
        // If namespace is known, proceed. In a more advanced version of the operator, perhaps
        // the namespace could be checked for existence first.
        Some(namespace) => namespace,
    };

    // Name of the MaskProvider resource is used to name the subresources as well.
    let name = instance.name_any();

    #[cfg(feature = "metrics")]
    metrics::PROVIDER_RECONCILE_COUNTER
        .with_label_values(&[&name, &namespace])
        .inc();

    // Benchmark the read phase of reconciliation.
    #[cfg(feature = "metrics")]
    let start = std::time::Instant::now();

    // Read phase of reconciliation determines goal during the write phase.
    let action = determine_action(client.clone(), &name, &namespace, &instance).await?;

    if action != MaskProviderAction::NoOp {
        println!("{}/{} ACTION: {:?}", namespace, name, action.to_str());
    }

    // Report the read phase performance.
    #[cfg(feature = "metrics")]
    metrics::PROVIDER_READ_HISTOGRAM
        .with_label_values(&[&name, &namespace, action.to_str()])
        .observe(start.elapsed().as_secs_f64());

    // Increment the counter for the action.
    #[cfg(feature = "metrics")]
    metrics::PROVIDER_ACTION_COUNTER
        .with_label_values(&[&name, &namespace, action.to_str()])
        .inc();

    // Benchmark the write phase of reconciliation.
    #[cfg(feature = "metrics")]
    let timer = match action {
        // Don't measure performance for NoOp actions.
        MaskProviderAction::NoOp => None,
        // Start a performance timer for the write phase.
        _ => Some(
            metrics::PROVIDER_WRITE_HISTOGRAM
                .with_label_values(&[&name, &namespace, action.to_str()])
                .start_timer(),
        ),
    };

    // Performs action as decided by the `determine_action` function.
    // This is the write phase of reconciliation.
    let result = match action {
        MaskProviderAction::Pending => {
            // Give the `MaskProvider` resource a finalizer. This will be done
            // regardless of whether we do it now, but doing it now might
            // increase performance.
            let instance = finalizer::add(client.clone(), &name, &namespace).await?;

            // Update the phase of the `MaskProvider` resource to Pending.
            actions::pending(client, &instance).await?;

            // Requeue immediately.
            Action::requeue(Duration::ZERO)
        }
        MaskProviderAction::AddFinalizer => {
            // Ensure the finalizer is present on the `MaskProvider` resource.
            finalizer::add(client, &name, &namespace).await?;

            // Requeue immediately.
            Action::requeue(Duration::ZERO)
        }
        MaskProviderAction::Delete => {
            // Delete the verification Pod.
            actions::delete_verify_pod(client.clone(), &name, &namespace).await?;

            // Delete the verification Mask.
            actions::delete_verify_mask(client.clone(), &name, &namespace).await?;

            // Delete Secrets in namespaces that use this `MaskProvider`.
            // This will prevent `Masks` from continuing to use the credentials
            // assigned to them by this `MaskProvider`.
            actions::unassign_all(client.clone(), &name, &namespace, &instance).await?;

            // Remove the finalizer, which will allow the MaskProvider resource to be deleted.
            finalizer::delete(client, &name, &namespace).await?;

            // No need to requeue as the resource is being deleted.
            Action::await_change()
        }
        MaskProviderAction::SecretNotFound(secret_name) => {
            // Reflect the error in the status object.
            actions::secret_missing(client, &instance, &secret_name).await?;

            // Requeue after a while if the resource doesn't change.
            Action::requeue(PROBE_INTERVAL)
        }
        MaskProviderAction::CreateVerifyMask => {
            // Create the verification Mask.
            actions::create_verify_mask(client.clone(), &name, &namespace, &instance).await?;

            // Indicate that verification is in progress.
            actions::verify_progress(
                client,
                &instance,
                None,
                "Created verification Mask.".to_owned(),
            )
            .await?;

            // Requeue after a short delay to allow the verification time to complete.
            Action::requeue(PROBE_INTERVAL)
        }
        MaskProviderAction::CreateVerifyPod(mask) => {
            // Create the verification pod.
            let pod =
                actions::create_verify_pod(client.clone(), &name, &namespace, &instance, &mask)
                    .await?;

            // Indicate that verification is in progress.
            actions::verify_progress(
                client,
                &instance,
                pod.metadata.creation_timestamp,
                "Created verification Pod.".to_owned(),
            )
            .await?;

            // Requeue after a short delay to allow the verification time to complete.
            Action::requeue(PROBE_INTERVAL)
        }
        MaskProviderAction::Verifying {
            start_time,
            message,
        } => {
            // Post the progress to the status object.
            actions::verify_progress(client, &instance, start_time, message).await?;

            // Requeue after a short delay to allow the verification time to complete.
            Action::requeue(PROBE_INTERVAL)
        }
        MaskProviderAction::VerifyFailed(message) => {
            // Update the phase of the `MaskProvider` resource to Verified.
            actions::verify_failed(client.clone(), &instance, message).await?;

            // Delete the verification Pod so it can be recreated.
            actions::delete_verify_pod(client.clone(), &name, &namespace).await?;

            // Delete the verification Mask so it can be recreated.
            actions::delete_verify_mask(client, &name, &namespace).await?;

            // Requeue after a delay so the user has time to see the error phase.
            Action::requeue(PROBE_INTERVAL)
        }
        MaskProviderAction::Verified => {
            // Set the timestamp of when the verification completed.
            actions::verified(client.clone(), &instance).await?;

            // Delete the verification Pod.
            actions::delete_verify_pod(client.clone(), &name, &namespace).await?;

            // Delete the verification Mask.
            actions::delete_verify_mask(client, &name, &namespace).await?;

            // Requeue immediately to proceed with reconciliation.
            Action::requeue(Duration::ZERO)
        }
        MaskProviderAction::Ready => {
            // Update the phase of the `MaskProvider` resource to Ready.
            actions::ready(client, &instance).await?;

            // Requeue after a short delay.
            Action::requeue(PROBE_INTERVAL)
        }
        MaskProviderAction::Active { active_slots } => {
            // Update the phase of the `MaskProvider` resource to Active.
            actions::active(client, &instance, active_slots).await?;

            // Requeue after a short delay.
            Action::requeue(PROBE_INTERVAL)
        }
        // The resource is already in desired state, do nothing and re-check after 10 seconds
        MaskProviderAction::NoOp => Action::requeue(PROBE_INTERVAL),
    };

    #[cfg(feature = "metrics")]
    if let Some(timer) = timer {
        timer.observe_duration();
    }

    Ok(result)
}

/// needs_pending returns true if the `MaskProvider` resource
/// requires a status update to set the phase to Pending.
/// This should be the first action for any managed resource.
fn needs_pending(instance: &MaskProvider) -> bool {
    instance.status.is_none() || instance.status.as_ref().unwrap().phase.is_none()
}

/// Returns the phase of the MaskProvider.
pub fn get_provider_phase(instance: &MaskProvider) -> Result<(MaskProviderPhase, Duration), Error> {
    let status = instance
        .status
        .as_ref()
        .ok_or_else(|| Error::UserInputError("No status".to_string()))?;
    let phase = status
        .phase
        .ok_or_else(|| Error::UserInputError("No phase".to_string()))?;
    let last_updated: chrono::DateTime<Utc> = status
        .last_updated
        .as_ref()
        .ok_or_else(|| Error::UserInputError("No lastUpdated".to_string()))?
        .parse()?;
    let age: chrono::Duration = Utc::now() - last_updated;
    Ok((phase, age.to_std()?))
}

/// Gets the secret that contains the credentials for the MaskProvider.
async fn get_secret(
    client: Client,
    namespace: &str,
    provider: &MaskProvider,
) -> Result<Option<Secret>, Error> {
    let api: Api<Secret> = Api::namespaced(client, namespace);
    match api.get(&provider.spec.secret).await {
        Ok(secret) => Ok(Some(secret)),
        Err(kube::Error::Api(ae)) if ae.code == 404 => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Returns true if the MaskProvider is missing the finalizer.
fn needs_finalizer(instance: &MaskProvider) -> bool {
    !instance.finalizers().iter().any(|f| f == FINALIZER_NAME)
}

/// Resources arrives into reconciliation queue in a certain state. This function looks at
/// the state of given `MaskProvider` resource and decides which actions needs to be performed.
/// The finite set of possible actions is represented by the `MaskProviderAction` enum.
///
/// # Arguments
/// - `MaskProvider`: A reference to `MaskProvider` being reconciled to decide next action upon.
async fn determine_action(
    client: Client,
    name: &str,
    namespace: &str,
    instance: &MaskProvider,
) -> Result<MaskProviderAction, Error> {
    if instance.meta().deletion_timestamp.is_some() {
        return Ok(MaskProviderAction::Delete);
    }

    // Ensure that the resource has a status object with a phase.
    // The rest of the controller code relies on the presence
    // of both these fields and will panic if they are not present.
    if needs_pending(instance) {
        // This should be the first action for any freshly created
        // MaskProvider resources. It will be immediately requeued.
        return Ok(MaskProviderAction::Pending);
    }

    // Ensure the resource has a finalizer so child resources
    // in other namespaces can be cleaned up before deletion.
    if needs_finalizer(instance) {
        return Ok(MaskProviderAction::AddFinalizer);
    }

    // Ensure the MaskProvider credentials secret exists.
    if get_secret(client.clone(), namespace, instance)
        .await?
        .is_none()
    {
        // The resource specifies using a Secret that doesn't exist.
        // This is the only error state for the MaskProvider resource.
        return Ok(MaskProviderAction::SecretNotFound(
            instance.spec.secret.clone(),
        ));
    }

    // Check if the MaskProvider requires verification.
    if let Some(action) = determine_verify_action(client.clone(), name, namespace, instance).await?
    {
        return Ok(action);
    }

    // Remaining actions aim to keep the status object current.
    determine_status_action(client, namespace, instance).await
}

lazy_static! {
    static ref DEFAULT_VERIFY_SPEC: MaskProviderVerifySpec = Default::default();
}

const DEFAULT_VERIFY_TIMEOUT: Duration = Duration::from_secs(60);

/// Gets the verification Mask for the MaskProvider.
async fn get_verify_mask(
    client: Client,
    name: &str,
    namespace: &str,
) -> Result<Option<Mask>, Error> {
    let api: Api<Mask> = Api::namespaced(client, namespace);
    let name = get_verify_mask_name(name);
    match api.get(&name).await {
        Ok(pod) => Ok(Some(pod)),
        Err(kube::Error::Api(ae)) if ae.code == 404 => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Gets the verification pod for the MaskProvider.
async fn get_verify_pod(client: Client, name: &str, namespace: &str) -> Result<Option<Pod>, Error> {
    let api: Api<Pod> = Api::namespaced(client, namespace);
    match api.get(name).await {
        Ok(pod) => Ok(Some(pod)),
        Err(kube::Error::Api(ae)) if ae.code == 404 => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Returns the amount of time that has passed since the Pod's creation.
fn get_pod_age(pod: &Pod) -> Result<Duration, Error> {
    Ok((chrono::Utc::now()
        - pod
            .metadata
            .creation_timestamp
            .as_ref()
            .ok_or_else(|| Error::UserInputError("Pod creation timestamp is missing".to_string()))?
            .0)
        .to_std()?)
}

/// Returns the amount of time the verification pod is allowed to run
/// before it is considered a failure.
fn get_verify_timeout(instance: &MaskProvider) -> Duration {
    instance
        .spec
        .verify
        .as_ref()
        .map_or(None, |v| v.timeout.as_deref())
        .map_or(None, |t| parse_duration::parse(t).ok())
        .unwrap_or(DEFAULT_VERIFY_TIMEOUT)
}

/// Determines the action given that the verification Mask is present
/// and the Pod is not.
fn determine_verify_mask_action(mask: Mask) -> Result<MaskProviderAction, Error> {
    Ok(match mask.status.as_ref().map_or(None, |s| s.phase) {
        // Controller is still processing the Mask.
        Some(MaskPhase::Pending) | None => MaskProviderAction::Verifying {
            start_time: None,
            message: "Waiting on the controller for the verification Mask.".to_owned(),
        },
        // The Mask is ready to be used in the verification Pod.
        // It should never be Active here, but if it, we know the
        // Pod doesn't exist and we shouldn't be exceeding maxSlots.
        Some(MaskPhase::Ready) | Some(MaskPhase::Active) => {
            MaskProviderAction::CreateVerifyPod(mask)
        }
        // The MaskProvider has too many active slots, we will have to wait.
        Some(MaskPhase::Waiting) => MaskProviderAction::Verifying {
            start_time: None,
            message: "Waiting for the verification Mask to be assigned a slot.".to_owned(),
        },
        // Unreachable branch: failed to assign the MaskProvider.
        Some(MaskPhase::ErrNoProviders) => MaskProviderAction::VerifyFailed(
            "Verification Mask observed unexpected ErrNoProviders.".to_owned(),
        ),
    })
}

/// Determines the action given that the verification Pod is present.
fn determine_verify_pod_action(
    instance: &MaskProvider,
    pod: &Pod,
) -> Result<MaskProviderAction, Error> {
    // Examine the status object of the pod.
    let status = pod
        .status
        .as_ref()
        .ok_or_else(|| Error::UserInputError("Pod status is missing".to_string()))?;
    let phase = status
        .phase
        .as_deref()
        .ok_or_else(|| Error::UserInputError("Pod phase is missing".to_string()))?;

    // Since the probe container will exit with code 0, the pod
    // may not be in the "Succeeded" phase. On my kubernetes cluster
    // (DigitalOcean w/ containerd) the pods enter the phase Running
    // (but it will read NotReady), and the container status can be
    // inspected to determine the VPN connection was successful.
    if is_probe_successful(status) {
        return Ok(MaskProviderAction::Verified);
    }

    Ok(match phase {
        // Verification pod is waiting to be scheduled.
        // This may be an error if the pod isn't able to be scheduled.
        "Pending" => match check_pod_scheduling_error(status) {
            Some(message) => MaskProviderAction::VerifyFailed(message),
            None => check_verify_timeout(instance, &pod)?,
        },
        // Verification pod is still waiting for the IP to change.
        "Running" => check_verify_timeout(instance, &pod)?,
        // Verification has completed (new IP obtained).
        // This is what should be observed according to the
        // Kubernetes docs, but it doesn't seem to be the case.
        "Succeeded" => MaskProviderAction::Verified,
        // Unknown error.
        _ => MaskProviderAction::VerifyFailed(
            "Unknown error occurred during verification.".to_owned(),
        ),
    })
}

/// Returns the action given that the verification Pod
/// is in a Pending or Running phase. Checks to see if
/// the verification attempt has timed out.
fn check_verify_timeout(instance: &MaskProvider, pod: &Pod) -> Result<MaskProviderAction, Error> {
    // Make sure the verification pod isn't too old.
    // If it goes past the timeout, it doesn't matter what
    // phase it's in, it will be considered a failure.
    Ok(if get_pod_age(pod)? > get_verify_timeout(instance) {
        MaskProviderAction::VerifyFailed(
            "Verification timed out waiting for Pod to schedule.".to_owned(),
        )
    } else {
        // Still waiting for pod to be scheduled.
        MaskProviderAction::Verifying {
            start_time: pod.metadata.creation_timestamp.clone(),
            message: "Waiting on verification Pod to start.".to_owned(),
        }
    })
}

/// Returns true if the pod's status indicates the probe
/// was successful and therefore verification has passed.
/// There is a quirk on Kubernetes where a multicontainer
/// pod that has only one container exit will be in the
/// Running phase on the yaml, but it will be displayed as
/// NotReady by kubectl. The container statuses can be inspected
/// to determine if the probe was successful. Because of the
/// discrepancy in the apparent and actual phases, this doesn't
/// look at the phase at all.
fn is_probe_successful(status: &PodStatus) -> bool {
    status
        .container_statuses
        .as_ref()
        .map_or(None, |cs| {
            cs.iter().filter(|s| s.name == VPN_CONTAINER_NAME).next()
        })
        .map_or(false, |cs| {
            // VPN container should still be running.
            cs.state.as_ref().map_or(false, |s| s.running.is_some())
        })
        && status
            .container_statuses
            .as_ref()
            .map_or(None, |cs| {
                cs.iter().filter(|s| s.name == PROBE_CONTAINER_NAME).next()
            })
            .map_or(false, |cs| {
                // Probe container should have exited with code 0.
                cs.state.as_ref().map_or(false, |s| {
                    s.terminated.as_ref().map_or(false, |t| t.exit_code == 0)
                })
            })
}

/// Checks if verification is necessary and returns the appropriate action.
async fn determine_verify_action(
    client: Client,
    name: &str,
    namespace: &str,
    instance: &MaskProvider,
) -> Result<Option<MaskProviderAction>, Error> {
    let verify = match instance.spec.verify {
        // User is requesting verification be skipped.
        Some(ref verify) if verify.skip.unwrap_or(false) => return Ok(None),
        // Use the specified verification settings.
        Some(ref verify) => verify,
        // Use default verification settings.
        None => &DEFAULT_VERIFY_SPEC,
    };

    // Check if the verify pod exists. Its existence implies that
    // verification was required at some point.
    if let Some(pod) = get_verify_pod(client.clone(), name, namespace).await? {
        // Verification Pod exists. Examine its status object.
        return Ok(Some(determine_verify_pod_action(instance, &pod)?));
    }

    // Check if the verify Mask exists. Its existence implies that
    // verification was required at some point. We may be doing a
    // periodic verification and it's still important not to exceed
    // the spec's maxSlots.
    if let Some(mask) = get_verify_mask(client.clone(), name, namespace).await? {
        // Verification Mask exists. Examine its status object.
        return Ok(Some(determine_verify_mask_action(mask)?));
    }

    // Determine if we need to verify the credentials.
    if let Some(ref last_verified) = instance.status.as_ref().unwrap().last_verified {
        // The service has been verified before.
        let interval = match verify.interval {
            // Verification has passed once and the user is not
            // requesting periodic verification.
            None => return Ok(None),
            // User is requesting periodic verification.
            Some(ref interval) => interval,
        };
        // Parse the interval spec into a Duration.
        let interval = chrono::Duration::from_std(parse_duration::parse(interval)?)?;
        // Determine the age of the verificataion.
        let last_verified: chrono::DateTime<Utc> = last_verified.parse()?;
        let age: chrono::Duration = Utc::now() - last_verified;
        if age < interval {
            // Verification is up to date.
            return Ok(None);
        }
        // Verification is stale.
    }

    // Create the verification resources.
    Ok(Some(MaskProviderAction::CreateVerifyMask))
}

/// Returns the number of reservation ConfigMaps for a MaskProvider.
async fn count_reservations(
    client: Client,
    namespace: &str,
    instance: &MaskProvider,
) -> Result<usize, Error> {
    // Only count reservations that belong to this specific MaskProvider.
    // Filtering this way excludes reservations from deleted resources
    // that were immediately recreated.
    let uid = instance.metadata.uid.as_deref().unwrap();

    // Count the ConfigMaps with the MaskProvider as the owner.
    Ok(Api::<ConfigMap>::namespaced(client, namespace)
        .list(&ListParams::default())
        .await?
        .into_iter()
        .filter(|cm| {
            // Only inspect ConfigMaps owned by this MaskProvider.
            cm.metadata
                .owner_references
                .as_ref()
                .map_or(false, |ors| ors.iter().any(|or| or.uid == uid))
        })
        .count())
}

/// Determines the action given that the only thing left to do
/// is periodically keeping the Active phase up-to-date.
async fn determine_status_action(
    client: Client,
    namespace: &str,
    instance: &MaskProvider,
) -> Result<MaskProviderAction, Error> {
    // Count the ConfigMaps with the MaskProvider as the owner.
    let active_slots = count_reservations(client, namespace, instance).await?;
    let (phase, age) = get_provider_phase(instance)?;
    if active_slots > 0 {
        if phase != MaskProviderPhase::Active || age > PROBE_INTERVAL {
            // Keep the Active status up to date.
            return Ok(MaskProviderAction::Active { active_slots });
        }
    } else {
        if phase != MaskProviderPhase::Ready || age > PROBE_INTERVAL {
            // Keep the Ready status up to date.
            return Ok(MaskProviderAction::Ready);
        }
    }
    // Nothing to do, resource is fully reconciled.
    Ok(MaskProviderAction::NoOp)
}

/// Actions to be taken when a reconciliation fails - for whatever reason.
/// Prints out the error to `stderr` and requeues the resource for another reconciliation after
/// five seconds.
///
/// # Arguments
/// - `instance`: The erroneous resource.
/// - `error`: A reference to the `kube::Error` that occurred during reconciliation.
/// - `_context`: Unused argument. Context Data "injected" automatically by kube-rs.
fn on_error(instance: Arc<MaskProvider>, error: &Error, _context: Arc<ContextData>) -> Action {
    eprintln!("Reconciliation error:\n{:?}.\n{:?}", error, instance);
    Action::requeue(Duration::from_secs(5))
}

fn check_pod_scheduling_error(status: &PodStatus) -> Option<String> {
    let conditions: &Vec<_> = match status.conditions.as_ref() {
        Some(conditions) => conditions,
        None => return None,
    };
    for condition in conditions {
        if condition.type_ == "PodScheduled" && condition.status == "False" {
            return Some(
                condition
                    .message
                    .as_deref()
                    .unwrap_or("PodScheduled == False, but no message was provided.")
                    .to_owned(),
            );
        }
    }
    None
}
