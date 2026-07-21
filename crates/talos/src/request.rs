//! Low-level gRPC request helpers shared by every Machine API call: targeting
//! a specific node via the `nodes` metadata header, and interpreting
//! the per-node status embedded in a proxied response.

use std::time::Duration;

use talos_api_rs::api::common::Metadata;
use talos_api_rs::api::machine::machine_service_client::MachineServiceClient;
use tonic::metadata::MetadataValue;
use tonic::transport::Channel;
use tonic::Request;

use crate::error::TalosError;
use crate::Backend;

const RPC_TIMEOUT: Duration = Duration::from_secs(10);
const NODES_METADATA_KEY: &str = "nodes";
const NODE_METADATA_KEY: &str = "node";

pub(crate) fn targeted_single<T>(node: &str, body: T) -> Result<Request<T>, TalosError> {
    let value: MetadataValue<_> = node
        .parse()
        .map_err(|_| TalosError::Config(format!("invalid node target {node:?}")))?;
    let mut request = Request::new(body);
    request.set_timeout(RPC_TIMEOUT);
    request.metadata_mut().insert(NODE_METADATA_KEY, value);
    Ok(request)
}

pub(crate) fn targeted<T>(node: &str, body: T) -> Result<Request<T>, TalosError> {
    targeted_with_timeout(node, body, RPC_TIMEOUT)
}

pub(crate) fn targeted_unbounded<T>(node: &str, body: T) -> Result<Request<T>, TalosError> {
    let value: MetadataValue<_> = node
        .parse()
        .map_err(|_| TalosError::Config(format!("invalid node target {node:?}")))?;
    let mut request = Request::new(body);
    request.metadata_mut().insert(NODES_METADATA_KEY, value);
    Ok(request)
}

pub(crate) fn targeted_with_timeout<T>(
    node: &str,
    body: T,
    timeout: Duration,
) -> Result<Request<T>, TalosError> {
    let value: MetadataValue<_> = node
        .parse()
        .map_err(|_| TalosError::Config(format!("invalid node target {node:?}")))?;
    let mut request = Request::new(body);
    request.set_timeout(timeout);
    request.metadata_mut().insert(NODES_METADATA_KEY, value);
    Ok(request)
}

pub(crate) fn timestamp(ts: &prost_types::Timestamp) -> Result<String, TalosError> {
    let nanos = u32::try_from(ts.nanos)
        .ok()
        .filter(|nanos| *nanos < 1_000_000_000)
        .ok_or_else(|| TalosError::Upstream("invalid protobuf timestamp nanoseconds".into()))?;
    let value = time::OffsetDateTime::from_unix_timestamp(ts.seconds)
        .and_then(|value| value.replace_nanosecond(nanos))
        .map_err(|error| TalosError::Upstream(format!("invalid protobuf timestamp: {error}")))?;
    value
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|error| TalosError::Upstream(format!("failed to format timestamp: {error}")))
}

pub(crate) fn metadata_failure<'a>(
    items: impl IntoIterator<Item = Option<&'a Metadata>>,
) -> Option<String> {
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

pub(crate) fn ensure_metadata<'a>(
    items: impl IntoIterator<Item = Option<&'a Metadata>>,
) -> Result<(), TalosError> {
    match metadata_failure(items) {
        Some(error) => Err(TalosError::Upstream(error)),
        None => Ok(()),
    }
}

impl Backend {
    pub(crate) async fn call<T, F, Fut>(
        channel: &Channel,
        node: &str,
        call: F,
    ) -> Result<T, TalosError>
    where
        F: FnOnce(MachineServiceClient<Channel>, Request<()>) -> Fut,
        Fut: std::future::Future<Output = Result<tonic::Response<T>, tonic::Status>>,
    {
        let value: MetadataValue<_> = node
            .parse()
            .map_err(|_| TalosError::Config(format!("invalid node target {node:?}")))?;
        let mut request = Request::new(());
        request.set_timeout(RPC_TIMEOUT);
        request.metadata_mut().insert(NODES_METADATA_KEY, value);
        Ok(call(MachineServiceClient::new(channel.clone()), request)
            .await?
            .into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targeted_request_sets_node_metadata() {
        let request = targeted("worker-1", ()).unwrap();
        assert_eq!(request.metadata().get("nodes").unwrap(), "worker-1");
        assert!(request.metadata().get("x-talos-node").is_none());
    }

    #[test]
    fn single_target_request_uses_one_to_one_proxying() {
        let request = targeted_single("worker-1", ()).unwrap();
        assert_eq!(request.metadata().get("node").unwrap(), "worker-1");
        assert!(request.metadata().get("nodes").is_none());
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
            })
            .unwrap(),
            "2023-11-14T22:13:20.000000042Z"
        );
    }

    #[test]
    fn rejects_invalid_protobuf_timestamp() {
        assert!(timestamp(&prost_types::Timestamp {
            seconds: 0,
            nanos: 1_000_000_000,
        })
        .is_err());
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
}
