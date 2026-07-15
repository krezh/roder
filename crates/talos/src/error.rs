//! Talos API error type.

#[derive(Debug, thiserror::Error)]
pub enum TalosError {
    #[error("failed to read Talos in-cluster config: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid Talos in-cluster config: {0}")]
    Config(String),
    #[error("Talos API transport error: {0}")]
    Transport(#[from] tonic::transport::Error),
    #[error("Talos API request failed: {0}")]
    Status(#[from] tonic::Status),
    #[error("Talos API request timed out: {0}")]
    Timeout(String),
    #[error("Talos API upstream request failed: {0}")]
    Upstream(String),
}

impl TalosError {
    pub fn is_unavailable(&self) -> bool {
        matches!(self, Self::Status(status) if status.code() == tonic::Code::Unavailable)
    }

    pub fn is_timeout(&self) -> bool {
        matches!(self, Self::Timeout(_))
            || matches!(self, Self::Status(status) if status.code() == tonic::Code::DeadlineExceeded)
    }
}
