//! Cluster liveness and the category a kind is filed under in the sidebar.

use serde::{Deserialize, Serialize};

/// Liveness/readiness payload served at `/health`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Health {
    pub status: HealthStatus,
    pub version: &'static str,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    Ok,
    Degraded,
}

impl Health {
    pub fn ok() -> Self {
        Self {
            status: HealthStatus::Ok,
            version: env!("CARGO_PKG_VERSION"),
        }
    }
}

/// Navigation grouping for a resource kind in the sidebar.
///
/// Fixed variants map to well-known k8s API groups. `Custom(String)` carries
/// the base domain of the CRD's API group (e.g. `"coreos.com"`) so that each
/// third-party operator gets its own collapsible section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum Category {
    Workloads,
    Config,
    Network,
    Storage,
    Rbac,
    Flux,
    ExternalSecrets,
    CertManager,
    Rook,
    CloudNativePg,
    PrometheusOperator,
    Cluster,
    Custom(String),
}

impl Category {
    pub fn label(&self) -> String {
        match self {
            Category::Workloads => "Workloads".into(),
            Category::Config => "Config".into(),
            Category::Network => "Network".into(),
            Category::Storage => "Storage".into(),
            Category::Rbac => "RBAC".into(),
            Category::Flux => "Flux".into(),
            Category::ExternalSecrets => "External Secrets".into(),
            Category::CertManager => "cert-manager".into(),
            Category::Rook => "Rook Ceph".into(),
            Category::CloudNativePg => "CloudNativePG".into(),
            Category::PrometheusOperator => "Prometheus Operator".into(),
            Category::Cluster => "Cluster".into(),
            Category::Custom(name) => name.clone(),
        }
    }

    /// Stable display ordering of categories in the sidebar.
    pub fn order(&self) -> u8 {
        match self {
            Category::Cluster => 0,
            Category::Workloads => 1,
            Category::Config => 2,
            Category::Network => 3,
            Category::Storage => 4,
            Category::Rbac => 5,
            Category::Flux => 6,
            Category::ExternalSecrets => 7,
            Category::CertManager => 8,
            Category::Rook => 9,
            Category::CloudNativePg => 10,
            Category::PrometheusOperator => 11,
            Category::Custom(_) => 12,
        }
    }

    /// True for dynamically-derived categories (third-party CRD groups).
    pub fn is_dynamic(&self) -> bool {
        matches!(self, Category::Custom(_))
    }
}
