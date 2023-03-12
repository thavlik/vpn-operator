use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Provider is a resource that represents a VPN service provider.
/// It contains a reference to a Secret containing the  credentials
/// to connect to the VPN service, and a maximum number of clients
/// that can connect to the VPN at any one time.
#[derive(CustomResource, Serialize, Default, Deserialize, Debug, PartialEq, Clone, JsonSchema)]
#[kube(
    group = "vpn.beebs.dev",
    version = "v1",
    kind = "Provider",
    plural = "providers",
    derive = "PartialEq",
    status = "ProviderStatus",
    namespaced
)]
#[kube(derive = "Default")]
#[kube(
    printcolumn = "{\"jsonPath\": \".status.activeSlots\", \"name\": \"IN USE\", \"type\": \"integer\" }"
)]
#[kube(
    printcolumn = "{\"jsonPath\": \".status.phase\", \"name\": \"PHASE\", \"type\": \"string\" }"
)]
#[kube(
    printcolumn = "{\"jsonPath\": \".status.lastUpdated\", \"name\": \"AGE\", \"type\": \"date\" }"
)]
pub struct ProviderSpec {
    /// Maximum number of clients allowed to connect to the VPN
    /// with these credentials at any one time.
    #[serde(rename = "maxSlots")]
    pub max_slots: usize,

    /// Reference to a Secret resource containing the env vars
    /// that will be injected into the gluetun container.
    pub secret: String,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq, JsonSchema)]
pub struct ProviderStatus {
    /// The current phase of the Provider.
    pub phase: Option<ProviderPhase>,

    /// A human-readable message indicating details about why the
    /// Provider is in this phase.
    pub message: Option<String>,

    /// Timestamp of when the status object was last updated.
    #[serde(rename = "lastUpdated")]
    pub last_updated: Option<String>,

    /// Number of active clients reserved by Mask resources.
    #[serde(rename = "activeSlots")]
    pub active_slots: Option<usize>,
}

#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, JsonSchema)]
pub enum ProviderPhase {
    /// The resource first appeared to the controller.
    Pending,

    /// The spec.secret resource is missing.
    ErrSecretNotFound,

    /// The resource is ready to be used.
    Active,
}

impl std::str::FromStr for ProviderPhase {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Pending" => Ok(ProviderPhase::Pending),
            "ErrSecretNotFound" => Ok(ProviderPhase::ErrSecretNotFound),
            "Active" => Ok(ProviderPhase::Active),
            _ => Err(()),
        }
    }
}

/// Mask is a resource that represents a VPN connection.
/// It reserves a slot with a Provider resource, and
/// creates a Secret resource containing the environment
/// variables to be injected into the gluetun container.
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
    /// Omit the field if you are okay with being assigned any provider.
    /// These values correspond to the Provider resource's metadata.labels["vpn.beebs.dev/provider"]
    pub providers: Option<Vec<String>>,
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Default, JsonSchema)]
pub struct MaskStatus {
    /// The current phase of the Mask.
    pub phase: Option<MaskPhase>,

    /// A human-readable message indicating details about why the
    /// Mask is in this phase.
    pub message: Option<String>,

    /// Timestamp of when the status object was last updated.
    #[serde(rename = "lastUpdated")]
    pub last_updated: Option<String>,

    /// Details for the assigned VPN service provider.
    pub provider: Option<AssignedProvider>,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq, JsonSchema)]
pub struct AssignedProvider {
    /// Name of the Provider resource.
    pub name: String,

    /// Namespace of the Provider resource.
    pub namespace: String,

    /// UID of the Provider resource. Used to ensure the
    /// reference is valid if case a Provider resource is
    /// deleted and recreated with the same name.
    pub uid: String,

    /// User index assigned to this Mask. This value must be
    /// less than the Provider's spec.maxClients, and is used
    /// to index the ConfigMap that reserves the connection.
    pub slot: usize,

    /// Name of the Secret resource which contains environment
    /// variables to be injected into the gluetun container.
    /// The controller will create this secret in the same
    /// namespace as the Mask resource. Its contents mirror
    /// the contents of the Provider's secret.
    pub secret: String,
}

#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, JsonSchema)]
pub enum MaskPhase {
    /// The resource first appeared to the controller.
    Pending,
    /// The VPN credentials are ready to be used.
    Active,
    /// The resource is waiting for a Provider to become available.
    Waiting,
    /// No Provider resources are available.
    ErrNoProviders,
}

impl std::str::FromStr for MaskPhase {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Pending" => Ok(MaskPhase::Pending),
            "Active" => Ok(MaskPhase::Active),
            "Waiting" => Ok(MaskPhase::Waiting),
            "ErrNoProviders" => Ok(MaskPhase::ErrNoProviders),
            _ => Err(()),
        }
    }
}
