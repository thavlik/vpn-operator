use chrono::Utc;
use futures::stream::StreamExt;
use k8s_openapi::api::core::v1::{ConfigMap, Secret};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;
use kube::{
    api::ListParams, client::Client, runtime::controller::Action, runtime::Controller, Api,
    Resource, ResourceExt,
};
use std::sync::Arc;
use tokio::time::Duration;

#[cfg(metrics)]
use crate::metrics::{PROVIDER_ACTION_COUNTER, PROVIDER_RECONCILE_COUNTER};

use super::{actions, finalizer};
use crate::util::{Error, FINALIZER_NAME, PROBE_INTERVAL};
pub use vpn_types::*;

/// Entrypoint for the `Provider` controller.
pub async fn run(client: Client) -> Result<(), Error> {
    println!("Starting Provider controller...");

    // Preparation of resources used by the `kube_runtime::Controller`
    let crd_api: Api<Provider> = Api::all(client.clone());
    let context: Arc<ContextData> = Arc::new(ContextData::new(client.clone()));

    // The controller comes from the `kube_runtime` crate and manages the reconciliation process.
    // It requires the following information:
    // - `kube::Api<T>` this controller "owns". In this case, `T = Provider`, as this controller owns the `Provider` resource,
    // - `kube::api::ListParams` to select the `Provider` resources with. Can be used for Provider filtering `Provider` resources before reconciliation,
    // - `reconcile` function with reconciliation logic to be called each time a resource of `Provider` kind is created/updated/deleted,
    // - `on_error` function to call whenever reconciliation fails.
    Controller::new(crd_api, ListParams::default())
        .owns(Api::<ConfigMap>::all(client), ListParams::default())
        .run(reconcile, on_error, context)
        .for_each(|reconciliation_result| async move {
            match reconciliation_result {
                Ok(_provider_resource) => {
                    //println!(
                    //    "Reconciliation successful. Resource: {:?}",
                    //    provider_resource
                    //);
                }
                Err(reconciliation_err) => {
                    eprintln!("Reconciliation error: {:?}", reconciliation_err)
                }
            }
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

/// Action to be taken upon an `Provider` resource during reconciliation
#[derive(Debug, PartialEq)]
enum ProviderAction {
    /// Set the `Provider` resource status.phase to Pending.
    Pending,

    /// Adds the finalizer to the `Provider` resource.
    AddFinalizer,

    /// Cleans up all subresources across all namespaces.
    Delete,

    /// Set the `Provider` resource status.phase to ErrSecretNotFound.
    SecretNotFound(String),

    /// Create a glueten pod and verify that the external IP changes.
    Verify,

    /// Set the status to Verifying.
    Verifying { start_time: Option<Time> },

    /// Set the status to Verified.
    Verified { end_time: Time },

    /// Set the status to ErrVerifyFailed.
    VerifyFailed(String),

    /// Set the `Provider` resource status.phase to Active.
    Active { active_slots: usize },

    /// This `Provider` resource is in desired state and requires no actions to be taken
    NoOp,
}

/// Reconciliation function for the `Provider` resource.
async fn reconcile(instance: Arc<Provider>, context: Arc<ContextData>) -> Result<Action, Error> {
    // The `Client` is shared -> a clone from the reference is obtained
    let client: Client = context.client.clone();

    // The resource of `Provider` kind is required to have a namespace set. However, it is not guaranteed
    // the resource will have a `namespace` set. Therefore, the `namespace` field on object's metadata
    // is optional and Rust forces the programmer to check for it's existence first.
    let namespace: String = match instance.namespace() {
        None => {
            // If there is no namespace to deploy to defined, reconciliation ends with an error immediately.
            return Err(Error::UserInputError(
                "Expected Provider resource to be namespaced. Can't deploy to an unknown namespace."
                    .to_owned(),
            ));
        }
        // If namespace is known, proceed. In a more advanced version of the operator, perhaps
        // the namespace could be checked for existence first.
        Some(namespace) => namespace,
    };

    // Name of the Provider resource is used to name the subresources as well.
    let name = instance.name_any();

    #[cfg(metrics)]
    PROVIDER_RECONCILE_COUNTER
        .with_label_values(&[&name, &namespace])
        .inc();

    // Read phase of reconciliation determines goal during the write phase.
    let action = determine_action(client.clone(), &name, &namespace, &instance).await?;

    if action != ProviderAction::NoOp {
        println!("{}/{} ACTION: {:?}", namespace, name, action);
    }

    #[cfg(metrics)]
    PROVIDER_ACTION_COUNTER
        .with_label_values(&[&name, &namespace, action.into()])
        .inc();

    // Performs action as decided by the `determine_action` function.
    // This is the write phase of reconciliation.
    match action {
        ProviderAction::Pending => {
            // Give the `Provider` resource a finalizer.
            let instance = finalizer::add(client.clone(), &name, &namespace).await?;

            // Update the phase of the `Provider` resource to Pending.
            actions::pending(client, &instance).await?;

            // Requeue immediately.
            Ok(Action::requeue(Duration::ZERO))
        }
        ProviderAction::AddFinalizer => {
            // Ensure the finalizer is present on the `Provider` resource.
            finalizer::add(client, &name, &namespace).await?;

            // Requeue immediately.
            Ok(Action::requeue(Duration::ZERO))
        }
        ProviderAction::Delete => {
            // Delete the verification pod.
            actions::delete_verify_pod(client.clone(), &name, &namespace).await?;

            // Delete Secrets in namespaces that use this `Provider`.
            // This will prevent `Masks` from continuing to use the credentials
            // assigned to them by this `Provider`.
            actions::unassign_all(client.clone(), &name, &namespace, &instance).await?;

            // Remove the finalizer, which will allow the Provider resource to be deleted.
            finalizer::delete(client, &name, &namespace).await?;

            // No need to requeue as the resource is being deleted.
            Ok(Action::await_change())
        }
        ProviderAction::SecretNotFound(secret_name) => {
            // Reflect the error in the status object.
            actions::secret_missing(client, &instance, &secret_name).await?;

            // Requeue after a while if the resource doesn't change.
            Ok(Action::requeue(PROBE_INTERVAL))
        }
        ProviderAction::Verify => {
            // Ensure the finalizer is present on the `Provider` resource.
            finalizer::add(client.clone(), &name, &namespace).await?;

            // Create the verification pod.
            actions::create_verify_pod(client.clone(), &name, &namespace, &instance).await?;

            // Indicate that verification is in progress.
            actions::verify_progress(client, &name, &namespace, None).await?;

            // Requeue after a short delay to allow the verification time to complete.
            Ok(Action::requeue(PROBE_INTERVAL))
        }
        ProviderAction::Verifying { start_time } => {
            // Post the progress to the status object.
            actions::verify_progress(client, &name, &namespace, start_time).await?;

            // Requeue after a short delay to allow the verification time to complete.
            Ok(Action::requeue(PROBE_INTERVAL))
        }
        ProviderAction::VerifyFailed(message) => {
            // Update the phase of the `Provider` resource to Verified.
            actions::verify_failed(client.clone(), &name, &namespace, message).await?;

            // Delete the verification pod so it can be recreated.
            actions::delete_verify_pod(client, &name, &namespace).await?;

            // Requeue after a delay so the user has time to see the error phase.
            Ok(Action::requeue(PROBE_INTERVAL))
        }
        ProviderAction::Verified { end_time } => {
            // Update the phase of the `Provider` resource to Verified.
            actions::verified(client.clone(), &name, &namespace, end_time).await?;

            // Delete the verification pod.
            actions::delete_verify_pod(client, &name, &namespace).await?;

            // Requeue after a short delay.
            Ok(Action::requeue(PROBE_INTERVAL))
        }
        ProviderAction::Active { active_slots } => {
            // Update the phase of the `Provider` resource to Active.
            actions::active(client, &instance, active_slots).await?;

            // Requeue after a short delay.
            Ok(Action::requeue(PROBE_INTERVAL))
        }
        // The resource is already in desired state, do nothing and re-check after 10 seconds
        ProviderAction::NoOp => Ok(Action::requeue(PROBE_INTERVAL)),
    }
}

/// needs_pending returns true if the `Provider` resource
/// requires a status update to set the phase to Pending.
/// This should be the first action for any managed resource.
fn needs_pending(instance: &Provider) -> bool {
    instance.status.is_none() || instance.status.as_ref().unwrap().phase.is_none()
}

/// Returns the phase of the Provider.
pub fn get_provider_phase(instance: &Provider) -> Result<(ProviderPhase, Duration), Error> {
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

/// Gets the secret that contains the credentials for the Provider.
async fn get_secret(
    client: Client,
    namespace: &str,
    provider: &Provider,
) -> Result<Option<Secret>, Error> {
    let api: Api<Secret> = Api::namespaced(client, namespace);
    match api.get(&provider.spec.secret).await {
        Ok(secret) => Ok(Some(secret)),
        Err(kube::Error::Api(ae)) if ae.code == 404 => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Returns true if the Provider is missing the finalizer.
fn needs_finalizer(instance: &Provider) -> bool {
    !instance.finalizers().iter().any(|f| f == FINALIZER_NAME)
}

/// Resources arrives into reconciliation queue in a certain state. This function looks at
/// the state of given `Provider` resource and decides which actions needs to be performed.
/// The finite set of possible actions is represented by the `ProviderAction` enum.
///
/// # Arguments
/// - `provider`: A reference to `Provider` being reconciled to decide next action upon.
async fn determine_action(
    client: Client,
    _name: &str,
    namespace: &str,
    instance: &Provider,
) -> Result<ProviderAction, Error> {
    if instance.meta().deletion_timestamp.is_some() {
        return Ok(ProviderAction::Delete);
    }

    // Ensure that the resource has a status object with a phase.
    // The rest of the controller code relies on the presence
    // of both these fields and will panic if they are not present.
    if needs_pending(instance) {
        // This should be the first action for any freshly created
        // Provider resources. It will be immediately requeued.
        return Ok(ProviderAction::Pending);
    }

    // Ensure the resource has a finalizer so child resources
    // in other namespaces can be cleaned up before deletion.
    if needs_finalizer(instance) {
        return Ok(ProviderAction::AddFinalizer);
    }

    // Ensure the Provider credentials secret exists.
    if get_secret(client.clone(), namespace, instance)
        .await?
        .is_none()
    {
        // The resource specifies using a Secret that doesn't exist.
        // This is the only error state for the Provider resource.
        return Ok(ProviderAction::SecretNotFound(instance.spec.secret.clone()));
    }

    // Remaining actions aim to keep the status object current.
    determine_status_action(client, namespace, instance).await
}

async fn count_reservations(
    client: Client,
    namespace: &str,
    instance: &Provider,
) -> Result<usize, Error> {
    // Only count reservations that belong to this specific Provider.
    // Filtering this way excludes reservations from deleted resources
    // that were immediately recreated.
    let uid = instance.metadata.uid.as_deref().unwrap();

    // List ConfigMaps owned by the Provider.
    let api: Api<ConfigMap> = Api::namespaced(client, namespace);
    let items = api
        .list(&ListParams::default())
        .await?
        .items
        .into_iter()
        .filter(|cm| {
            cm.metadata
                .owner_references
                .as_ref()
                .map_or(false, |ors| ors.iter().any(|or| or.uid == uid))
        })
        .collect::<Vec<_>>();

    // Count the ConfigMaps with the Provider as the owner.
    let active_slots = items.len();
    if active_slots > instance.spec.max_slots {
        // TODO: prune from the Provider controller.
        // Clamp the value at the true client maximum.
        // Max clients was probably decreased in the spec.
        return Ok(instance.spec.max_slots);
    }
    Ok(active_slots)
}

/// Determines the action given that the only thing left to do
/// is periodically keeping the Active phase up-to-date.
async fn determine_status_action(
    client: Client,
    namespace: &str,
    instance: &Provider,
) -> Result<ProviderAction, Error> {
    let (phase, age) = get_provider_phase(instance)?;
    if phase != ProviderPhase::Active || age > PROBE_INTERVAL {
        // Count the ConfigMaps with the Provider as the owner.
        let active_slots = count_reservations(client, namespace, instance).await?;
        // Keep the Active status up to date.
        return Ok(ProviderAction::Active { active_slots });
    }
    // Nothing to do, resource is fully reconciled.
    Ok(ProviderAction::NoOp)
}

/// Actions to be taken when a reconciliation fails - for whatever reason.
/// Prints out the error to `stderr` and requeues the resource for another reconciliation after
/// five seconds.
///
/// # Arguments
/// - `instance`: The erroneous resource.
/// - `error`: A reference to the `kube::Error` that occurred during reconciliation.
/// - `_context`: Unused argument. Context Data "injected" automatically by kube-rs.
fn on_error(instance: Arc<Provider>, error: &Error, _context: Arc<ContextData>) -> Action {
    eprintln!("Reconciliation error:\n{:?}.\n{:?}", error, instance);
    Action::requeue(Duration::from_secs(5))
}
