use chrono::Utc;
use futures::stream::StreamExt;
use kube::{
    api::ListParams, client::Client, runtime::controller::Action, runtime::Controller, Api,
    ResourceExt,
};
use std::sync::Arc;
use tokio::time::Duration;
use vpn_types::*;

use super::{actions, finalizer, util::get_consumer};
use crate::util::{Error, FINALIZER_NAME, PROBE_INTERVAL};

#[cfg(feature = "metrics")]
use super::metrics;

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
        .owns(Api::<MaskConsumer>::all(client), ListParams::default())
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

    /// Create a MaskConsumer to manage the provider assignment.
    CreateConsumer,

    /// Delete all subresources.
    Delete,

    /// Signals that the MaskConsumer is Waiting.
    Waiting,

    /// Signals that the Mask is actively consuming VPN credentials.
    Active,

    /// Signals that the MaskConsumer was unable to be assigned a provider.
    ErrNoProviders,

    /// The Mask resource is in desired state and requires no actions to be taken.
    NoOp,
}

impl MaskAction {
    fn to_str(&self) -> &str {
        match self {
            MaskAction::Pending => "Pending",
            MaskAction::CreateConsumer => "CreateConsumer",
            MaskAction::Delete => "Delete",
            MaskAction::Waiting => "Waiting",
            MaskAction::Active => "Active",
            MaskAction::ErrNoProviders => "ErrNoProviders",
            MaskAction::NoOp => "NoOp",
        }
    }
}

/// Returns true if the MaskConsumer is missing the finalizer.
fn needs_finalizer(instance: &Mask) -> bool {
    !instance.finalizers().iter().any(|f| f == FINALIZER_NAME)
}

