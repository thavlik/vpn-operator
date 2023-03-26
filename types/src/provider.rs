use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fmt, str::FromStr};

/// Defines overrides for the different containers in the verification pod.
/// The structure of these fields corresponds to the [`Container`](k8s_openapi::api::core::v1::Container)
/// schema. Validation is disabled for both peformance and simplicity, as [`k8s_openapi`]
/// doesn't currently implement [`schemars::JsonSchema`].
#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq, JsonSchema)]
pub struct MaskProviderVerifyContainerOverridesSpec {
    /// Customization for the init container that probes the initial IP address.
    /// The structure of this field corresponds to the [`Container`](k8s_openapi::api::core::v1::Container)
    /// schema. Validation is disabled for both peformance and simplicity.
    #[schemars(schema_with = "any_schema")]
    pub init: Option<Value>,

    /// Customization for the [gluetun](https://github.com/qdm12/gluetun) container
    /// that connects to the VPN. The structure of this field corresponds to the
    /// [`Container`](k8s_openapi::api::core::v1::Container) schema. Validation
    /// is disabled for both peformance and simplicity.
    #[schemars(schema_with = "any_schema")]
    pub vpn: Option<Value>,

    /// Customization for the container that probes the public IP address
    /// until it differs from the initial.
    /// The structure of this field corresponds to the [`Container`](k8s_openapi::api::core::v1::Container)
    /// schema. Validation is disabled for both peformance and simplicity.
    #[schemars(schema_with = "any_schema")]
    pub probe: Option<Value>,
}

/// Defines various overrides for the verification [`Pod`](k8s_openapi::api::core::v1::Pod).
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
/// to a [`MaskConsumer`].
#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq, JsonSchema)]
pub struct MaskProviderVerifySpec {
    /// If `true`, credentials verification is skipped entirely. This is useful
    /// if your [`MaskProviderSpec::secret`] can't be plugged into a gluetun
    /// container, but you still want to use vpn-operator. Defaults to `false`.
    pub skip: Option<bool>,

    /// Duration string for how long the verify pod is allowed to take before
    /// verification is considered failed. The controller doesn't inspect
    /// the gluetun logs, so the only way to know if verification has failed
    /// is if containers exit with nonzero codes or if this timeout has passed.
    /// In testing, the latter is more common. This value must be at least as
    /// long as your VPN service could possibly take to connect (e.g. `"60s"`).
    pub timeout: Option<String>,

    /// How often you want to verify the credentials (e.g. `"24h"`). If unset,
    /// the credentials are only verified once (unless [`skip=true`](MaskProviderVerifySpec::skip),
    /// then they are never verified).
    pub interval: Option<String>,

    /// Optional customization for the verification [`Pod`](k8s_openapi::api::core::v1::Pod).
    /// Use this to setup the image, networking, etc. These values are
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
    /// the [gluetun](https://github.com/qdm12/gluetun) container.
    /// The contents of this `Secret` will be copied to the namespace
    /// of any [`MaskConsumer`] that reserves a slot with the provider.
    /// The created `Secret` is owned by the `MaskConsumer` and will
    /// automatically be deleted whenever the [`MaskConsumer`] is
    /// deleted, which happens when the provider is unassigned or
    /// the [`Mask`] itself is deleted.
    pub secret: String,

    /// Maximum number of [`MaskConsumer`] resources that can be assigned
    /// this [`MaskProvider`] at any given time. Used to prevent excessive
    /// connections to the VPN service, which could result in account
    /// suspension with some providers.
    #[serde(rename = "maxSlots")]
    pub max_slots: usize,

    /// Optional list of short names that [`Mask`] resources can use to
    /// refer to this [`MaskProvider`] at the exclusion of others.
    /// Only one of these has to match one entry in [`MaskSpec::providers`]
    /// for this [`MaskProvider`] to be considered suitable for the [`Mask`].
    ///
    /// Example values might be the role of the service (`"default"` or `"preferred"`),
    /// the service name (`"nordvpn"`, `"atlasvpn"`), or even region names
    /// (`"us-west"`, `"uk-london"`) - whatever makes sense for you.
    pub tags: Option<Vec<String>>,

    /// Optional list of namespaces that are allowed to use this [`MaskProvider`].
    /// Even if the [`Mask`] expresses a preference for this provider in
    /// [`MaskSpec::providers`], it can only be assigned if it's in one of these
    /// namespaces. If unset, all [`Mask`] namespaces are permitted.
    pub namespaces: Option<Vec<String>>,

    /// VPN service verification options. Used to ensure the credentials
    /// are valid before assigning the [`MaskProvider`] to [`Mask`] resources.
    /// Enabled by default. Set [`skip=true`](MaskProviderVerifySpec::skip) to
    /// disable verification.
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
    /// The [`MaskProvider`] resource first appeared to the controller.
    Pending,

    /// The credentials are being verified with [gluetun](https://github.com/qdm12/gluetun).
    Verifying,

    /// Verification is complete. The [`MaskProviderStatus::phase`] will become
    /// [`Ready`](MaskProviderPhase::Ready) or [`Active`](MaskProviderPhase::Active)
    /// next reconciliation.
    Verified,

    /// The [`MaskProvider`] is ready to be assigned to [`MaskConsumer`] resources.
    Ready,

    /// The [`MaskProvider`] is assigned to one or more [`MaskConsumer`] resources.
    Active,

    /// Resource deletion is pending garbage collection.
    Terminating,

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
            "Terminating" => Ok(MaskProviderPhase::Terminating),
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
            MaskProviderPhase::Terminating => write!(f, "Terminating"),
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
