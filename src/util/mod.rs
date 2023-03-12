use std::backtrace::Backtrace;

pub mod patch;

/// Name of the kubernetes resource finalizer field.
pub const FINALIZER_NAME: &str = "vpn.beebs.dev/finalizer";

/// Name of the label in the Secret metadata corresponding
/// to the owner Provider UID.
pub const PROVIDER_UID_LABEL: &str = "vpn.beebs.dev/owner";

/// Name of the label in the Provider metadata corresponding
/// to the "short name" that can be used in a Mask's spec.
pub const PROVIDER_NAME_LABEL: &str = "vpn.beebs.dev/provider";

/// All errors possible to occur during reconciliation
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Any error originating from the `kube-rs` crate
    #[error("Kubernetes reported error: {source}")]
    KubeError {
        #[from]
        source: kube::Error,
        backtrace: Backtrace,
    },
    /// Error in user input or Mask resource definition, typically missing fields.
    #[error("Invalid Mask CRD: {0}")]
    UserInputError(String),

    /// Chrono date parsing error
    #[error("Failed to parse DateTime: {source}")]
    ChronoError {
        #[from]
        source: chrono::ParseError,
    },

    #[error("Out of range: {source}")]
    OutOfRangeError {
        #[from]
        source: chrono::OutOfRangeError,
    },
}
