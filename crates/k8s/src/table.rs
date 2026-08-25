//! Low-level transport for Kubernetes `meta.k8s.io/v1` Table responses.

use std::pin::Pin;

use futures::{AsyncBufReadExt, Stream, StreamExt};
use http::header::ACCEPT;
use kube::api::{DynamicObject, ListParams, WatchParams};
use kube::core::{request::Request, ApiResource, Resource};
use kube::Client;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use thiserror::Error;

const TABLE_ACCEPT: &str = "application/json;as=Table;v=v1;g=meta.k8s.io";

fn null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TableListMeta {
    #[serde(default)]
    pub resource_version: String,
    #[serde(default, rename = "continue")]
    pub continue_: String,
    #[serde(default)]
    pub remaining_item_count: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TableColumnDefinition {
    #[serde(default)]
    pub name: String,
    #[serde(default, rename = "type")]
    pub type_: String,
    #[serde(default)]
    pub format: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub priority: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct TableRowCondition {
    #[serde(default, rename = "type")]
    pub type_: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub message: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct TableRow {
    #[serde(default, deserialize_with = "null_default")]
    pub cells: Vec<Value>,
    #[serde(default, deserialize_with = "null_default")]
    pub conditions: Vec<TableRowCondition>,
    #[serde(default)]
    pub object: Option<DynamicObject>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Table {
    #[serde(default)]
    pub api_version: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub metadata: TableListMeta,
    #[serde(default, deserialize_with = "null_default")]
    pub column_definitions: Vec<TableColumnDefinition>,
    #[serde(default, deserialize_with = "null_default")]
    pub rows: Vec<TableRow>,
}

impl Table {
    pub(crate) fn resource_version(&self) -> Option<&str> {
        if !self.metadata.resource_version.is_empty() {
            return Some(&self.metadata.resource_version);
        }
        self.rows
            .iter()
            .filter_map(|row| row.object.as_ref())
            .find_map(|object| object.metadata.resource_version.as_deref())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TableStatus {
    #[serde(default)]
    pub api_version: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub metadata: TableListMeta,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub details: Option<Value>,
    #[serde(default)]
    pub code: u16,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "object", rename_all = "UPPERCASE")]
pub(crate) enum TableWatchEvent {
    Added(Table),
    Modified(Table),
    Deleted(Table),
    Bookmark(Table),
    Error(TableStatus),
}

#[derive(Debug, Error)]
pub(crate) enum TableError {
    #[error("failed to build Kubernetes Table request: {0}")]
    Build(#[from] kube::core::request::Error),
    #[error("invalid Kubernetes Table request URI: {0}")]
    Uri(#[from] http::uri::InvalidUri),
    #[error("Kubernetes Table request failed: {0}")]
    Client(#[from] kube::Error),
    #[error("failed to read Kubernetes Table watch: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to decode Kubernetes Table watch event: {0}")]
    Json(#[from] serde_json::Error),
}

impl TableError {
    pub(crate) fn status_code(&self) -> Option<u16> {
        match self {
            Self::Client(kube::Error::Api(response)) => Some(response.code),
            _ => None,
        }
    }

    pub(crate) fn is_permanent(&self) -> bool {
        matches!(self, Self::Build(_) | Self::Uri(_) | Self::Json(_))
            || self
                .status_code()
                .is_some_and(|code| (400..500).contains(&code) && code != 401 && code != 403)
    }
}

pub(crate) type TableWatchStream =
    Pin<Box<dyn Stream<Item = Result<TableWatchEvent, TableError>> + Send>>;

#[derive(Clone)]
pub(crate) struct TableApi {
    client: Client,
    request: Request,
}

impl TableApi {
    pub fn new(client: Client, resource: &ApiResource, namespace: Option<&str>) -> Self {
        let path = DynamicObject::url_path(resource, namespace);
        Self {
            client,
            request: Request::new(path),
        }
    }

    pub async fn list(&self, params: &ListParams) -> Result<Table, TableError> {
        let request = self.list_request(params)?;
        Ok(self.client.request(request).await?)
    }

    pub async fn watch(
        &self,
        params: &WatchParams,
        resource_version: &str,
    ) -> Result<TableWatchStream, TableError> {
        let request = self.watch_request(params, resource_version)?;
        let reader = self.client.request_stream(request).await?;
        let events = reader.lines().map(|line| {
            let line = line?;
            Ok(serde_json::from_str(&line)?)
        });
        Ok(Box::pin(events))
    }

    fn list_request(&self, params: &ListParams) -> Result<http::Request<Vec<u8>>, TableError> {
        table_request(self.request.list(params)?)
    }

    fn watch_request(
        &self,
        params: &WatchParams,
        resource_version: &str,
    ) -> Result<http::Request<Vec<u8>>, TableError> {
        table_request(self.request.watch(params, resource_version)?)
    }
}

fn table_request(
    mut request: http::Request<Vec<u8>>,
) -> Result<http::Request<Vec<u8>>, TableError> {
    let uri = request.uri().to_string().replacen("?&", "?", 1);
    let separator = if uri.ends_with('?') { "" } else { "&" };
    *request.uri_mut() = format!("{uri}{separator}includeObject=Object").parse()?;
    request
        .headers_mut()
        .insert(ACCEPT, http::HeaderValue::from_static(TABLE_ACCEPT));
    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::{TableApi, TableWatchEvent, TABLE_ACCEPT};
    use http::header::{ACCEPT, CONTENT_TYPE};
    use kube::api::{ListParams, WatchParams};
    use kube::core::ApiResource;
    use serde_json::{json, Value};

    fn resource(group: &str, version: &str, kind: &str, plural: &str) -> ApiResource {
        ApiResource {
            group: group.into(),
            version: version.into(),
            api_version: if group.is_empty() {
                version.into()
            } else {
                format!("{group}/{version}")
            },
            kind: kind.into(),
            plural: plural.into(),
        }
    }

    fn api(resource: &ApiResource, namespace: Option<&str>) -> TableApi {
        let client = crate::client::ClusterAccess::for_test()
            .client()
            .as_ref()
            .clone();
        TableApi::new(client, resource, namespace)
    }

    #[test]
    fn deserializes_table_rows_and_list_metadata() {
        let table: super::Table = serde_json::from_value(json!({
            "apiVersion": "meta.k8s.io/v1",
            "kind": "Table",
            "metadata": {
                "resourceVersion": "42",
                "continue": "next-page",
                "remainingItemCount": 3
            },
            "columnDefinitions": [{
                "name": "Name",
                "type": "string",
                "format": "name",
                "description": "object name",
                "priority": 0
            }],
            "rows": [{
                "cells": ["pod-a", 2, true, null],
                "conditions": [{"type": "Ready", "status": "True"}],
                "object": {
                    "apiVersion": "v1",
                    "kind": "Pod",
                    "metadata": {"name": "pod-a", "namespace": "team-a"},
                    "spec": {"nodeName": "node-a"}
                }
            }]
        }))
        .unwrap();

        assert_eq!(table.metadata.resource_version, "42");
        assert_eq!(table.metadata.continue_, "next-page");
        assert_eq!(table.metadata.remaining_item_count, Some(3));
        assert_eq!(table.column_definitions[0].type_, "string");
        assert_eq!(
            table.rows[0].cells,
            [json!("pod-a"), json!(2), json!(true), Value::Null]
        );
        let object = table.rows[0].object.as_ref().unwrap();
        assert_eq!(object.metadata.name.as_deref(), Some("pod-a"));
        assert_eq!(object.data["spec"]["nodeName"], "node-a");
    }

    #[test]
    fn deserializes_watch_events_bookmarks_and_status() {
        let added: TableWatchEvent = serde_json::from_value(json!({
            "type": "ADDED",
            "object": {
                "apiVersion": "meta.k8s.io/v1",
                "kind": "Table",
                "metadata": {"resourceVersion": "8"},
                "rows": [{"cells": ["pod-a"], "object": {"metadata": {"name": "pod-a"}}}]
            }
        }))
        .unwrap();
        let bookmark: TableWatchEvent = serde_json::from_value(json!({
            "type": "BOOKMARK",
            "object": {
                "apiVersion": "meta.k8s.io/v1",
                "kind": "Table",
                "metadata": {"resourceVersion": "9"},
                "columnDefinitions": null,
                "rows": null
            }
        }))
        .unwrap();
        let error: TableWatchEvent = serde_json::from_value(json!({
            "type": "ERROR",
            "object": {
                "apiVersion": "v1",
                "kind": "Status",
                "status": "Failure",
                "message": "too old resource version",
                "reason": "Expired",
                "code": 410
            }
        }))
        .unwrap();

        assert!(matches!(added, TableWatchEvent::Added(table) if table.rows.len() == 1));
        assert!(
            matches!(bookmark, TableWatchEvent::Bookmark(mark) if mark.metadata.resource_version == "9" && mark.column_definitions.is_empty() && mark.rows.is_empty())
        );
        assert!(
            matches!(error, TableWatchEvent::Error(status) if status.code == 410 && status.reason == "Expired")
        );
    }

    #[test]
    fn table_resource_version_falls_back_to_embedded_object() {
        let table: super::Table = serde_json::from_value(json!({
            "apiVersion": "meta.k8s.io/v1",
            "kind": "Table",
            "metadata": {},
            "rows": [{
                "cells": ["pod-a"],
                "object": {"metadata": {"name": "pod-a", "resourceVersion": "17"}}
            }]
        }))
        .unwrap();
        assert_eq!(table.resource_version(), Some("17"));
    }

    #[tokio::test]
    async fn list_request_has_namespaced_path_pagination_and_table_header() {
        let ar = resource("apps", "v1", "Deployment", "deployments");
        let params = ListParams::default()
            .labels("app=roder,tier=api")
            .limit(50)
            .continue_token("next/page");
        let request = api(&ar, Some("team-a")).list_request(&params).unwrap();

        assert_eq!(request.method(), "GET");
        assert_eq!(
            request.uri().to_string(),
            "/apis/apps/v1/namespaces/team-a/deployments?labelSelector=app%3Droder%2Ctier%3Dapi&limit=50&continue=next%2Fpage&includeObject=Object"
        );
        assert_eq!(request.headers()[ACCEPT], TABLE_ACCEPT);
        assert!(!request.headers().contains_key(CONTENT_TYPE));
    }

    #[tokio::test]
    async fn list_request_supports_resource_version_on_core_all_namespace_path() {
        let ar = resource("", "v1", "Pod", "pods");
        let request = api(&ar, None)
            .list_request(&ListParams::default().at("123"))
            .unwrap();

        assert_eq!(
            request.uri().to_string(),
            "/api/v1/pods?resourceVersion=123&includeObject=Object"
        );
        assert_eq!(request.headers()[ACCEPT], TABLE_ACCEPT);
    }

    #[tokio::test]
    async fn watch_request_has_selector_version_bookmarks_and_table_options() {
        let ar = resource("example.com", "v1", "Widget", "widgets");
        let params = WatchParams::default()
            .labels("environment=prod")
            .timeout(30);
        let request = api(&ar, Some("team-a"))
            .watch_request(&params, "456")
            .unwrap();

        assert_eq!(request.method(), "GET");
        assert_eq!(
            request.uri().to_string(),
            "/apis/example.com/v1/namespaces/team-a/widgets?watch=true&timeoutSeconds=30&labelSelector=environment%3Dprod&allowWatchBookmarks=true&resourceVersion=456&includeObject=Object"
        );
        assert_eq!(request.headers()[ACCEPT], TABLE_ACCEPT);
        assert!(!request.headers().contains_key(CONTENT_TYPE));
    }

    #[tokio::test]
    #[ignore]
    async fn live_cluster_serves_service_tables_and_watch() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let access = crate::client::ClusterAccess::connect_with_default()
            .await
            .expect("connect to current cluster");
        let ar = resource("", "v1", "Service", "services");
        let api = TableApi::new((*access.client()).clone(), &ar, Some("kube-system"));
        let table = api.list(&ListParams::default().limit(500)).await.unwrap();
        let columns = table
            .column_definitions
            .iter()
            .filter(|column| column.priority == 0)
            .map(|column| column.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            columns,
            [
                "Name",
                "Type",
                "Cluster-IP",
                "External-IP",
                "Port(s)",
                "Age"
            ]
        );
        assert!(table.rows.iter().all(|row| row.object.is_some()));
        let _stream = api
            .watch(
                &WatchParams::default().timeout(1),
                &table.metadata.resource_version,
            )
            .await
            .expect("start Table watch");
    }
}
