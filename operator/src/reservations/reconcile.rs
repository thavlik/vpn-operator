use chrono::Utc;
use futures::stream::StreamExt;
use kube::Resource;
use kube::ResourceExt;
use kube::{
    api::ListParams, client::Client, runtime::controller::Action, runtime::Controller, Api,
};
use std::sync::Arc;
use tokio::time::Duration;
use vpn_types::*;

use super::actions;
use crate::util::{
    finalizer::{self, FINALIZER_NAME},
    Error, PROBE_INTERVAL,
};

#[cfg(feature = "metrics")]
use crate::util::metrics::ControllerMetrics;

/// Entrypoint for the `MaskReservation` controller.
pub async fn run(client: Client) -> Result<(), Error> {
    println!("Starting MaskReservation controller...");

    // Preparation of resources used by the `kube_runtime::Controller`
    let crd_api: Api<MaskReservation> = Api::all(client.clone());
    let context: Arc<ContextData> = Arc::new(ContextData::new(client.clone()));

    // The controller comes from the `kube_runtime` crate and manages the reconciliation process.
    // It requires the following information:
    // - `kube::Api<T>` this controller "owns". In this case, `T = MaskReservation`, as this controller owns the `MaskReservation` resource,
    // - `kube::api::ListParams` to select the `MaskReservation` resources with. Can be used for MaskReservation filtering `MaskReservation` resources before reconciliation,
    // - `reconcile` function with reconciliation logic to be called each time a resource of `MaskReservation` kind is created/updated/deleted,
    // - `on_error` function to call whenever reconciliation fails.
    Controller::new(crd_api, ListParams::default())
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
    
    #[cfg(feature = "metrics")]
    metrics: ControllerMetrics,
}

impl ContextData {
    /// Constructs a new instance of ContextData.
    ///
    /// # Arguments:
    /// - `client`: A Kubernetes client to make Kubernetes REST API requests with. Resources
    /// will be created and deleted with this client.
    pub fn new(client: Client) -> Self {
        #[cfg(feature = "metrics")]
        {
            return ContextData {
                client,
                metrics: ControllerMetrics::new("reservations"),
            }
        }
        #[cfg(not(feature = "metrics"))]
        {
            return ContextData { client }
        }
    }
}

/// Action to be taken upon an [`MaskReservation`] resource during reconciliation
#[derive(Debug, PartialEq)]
enum ReservationAction {
    /// Set the [`MaskReservationStatus::phase`] to [`Pending`](MaskReservationPhase::Pending)
    /// and add a finalizer to the resource.
    Pending,

    /// Delete all subresources and the associated [`MaskConsumer`].
    /// If `delete_resource` is true, the [`MaskReservation`] resource will be deleted as well.
    /// This is triggered when the referenced [`MaskConsumer`] is deleted.
    Delete { delete_resource: bool },

    /// Signals that the [`MaskReservation`] belongs to a [`MaskConsumer`] that exists.
    /// This is the desired state of the resource when everything is working as expected.
    Active,

    /// The [`MaskReservation`] resource is in desired state and requires no actions to be taken.
    NoOp,
}

impl ReservationAction {
    fn to_str(&self) -> &str {
        match self {
            ReservationAction::Pending => "Pending",
            ReservationAction::Delete { .. } => "Delete",
            ReservationAction::Active => "Active",
            ReservationAction::NoOp => "NoOp",
        }
    }
}

/// Returns true if the [`MaskReservation`] resource requires a status
/// update to set the phase to `Pending`. This should be the first action
/// for any managed resource.
fn needs_pending(instance: &MaskReservation) -> bool {
    needs_finalizer(instance) || instance.status.as_ref().map_or(true, |s| s.phase.is_none())
}

/// Returns true if the [`MaskReservation`] is missing the finalizer.
fn needs_finalizer(instance: &MaskReservation) -> bool {
    !instance.finalizers().iter().any(|f| f == FINALIZER_NAME)
}