/// needs_pending returns true if the `Mask` resource
/// requires a status update to set the phase to Pending.
/// This should be the first action for any managed resource.
fn needs_pending(instance: &Mask) -> bool {
    needs_finalizer(instance) || instance.status.as_ref().map_or(true, |s| s.phase.is_none())
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
    #[cfg(feature = "metrics")]
    metrics::MASK_RECONCILE_COUNTER
        .with_label_values(&[&name, &namespace])
        .inc();

    // Benchmark the read phase of reconciliation.
    #[cfg(feature = "metrics")]
    let start = std::time::Instant::now();

    // Read phase of reconciliation determines goal during the write phase.
    let action = determine_action(client.clone(), &name, &namespace, &instance).await?;

    if action != MaskAction::NoOp {
        println!("{}/{} ACTION: {:?}", namespace, name, action);
    }

    // Report the read phase performance.
    #[cfg(feature = "metrics")]
    metrics::MASK_READ_HISTOGRAM
        .with_label_values(&[&name, &namespace, action.to_str()])
        .observe(start.elapsed().as_secs_f64());

    // Increment the counter for the action.
    #[cfg(feature = "metrics")]
    metrics::MASK_ACTION_COUNTER
        .with_label_values(&[&name, &namespace, action.to_str()])
        .inc();

    // Benchmark the write phase of reconciliation.
    #[cfg(feature = "metrics")]
    let timer = match action {
        // Don't measure performance for NoOp actions.
        MaskAction::NoOp => None,
        // Start a performance timer for the write phase.
        _ => Some(
            metrics::MASK_WRITE_HISTOGRAM
                .with_label_values(&[&name, &namespace, action.to_str()])
                .start_timer(),
        ),
    };

    // Performs action as decided by the `determine_action` function.
    // This is the write phase of reconciliation.
    let result = match action {
        MaskAction::Pending => {
            // Add the finalizer to the Mask resource.
            let instance = finalizer::add(client.clone(), &name, &namespace).await?;

            // Update the phase of the `Mask` resource to Pending.
            actions::pending(client, &instance).await?;

            // Requeue immediately.
            Action::requeue(Duration::ZERO)
        }
        MaskAction::Delete => {
            // Note: we don't need to manually delete the MaskConsumer resource.
            // Kubernetes will delete it automatically because of the owner reference.

            // Remove the finalizer, which will allow the Mask resource to be deleted.
            finalizer::delete(client, &name, &namespace).await?;

            // Makes no sense to requeue after deleting, as the resource is gone.
            Action::await_change()
        }
        MaskAction::Waiting => {
            // Update the phase to Waiting.
            actions::waiting(client, &instance).await?;

            // Try again after a short delay.
            Action::requeue(PROBE_INTERVAL)
        }
        MaskAction::Active => {
            // Update the phase to Active.
            actions::active(client, &instance).await?;

            // Resource is fully reconciled.
            Action::requeue(PROBE_INTERVAL)
        }
        MaskAction::CreateConsumer => {
            // Immediately update the phase to Waiting.
            actions::waiting(client.clone(), &instance).await?;

            // Create the MaskConsumer object that will manage provider assignment.
            actions::create_consumer(client, &name, &namespace, &instance).await?;

            // Requeue after a short delay to give the MaskConsumer time to reconcile.
            Action::requeue(PROBE_INTERVAL)
        }
        MaskAction::ErrNoProviders => {
            // Reflect the error in the status object.
            actions::err_no_providers(client, &instance).await?;

            // Requeue after a short delay to allow time for a valid MaskProvider to appear.
            Action::requeue(PROBE_INTERVAL)
        }
        // The resource is already in desired state, do nothing and re-check after 10 seconds
        MaskAction::NoOp => Action::requeue(PROBE_INTERVAL),
    };

    #[cfg(feature = "metrics")]
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

/// Resources arrives into reconciliation queue in a certain state. This function looks at
/// the state of given `Mask` resource and decides which actions needs to be performed.
/// The finite set of possible actions is represented by the `MaskAction` enum.
///
/// # Arguments
/// - `instance`: A reference to `Mask` being reconciled to decide next action upon.
async fn determine_action(
    client: Client,
    _name: &str,
    _namespace: &str,
    instance: &Mask,
) -> Result<MaskAction, Error> {
    if instance.metadata.deletion_timestamp.is_some() {
        return Ok(MaskAction::Delete);
    }

    // The rest of the controller code assumes the presence of the
    // status object and its phase field. If neither of these exist,
    // the first thing that should be done is initializing them.
    if needs_pending(instance) {
        return Ok(MaskAction::Pending);
    }

    // Get the child MaskConsumer resource that will manage provider
    // assignment and be deleted whenever the provider is unassigned.
    let consumer = match get_consumer(client.clone(), instance).await? {
        // MaskConsumer has not been created yet.
        None => return Ok(MaskAction::CreateConsumer),
        // MaskConsumer has already been created.
        Some(consumer) => consumer,
    };

    // Keep the status object synchronized with the MaskConsumer's status.
    determine_status_action(instance, &consumer)
}

/// Helper function used to run an action if the phase of the `Mask`
/// doesn't match the desired value or if the status object is stale.
fn recent_status(instance: &Mask, phase: MaskPhase, action: MaskAction) -> MaskAction {
    let (cur_phase, age) = get_mask_phase(instance).unwrap();
    if cur_phase != phase || age > PROBE_INTERVAL {
        action
    } else {
        MaskAction::NoOp
    }
}

/// Determines the action given that the only thing left to do
/// is periodically keeping the phase in sync with the consumer.
fn determine_status_action(instance: &Mask, consumer: &MaskConsumer) -> Result<MaskAction, Error> {
    Ok(consumer
        .status
        .as_ref()
        .map_or(None, |s| s.phase)
        .map(|p| match p {
            // Inherit Pending, Waiting, and Terminating phases as Waiting.
            MaskConsumerPhase::Pending
            | MaskConsumerPhase::Waiting
            | MaskConsumerPhase::Terminating => {
                recent_status(instance, MaskPhase::Waiting, MaskAction::Waiting)
            }
            // Inherit the Active phase at a regular interval.
            MaskConsumerPhase::Active => {
                recent_status(instance, MaskPhase::Active, MaskAction::Active)
            }
            // No providers error, use the ErrNoProviders phase.
            MaskConsumerPhase::ErrNoProviders => recent_status(
                instance,
                MaskPhase::ErrNoProviders,
                MaskAction::ErrNoProviders,
            ),
        })
        // If the MaskConsumer has no phase, do nothing.
        .unwrap_or(MaskAction::NoOp))
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
