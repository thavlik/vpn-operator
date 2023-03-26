#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Kubernetes reported error: {source}")]
    KubeError {
        #[from]
        source: kube::Error,
    },

    #[error("Invalid user input: {0}")]
    UserInputError(String),

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
