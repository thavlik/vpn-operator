use std::time::Duration;

pub mod patch;

mod error;
mod merge;

pub use error::*;
pub use merge::deep_merge;

/// The default interval for requeuing a managed resource.
pub const PROBE_INTERVAL: Duration = Duration::from_secs(12);

/// Name of the kubernetes resource finalizer field.
pub const FINALIZER_NAME: &str = "vpn.beebs.dev/finalizer";

/// Name of the label in the Secret metadata corresponding
/// to the owner Provider UID.
pub const PROVIDER_UID_LABEL: &str = "vpn.beebs.dev/owner";

/// Name of the label in the Provider metadata corresponding
/// to the "short name" that can be used in a Mask's spec.
/// Multiple names can be specified by separating them with
/// a comma.
pub const PROVIDER_NAME_LABEL: &str = "vpn.beebs.dev/provider";

/// Name of the kubernetes resource manager.
pub const MANAGER_NAME: &str = "";

/// A label that a Mask will have in order to be assigned
/// a Provider that any phase other than Active. The value
/// must correspond to the UUID of the Provider being verified.
pub const VERIFICATION_LABEL: &str = "vpn.beebs.dev/verify";
