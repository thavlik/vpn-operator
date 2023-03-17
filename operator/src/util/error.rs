/// All errors possible to occur during reconciliation
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Any error originating from the `kube-rs` crate
    #[error("Kubernetes reported error: {source}")]
    KubeError {
        #[from]
        source: kube::Error,
    },

    /// Usually an error in a Mask, Provider, or a Provider's secret.
    #[error("Invalid user input: {0}")]
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

    #[error("Json error: {source}")]
    JsonError {
        #[from]
        source: serde_json::Error,
    },

    #[error("Parse duration: {source}")]
    ParseDurationError {
        #[from]
        source: parse_duration::parse::Error,
    },
}
