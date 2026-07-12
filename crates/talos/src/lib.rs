//! Direct Talos machine API access using Talos's native in-cluster credentials.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::Duration;

use base64::Engine;
use futures::Stream;
use roder_core::{
    TalosDisk, TalosDiskStat, TalosMount, TalosNetworkInterface, TalosNode, TalosService,
    TalosServiceEvent, TalosVolume,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use talos_api_rs::api::common::Metadata;
use talos_api_rs::api::machine::machine_service_client::MachineServiceClient;
use talos_api_rs::api::machine::{
    DmesgRequest, RebootRequest, ServiceRestartRequest, ServiceStartRequest, ServiceStopRequest,
    ShutdownRequest,
};
use tonic::metadata::MetadataValue;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Endpoint, Identity};
use tonic::Request;

mod cosi;

pub const IN_CLUSTER_CONFIG: &str = "/var/run/secrets/talos.dev/config";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const RPC_TIMEOUT: Duration = Duration::from_secs(10);
const DMESG_TIMEOUT: Duration = Duration::from_secs(15);
const DMESG_INITIAL_LINES: usize = 500;
const DMESG_MAX_LINE_BYTES: usize = 1024 * 1024;

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

pub struct Backend {
    config_path: PathBuf,
    connection: tokio::sync::RwLock<Connection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigSnapshot {
    pub fingerprint: String,
    pub fields: BTreeMap<String, ConfigField>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigField {
    Plain(String),
    Sensitive(String),
}

struct Connection {
    config: String,
    channel: Channel,
}

impl Backend {
    /// Connect using the generated in-cluster config, or a local talosconfig
    /// (`TALOSCONFIG`, then `~/.talos/config`) when running outside the cluster.
    pub async fn connect_in_cluster() -> Result<Option<Self>, TalosError> {
        let Some(path) = talos_config_path() else {
            return Ok(None);
        };

        let raw = std::fs::read_to_string(&path)?;
        let channel = connect(&raw).await?;
        Ok(Some(Self {
            config_path: path,
            connection: tokio::sync::RwLock::new(Connection {
                config: raw,
                channel,
            }),
        }))
    }

    /// Fetch independent status groups concurrently through the in-cluster API proxy.
    pub async fn node(&self, node: &str) -> Result<TalosNode, TalosError> {
        let channel = self.current_channel().await?;
        let (version, services, mounts, network, disks, inventory, volumes, links, addresses) = tokio::join!(
            Self::call(&channel, node, |mut c, r| async move { c.version(r).await }),
            Self::call(
                &channel,
                node,
                |mut c, r| async move { c.service_list(r).await }
            ),
            Self::call(&channel, node, |mut c, r| async move { c.mounts(r).await }),
            Self::call(&channel, node, |mut c, r| async move {
                c.network_device_stats(r).await
            }),
            Self::call(
                &channel,
                node,
                |mut c, r| async move { c.disk_stats(r).await }
            ),
            cosi::list(channel.clone(), node, "runtime", "Disks.block.talos.dev"),
            cosi::list(
                channel.clone(),
                node,
                "runtime",
                "VolumeStatuses.block.talos.dev"
            ),
            cosi::list(
                channel.clone(),
                node,
                "network",
                "LinkStatuses.net.talos.dev"
            ),
            cosi::list(
                channel.clone(),
                node,
                "network",
                "AddressStatuses.net.talos.dev"
            ),
        );

        let mut errors = BTreeMap::new();
        let version = match version {
            Ok(response) => {
                if let Some(error) = metadata_failure(
                    response
                        .messages
                        .iter()
                        .map(|message| message.metadata.as_ref()),
                ) {
                    errors.insert("version".into(), error);
                    String::new()
                } else {
                    response
                        .messages
                        .first()
                        .and_then(|m| m.version.as_ref())
                        .map(|v| v.tag.clone())
                        .unwrap_or_default()
                }
            }
            Err(error) => {
                errors.insert("version".into(), error.to_string());
                String::new()
            }
        };
        let services = match services {
            Ok(response) => {
                if let Some(error) = metadata_failure(
                    response
                        .messages
                        .iter()
                        .map(|message| message.metadata.as_ref()),
                ) {
                    errors.insert("services".into(), error);
                    Vec::new()
                } else {
                    response
                        .messages
                        .into_iter()
                        .flat_map(|m| m.services)
                        .map(|s| {
                            let health = s.health.as_ref();
                            TalosService {
                                id: s.id,
                                state: s.state,
                                healthy: health.is_some_and(|h| h.healthy),
                                message: health.map(|h| h.last_message.clone()).unwrap_or_default(),
                                health_unknown: health.is_none_or(|h| h.unknown),
                                last_change: health
                                    .and_then(|h| h.last_change.as_ref())
                                    .map(timestamp),
                                events: s
                                    .events
                                    .map(|events| {
                                        events
                                            .events
                                            .into_iter()
                                            .map(|event| TalosServiceEvent {
                                                state: event.state,
                                                message: event.msg,
                                                timestamp: event.ts.as_ref().map(timestamp),
                                            })
                                            .collect()
                                    })
                                    .unwrap_or_default(),
                            }
                        })
                        .collect()
                }
            }
            Err(error) => {
                errors.insert("services".into(), error.to_string());
                Vec::new()
            }
        };
        let mounts = match mounts {
            Ok(response) => {
                if let Some(error) = metadata_failure(
                    response
                        .messages
                        .iter()
                        .map(|message| message.metadata.as_ref()),
                ) {
                    errors.insert("mounts".into(), error);
                    Vec::new()
                } else {
                    response
                        .messages
                        .into_iter()
                        .flat_map(|m| m.stats)
                        .map(|m| TalosMount {
                            filesystem: m.filesystem,
                            mounted_on: m.mounted_on,
                            size: m.size,
                            available: m.available,
                        })
                        .collect()
                }
            }
            Err(error) => {
                errors.insert("mounts".into(), error.to_string());
                Vec::new()
            }
        };
        let network_stats: Vec<TalosNetworkInterface> = match network {
            Ok(response) => {
                if let Some(error) = metadata_failure(
                    response
                        .messages
                        .iter()
                        .map(|message| message.metadata.as_ref()),
                ) {
                    errors.insert("network".into(), error);
                    Vec::new()
                } else {
                    response
                        .messages
                        .into_iter()
                        .flat_map(|m| m.devices)
                        .map(|d| TalosNetworkInterface {
                            name: d.name,
                            addresses: Vec::new(),
                            link_up: None,
                            operational_state: None,
                            hardware_address: None,
                            mtu: None,
                            speed_mbps: None,
                            duplex: None,
                            kind: None,
                            rx_bytes: d.rx_bytes,
                            tx_bytes: d.tx_bytes,
                            rx_errors: d.rx_errors,
                            tx_errors: d.tx_errors,
                            rx_dropped: d.rx_dropped,
                            tx_dropped: d.tx_dropped,
                        })
                        .collect()
                }
            }
            Err(error) => {
                errors.insert("network".into(), error.to_string());
                Vec::new()
            }
        };
        let disks = match disks {
            Ok(response) => {
                if let Some(error) = metadata_failure(
                    response
                        .messages
                        .iter()
                        .map(|message| message.metadata.as_ref()),
                ) {
                    errors.insert("disks".into(), error);
                    Vec::new()
                } else {
                    response
                        .messages
                        .into_iter()
                        .flat_map(|m| m.devices)
                        .map(|d| TalosDiskStat {
                            name: d.name,
                            read_bytes: d.read_sectors.saturating_mul(512),
                            write_bytes: d.write_sectors.saturating_mul(512),
                            reads: d.read_completed,
                            writes: d.write_completed,
                            io_in_progress: d.io_in_progress,
                            io_time_ms: d.io_time_ms,
                        })
                        .collect()
                }
            }
            Err(error) => {
                errors.insert("disks".into(), error.to_string());
                Vec::new()
            }
        };

        let disk_inventory = match inventory {
            Ok(resources) => resources
                .into_iter()
                .filter(|resource| {
                    !bool_field(&resource.spec, "cdrom")
                        && (!string_field(&resource.spec, "model").is_empty()
                            || !string_field(&resource.spec, "transport").is_empty())
                })
                .map(|resource| TalosDisk {
                    name: resource.id,
                    path: string_field(&resource.spec, "dev_path"),
                    size: u64_field(&resource.spec, "size"),
                    model: optional_string_field(&resource.spec, "model"),
                    serial: optional_string_field(&resource.spec, "serial"),
                    transport: optional_string_field(&resource.spec, "transport"),
                    wwid: optional_string_field(&resource.spec, "wwid"),
                    rotational: bool_field(&resource.spec, "rotational"),
                    readonly: bool_field(&resource.spec, "readonly"),
                })
                .collect(),
            Err(error) => {
                errors.insert("disk inventory".into(), error.to_string());
                Vec::new()
            }
        };

        let volumes = match volumes {
            Ok(resources) => resources
                .into_iter()
                .filter(|resource| string_field(&resource.spec, "type") == "partition")
                .map(|resource| {
                    let path = string_field(&resource.spec, "location");
                    let target = resource
                        .spec
                        .pointer("/mountSpec/targetPath")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default();
                    let usage = mounts.iter().find(|mount| {
                        mount.filesystem == path
                            || (!target.is_empty() && mount.mounted_on == target)
                    });
                    let encryption = optional_string_field(&resource.spec, "encryptionProvider")
                        .filter(|provider| provider != "none");
                    TalosVolume {
                        name: resource.id,
                        path,
                        parent_path: optional_string_field(&resource.spec, "parentLocation"),
                        partition_index: u32_field(&resource.spec, "partitionIndex"),
                        size: u64_field(&resource.spec, "size"),
                        filesystem: optional_string_field(&resource.spec, "filesystem")
                            .filter(|filesystem| filesystem != "none"),
                        phase: string_field(&resource.spec, "phase"),
                        encryption,
                        used_bytes: usage.map(|mount| mount.size.saturating_sub(mount.available)),
                        available_bytes: usage.map(|mount| mount.available),
                    }
                })
                .collect(),
            Err(error) => {
                errors.insert("volumes".into(), error.to_string());
                Vec::new()
            }
        };

        let mut interface_map: BTreeMap<String, TalosNetworkInterface> = network_stats
            .into_iter()
            .map(|interface| (interface.name.clone(), interface))
            .collect();
        match links {
            Ok(resources) => {
                for resource in resources {
                    let name = resource.id;
                    let interface = interface_map
                        .entry(name.clone())
                        .or_insert_with(|| empty_interface(name));
                    interface.link_up = Some(bool_field(&resource.spec, "linkState"));
                    interface.operational_state =
                        optional_string_field(&resource.spec, "operationalState");
                    interface.hardware_address =
                        optional_string_field(&resource.spec, "hardwareAddr");
                    interface.mtu = u32_field(&resource.spec, "mtu");
                    interface.speed_mbps = u32_field(&resource.spec, "speedMbit")
                        .filter(|speed| *speed != u32::MAX && *speed > 0);
                    interface.duplex = optional_string_field(&resource.spec, "duplex")
                        .filter(|duplex| duplex != "Unknown");
                    interface.kind = optional_string_field(&resource.spec, "kind");
                }
            }
            Err(error) => {
                errors.insert("network links".into(), error.to_string());
            }
        }
        match addresses {
            Ok(resources) => {
                for resource in resources {
                    let name = string_field(&resource.spec, "linkName");
                    let address = string_field(&resource.spec, "address");
                    if name.is_empty() || address.is_empty() {
                        continue;
                    }
                    interface_map
                        .entry(name.clone())
                        .or_insert_with(|| empty_interface(name))
                        .addresses
                        .push(address);
                }
            }
            Err(error) => {
                errors.insert("network addresses".into(), error.to_string());
            }
        }
        let interfaces = interface_map
            .into_values()
            .filter(interesting_interface)
            .collect();

        if errors.len() == 9 {
            return Err(TalosError::Upstream(
                "all Talos status sections failed".into(),
            ));
        }

        Ok(TalosNode {
            node: node.to_string(),
            version,
            control_plane: false,
            services,
            mounts,
            interfaces,
            disk_inventory,
            volumes,
            disks,
            config_fingerprint: None,
            errors,
        })
    }

    /// Return a non-secret projection of the active machine configuration.
    pub async fn config_snapshot(&self, node: &str) -> Result<ConfigSnapshot, TalosError> {
        let resources = cosi::list(
            self.current_channel().await?,
            node,
            "config",
            "MachineConfigs.config.talos.dev",
        )
        .await?;
        let config = resources.into_iter().next().ok_or_else(|| {
            TalosError::Upstream("machine configuration resource is empty".into())
        })?;
        Ok(config_snapshot(&config.spec))
    }

    pub async fn dmesg(
        &self,
        node: &str,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<String, TalosError>> + Send>>, TalosError> {
        let channel = self.current_channel().await?;
        let snapshot_request = targeted_with_timeout(
            node,
            DmesgRequest {
                follow: false,
                tail: false,
            },
            DMESG_TIMEOUT,
        )?;
        let mut snapshot_stream = MachineServiceClient::new(channel.clone())
            .dmesg(snapshot_request)
            .await?
            .into_inner();
        let initial = tokio::time::timeout(DMESG_TIMEOUT, async {
            let mut lines = VecDeque::with_capacity(DMESG_INITIAL_LINES);
            while let Some(chunk) = snapshot_stream.message().await? {
                if let Some(error) = metadata_failure(std::iter::once(chunk.metadata.as_ref())) {
                    return Err(TalosError::Upstream(error));
                }
                if lines.len() == DMESG_INITIAL_LINES {
                    lines.pop_front();
                }
                lines.push_back(dmesg_line(&chunk.bytes));
            }
            Ok::<_, TalosError>(lines)
        })
        .await
        .map_err(|_| TalosError::Timeout("dmesg snapshot".into()))??;

        let follow_request = targeted_unbounded(
            node,
            DmesgRequest {
                follow: true,
                tail: true,
            },
        )?;

        Ok(Box::pin(async_stream::try_stream! {
            for line in initial {
                yield line;
            }
            let mut stream = MachineServiceClient::new(channel)
                .dmesg(follow_request)
                .await?
                .into_inner();
            while let Some(chunk) = stream.message().await? {
                if let Some(error) = metadata_failure(std::iter::once(chunk.metadata.as_ref())) {
                    Err(TalosError::Upstream(error))?;
                }
                yield dmesg_line(&chunk.bytes);
            }
        }))
    }

    pub async fn service_action(
        &self,
        node: &str,
        service: &str,
        action: &str,
    ) -> Result<(), TalosError> {
        let channel = self.current_channel().await?;
        let mut client = MachineServiceClient::new(channel);
        match action {
            "start" => {
                let response = client
                    .service_start(targeted(node, ServiceStartRequest { id: service.into() })?)
                    .await?
                    .into_inner();
                ensure_metadata(
                    response
                        .messages
                        .iter()
                        .map(|message| message.metadata.as_ref()),
                )?;
            }
            "stop" => {
                let response = client
                    .service_stop(targeted(node, ServiceStopRequest { id: service.into() })?)
                    .await?
                    .into_inner();
                ensure_metadata(
                    response
                        .messages
                        .iter()
                        .map(|message| message.metadata.as_ref()),
                )?;
            }
            "restart" => {
                let response = client
                    .service_restart(targeted(
                        node,
                        ServiceRestartRequest { id: service.into() },
                    )?)
                    .await?
                    .into_inner();
                ensure_metadata(
                    response
                        .messages
                        .iter()
                        .map(|message| message.metadata.as_ref()),
                )?;
            }
            _ => {
                return Err(TalosError::Config(format!(
                    "unknown service action {action:?}"
                )))
            }
        }
        Ok(())
    }

    pub async fn power_action(&self, node: &str, action: &str) -> Result<(), TalosError> {
        let channel = self.current_channel().await?;
        let mut client = MachineServiceClient::new(channel);
        match action {
            "reboot" => {
                let response = client
                    .reboot(targeted(node, RebootRequest { mode: 0 })?)
                    .await?
                    .into_inner();
                ensure_metadata(
                    response
                        .messages
                        .iter()
                        .map(|message| message.metadata.as_ref()),
                )?;
            }
            "shutdown" => {
                let response = client
                    .shutdown(targeted(node, ShutdownRequest { force: false })?)
                    .await?
                    .into_inner();
                ensure_metadata(
                    response
                        .messages
                        .iter()
                        .map(|message| message.metadata.as_ref()),
                )?;
            }
            _ => {
                return Err(TalosError::Config(format!(
                    "unknown power action {action:?}"
                )))
            }
        }
        Ok(())
    }

    async fn current_channel(&self) -> Result<Channel, TalosError> {
        let raw = std::fs::read_to_string(&self.config_path)?;
        {
            let current = self.connection.read().await;
            if current.config == raw {
                return Ok(current.channel.clone());
            }
        }

        let channel = connect(&raw).await?;
        let mut current = self.connection.write().await;
        if current.config != raw {
            current.config = raw;
            current.channel = channel;
        }
        Ok(current.channel.clone())
    }

    async fn call<T, F, Fut>(channel: &Channel, node: &str, call: F) -> Result<T, TalosError>
    where
        F: FnOnce(MachineServiceClient<Channel>, Request<()>) -> Fut,
        Fut: std::future::Future<Output = Result<tonic::Response<T>, tonic::Status>>,
    {
        let value: MetadataValue<_> = node
            .parse()
            .map_err(|_| TalosError::Config(format!("invalid node target {node:?}")))?;
        let mut request = Request::new(());
        request.set_timeout(RPC_TIMEOUT);
        request.metadata_mut().insert("x-talos-node", value);
        Ok(call(MachineServiceClient::new(channel.clone()), request)
            .await?
            .into_inner())
    }
}

fn talos_config_path() -> Option<PathBuf> {
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

fn targeted<T>(node: &str, body: T) -> Result<Request<T>, TalosError> {
    targeted_with_timeout(node, body, RPC_TIMEOUT)
}

fn targeted_unbounded<T>(node: &str, body: T) -> Result<Request<T>, TalosError> {
    let value: MetadataValue<_> = node
        .parse()
        .map_err(|_| TalosError::Config(format!("invalid node target {node:?}")))?;
    let mut request = Request::new(body);
    request.metadata_mut().insert("x-talos-node", value);
    Ok(request)
}

fn targeted_with_timeout<T>(
    node: &str,
    body: T,
    timeout: Duration,
) -> Result<Request<T>, TalosError> {
    let value: MetadataValue<_> = node
        .parse()
        .map_err(|_| TalosError::Config(format!("invalid node target {node:?}")))?;
    let mut request = Request::new(body);
    request.set_timeout(timeout);
    request.metadata_mut().insert("x-talos-node", value);
    Ok(request)
}

fn timestamp(ts: &prost_types::Timestamp) -> String {
    format!("{}.{:09}Z", ts.seconds, ts.nanos)
}

fn empty_interface(name: String) -> TalosNetworkInterface {
    TalosNetworkInterface {
        name,
        addresses: Vec::new(),
        link_up: None,
        operational_state: None,
        hardware_address: None,
        mtu: None,
        speed_mbps: None,
        duplex: None,
        kind: None,
        rx_bytes: 0,
        tx_bytes: 0,
        rx_errors: 0,
        tx_errors: 0,
        rx_dropped: 0,
        tx_dropped: 0,
    }
}

fn interesting_interface(interface: &TalosNetworkInterface) -> bool {
    let virtual_prefixes = ["cilium_", "lxc", "veth", "dummy", "ip6tnl", "tunl"];
    if interface.name == "lo"
        || virtual_prefixes
            .iter()
            .any(|prefix| interface.name.starts_with(prefix))
    {
        return false;
    }
    !interface.addresses.is_empty()
        || interface
            .hardware_address
            .as_deref()
            .is_some_and(|address| address != "00:00:00:00:00:00")
        || matches!(interface.kind.as_deref(), Some("bond" | "bridge" | "vlan"))
}

fn string_field(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string()
}

fn optional_string_field(value: &serde_json::Value, key: &str) -> Option<String> {
    let value = string_field(value, key);
    (!value.is_empty()).then_some(value)
}

fn u64_field(value: &serde_json::Value, key: &str) -> u64 {
    value.get(key).and_then(|value| value.as_u64()).unwrap_or(0)
}

fn u32_field(value: &serde_json::Value, key: &str) -> Option<u32> {
    value
        .get(key)
        .and_then(|value| value.as_u64())
        .and_then(|value| value.try_into().ok())
}

fn bool_field(value: &serde_json::Value, key: &str) -> bool {
    value
        .get(key)
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

fn config_snapshot(config: &serde_json::Value) -> ConfigSnapshot {
    const SAFE_ROOTS: &[&str] = &[
        "/machine/type",
        "/machine/install",
        "/machine/network",
        "/machine/kubelet",
        "/machine/features",
        "/machine/sysctls",
        "/machine/kernel/modules",
        "/cluster/clusterName",
        "/cluster/network",
        "/cluster/discovery",
        "/cluster/apiServer",
        "/cluster/controllerManager",
        "/cluster/scheduler",
        "/cluster/etcd",
    ];

    let mut fields = BTreeMap::new();
    for root in SAFE_ROOTS {
        if let Some(value) = config.pointer(root) {
            flatten_config(root, value, &mut fields);
        }
    }
    let mut digest = Sha256::new();
    for (path, value) in &fields {
        digest.update(path.as_bytes());
        digest.update([0]);
        match value {
            ConfigField::Plain(value) | ConfigField::Sensitive(value) => {
                digest.update(value.as_bytes())
            }
        }
        digest.update([0]);
    }
    ConfigSnapshot {
        fingerprint: format!("{:x}", digest.finalize()),
        fields,
    }
}

fn flatten_config(
    path: &str,
    value: &serde_json::Value,
    fields: &mut BTreeMap<String, ConfigField>,
) {
    if sensitive_path(path) {
        let mut digest = Sha256::new();
        digest.update(serde_json::to_vec(value).unwrap_or_default());
        fields.insert(
            path.into(),
            ConfigField::Sensitive(format!("{:x}", digest.finalize())),
        );
        return;
    }
    match value {
        serde_json::Value::Object(values) => {
            for (key, value) in values {
                flatten_config(&format!("{path}/{key}"), value, fields);
            }
        }
        serde_json::Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                flatten_config(&format!("{path}/{index}"), value, fields);
            }
        }
        serde_json::Value::String(value) => {
            fields.insert(path.into(), ConfigField::Plain(value.clone()));
        }
        value => {
            fields.insert(path.into(), ConfigField::Plain(value.to_string()));
        }
    }
}

fn sensitive_path(path: &str) -> bool {
    let key = path
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    key == "ca"
        || key.contains("token")
        || key.contains("secret")
        || key.contains("password")
        || key.contains("certificate")
        || key.ends_with("key")
        || key.ends_with("crt")
}

fn metadata_failure<'a>(items: impl IntoIterator<Item = Option<&'a Metadata>>) -> Option<String> {
    items.into_iter().flatten().find_map(|metadata| {
        if !metadata.error.is_empty() {
            Some(metadata.error.clone())
        } else {
            metadata
                .status
                .as_ref()
                .and_then(|status| (!status.message.is_empty()).then(|| status.message.clone()))
        }
    })
}

fn dmesg_line(bytes: &[u8]) -> String {
    let truncated = bytes.len() > DMESG_MAX_LINE_BYTES;
    let bytes = &bytes[..bytes.len().min(DMESG_MAX_LINE_BYTES)];
    let line = String::from_utf8_lossy(bytes)
        .trim_end_matches(&['\r', '\n'][..])
        .to_string();
    if truncated {
        format!("{line} [line truncated]")
    } else {
        line
    }
}

fn ensure_metadata<'a>(
    items: impl IntoIterator<Item = Option<&'a Metadata>>,
) -> Result<(), TalosError> {
    match metadata_failure(items) {
        Some(error) => Err(TalosError::Upstream(error)),
        None => Ok(()),
    }
}

async fn connect(raw: &str) -> Result<Channel, TalosError> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt as _;

