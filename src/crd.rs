use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(CustomResource, Serialize, Default, Deserialize, Debug, PartialEq, Clone, JsonSchema)]
#[kube(
    group = "vpn.beebs.dev",
    version = "v1",
    kind = "Provider",
    plural = "Providers",
    derive = "PartialEq",
    status = "ProviderStatus",
    namespaced
)]
#[kube(derive = "Default")]
pub struct ProviderSpec {
    /// Maximum number of clients allowed to connect to the VPN
    /// with these credentials at any one time.
    #[serde(rename = "maxClients")]
    pub max_clients: u32,

    /// Reference to a Secret resource containing the env vars
    /// that will be injected into the gluetun container.
    pub secret: String,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq, JsonSchema)]
pub struct ProviderStatus {}

#[derive(CustomResource, Serialize, Deserialize, Default, Debug, PartialEq, Clone, JsonSchema)]
#[kube(
    group = "vpn.beebs.dev",
    version = "v1",
    kind = "Mask",
    plural = "Masks",
    derive = "PartialEq",
    status = "MaskStatus",
    namespaced
)]
#[kube(derive = "Default")]
pub struct MaskSpec {}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Default, JsonSchema)]
pub struct MaskStatus {
    pub phase: Option<MaskPhase>,

    pub message: Option<String>,

    /// Timestamp of when the status object was last updated.
    #[serde(rename = "lastUpdated")]
    pub last_updated: Option<String>,

    /// The assigned provider.
    pub provider: Option<AssignedProvider>,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq, JsonSchema)]
pub struct AssignedProvider {
    /// Name of the Provider resource.
    pub name: String,

    /// Namespace of the Provider resource.
    pub namespace: String,

    /// User index assigned to this Mask. This value must be
    /// less than the Provider's spec.maxClients, and is used
    /// to index the ConfigMap that reserves the connection.
    pub id: u32,

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
