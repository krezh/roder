//! Building a Talos gRPC channel from a talosconfig file (the generated
//! in-cluster config, or a local `~/.talos/config` for out-of-cluster dev).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine;
use serde::Deserialize;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Endpoint, Identity};

use crate::error::TalosError;

pub const IN_CLUSTER_CONFIG: &str = "/var/run/secrets/talos.dev/config";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Deserialize)]
struct Config {
    context: String,
    contexts: HashMap<String, Context>,
}

#[derive(Deserialize)]
struct Context {
    endpoints: Vec<String>,
    ca: String,
    crt: String,
    key: String,
}

pub(crate) fn talos_config_path() -> Option<PathBuf> {
    let in_cluster = Path::new(IN_CLUSTER_CONFIG);
    if in_cluster.is_file() {
        return Some(in_cluster.into());
    }
    if let Some(path) = std::env::var_os("TALOSCONFIG").map(PathBuf::from) {
        if path.is_file() {
            return Some(path);
        }
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".talos/config"))
        .filter(|path| path.is_file())
}

pub(crate) async fn connect(raw: &str) -> Result<Channel, TalosError> {
    let config: Config =
        serde_yaml::from_str(raw).map_err(|e| TalosError::Config(format!("invalid YAML: {e}")))?;
    let context = config.contexts.get(&config.context).ok_or_else(|| {
        TalosError::Config(format!("context {:?} does not exist", config.context))
    })?;
    let endpoint = context
        .endpoints
        .first()
        .ok_or_else(|| TalosError::Config("active context has no endpoint".into()))?;
    let endpoint = if endpoint.contains("://") {
        endpoint.clone()
    } else if endpoint.contains(':') {
        format!("https://{endpoint}")
    } else {
        format!("https://{endpoint}:50000")
    };

    let ca = decode_config_value("ca", &context.ca)?;
    let crt = decode_config_value("crt", &context.crt)?;
    let key = normalize_private_key(decode_config_value("key", &context.key)?);
    let tls = ClientTlsConfig::new()
        .ca_certificate(Certificate::from_pem(ca))
        .identity(Identity::from_pem(crt, key));
    Endpoint::from_shared(endpoint)
        .map_err(|e| TalosError::Config(e.to_string()))?
        .connect_timeout(CONNECT_TIMEOUT)
        .tls_config(tls)?
        .connect()
        .await
        .map_err(TalosError::from)
}

fn normalize_private_key(key: Vec<u8>) -> Vec<u8> {
    let Ok(pem) = std::str::from_utf8(&key) else {
        return key;
    };
    if pem.contains("-----BEGIN ED25519 PRIVATE KEY-----") {
        return pem
            .replace(
                "-----BEGIN ED25519 PRIVATE KEY-----",
                "-----BEGIN PRIVATE KEY-----",
            )
            .replace(
                "-----END ED25519 PRIVATE KEY-----",
                "-----END PRIVATE KEY-----",
            )
            .into_bytes();
    }
    key
}

fn decode_config_value(name: &str, value: &str) -> Result<Vec<u8>, TalosError> {
    if value.contains("-----BEGIN") {
        return Ok(value.as_bytes().to_vec());
    }
    base64::engine::general_purpose::STANDARD
        .decode(value.trim())
        .map_err(|e| TalosError::Config(format!("invalid base64 {name}: {e}")))
}
