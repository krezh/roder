//! Fetching independent Talos status groups (version, services, mounts,
//! network, disks, disk inventory, volumes) concurrently through the
//! in-cluster API proxy, and assembling them into one `TalosNode`.

use std::collections::BTreeMap;

use roder_core::{
    TalosDisk, TalosDiskStat, TalosMount, TalosNetworkInterface, TalosNode, TalosService,
    TalosServiceEvent, TalosVolume,
};

use crate::cosi;
use crate::error::TalosError;
use crate::request::{metadata_failure, timestamp};
use crate::Backend;

impl Backend {
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