/// Reconciliation function for the [`MaskReservation`] resource.
async fn reconcile(
    instance: Arc<MaskReservation>,
    context: Arc<ContextData>,
) -> Result<Action, Error> {
    // The `Client` is shared -> a clone from the reference is obtained
    let client: Client = context.client.clone();

    // The resource of `MaskReservation` kind is required to have a namespace set. However, it is not guaranteed
    // the resource will have a `namespace` set. Therefore, the `namespace` field on object's metadata
    // is optional and Rust forces the programmer to check for it's existence first.
    let namespace: String = match instance.namespace() {
        None => {
            // If there is no namespace to deploy to defined, reconciliation ends with an error immediately.
            return Err(Error::UserInputError(
                "Expected MaskReservation resource to be namespaced. Can't deploy to an unknown namespace."
                    .to_owned(),
            ));
        }
        // If namespace is known, proceed. In a more advanced version of the operator, perhaps
        // the namespace could be checked for existence first.
        Some(namespace) => namespace,
    };

    // Name of the MaskReservation resource is used to name the subresources as well.
    let name = instance.name_any();

    // Increment total number of reconciles for the MaskReservation resource.
    #[cfg(feature = "metrics")]
    context.metrics.reconcile_counter
        .with_label_values(&[&name, &namespace])
        .inc();

    // Benchmark the read phase of reconciliation.
    #[cfg(feature = "metrics")]
    let start = std::time::Instant::now();

    // Read phase of reconciliation determines goal during the write phase.
    let action = determine_action(client.clone(), &name, &namespace, &instance).await?;

    if action != ReservationAction::NoOp {
        println!("{}/{} ACTION: {:?}", namespace, name, action);
    }

    // Report the read phase performance.
    #[cfg(feature = "metrics")]
    context.metrics.read_histogram
        .with_label_values(&[&name, &namespace, action.to_str()])
        .observe(start.elapsed().as_secs_f64());

    // Increment the counter for the action.
    #[cfg(feature = "metrics")]
    context.metrics.action_counter
        .with_label_values(&[&name, &namespace, action.to_str()])
        .inc();

    // Benchmark the write phase of reconciliation.
    #[cfg(feature = "metrics")]
    let timer = match action {
        // Don't measure performance for NoOp actions.
        ReservationAction::NoOp => None,
        // Start a performance timer for the write phase.
        _ => Some(
            context.metrics.write_histogram
                .with_label_values(&[&name, &namespace, action.to_str()])
                .start_timer(),
        ),
    };

    // Performs action as decided by the `determine_action` function.
    // This is the write phase of reconciliation.
    let result = match action {
        ReservationAction::Pending => {
            // Add the finalizer. This will prevent the reservation from
            // being deleted before the associated MaskConsumer is removed,
            // effectively preventing the slot from being reprovisioned until
            // we know for sure that the connection is severed.
            let instance = finalizer::add(client.clone(), &name, &namespace).await?;

            // Update the phase to Pending.
            actions::pending(client, &instance).await?;

            // Requeue immediately.
            Action::requeue(Duration::ZERO)
        }
        ReservationAction::Delete { delete_resource } => {
            // Show that the reservation is being terminated.
            actions::terminating(client.clone(), &instance).await?;

            // Delete the associated MaskConsumer so the slot isn't reassigned
            // before all Pods using the credentials are truly disconnected.
            let result = if actions::delete_consumer(client.clone(), &instance).await? {
                // Remove the finalizer, which will allow the MaskReservation resource to be deleted.
                finalizer::delete::<MaskReservation>(client.clone(), &name, &namespace).await?;

                // Makes no sense to requeue after deleting, as the resource is gone.
                Action::await_change()
            } else {
                // Still waiting on MaskConsumer to be deleted, keep the finalizer.
                Action::requeue(PROBE_INTERVAL)
            };

            if delete_resource {
                // Delete the MaskReservation resource itself. This will happen when
                // the referenced MaskConsumer is deleted.
                actions::delete(client.clone(), &name, &namespace).await?;
            }

            result
        }
        ReservationAction::Active => {
            // Update the phase to Active, meaning the reservation is in use.
            actions::active(client, &instance).await?;

            // Resource is fully reconciled.
            Action::requeue(PROBE_INTERVAL)
        }
        // The resource is already in desired state, do nothing and re-check after 10 seconds
        ReservationAction::NoOp => Action::requeue(PROBE_INTERVAL),
    };

    #[cfg(feature = "metrics")]
    if let Some(timer) = timer {
        timer.observe_duration();
    }

    Ok(result)
}

