use thiserror::Error;

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("OIDC discovery failed for issuer {0}: {1}")]
    Discovery(String, String),

    #[error("invalid OIDC configuration: {0}")]
    Config(String),

    #[error("token exchange failed: {0}")]
    Exchange(String),

    #[error("token refresh failed: {0}")]
    Refresh(String),

    #[error("ID token verification failed: {0}")]
    Verification(String),

    #[error("provider did not return an ID token")]
    MissingIdToken,

    #[error("provider did not return a refresh token (is the offline_access scope allowed?)")]
    MissingRefreshToken,

    #[error("user is not a member of any allowed group")]
    Forbidden,
}

pub type Result<T> = std::result::Result<T, AuthError>;