    #[test]
    fn targeted_request_sets_node_metadata() {
        let request = targeted("worker-1", ()).unwrap();
        assert_eq!(request.metadata().get("x-talos-node").unwrap(), "worker-1");
    }

    #[test]
    fn targeted_request_rejects_invalid_metadata() {
        assert!(targeted("bad\nnode", ()).is_err());
    }

    #[test]
    fn formats_protobuf_timestamp() {
        assert_eq!(
            timestamp(&prost_types::Timestamp {
                seconds: 1_700_000_000,
                nanos: 42,
            }),
            "1700000000.000000042Z"
        );
    }

    #[test]
    fn reports_proxy_metadata_errors() {
        let metadata = Metadata {
            hostname: "worker-1".into(),
            error: "permission denied".into(),
            status: None,
        };
        assert_eq!(
            metadata_failure([Some(&metadata)]).as_deref(),
            Some("permission denied")
        );
    }

    #[test]
    fn config_snapshot_redacts_sensitive_values() {
        let first = config_snapshot(&serde_json::json!({
            "machine": {
                "type": "controlplane",
                "network": { "hostname": "node-1", "wireguard": { "privateKey": "secret-a" } }
            },
            "cluster": { "clusterName": "prod" }
        }));
        let second = config_snapshot(&serde_json::json!({
            "machine": {
                "type": "controlplane",
                "network": { "hostname": "node-1", "wireguard": { "privateKey": "secret-b" } }
            },
            "cluster": { "clusterName": "prod" }
        }));
        assert!(matches!(
            first.fields.get("/machine/network/wireguard/privateKey"),
            Some(ConfigField::Sensitive(_))
        ));
        assert_ne!(first.fingerprint, second.fingerprint);
        assert!(!format!("{first:?}").contains("secret-a"));
    }

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
