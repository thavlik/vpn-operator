use chrono::Utc;
use futures::stream::StreamExt;
use k8s_openapi::api::core::v1::Secret;
use kube::Resource;
use kube::ResourceExt;
use kube::{
    api::ListParams, client::Client, runtime::controller::Action, runtime::Controller, Api,
};
use std::sync::Arc;
use tokio::time::Duration;
pub use vpn_types::*;

#[cfg(metrics)]
use crate::metrics::{MASK_ACTION_COUNTER, MASK_RECONCILE_COUNTER, MASK_WRITE_PERF};

use super::{
    actions::{self, owns_reservation},
    finalizer,
};
use crate::util::{Error, PROBE_INTERVAL};

/// Entrypoint for the `Mask` controller.
pub async fn run(client: Client) -> Result<(), Error> {
    println!("Starting Mask controller...");

    // Preparation of resources used by the `kube_runtime::Controller`
    let crd_api: Api<Mask> = Api::all(client.clone());
    let context: Arc<ContextData> = Arc::new(ContextData::new(client.clone()));

    // The controller comes from the `kube_runtime` crate and manages the reconciliation process.
    // It requires the following information:
    // - `kube::Api<T>` this controller "owns". In this case, `T = Mask`, as this controller owns the `Mask` resource,
    // - `kube::api::ListParams` to select the `Mask` resources with. Can be used for Mask filtering `Mask` resources before reconciliation,
    // - `reconcile` function with reconciliation logic to be called each time a resource of `Mask` kind is created/updated/deleted,
    // - `on_error` function to call whenever reconciliation fails.
    Controller::new(crd_api, ListParams::default())
        .owns(Api::<Secret>::all(client), ListParams::default())
        .run(reconcile, on_error, context)
        .for_each(|_reconciliation_result| async move {
            //match reconciliation_result {
            //    Ok(_mask_resource) => {
            //        //println!("Reconciliation successful. Resource: {:?}", mask_resource);
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

/// Action to be taken upon an `Mask` resource during reconciliation
#[derive(Debug, PartialEq)]
enum MaskAction {
    /// Set the Mask's phase to Pending.
    Pending,
    /// Assign the Mask a VPN provider.
    Assign,
    /// Delete all subresources.
    Delete,
    /// Create the credentials secret for the Mask.
    CreateSecret,
    /// Signals the Mask is ready to be used.
    Active,
    /// The Mask resource is in desired state and requires no actions to be taken.
    NoOp,
}

/// needs_pending returns true if the `Mask` resource
/// requires a status update to set the phase to Pending.
/// This should be the first action for any managed resource.
fn needs_pending(instance: &Mask) -> bool {
    instance.status.is_none() || instance.status.as_ref().unwrap().phase.is_none()
}

/// Returns the Mask's assigned provider from its status object.
fn get_assigned_provider(instance: &Mask) -> Option<&AssignedProvider> {
    match instance.status {
        None => None,
        Some(ref status) => status.provider.as_ref(),
    }
}

/// Reconciliation function for the `Mask` resource.
async fn reconcile(instance: Arc<Mask>, context: Arc<ContextData>) -> Result<Action, Error> {
    // The `Client` is shared -> a clone from the reference is obtained
    let client: Client = context.client.clone();

    // The resource of `Mask` kind is required to have a namespace set. However, it is not guaranteed
    // the resource will have a `namespace` set. Therefore, the `namespace` field on object's metadata
    // is optional and Rust forces the programmer to check for it's existence first.
    let namespace: String = match instance.namespace() {
        None => {
            // If there is no namespace to deploy to defined, reconciliation ends with an error immediately.
            return Err(Error::UserInputError(
                "Expected Mask resource to be namespaced. Can't deploy to an unknown namespace."
                    .to_owned(),
            ));
        }
        // If namespace is known, proceed. In a more advanced version of the operator, perhaps
        // the namespace could be checked for existence first.
        Some(namespace) => namespace,
    };

    // Name of the Mask resource is used to name the subresources as well.
    let name = instance.name_any();

    // Increment total number of reconciles for the Mask resource.
    #[cfg(metrics)]
    MASK_RECONCILE_COUNTER
        .with_label_values(&[&name, &namespace])
        .inc();

    // Benchmark the read phase of reconciliation.
    #[cfg(metrics)]
    let start = std::time::Instant::now();

    // Read phase of reconciliation determines goal during the write phase.
    let action = determine_action(client.clone(), &name, &namespace, &instance).await?;

    // Report the read phase performance.
    #[cfg(metrics)]
    MASK_READ_PERF
        .with_label_values(&[&name, &namespace, action.into()])
        .observe(start.elapsed().as_secs_f64());

    // Increment the counter for the action.
    #[cfg(metrics)]
    MASK_ACTION_COUNTER
        .with_label_values(&[&name, &namespace, action.into()])
        .inc();

    // Benchmark the write phase of reconciliation.
    #[cfg(metrics)]
    let timer = match action {
        // Don't measure time for NoOp actions.
        MaskAction::NoOp => None,
        // Start a performance timer for the write phase.
        _ => Some(
            MASK_WRITE_PERF
                .with_label_values(&[&name, &namespace, action.into()])
                .start_timer(),
        ),
    };

    if action != MaskAction::NoOp {
        println!("{}/{} ACTION: {:?}", namespace, name, action);
    }

    // Performs action as decided by the `determine_action` function.
    // This is the write phase of reconciliation.
    let result = match action {
        MaskAction::Pending => {
            // Update the phase of the `Provider` resource to Pending.
            actions::pending(client, &instance).await?;

            // Requeue immediately.
            Action::requeue(Duration::ZERO)
        }
        MaskAction::Assign => {
            // Remove any current provider from the Mask's status object.
            // Note: this will not delete the reservation ConfigMap.
            actions::unassign_provider(client.clone(), &name, &namespace, &instance).await?;

            // Add a finalizer so that the Mask resource is not deleted before
            // the reservation ConfigMap and credentials secret are deleted.
            let instance = finalizer::add(client.clone(), &name, &namespace).await?;

            // Assign a new provider to the Mask.
            if !actions::assign_provider(client.clone(), &name, &namespace, &instance).await? {
                // Failed to assign a provider. Wait a bit and retry.
                return Ok(Action::requeue(PROBE_INTERVAL));
            }

            // Requeue immediately to set the phase to "Active".
            Action::requeue(Duration::ZERO)
        }
        MaskAction::Delete => {
            // Delete the reservation ConfigMap from the Provider's namespace.
            actions::delete_reservation(client.clone(), &name, &namespace, &instance).await?;

            // Delete the credentials secret from the Mask's namespace.
            actions::delete_secret(client.clone(), &namespace, &instance).await?;

            // Remove the finalizer, which will allow the Mask resource to be deleted.
            finalizer::delete(client, &name, &namespace).await?;

            // Makes no sense to requeue after deleting, as the resource is gone.
            Action::await_change()
        }
        MaskAction::Active => {
            // Update the phase to Active.
            actions::active(client.clone(), &instance).await?;

            // Resource is fully reconciled.
            Action::requeue(PROBE_INTERVAL)
        }
        MaskAction::CreateSecret => {
            // Create the credentials env secret in the Mask's namespace.
            actions::create_secret(client.clone(), &namespace, &instance).await?;

            // Requeue immediately to set the phase to Active.
            Action::requeue(Duration::ZERO)
        }
        // The resource is already in desired state, do nothing and re-check after 10 seconds
        MaskAction::NoOp => Action::requeue(PROBE_INTERVAL),
    };

    #[cfg(metrics)]
    if let Some(timer) = timer {
        timer.observe_duration();
    }

    Ok(result)
}

/// Returns the phase of the Mask.
pub fn get_mask_phase(instance: &Mask) -> Result<(MaskPhase, Duration), Error> {
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

/// Gets the Secret that contains the credentials for the Mask.
/// Even if the Secret exists, this may still return None if
/// the Secret's provider label doesn't match the expected uid.
async fn get_secret(
    client: Client,
    name: &str,
    namespace: &str,
    provider: &AssignedProvider,
) -> Result<Option<Secret>, Error> {
    let api: Api<Secret> = Api::namespaced(client, namespace);
    // Because the Secret's name includese the uid, we don't
    // have the to check the resource labels for a match.
    let secret_name = format!("{}-{}", name, &provider.uid);
    match api.get(&secret_name).await {
        Ok(secret) => Ok(Some(secret)),
        Err(kube::Error::Api(ae)) if ae.code == 404 => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Determines the action given that the provider has been assigned.
async fn determine_provider_action(
    client: Client,
    name: &str,
    namespace: &str,
    instance: &Mask,
    provider: &AssignedProvider,
) -> Result<Option<MaskAction>, Error> {
    // Ensure that the ConfigMap reserving the connection with the Provider exists.
    // If the ConfigMap no longer exists, we need to immediately remove the
    // current provider from the Mask status and assign a new one.
    if !owns_reservation(client.clone(), name, namespace, instance).await? {
        return Ok(Some(MaskAction::Assign));
    }

    // Ensure the Secret containing the env credentials exists.
    // The Secret should exist in the same namespace as the Mask.
    if get_secret(client, name, namespace, provider)
        .await?
        .is_none()
    {
        // The Mask has reserved a connection with the Provider,
        // but for some reason the secret doesn't exist.
        return Ok(Some(MaskAction::CreateSecret));
    }

    // Both the ConfigMap reserving the connection and the secret
    // containing the env credentials exist, so the Mask is in
    // the desired state.
    Ok(None)
}

/// Resources arrives into reconciliation queue in a certain state. This function looks at
/// the state of given `Mask` resource and decides which actions needs to be performed.
/// The finite set of possible actions is represented by the `MaskAction` enum.
///
/// # Arguments
/// - `mask`: A reference to `Mask` being reconciled to decide next action upon.
async fn determine_action(
    client: Client,
    name: &str,
    namespace: &str,
    instance: &Mask,
) -> Result<MaskAction, Error> {
    if instance.meta().deletion_timestamp.is_some() {
        return Ok(MaskAction::Delete);
    }

    // The rest of the controller code assumes the presence of the
    // status object and its phase field. If neither of these exist,
    // the first thing that should be done is initializing them.
    if needs_pending(instance) {
        return Ok(MaskAction::Pending);
    }

    // Get the assigned provider details from the status.
    let provider = match get_assigned_provider(instance) {
        // Provider has not been assigned yet.
        None => return Ok(MaskAction::Assign),
        // Provider has already been assigned.
        Some(provider) => provider,
    };

    // Determine if we need to take an action given that the Mask
    // resource has been assigned a VPN provider.
    if let Some(action) =
        determine_provider_action(client, name, namespace, instance, provider).await?
    {
        return Ok(action);
    }

    // Keep the Active status up-to-date.
    determine_status_action(instance)
}

/// Determines the action given that the only thing left to do
/// is periodically keeping the Active phase up-to-date.
fn determine_status_action(instance: &Mask) -> Result<MaskAction, Error> {
    // Ensure the phase is Active and recent.
    let (phase, age) = get_mask_phase(instance)?;
    if phase != MaskPhase::Active || age > PROBE_INTERVAL {
        // Keep the Active status up-to-date.
        return Ok(MaskAction::Active);
    }
    // Nothing to do, resource is fully reconciled.
    Ok(MaskAction::NoOp)
}

/// Actions to be taken when a reconciliation fails - for whatever reason.
/// Prints out the error to `stderr` and requeues the resource for another reconciliation after
/// five seconds.
///
/// # Arguments
/// - `instance`: The erroneous resource.
/// - `error`: A reference to the `kube::Error` that occurred during reconciliation.
/// - `_context`: Unused argument. Context Data "injected" automatically by kube-rs.
fn on_error(instance: Arc<Mask>, error: &Error, _context: Arc<ContextData>) -> Action {
    eprintln!("Reconciliation error:\n{:?}.\n{:?}", error, instance);
    Action::requeue(Duration::from_secs(5))
}
