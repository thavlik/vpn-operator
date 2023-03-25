use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

/// [`MaskReservationSpec`] describes the configuration for a [`MaskReservation`] resource,
/// which is used to garbage collect slots by deleting a corresponding [`MaskConsumer`] in
/// the [`Mask`]'s namespace before removing the finalizer on this object.
///
/// Note: The [`MaskReservation`] resource is only for internal use by the controller, and should
/// never be created or manipulated directly.
#[derive(CustomResource, Serialize, Deserialize, Default, Debug, PartialEq, Clone, JsonSchema)]
#[kube(
    group = "vpn.beebs.dev",
    version = "v1",
    kind = "MaskReservation",
    plural = "maskreservations",
    derive = "PartialEq",
    status = "MaskReservationStatus",
    namespaced
)]
#[kube(derive = "Default")]
#[kube(
    printcolumn = "{\"jsonPath\": \".status.phase\", \"name\": \"PHASE\", \"type\": \"string\" }"
)]
#[kube(
    printcolumn = "{\"jsonPath\": \".status.lastUpdated\", \"name\": \"AGE\", \"type\": \"date\" }"
)]
pub struct MaskReservationSpec {
    /// Name of the [`MaskConsumer`] resource reserving the slot. If it does
    /// not exist, this [`MaskReservation`] will be deleted. The creation order
    /// is the [`MaskConsumer`] first, then this [`MaskReservation`], then update
    /// the status object of the [`Mask`] to point to the [`MaskConsumer`].
    pub name: String,

    /// Namespace of the [`MaskConsumer`] resource reserving the slot.
    pub namespace: String,

    /// UID of the [`MaskConsumer`] resource reserving the slot.
    pub uid: String,
}

/// Status object for the [`MaskReservation`] resource.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Default, JsonSchema)]
pub struct MaskReservationStatus {
    /// A short description of the [`MaskReservation`] resource's current state.
    pub phase: Option<MaskReservationPhase>,

    /// A human-readable message indicating details about why the
    /// [`MaskReservation`] is in this phase.
    pub message: Option<String>,

    /// Timestamp of when the [`MaskReservationStatus`] object was last updated.
    #[serde(rename = "lastUpdated")]
    pub last_updated: Option<String>,
}

/// A short description of the [`MaskReservation`] resource's current state.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, JsonSchema)]
pub enum MaskReservationPhase {
    /// The [`MaskReservation`] resource first appeared to the controller.
    Pending,

    /// The [`MaskReservation`] is in use by a valid [`MaskConsumer`].
    Active,

    /// Deletion of the [`MaskReservation`] is pending the deletion of
    /// its corresponding [`MaskConsumer`].
    Terminating,
}

impl FromStr for MaskReservationPhase {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Pending" => Ok(MaskReservationPhase::Pending),
            "Active" => Ok(MaskReservationPhase::Active),
            "Terminating" => Ok(MaskReservationPhase::Terminating),
            _ => Err(()),
        }
    }
}

impl fmt::Display for MaskReservationPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MaskReservationPhase::Pending => write!(f, "Pending"),
            MaskReservationPhase::Active => write!(f, "Active"),
            MaskReservationPhase::Terminating => write!(f, "Terminating"),
        }
    }
}
