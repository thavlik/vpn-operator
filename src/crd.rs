use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq, JsonSchema)]
pub struct ProviderSecret {
    pub name: String,
    pub namespace: String,
}

#[derive(CustomResource, Serialize, Default, Deserialize, Debug, PartialEq, Clone, JsonSchema)]
#[kube(
    group = "vpn.beebs.dev",
    version = "v1",
    kind = "Provider",
    plural = "Providers",
    derive = "PartialEq",
    status = "ProviderStatus"
)]
#[kube(derive = "Default")]
pub struct ProviderSpec {
    /// Maximum number of clients allowed to connect to the VPN
    /// with these credentials at any one time.
    #[serde(rename = "maxClients")]
    pub max_clients: u32,

    /// Reference to a Secret resource containing the env vars
    /// that will be injected into the gluetun container.
    /// This allows Providers to be cluster-scoped while Masks
    /// are namespaced, and all of the provider secrets can
    /// be stored in a single namespace.
    pub secret: ProviderSecret,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq, JsonSchema)]
pub struct ProviderStatus {
}

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
pub struct MaskSpec {
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Default, JsonSchema)]
pub struct MaskStatus {
    pub phase: Option<String>,

    /// Timestamp of when the status object was last updated.
    #[serde(rename = "lastUpdated")]
    pub last_updated: Option<String>,

    /// Name of the Provider resource, representing the service
    /// and credentials to be used by the gluetun container.
    pub provider: Option<String>,

    /// Name of the Secret resource which contains environment
    /// variables to be injected into the gluetun container.
    /// The controller will create this secret in the same
    /// namespace as the Mask resource. Its contents mirror
    /// the contents of the Provider's secret.
    pub secret: Option<String>,
}

