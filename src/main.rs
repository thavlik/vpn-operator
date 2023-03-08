use std::sync::Arc;

use futures::stream::StreamExt;
use kube::Resource;
use kube::ResourceExt;
use kube::{
    api::ListParams, client::Client, runtime::controller::Action, runtime::Controller, Api,
};
use tokio::time::Duration;

use crate::crd::{Provider, Mask};

pub mod crd;

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
enum MaskAction {
    /// Create the subresources, this includes spawning `n` pods with Mask service
    Create,
    /// Delete all subresources created in the `Create` phase
    Delete,
    /// This `Mask` resource is in desired state and requires no actions to be taken
    NoOp,
}

async fn reconcile(mask: Arc<Mask>, context: Arc<ContextData>) -> Result<Action, Error> {
    // The `Client` is shared -> a clone from the reference is obtained
    let client: Client = context.client.clone();

    // The resource of `Mask` kind is required to have a namespace set. However, it is not guaranteed
    // the resource will have a `namespace` set. Therefore, the `namespace` field on object's metadata
    // is optional and Rust forces the programmer to check for it's existence first.
    let namespace: String = match mask.namespace() {
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
    let name = mask.name_any();

    // Performs action as decided by the `determine_action` function.
    match determine_action(&mask) {
        MaskAction::Create => {
            Ok(Action::requeue(Duration::from_secs(10)))
        }
        MaskAction::Delete => {
            Ok(Action::await_change()) // Makes no sense to delete after a successful delete, as the resource is gone
        }
        // The resource is already in desired state, do nothing and re-check after 10 seconds
        MaskAction::NoOp => Ok(Action::requeue(Duration::from_secs(10))),
    }
}

/// Resources arrives into reconciliation queue in a certain state. This function looks at
/// the state of given `Mask` resource and decides which actions needs to be performed.
/// The finite set of possible actions is represented by the `MaskAction` enum.
///
/// # Arguments
/// - `mask`: A reference to `Mask` being reconciled to decide next action upon.
fn determine_action(mask: &Mask) -> MaskAction {
    return if mask.meta().deletion_timestamp.is_some() {
        MaskAction::Delete
    } else if mask
        .meta()
        .finalizers
        .as_ref()
        .map_or(true, |finalizers| finalizers.is_empty())
    {
        MaskAction::Create
    } else {
        MaskAction::NoOp
    };
}

/// Actions to be taken when a reconciliation fails - for whatever reason.
/// Prints out the error to `stderr` and requeues the resource for another reconciliation after
/// five seconds.
///
/// # Arguments
/// - `mask`: The erroneous resource.
/// - `error`: A reference to the `kube::Error` that occurred during reconciliation.
/// - `_context`: Unused argument. Context Data "injected" automatically by kube-rs.
fn on_error(mask: Arc<Mask>, error: &Error, _context: Arc<ContextData>) -> Action {
    eprintln!("Reconciliation error:\n{:?}.\n{:?}", error, mask);
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
}