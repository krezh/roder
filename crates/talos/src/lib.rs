//! Direct Talos machine API access using Talos's native in-cluster credentials.

use std::path::PathBuf;

use tonic::transport::Channel;

mod actions;
mod config_snapshot;
mod connection;
mod cosi;
mod dmesg;
mod error;
mod node_status;
mod request;

pub use config_snapshot::{ConfigField, ConfigSnapshot};
pub use connection::IN_CLUSTER_CONFIG;
pub use error::TalosError;

pub struct Backend {
    config_path: PathBuf,
    connection: tokio::sync::RwLock<Connection>,
}

struct Connection {
    config: String,
    channel: Channel,
}

impl Backend {
    /// Connect using the generated in-cluster config, or a local talosconfig
    /// (`TALOSCONFIG`, then `~/.talos/config`) when running outside the cluster.
    pub async fn connect_in_cluster() -> Result<Option<Self>, TalosError> {
        let Some(path) = connection::talos_config_path() else {
            return Ok(None);
        };

        let raw = std::fs::read_to_string(&path)?;
        let channel = connection::connect(&raw).await?;
        Ok(Some(Self {
            config_path: path,
            connection: tokio::sync::RwLock::new(Connection {
                config: raw,
                channel,
            }),
        }))
    }

    async fn current_channel(&self) -> Result<Channel, TalosError> {
        let raw = std::fs::read_to_string(&self.config_path)?;
        {
            let current = self.connection.read().await;
            if current.config == raw {
                return Ok(current.channel.clone());
            }
        }

        let channel = connection::connect(&raw).await?;
        let mut current = self.connection.write().await;
        if current.config != raw {
            current.config = raw;
            current.channel = channel;
        }
        Ok(current.channel.clone())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use futures::StreamExt as _;

    #[tokio::test]
    #[ignore = "requires Talos credentials and RODER_TALOS_TEST_NODE"]
    async fn live_integrations() {
        let node = std::env::var("RODER_TALOS_TEST_NODE").expect("test node");
        let backend = Backend::connect_in_cluster()
            .await
            .expect("connect")
            .expect("Talos config");
        let status = backend.node(&node).await.expect("node status");
        assert!(!status.disk_inventory.is_empty());
        assert!(status
            .interfaces
            .iter()
            .any(|interface| interface.link_up.is_some()));
        assert!(status
            .interfaces
            .iter()
            .any(|interface| !interface.addresses.is_empty()));
        assert!(!status.volumes.is_empty());
        assert!(status
            .volumes
            .iter()
            .any(|volume| volume.used_bytes.is_some()));
        let mut logs = backend.dmesg(&node).await.expect("dmesg stream");
        let first_line = tokio::time::timeout(Duration::from_secs(5), logs.next())
            .await
            .expect("dmesg timeout")
            .expect("dmesg ended")
            .expect("dmesg line");
        assert!(!first_line.is_empty());
    }
}
