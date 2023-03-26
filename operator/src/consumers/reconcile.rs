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
use vpn_types::*;

use super::actions;
use crate::util::{
    finalizer::{self, FINALIZER_NAME},
    Error, PROBE_INTERVAL,
};

#[cfg(feature = "metrics")]
use crate::util::metrics::ControllerMetrics;

/// Entrypoint for the `MaskConsumer` controller.
pub async fn run(client: Client) -> Result<(), Error> {
    println!("Starting MaskConsumer controller...");

    // Preparation of resources used by the `kube_runtime::Controller`
    let crd_api: Api<MaskConsumer> = Api::all(client.clone());
    let context: Arc<ContextData> = Arc::new(ContextData::new(client.clone()));

    // The controller comes from the `kube_runtime` crate and manages the reconciliation process.
    // It requires the following information:
    // - `kube::Api<T>` this controller "owns". In this case, `T = MaskConsumer`, as this controller owns the `MaskConsumer` resource,
    // - `kube::api::ListParams` to select the `MaskConsumer` resources with. Can be used for MaskConsumer filtering `MaskConsumer` resources before reconciliation,
    // - `reconcile` function with reconciliation logic to be called each time a resource of `MaskConsumer` kind is created/updated/deleted,
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
                metrics: ControllerMetrics::new("consumers"),
            };
        }
        #[cfg(not(feature = "metrics"))]
        {
            return ContextData { client };
        }
    }
}

/// Action to be taken upon an `MaskConsumer` resource during reconciliation
#[derive(Debug, PartialEq)]
enum ConsumerAction {
    /// Set the [`MaskConsumer`]'s phase to [`Pending`](MaskConsumerPhase::Pending)
    /// and add the finalizer to ensure proper garbage collection.
    Pending,

    /// Delete all subresources and remove finalizer only when all subresources are deleted.
    /// If `delete_resource` is true, the [`MaskConsumer`] resource will be deleted as well.
    Delete { delete_resource: bool },

    /// Attempt to assign the [`MaskConsumer`] a [`MaskProvider`].
    Assign,

    /// Create the credentials [`Secret`](k8s_openapi::api::core::v1::Secret) for the [`MaskConsumer`].
    CreateSecret,

    /// Signals that the [`MaskConsumer`] is fully reconciled.
    Active,

    /// The [`MaskConsumer`] resource is in desired state and requires no actions to be taken.
    NoOp,
}

impl ConsumerAction {
    fn to_str(&self) -> &str {
        match self {
            ConsumerAction::Pending => "Pending",
            ConsumerAction::Delete { .. } => "Delete",
            ConsumerAction::Assign => "Assign",
            ConsumerAction::CreateSecret => "CreateSecret",
            ConsumerAction::Active => "Active",
            ConsumerAction::NoOp => "NoOp",
        }
    }
}

/// Returns true if the MaskConsumer is missing the finalizer.
fn needs_finalizer(instance: &MaskConsumer) -> bool {
    !instance.finalizers().iter().any(|f| f == FINALIZER_NAME)
}

/// Returns true if the `MaskConsumer` resource requires a status
/// update to set the phase to Pending. This should be the first action
/// for any managed resource.
fn needs_pending(instance: &MaskConsumer) -> bool {
    needs_finalizer(instance) || instance.status.as_ref().map_or(true, |s| s.phase.is_none())
}

