//! OIDC authentication for roder. The authorization-code flow
//! (PKCE + state + nonce) yields an **ID token** that is later passed straight
//! through to the Kubernetes API server as a bearer, so RBAC is the user's real
//! identity. This crate owns discovery, the auth URL, code exchange + verification,
//! the `allowed_groups` gate, and refresh.

mod error;
mod provider;

pub use error::{AuthError, Result};
pub use provider::{LoginStart, OidcConfig, OidcProvider};

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// The verified identity extracted from an ID token.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Identity {
    pub subject: String,
    pub email: Option<String>,
    pub name: Option<String>,
    pub groups: Vec<String>,
}

/// The token set from a successful login or refresh. `id_token` is the value
/// passed through to the Kubernetes API server.
#[derive(Debug, Clone)]
pub struct Tokens {
    pub id_token: String,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: OffsetDateTime,
    pub identity: Identity,
}

impl Tokens {
    /// Whether the ID token is at/near expiry (1-minute safety buffer), i.e. it
    /// should be refreshed before the next API call / watch reconnect.
    pub fn needs_refresh(&self) -> bool {
        self.expires_at - OffsetDateTime::now_utc() <= time::Duration::minutes(1)
    }
}