/// Returns the phase of the MaskReservation.
pub fn get_reservation_phase(
    instance: &MaskReservation,
) -> Result<(MaskReservationPhase, Duration), Error> {
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
/// the state of given `MaskReservation` resource and decides which actions needs to be performed.
/// The finite set of possible actions is represented by the `ReservationAction` enum.
///
/// # Arguments
/// - `instance`: A reference to `MaskReservation` being reconciled to decide next action upon.
async fn determine_action(
    client: Client,
    _name: &str,
    _namespace: &str,
    instance: &MaskReservation,
) -> Result<ReservationAction, Error> {
    if instance.meta().deletion_timestamp.is_some() {
        return Ok(ReservationAction::Delete {
            delete_resource: false,
        });
    }

    // The rest of the controller code assumes the presence of the
    // status object and its phase field. If neither of these exist,
    // the first thing that should be done is initializing them.
    if needs_pending(instance) {
        return Ok(ReservationAction::Pending);
    }

    if get_consumer(client, instance).await?.is_none() {
        return Ok(ReservationAction::Delete {
            delete_resource: true,
        });
    }

    determine_status_action(instance)
}

/// Returns the `MaskConsumer` referenced by the `MaskReservation`.
async fn get_consumer(
    client: Client,
    instance: &MaskReservation,
) -> Result<Option<MaskConsumer>, Error> {
    let mc_api: Api<MaskConsumer> = Api::namespaced(client, &instance.spec.namespace);
    match mc_api.get(&instance.spec.name).await {
        // Ensure the UID matches so we don't accidentally reference
        // the wrong MaskConsumer.
        Ok(consumer)
            if consumer
                .metadata
                .uid
                .as_deref()
                .map_or(false, |uid| uid == instance.spec.uid) =>
        {
            // UID matches, associated MaskConsumer is still around.
            Ok(Some(consumer))
        }
        // UID doesn't match; MaskConsumer has been deleted.
        Ok(_) => Ok(None),
        // MaskConsumer doesn't exist.
        Err(kube::Error::Api(ae)) if ae.code == 404 => Ok(None),
        // Some other error occurred.
        Err(e) => Err(e.into()),
    }
}

/// Determines the action given that the only thing left to do
/// is periodically keeping the Ready/Active phase up-to-date.
fn determine_status_action(instance: &MaskReservation) -> Result<ReservationAction, Error> {
    let (phase, age) = get_reservation_phase(instance)?;
    if phase != MaskReservationPhase::Active || age > PROBE_INTERVAL {
        Ok(ReservationAction::Active)
    } else {
        Ok(ReservationAction::NoOp)
    }
}

/// Actions to be taken when a reconciliation fails - for whatever reason.
/// Prints out the error to `stderr` and requeues the resource for another reconciliation after
/// five seconds.
///
/// # Arguments
/// - `instance`: The erroneous resource.
/// - `error`: A reference to the `kube::Error` that occurred during reconciliation.
/// - `_context`: Unused argument. Context Data "injected" automatically by kube-rs.
fn on_error(instance: Arc<MaskReservation>, error: &Error, _context: Arc<ContextData>) -> Action {
    eprintln!("Reconciliation error:\n{:?}.\n{:?}", error, instance);
    Action::requeue(Duration::from_secs(5))
}
