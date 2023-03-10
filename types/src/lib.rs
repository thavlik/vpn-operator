use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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
#[kube(printcolumn = "{\"jsonPath\": \".status.activeSlots\", \"name\": \"IN USE\", \"type\": \"integer\" }")]
#[kube(printcolumn = "{\"jsonPath\": \".status.phase\", \"name\": \"PHASE\", \"type\": \"string\" }")]
#[kube(printcolumn = "{\"jsonPath\": \".status.lastUpdated\", \"name\": \"AGE\", \"type\": \"date\" }")]
pub struct ProviderSpec {
    /// Maximum number of clients allowed to connect to the VPN
    /// with these credentials at any one time.
    #[serde(rename = "maxClients")]
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
    /// Provider is in this condition.
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
    ErrSecretMissing,

    /// The resource is ready to be used.
    Active,
}

impl std::str::FromStr for ProviderPhase {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Pending" => Ok(ProviderPhase::Pending),
            "ErrSecretMissing" => Ok(ProviderPhase::ErrSecretMissing),
            "Active" => Ok(ProviderPhase::Active),
            _ => Err(()),
        }
    }
}


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
#[kube(printcolumn = "{\"jsonPath\": \".status.phase\", \"name\": \"PHASE\", \"type\": \"string\" }")]
#[kube(printcolumn = "{\"jsonPath\": \".status.lastUpdated\", \"name\": \"AGE\", \"type\": \"date\" }")]
pub struct MaskSpec {}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Default, JsonSchema)]
pub struct MaskStatus {
    pub phase: Option<MaskPhase>,

    pub message: Option<String>,

    /// Timestamp of when the status object was last updated.
    #[serde(rename = "lastUpdated")]
    pub last_updated: Option<String>,

    /// The assigned VPN service provider.
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
    Pending,
    Active,
    ErrNoProvidersAvailable,
}

impl std::str::FromStr for MaskPhase {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Pending" => Ok(MaskPhase::Pending),
            "Active" => Ok(MaskPhase::Active),
            "ErrNoProvidersAvailable" => Ok(MaskPhase::ErrNoProvidersAvailable),
            _ => Err(()),
        }
    }
}