/// Reconciliation function for the `MaskConsumer` resource.
async fn reconcile(
    instance: Arc<MaskConsumer>,
    context: Arc<ContextData>,
) -> Result<Action, Error> {
    // The `Client` is shared -> a clone from the reference is obtained
    let client: Client = context.client.clone();

    // The resource of `MaskConsumer` kind is required to have a namespace set. However, it is not guaranteed
    // the resource will have a `namespace` set. Therefore, the `namespace` field on object's metadata
    // is optional and Rust forces the programmer to check for it's existence first.
    let namespace: String = match instance.namespace() {
        None => {
            // If there is no namespace to deploy to defined, reconciliation ends with an error immediately.
            return Err(Error::UserInputError(
                "Expected MaskConsumer resource to be namespaced. Can't deploy to an unknown namespace."
                    .to_owned(),
            ));
        }
        // If namespace is known, proceed. In a more advanced version of the operator, perhaps
        // the namespace could be checked for existence first.
        Some(namespace) => namespace,
    };

    // Name of the MaskConsumer resource is used to name the subresources as well.
    let name = instance.name_any();

    // Increment total number of reconciles for the MaskConsumer resource.
    #[cfg(feature = "metrics")]
    context
        .metrics
        .reconcile_counter
        .with_label_values(&[&name, &namespace])
        .inc();

    // Benchmark the read phase of reconciliation.
    #[cfg(feature = "metrics")]
    let start = std::time::Instant::now();

    // Read phase of reconciliation determines goal during the write phase.
    let action = determine_action(client.clone(), &name, &namespace, &instance).await?;

    if action != ConsumerAction::NoOp {
        println!("{}/{} ACTION: {:?}", namespace, name, action);
    }

    // Report the read phase performance.
    #[cfg(feature = "metrics")]
    context
        .metrics
        .read_histogram
        .with_label_values(&[&name, &namespace, action.to_str()])
        .observe(start.elapsed().as_secs_f64());

    // Increment the counter for the action.
    #[cfg(feature = "metrics")]
    context
        .metrics
        .action_counter
        .with_label_values(&[&name, &namespace, action.to_str()])
        .inc();

    // Benchmark the write phase of reconciliation.
    #[cfg(feature = "metrics")]
    let timer = match action {
        // Don't measure performance for NoOp actions.
        ConsumerAction::NoOp => None,
        // Start a performance timer for the write phase.
        _ => Some(
            context
                .metrics
                .write_histogram
                .with_label_values(&[&name, &namespace, action.to_str()])
                .start_timer(),
        ),
    };

    // Performs action as decided by the `determine_action` function.
    // This is the write phase of reconciliation.
    let result = match action {
        ConsumerAction::Pending => {
            // Add a finalizer so the resource can be properly garbage collected.
            let instance = finalizer::add(client.clone(), &name, &namespace).await?;

            // Update the phase to Pending.
            actions::pending(client, &instance).await?;

            // Requeue immediately.
            Action::requeue(Duration::ZERO)
        }
        ConsumerAction::Delete { delete_resource } => {
            // Show that the reservation is being terminated.
            actions::terminating(client.clone(), &instance).await?;

            // Remove the finalizer from the MaskConsumer resource.
            finalizer::delete::<MaskConsumer>(client.clone(), &name, &namespace).await?;

            if delete_resource {
                // Delete the `MaskConsumer` resource itself. This will be
                // triggered whenever the MaskReservation that reserves a slot
                // with the provider could not be found.
                actions::delete(client, &name, &namespace).await?;
            }

            // Child resources will be deleted by kubernetes.
            Action::await_change()
        }
        ConsumerAction::Assign => {
            // Assign a new provider to the MaskConsumer.
            if !actions::assign_provider(client.clone(), &name, &namespace, &instance).await? {
                // Failed to assign a provider. Wait a bit and retry.
                return Ok(Action::requeue(PROBE_INTERVAL));
            }

            // Requeue immediately to set the phase to "Active".
            Action::requeue(Duration::ZERO)
        }
        ConsumerAction::CreateSecret => {
            // Create the credentials env secret in the MaskConsumer's namespace.
            actions::create_secret(client.clone(), &namespace, &instance).await?;

            // Requeue immediately to set the phase to Active.
            Action::requeue(Duration::ZERO)
        }
        ConsumerAction::Active => {
            // Update the phase to Active, meaning the reservation is in use.
            actions::active(client, &instance).await?;

            // Resource is fully reconciled.
            Action::requeue(PROBE_INTERVAL)
        }
        // The resource is already in desired state, do nothing and re-check after 10 seconds
        ConsumerAction::NoOp => Action::requeue(PROBE_INTERVAL),
    };

    #[cfg(feature = "metrics")]
    if let Some(timer) = timer {
        timer.observe_duration();
    }

    Ok(result)
}

