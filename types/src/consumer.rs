use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

/// Found in [`MaskConsumerStatus::provider`], this struct contains
/// details about the [`MaskProvider`] assigned to this [`Mask`].
#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq, JsonSchema)]
pub struct AssignedProvider {
    /// Name of the assigned [`MaskProvider`] resource.
    pub name: String,

    /// Namespace of the assigned [`MaskProvider`] resource.
    pub namespace: String,

    /// UID of the assigned [`MaskProvider`] resource. Used to ensure
    /// the reference is valid in case the [`MaskProvider`] is deleted
    /// and quickly recreated with the same name.
    pub uid: String,

    /// Slot index assigned to this [`Mask`]. This value must be
    /// less than [`MaskProviderSpec::max_slots`], and is used
    /// to index the [`MaskReservation`] that reserves the slot.
    pub slot: usize,

    /// UID of the corresponding [`MaskReservation`] resource. This is effectively
    /// a cross-namespace owner reference, enforced via finalizers.
    pub reservation: String,

    /// Name of the [`Secret`](k8s_openapi::api::core::v1::Secret) resource
    /// which contains environment variables to be injected into a
    /// [gluetun](https://github.com/qdm12/gluetun) container. The controller
    /// will create this in the same namespace as the [`Mask`] resource.
    /// Its contents mirror that of the [`Secret`](k8s_openapi::api::core::v1::Secret)
    /// referenced by [`MaskProviderSpec::secret`].
    pub secret: String,
}

/// [`MaskConsumerSpec`] describes the configuration for a [`MaskConsumer`] resource,
/// which is used to garbage collect resources that consume VPN credentials when they
/// are unassigned from a [`Mask`]. This resource will always have a `Mask` as its owner.
/// It corresponds with a singular [`MaskReservation`] resource in the [`MaskProvider`]'s
/// namespace, which reserves a slot with the provider.
///
/// The [`MaskConsumer`] is the first resource to be allocated during assignment. Once
/// a [`MaskProvider`] has been assigned to [`MaskConsumerStatus::provider`], the controller
/// will update the [`MaskStatus::consumer`] to point to the [`MaskConsumer`]. This order
/// is important because the [`MaskReservation`] reserving the slot will be garbage collected
/// if the [`MaskConsumer`] doesn't exist.
///
/// [`MaskConsumer`] resources are created by the controller. Any resources that consume
/// VPN credentials should have an owner reference to it - either directly or indirectly
/// through one of its parents - that way any connections to the service will be guaranteed
/// severed before the slot is reprovisioned. This paradigm allows garbage collection to be
/// agnostic to how credentials are consumed.
#[derive(CustomResource, Serialize, Deserialize, Default, Debug, PartialEq, Clone, JsonSchema)]
#[kube(
    group = "vpn.beebs.dev",
    version = "v1",
    kind = "MaskConsumer",
    plural = "maskconsumers",
    derive = "PartialEq",
    status = "MaskConsumerStatus",
    namespaced
)]
#[kube(derive = "Default")]
#[kube(
    printcolumn = "{\"jsonPath\": \".status.phase\", \"name\": \"PHASE\", \"type\": \"string\" }"
)]
#[kube(
    printcolumn = "{\"jsonPath\": \".status.lastUpdated\", \"name\": \"AGE\", \"type\": \"date\" }"
)]
pub struct MaskConsumerSpec {
    /// List of desired providers, inherited from the parent [`MaskSpec::providers`].
    pub providers: Option<Vec<String>>,
}

/// Status object for the [`MaskConsumer`] resource.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Default, JsonSchema)]
pub struct MaskConsumerStatus {
    /// A short description of the [`MaskConsumer`] resource's current state.
    pub phase: Option<MaskConsumerPhase>,

    /// A human-readable message indicating details about why the
    /// [`MaskConsumer`] is in this phase.
    pub message: Option<String>,

    /// Timestamp of when the [`MaskConsumerStatus`] object was last updated.
    #[serde(rename = "lastUpdated")]
    pub last_updated: Option<String>,

    /// Details about the assigned provider and credentials.
    pub provider: Option<AssignedProvider>,

    /// Name of the Pod that is consuming the credentials.
    pub pod: Option<String>,
}

/// A short description of the [`MaskConsumer`] resource's current state.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, JsonSchema)]
pub enum MaskConsumerPhase {
    /// The [`MaskConsumer`] resource first appeared to the controller.
    Pending,

    /// The [`MaskConsumer`] is waiting for an open slot with a suitable [`MaskProvider`].
    Waiting,

    /// The [`MaskConsumer`] is consuming the VPN credentials on a reserved slot.
    Active,

    /// Deletion of the [`MaskConsumer`] is pending garbage collection.
    Terminating,

    /// No suitable [`MaskProvider`] resources were found.
    ErrNoProviders,
}

impl FromStr for MaskConsumerPhase {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Pending" => Ok(MaskConsumerPhase::Pending),
            "Waiting" => Ok(MaskConsumerPhase::Waiting),
            "Active" => Ok(MaskConsumerPhase::Active),
            "Terminating" => Ok(MaskConsumerPhase::Terminating),
            "ErrNoProviders" => Ok(MaskConsumerPhase::ErrNoProviders),
            _ => Err(()),
        }
    }
}

impl fmt::Display for MaskConsumerPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MaskConsumerPhase::Pending => write!(f, "Pending"),
            MaskConsumerPhase::Waiting => write!(f, "Waiting"),
            MaskConsumerPhase::Active => write!(f, "Active"),
            MaskConsumerPhase::Terminating => write!(f, "Terminating"),
            MaskConsumerPhase::ErrNoProviders => write!(f, "ErrNoProviders"),
        }
    }
}
