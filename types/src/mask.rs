use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

/// [`MaskSpec`] describes the configuration for a [`Mask`] resource,
/// which is the mechanism for reserving slots with [`MaskProvider`] resources.
/// The controller will create a [`MaskConsumer`] resource for each [`Mask`]
/// that will be updated when it is assigned a [`MaskProvider`] and deleted
/// whenever the provider is unassigned. This way any resources that consume
/// the credentials can be garbage collected by using the [`MaskConsumer`] as
/// an owner reference.
///
/// Once a [`Mask`] is assigned a suitable provider through its [`MaskConsumer`],
/// the controller copies the provider's credentials to a [`Secret`](k8s_openapi::api::core::v1::Secret)
/// owned by the [`MaskConsumer`] and references it as [`AssignedProvider::secret`]
/// within [`MaskConsumerStatus::provider`]. The credentials are then ready to be used
/// be a container, or however your application uses them.
#[derive(CustomResource, Serialize, Deserialize, Default, Debug, PartialEq, Clone, JsonSchema)]
#[kube(
    group = "vpn.beebs.dev",
    version = "v1",
    kind = "Mask",
    plural = "masks",
    derive = "PartialEq",
    status = "MaskStatus",
    namespaced
)]
#[kube(derive = "Default")]
#[kube(
    printcolumn = "{\"jsonPath\": \".status.phase\", \"name\": \"PHASE\", \"type\": \"string\" }"
)]
#[kube(
    printcolumn = "{\"jsonPath\": \".status.lastUpdated\", \"name\": \"AGE\", \"type\": \"date\" }"
)]
pub struct MaskSpec {
    /// Optional list of providers to use at the exclusion of others.
    /// Omit if you are okay with being assigned any [`MaskProvider`].
    /// These values correspond to [`MaskProviderSpec::tags`], and
    /// only one of them has to match for the [`MaskProvider`] to be
    /// considered suitable.
    pub providers: Option<Vec<String>>,
}

/// Status object for the [`Mask`] resource.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Default, JsonSchema)]
pub struct MaskStatus {
    /// A short description of the [`Mask`] resource's current state.
    pub phase: Option<MaskPhase>,

    /// A human-readable message indicating details about why the
    /// [`Mask`] is in this phase.
    pub message: Option<String>,

    /// Timestamp of when the [`MaskStatus`] object was last updated.
    #[serde(rename = "lastUpdated")]
    pub last_updated: Option<String>,
}

/// A short description of the [`Mask`] resource's current state.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, JsonSchema)]
pub enum MaskPhase {
    /// The [`Mask`] resource first appeared to the controller.
    Pending,

    /// The [`MaskConsumer`] is waiting for an open slot with a suitable [`MaskProvider`].
    Waiting,

    /// The [`MaskConsumer`] resource's assigned credentials are in use by a Pod.
    Active,

    /// Resource deletion is pending garbage collection.
    Terminating,

    /// No suitable [`MaskProvider`] resources were found.
    ErrNoProviders,
}

impl FromStr for MaskPhase {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Pending" => Ok(MaskPhase::Pending),
            "Active" => Ok(MaskPhase::Active),
            "Waiting" => Ok(MaskPhase::Waiting),
            "Terminating" => Ok(MaskPhase::Terminating),
            "ErrNoProviders" => Ok(MaskPhase::ErrNoProviders),
            _ => Err(()),
        }
    }
}

impl fmt::Display for MaskPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MaskPhase::Pending => write!(f, "Pending"),
            MaskPhase::Active => write!(f, "Active"),
            MaskPhase::Waiting => write!(f, "Waiting"),
            MaskPhase::Terminating => write!(f, "Terminating"),
            MaskPhase::ErrNoProviders => write!(f, "ErrNoProviders"),
        }
    }
}
