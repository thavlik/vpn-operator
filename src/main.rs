use std::sync::Arc;
use futures::stream::StreamExt;
use k8s_openapi::api::core::v1::Secret;
use kube::Resource;
use kube::ResourceExt;
use kube::{
    api::ListParams, client::Client, runtime::controller::Action, runtime::Controller, Api,
};
use tokio::time::Duration;
pub use vpn_types::*;

mod actions;
mod finalizer;

#[cfg(test)]
mod test;

use actions::owns_reservation;

#[tokio::main]
async fn main() {
    // First, a Kubernetes client must be obtained using the `kube` crate
    // The client will later be moved to the custom controller
    let kubernetes_client: Client = Client::try_default()
        .await
        .expect("Expected a valid KUBECONFIG environment variable.");

    // Preparation of resources used by the `kube_runtime::Controller`
    let crd_api: Api<Mask> = Api::all(kubernetes_client.clone());
    let context: Arc<ContextData> = Arc::new(ContextData::new(kubernetes_client.clone()));

    // The controller comes from the `kube_runtime` crate and manages the reconciliation process.
    // It requires the following information:
    // - `kube::Api<T>` this controller "owns". In this case, `T = Mask`, as this controller owns the `Mask` resource,
    // - `kube::api::ListParams` to select the `Mask` resources with. Can be used for Mask filtering `Mask` resources before reconciliation,
    // - `reconcile` function with reconciliation logic to be called each time a resource of `Mask` kind is created/updated/deleted,
    // - `on_error` function to call whenever reconciliation fails.
    Controller::new(crd_api.clone(), ListParams::default())
        .run(reconcile, on_error, context)
        .for_each(|reconciliation_result| async move {
            match reconciliation_result {
                Ok(mask_resource) => {
                    println!("Reconciliation successful. Resource: {:?}", mask_resource);
                }
                Err(reconciliation_err) => {
                    eprintln!("Reconciliation error: {:?}", reconciliation_err)
                }
            }
        })
        .await;
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
    /// Assign the Mask a provider.
    Assign,
    /// Delete all subresources.
    Delete,
    /// Create the credentials secret for the Mask.
    CreateSecret,
    ///
    SetActive,
    /// This `Mask` resource is in desired state and requires no actions to be taken
    NoOp,
}

fn get_assigned_provider(instance: &Mask) -> Option<&AssignedProvider> {
    match instance.status {
        None => None,
        Some(ref status) => status.provider.as_ref(),
    }
}

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

    let action = determine_action(client.clone(), &name, &namespace, &instance).await?;

    if action != MaskAction::NoOp {
        println!("{}/{} ACTION: {:?}", namespace, name, action);
    }

    // Performs action as decided by the `determine_action` function.
    match action {
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
                return Ok(Action::requeue(Duration::from_secs(3)));
            }

            // Requeue immediately to set the phase to "Active".
            Ok(Action::requeue(Duration::ZERO))
        }
        MaskAction::Delete => {
            // Delete the reservation ConfigMap from the Provider's namespace.
            actions::delete_reservation(client.clone(), &name, &namespace, &instance).await?;

            // Delete the credentials secret from the Mask's namespace.
            actions::delete_secret(client.clone(), &namespace, &instance).await?;

            // Remove the finalizer, which will allow the Mask resource to be deleted.
            finalizer::delete(client, &name, &namespace).await?;

            // Makes no sense to requeue after deleting, as the resource is gone.
            Ok(Action::await_change())
        }
        MaskAction::SetActive => {
            // Update the phase to Active.
            actions::set_active(client.clone(), &name, &namespace, &instance).await?;

            // Resource is fully reconciled. Requeue after a short delay.
            Ok(Action::requeue(Duration::from_secs(10)))
        }
        MaskAction::CreateSecret => {
            // Create the credentials env secret in the Mask's namespace.
            actions::create_secret(client.clone(), &name, &namespace, &instance).await?;

            // Requeue immediately to set the phase to Active.
            Ok(Action::requeue(Duration::ZERO))
        }
        // The resource is already in desired state, do nothing and re-check after 10 seconds
        MaskAction::NoOp => Ok(Action::requeue(Duration::from_secs(10))),
    }
}

/// Returns the phase of the Mask.
pub fn get_mask_phase(instance: &Mask) -> Result<MaskPhase, Error> {
    Ok(instance
        .status
        .as_ref()
        .ok_or_else(|| Error::UserInputError("No status".to_string()))?
        .phase
        .ok_or_else(|| Error::UserInputError("No phase".to_string()))?)
}

/// Gets the secret that contains the credentials for the Mask.
async fn get_secret(
    client: Client,
    name: &str,
    namespace: &str,
    provider: &AssignedProvider,
) -> Result<Option<Secret>, Error> {
    let api: Api<Secret> = Api::namespaced(client, namespace);
    let secret_name = format!("{}-{}", name, &provider.name);
    match api.get(&secret_name).await {
        Ok(pod) => Ok(Some(pod)),
        Err(e) => match &e {
            kube::Error::Api(ae) => match ae.code {
                // If the resource does not exist, return None
                404 => Ok(None),
                // If the resource exists but we can't access it, return an error
                _ => Err(e.into()),
            },
            _ => Err(e.into()),
        },
    }
}

/// Determines the action given that the provider has been assigned.
async fn determine_provider_action(
    client: Client,
    name: &str,
    namespace: &str,
    instance: &Mask,
    provider: &AssignedProvider,
) -> Result<MaskAction, Error> {
    // Ensure that the ConfigMap reserving the connection with the Provider exists.
    // If the ConfigMap no longer exists, we need to immediately remove the
    // existing provider from the Mask status and assign a new one.
    if !owns_reservation(client.clone(), name, namespace, instance).await? {
        // Reassign the provider. For whatever reason, the ConfigMap
        // reserving the connection with the Provider no longer exists.
        return Ok(MaskAction::Assign);
    }

    // Ensure the Secret containing the env credentials exists.
    if get_secret(client, name, namespace, provider)
        .await?
        .is_none()
    {
        // The Mask has reserved a connection with the Provider,
        // but for some reason the secret doesn't exist.
        return Ok(MaskAction::CreateSecret);
    }

    match get_mask_phase(instance)? {
        // Both the ConfigMap reserving the connection and the secret
        // containing the env credentials exist, so the Mask is in
        // the desired state.
        MaskPhase::Active => Ok(MaskAction::NoOp),
        // Set the phase to Active now that everything is reconciled.
        _ => Ok(MaskAction::SetActive),
    }
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

    // Get the assigned provider details from the status.
    let provider = match get_assigned_provider(instance) {
        // Provider has not been assigned yet.
        None => return Ok(MaskAction::Assign),
        // Provider has already been assigned.
        Some(provider) => provider,
    };

    determine_provider_action(client, name, namespace, instance, provider).await
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

/// All errors possible to occur during reconciliation
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Any error originating from the `kube-rs` crate
    #[error("Kubernetes reported error: {source}")]
    KubeError {
        #[from]
        source: kube::Error,
    },
    /// Error in user input or Mask resource definition, typically missing fields.
    #[error("Invalid Mask CRD: {0}")]
    UserInputError(String),
    /// Invalid value in the status.phase field.
    #[error("Invalid Mask phase: {0}")]
    InvalidPhase(String),
}
