use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fmt, str::FromStr};

/// Defines overrides for the different containers in the verification pod.
/// The structure of these fields corresponds to the [`Container`](k8s_openapi::api::core::v1::Container)
/// schema. Validation is disabled for both peformance and simplicity, as [`k8s_openapi`]
/// doesn't currently implement JsonSchema.
#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq, JsonSchema)]
pub struct MaskProviderVerifyContainerOverridesSpec {
    /// Customization for the init container that gets the initial IP address.
    /// The structure of this field corresponds to the [`Container`](k8s_openapi::api::core::v1::Container)
    /// schema. Validation is disabled for both peformance and simplicity.
    #[schemars(schema_with = "any_schema")]
    pub init: Option<Value>,

    /// Customization for the gluetun container that connects to the VPN.
    /// The structure of this field corresponds to the [`Container`](k8s_openapi::api::core::v1::Container)
    /// schema. Validation is disabled for both peformance and simplicity.
    #[schemars(schema_with = "any_schema")]
    pub vpn: Option<Value>,

    /// Customization for the container that checks the public IP address
    /// until it differs from the initial.
    /// The structure of this field corresponds to the [`Container`](k8s_openapi::api::core::v1::Container)
    /// schema. Validation is disabled for both peformance and simplicity.
    #[schemars(schema_with = "any_schema")]
    pub probe: Option<Value>,
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq, JsonSchema)]
pub struct MaskProviderVerifyOverridesSpec {
    /// Optional customization for the verification [`Pod`](k8s_openapi::api::core::v1::Pod)'s
    /// different containers. Since the templating process will overwrite arrays,
    /// the containers can be overriden separately so as to avoid having to
    /// specify the full container array in [`MaskProviderVerifyOverridesSpec::pod`].
    pub containers: Option<MaskProviderVerifyContainerOverridesSpec>,

    /// Optional customization for the verification [`Pod`](k8s_openapi::api::core::v1::Pod) resource.
    /// The structure of this field corresponds to the [`Pod`](k8s_openapi::api::core::v1::Pod) schema.
    /// Validation is disabled for both peformance and simplicity.
    #[schemars(schema_with = "any_schema")]
    pub pod: Option<Value>,
}

/// Configuration for verifying the [`MaskProvider`] credentials.
/// Unless [`skip=true`](MaskProviderVerifySpec::skip), the credentials
/// are dialed with a [gluetun](https://github.com/qdm12/gluetun) container
/// to ensure they are valid before the [`MaskProvider`] can be assigned
/// to a [`Mask`].
#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq, JsonSchema)]
pub struct MaskProviderVerifySpec {
    /// If true, credentials verification is skipped entirely.
    pub skip: Option<bool>,

    /// [`Duration`](std::time::Duration) string for how long the verify
    /// pod is allowed to take before verification is considered failed.
    pub timeout: Option<String>,

    /// How often you want to verify the credentials (e.g. "1h30m")
    /// If unset, the credentials are only verified once.
    pub interval: Option<String>,

    /// Optional customization for the verification [`Pod`](k8s_openapi::api::core::v1::Pod).
    /// Use this to setup the image, networking, etc. These values are then
    /// merged onto the controller-created [`Pod`](k8s_openapi::api::core::v1::Pod).
    pub overrides: Option<MaskProviderVerifyOverridesSpec>,
}

/// [`MaskProviderSpec`] is the configuration for the [`MaskProvider`] resource,
/// which represents a VPN service provider. It specifies a reference to a
/// [`Secret`](k8s_openapi::api::core::v1::Secret) containing the credentials for
/// connecting to the VPN service, as well as other important details like the maximum
/// number of clients that can connect with the credentials at the same time.
#[derive(CustomResource, Serialize, Default, Deserialize, Debug, PartialEq, Clone, JsonSchema)]
#[kube(
    group = "vpn.beebs.dev",
    version = "v1",
    kind = "MaskProvider",
    plural = "maskproviders",
    derive = "PartialEq",
    status = "MaskProviderStatus",
    namespaced
)]
#[kube(derive = "Default")]
#[kube(
    printcolumn = "{\"jsonPath\": \".status.activeSlots\", \"name\": \"USED\", \"type\": \"integer\" }"
)]
#[kube(
    printcolumn = "{\"jsonPath\": \".status.phase\", \"name\": \"PHASE\", \"type\": \"string\" }"
)]
#[kube(
    printcolumn = "{\"jsonPath\": \".status.lastUpdated\", \"name\": \"AGE\", \"type\": \"date\" }"
)]
pub struct MaskProviderSpec {
    /// Reference to a [`Secret`](k8s_openapi::api::core::v1::Secret)
    /// resource containing the env vars that will be injected into
    /// the gluetun container.
    pub secret: String,

    /// Maximum number of [`Mask`] resources that can be assigned this
    /// [`MaskProvider`] at any given time. Used to prevent excessive
    /// connections to the VPN service, which could result in account
    /// suspension with some providers.
    #[serde(rename = "maxSlots")]
    pub max_slots: usize,

    /// Optional list of short names that [`Mask`] resources can use to
    /// refer to this service at the exclusion of others. Example values
    /// might be the role of the service (`"default"` or `"preferred"`),
    /// the service name (`"nordvpn"`, `"pia"`), or even region names
    /// (`"us-west"`, `"uk-london"`) - whatever makes sense for you.
    pub tags: Option<Vec<String>>,

    /// Optional list of namespaces that are allowed to use
    /// this [`MaskProvider`]. If unset, all namespaces are allowed.
    pub namespaces: Option<Vec<String>>,

    /// VPN service verification options, used to ensure the
    /// credentials are valid before allowing [`Mask`]s to use them.
    pub verify: Option<MaskProviderVerifySpec>,
}