/// Returns the phase of the MaskConsumer.
pub fn get_consumer_phase(instance: &MaskConsumer) -> Result<(MaskConsumerPhase, Duration), Error> {
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
/// the state of given `MaskConsumer` resource and decides which actions needs to be performed.
/// The finite set of possible actions is represented by the `ConsumerAction` enum.
///
/// # Arguments
/// - `instance`: A reference to `MaskConsumer` being reconciled to decide next action upon.
async fn determine_action(
    client: Client,
    name: &str,
    namespace: &str,
    instance: &MaskConsumer,
) -> Result<ConsumerAction, Error> {
    if instance.meta().deletion_timestamp.is_some() {
        return Ok(ConsumerAction::Delete {
            delete_resource: false,
        });
    }

    // The rest of the controller code assumes the presence of the
    // status object and its phase field. If neither of these exist,
    // the first thing that should be done is initializing them.
    if needs_pending(instance) {
        return Ok(ConsumerAction::Pending);
    }

    // See if the MaskConsumer should be assigned a MaskProvider.
    let provider = match get_assigned_provider(instance) {
        // We need to assign a MaskProvider to this MaskConsumer.
        None => return Ok(ConsumerAction::Assign),
        // MaskProvider has already been assigned.
        Some(p) => p,
    };

    // Ensure the MaskReservation that reserves the slot for the MaskConsumer exists.
    // If it does not exist, we should delete this MaskConsumer immediately.
    let _reservation = match get_reservation(client.clone(), provider).await? {
        // MaskReservation has been deleted, so we should delete this MaskConsumer.
        None => {
            return Ok(ConsumerAction::Delete {
                delete_resource: true,
            })
        }
        // MaskReservation still exists.
        Some(r) => r,
    };

    // Ensure the Secret containing the env credentials exists.
    // The Secret should exist in the same namespace as the MaskConsumer.
    if get_secret(client, name, namespace, provider)
        .await?
        .is_none()
    {
        return Ok(ConsumerAction::CreateSecret);
    }

    // Keep the Active status up-to-date.
    determine_status_action(instance)
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

/// Returns the MaskConsumer's assigned provider from its status object.
fn get_assigned_provider(instance: &MaskConsumer) -> Option<&AssignedProvider> {
    instance
        .status
        .as_ref()
        .map_or(None, |s| s.provider.as_ref())
}

/// Returns the [`MaskReservation`] resource referenced by the [`AssignedProvider`].
async fn get_reservation(
    client: Client,
    provider: &AssignedProvider,
) -> Result<Option<MaskReservation>, Error> {
    let reservation_name = format!("{}-{}", provider.name, provider.slot);
    let mr_api: Api<MaskReservation> = Api::namespaced(client, &provider.namespace);
    match mr_api.get(&reservation_name).await {
        // Ensure the MaskReservation's UID matches that in the AssignedProvider.
        Ok(mr)
            if mr
                .metadata
                .uid
                .as_deref()
                .map_or(false, |uid| uid == provider.reservation) =>
        {
            // Referenced MaskReservation still exists.
            Ok(Some(mr))
        }
        // MaskReservation has been reassigned as it has a different UID.
        Ok(_) => Ok(None),
        Err(kube::Error::Api(e)) if e.code == 404 => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Determines the action given that the only thing left to do
/// is periodically keeping the Active phase up-to-date.
fn determine_status_action(instance: &MaskConsumer) -> Result<ConsumerAction, Error> {
    let (phase, age) = get_consumer_phase(instance)?;
    if phase != MaskConsumerPhase::Active || age > PROBE_INTERVAL {
        Ok(ConsumerAction::Active)
    } else {
        Ok(ConsumerAction::NoOp)
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
fn on_error(instance: Arc<MaskConsumer>, error: &Error, _context: Arc<ContextData>) -> Action {
    eprintln!("Reconciliation error:\n{:?}.\n{:?}", error, instance);
    Action::requeue(Duration::from_secs(5))
}
