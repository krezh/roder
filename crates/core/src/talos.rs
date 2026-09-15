//! Talos node payloads — status, services, disks, network, config diffs.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Read-only status returned directly by a Talos node through the machine API.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TalosNode {
    pub node: String,
    pub version: String,
    pub control_plane: bool,
    pub services: Vec<TalosService>,
    pub mounts: Vec<TalosMount>,
    pub interfaces: Vec<TalosNetworkInterface>,
    pub disk_inventory: Vec<TalosDisk>,
    pub volumes: Vec<TalosVolume>,
    pub disks: Vec<TalosDiskStat>,
    pub config_fingerprint: Option<String>,
    /// Per-section upstream failures; successful sections remain populated.
    pub errors: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TalosDmesg {
    pub log: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TalosCapabilities {
    pub read: bool,
    pub actions: bool,
    pub config: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TalosService {
    pub id: String,
    pub state: String,
    pub healthy: bool,
    pub message: String,
    pub health_unknown: bool,
    pub last_change: Option<String>,
    pub events: Vec<TalosServiceEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TalosServiceEvent {
    pub state: String,
    pub message: String,
    pub timestamp: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TalosMount {
    pub filesystem: String,
    pub mounted_on: String,
    pub size: u64,
    pub available: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TalosNetworkInterface {
    pub name: String,
    pub addresses: Vec<String>,
    pub link_up: Option<bool>,
    pub operational_state: Option<String>,
    pub hardware_address: Option<String>,
    pub mtu: Option<u32>,
    pub speed_mbps: Option<u32>,
    pub duplex: Option<String>,
    pub kind: Option<String>,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_errors: u64,
    pub tx_errors: u64,
    pub rx_dropped: u64,
    pub tx_dropped: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TalosDisk {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub model: Option<String>,
    pub serial: Option<String>,
    pub transport: Option<String>,
    pub wwid: Option<String>,
    pub rotational: bool,
    pub readonly: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TalosVolume {
    pub name: String,
    pub path: String,
    pub parent_path: Option<String>,
    pub partition_index: Option<u32>,
    pub size: u64,
    pub filesystem: Option<String>,
    pub phase: String,
    pub encryption: Option<String>,
    pub used_bytes: Option<u64>,
    pub available_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TalosDiskStat {
    pub name: String,
    pub read_bytes: u64,
    pub write_bytes: u64,
    pub reads: u64,
    pub writes: u64,
    pub io_in_progress: u64,
    pub io_time_ms: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TalosConfigDiff {
    pub node: String,
    pub fingerprint: String,
    pub peers: Vec<TalosConfigPeerDiff>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TalosConfigPeerDiff {
    pub node: String,
    pub fingerprint: Option<String>,
    pub matches: Option<bool>,
    pub differences: Vec<TalosConfigDifference>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TalosConfigDifference {
    pub path: String,
    pub node_value: Option<String>,
    pub peer_value: Option<String>,
    pub sensitive: bool,
}