/// Status object for the [`MaskProvider`] resource.
#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq, JsonSchema)]
pub struct MaskProviderStatus {
    /// A short description of the [`MaskProvider`] resource's current state.
    pub phase: Option<MaskProviderPhase>,

    /// A human-readable message indicating details about why the
    /// [`MaskProvider`] is in this phase.
    pub message: Option<String>,

    /// Timestamp of when the [`MaskProviderStatus`] object was last updated.
    #[serde(rename = "lastUpdated")]
    pub last_updated: Option<String>,

    /// Timestamp of when the credentials were last verified.
    #[serde(rename = "lastVerified")]
    pub last_verified: Option<String>,

    /// Number of active slots reserved by [`Mask`] resources.
    #[serde(rename = "activeSlots")]
    pub active_slots: Option<usize>,
}

/// A short description of the [`MaskProvider`] resource's current state.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, JsonSchema)]
pub enum MaskProviderPhase {
    /// The resource first appeared to the controller.
    Pending,

    /// The credentials are being verified with a gluetun pod.
    Verifying,

    /// Verification is complete. The [`MaskProviderStatus::phase`] will become
    /// [`MaskProviderPhase::Ready`] or [`MaskProviderPhase::Active`] upon the
    /// next reconciliation.
    Verified,

    /// The [`MaskProvider`] is ready to be assigned to [`Mask`] resources.
    Ready,

    /// The [`MaskProvider`] is assigned to one or more [`Mask`] resources.
    Active,

    /// The [`Secret`](k8s_openapi::api::core::v1::Secret) resource referenced
    /// by [`MaskProviderSpec::secret`] is missing.
    ErrSecretNotFound,

    /// The credentials verification process failed.
    ErrVerifyFailed,
}

impl FromStr for MaskProviderPhase {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Pending" => Ok(MaskProviderPhase::Pending),
            "ErrSecretNotFound" => Ok(MaskProviderPhase::ErrSecretNotFound),
            "Verifying" => Ok(MaskProviderPhase::Verifying),
            "Verified" => Ok(MaskProviderPhase::Verified),
            "ErrVerifyFailed" => Ok(MaskProviderPhase::ErrVerifyFailed),
            "Ready" => Ok(MaskProviderPhase::Ready),
            "Active" => Ok(MaskProviderPhase::Active),
            _ => Err(()),
        }
    }
}

impl fmt::Display for MaskProviderPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MaskProviderPhase::Pending => write!(f, "Pending"),
            MaskProviderPhase::ErrSecretNotFound => write!(f, "ErrSecretNotFound"),
            MaskProviderPhase::Verifying => write!(f, "Verifying"),
            MaskProviderPhase::Verified => write!(f, "Verified"),
            MaskProviderPhase::ErrVerifyFailed => write!(f, "ErrVerifyFailed"),
            MaskProviderPhase::Ready => write!(f, "Ready"),
            MaskProviderPhase::Active => write!(f, "Active"),
        }
    }
}

/// [`MaskSpec`] describes the configuration for a [`Mask`] resource,
/// which is the mechanism for reserving slots with [`MaskProvider`] resources.
/// Once a [`Mask`] is assigned a suitable provider, the controller copies the
/// provider's credentials to a [`Secret`](k8s_openapi::api::core::v1::Secret)
/// owned by the [`Mask`] and references it as [`AssignedProvider::secret`]
/// within [`MaskStatus::provider`].
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

    /// Details for the assigned [`MaskProvider`].
    pub provider: Option<AssignedProvider>,
}

/// Details about the [`MaskProvider`] assigned to this [`Mask`].
/// If this object is not present, you should ensure that any
/// [`Pod`](k8s_openapi::api::core::v1::Pod) that was consuming
/// this [`Mask`] is terminated. Failure to do so may result in
/// more connections to the VPN service than allowed by
/// [`MaskProviderSpec::max_slots`].
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
    /// to index the [`ConfigMap`](k8s_openapi::api::core::v1::ConfigMap)
    /// that reserves the slot.
    pub slot: usize,

    /// Name of the [`Secret`](k8s_openapi::api::core::v1::Secret) resource
    /// which contains environment variables to be injected into the gluetun
    /// container. The controller will create this in the same namespace as
    /// the [`Mask`] resource. Its contents mirror that of the
    /// [`Secret`](k8s_openapi::api::core::v1::Secret) referenced
    /// by [`MaskProviderSpec::secret`].
    pub secret: String,
}

/// A short description of the [`Mask`] resource's current state.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, JsonSchema)]
pub enum MaskPhase {
    /// The resource first appeared to the controller.
    Pending,

    /// The resource is waiting for a slot with a [`MaskProvider`] to become available.
    Waiting,

    /// No suitable [`MaskProvider`] resources were found.
    ErrNoProviders,

    /// The resource's VPN service credentials are ready to be used.
    Ready,

    /// The resource's VPN service credentials are in use by a Pod.
    Active,
}

impl FromStr for MaskPhase {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Pending" => Ok(MaskPhase::Pending),
            "Ready" => Ok(MaskPhase::Ready),
            "Active" => Ok(MaskPhase::Active),
            "Waiting" => Ok(MaskPhase::Waiting),
            "ErrNoProviders" => Ok(MaskPhase::ErrNoProviders),
            _ => Err(()),
        }
    }
}

impl fmt::Display for MaskPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MaskPhase::Pending => write!(f, "Pending"),
            MaskPhase::Ready => write!(f, "Ready"),
            MaskPhase::Active => write!(f, "Active"),
            MaskPhase::Waiting => write!(f, "Waiting"),
            MaskPhase::ErrNoProviders => write!(f, "ErrNoProviders"),
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
