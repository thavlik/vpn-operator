use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::str::FromStr;

#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq, JsonSchema)]
pub struct ProviderVerifyContainerOverridesSpec {
    /// Customization for the init container that gets the initial IP address.
    /// The structure of this field corresponds to the Container schema. 
    /// Validation is disabled for both peformance and simplicity.
    #[schemars(schema_with = "any_schema")]
    pub init: Option<Value>,

    /// Customization for the gluetun container that connects to the VPN.
    /// The structure of this field corresponds to the Container schema. 
    /// Validation is disabled for both peformance and simplicity.
    #[schemars(schema_with = "any_schema")]
    pub vpn: Option<Value>,

    /// Customization for the container that checks the public IP address
    /// until it differs from the initial.
    /// The structure of this field corresponds to the Container schema. 
    /// Validation is disabled for both peformance and simplicity.
    #[schemars(schema_with = "any_schema")]
    pub probe: Option<Value>,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq, JsonSchema)]
pub struct ProviderVerifyOverridesSpec {
    // Optional customization for the verification pod's different containers.
    // Since jsonpatch requires all containers be specified, this is used to
    // configure each container individually.
    pub containers: Option<ProviderVerifyContainerOverridesSpec>,

    // Optional customization for the verification Pod resource:
    // https://kubernetes.io/docs/reference/kubernetes-api/workload-resources/pod-v1/#Pod
    /// The structure of this field corresponds to the Pod schema. 
    /// Validation is disabled for both peformance and simplicity.
    #[schemars(schema_with = "any_schema")]
    pub pod: Option<Value>,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq, JsonSchema)]
pub struct ProviderVerifySpec {
    /// If true, credentials verification is skipped entirely.
    pub skip: bool,

    /// Optional customization for the verification pod.
    /// Use this to set the image, networking, etc.
    /// It is merged onto the controller-created Pod.
    #[serde(rename = "overrides")]
    pub overrides: Option<ProviderVerifyOverridesSpec>,
}

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

    /// VPN service verification options, used to ensure the
    /// credentials are valid before allowing Masks to use them. 
    pub verify: Option<ProviderVerifySpec>,

    /// Reference to a Secret resource containing the env vars
    /// that will be injected into the gluetun container.
    pub secret: String,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq, JsonSchema)]
pub struct ProviderStatus {
    /// A short description of the Provider's current state.
    pub phase: Option<ProviderPhase>,

    /// A human-readable message indicating details about why the
    /// Provider is in this phase.
    pub message: Option<String>,

    /// Timestamp of when the status object was last updated.
    #[serde(rename = "lastUpdated")]
    pub last_updated: Option<String>,

    /// Timestamp of when the credentials were last verified.
    #[serde(rename = "lastVerified")]
    pub last_verified: Option<String>,

    /// Number of active clients reserved by Mask resources.
    #[serde(rename = "activeSlots")]
    pub active_slots: Option<usize>,
}

/// A short description of the Provider's current state.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, JsonSchema)]
pub enum ProviderPhase {
    /// The resource first appeared to the controller.
    Pending,

    /// The spec.secret resource is missing.
    ErrSecretNotFound,

    /// The credentials are being verified with a gluetun pod.
    Verifying,

    /// The credentials verification failed.
    ErrVerifyFailed,

    /// The resource is ready to be used.
    Active,
}

impl FromStr for ProviderPhase {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Pending" => Ok(ProviderPhase::Pending),
            "ErrSecretNotFound" => Ok(ProviderPhase::ErrSecretNotFound),
            "Verifying" => Ok(ProviderPhase::Verifying),
            "ErrVerifyFailed" => Ok(ProviderPhase::ErrVerifyFailed),
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
    /// A short description of the Mask's current state.
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
    /// Name of the assigned Provider resource.
    pub name: String,

    /// Namespace of the assigned Provider resource.
    pub namespace: String,

    /// UID of the assigned Provider resource. Used to ensure
    /// the reference is valid if case a Provider resource is
    /// deleted and recreated with the same name.
    pub uid: String,

    /// Slot index assigned to this Mask. This value must be
    /// less than the Provider's spec.maxClients, and is used
    /// to index the ConfigMap that reserves the slot.
    pub slot: usize,

    /// Name of the Secret resource which contains environment
    /// variables to be injected into the gluetun container.
    /// The controller will create this secret in the same
    /// namespace as the Mask resource. Its contents mirror
    /// the contents of the Provider's secret.
    pub secret: String,
}

/// A short description of the Mask's current state.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, JsonSchema)]
pub enum MaskPhase {
    /// The resource first appeared to the controller.
    Pending,
    
    /// The resource is waiting for a Provider to become available.
    Waiting,
    
    /// No Provider resources are available.
    ErrNoProviders,

    /// The VPN credentials are ready to be used.
    Active,
}

impl FromStr for MaskPhase {
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

/// Schema generator that disables validation for unknown fields.
/// The core Kubernetes resources currently do not implement
/// the JsonSchema trait, so instead of manually validating all
/// of the override types, this project is choosing to only
/// validate the override after it is merged into the resource.
/// This should should catch schema errors, but they will be
/// slightly more difficult to debug.
fn any_schema(_: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
    serde_json::from_value(serde_json::json!({
        "type": "object",
        "x-kubernetes-preserve-unknown-fields": true,
    }))
    .unwrap()
}
