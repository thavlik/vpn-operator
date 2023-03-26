use std::time::Duration;

pub mod finalizer;
pub mod metrics;
pub mod patch;

pub(crate) mod messages;

mod error;
mod merge;

pub use error::*;
pub use merge::deep_merge;

/// The default interval for requeuing a managed resource.
pub(crate) const PROBE_INTERVAL: Duration = Duration::from_secs(12);

/// Name of the label in the Secret metadata corresponding
/// to the originating Provider UID.
pub(crate) const PROVIDER_UID_LABEL: &str = "vpn.beebs.dev/owner";

/// Name of the kubernetes resource manager.
pub(crate) const MANAGER_NAME: &str = "vpn-operator";

/// A label that a Mask/MaskConsumer must have in order to force
/// assignment to a MaskProvider with a specific uid, even if the
/// MaskProvider has no open slots.
pub(crate) const VERIFICATION_LABEL: &str = "vpn.beebs.dev/verify";
