use std::collections::HashMap;

use crate::request::targeted;
use crate::TalosError;
use tonic::codegen::http::uri::PathAndQuery;
use tonic::transport::Channel;

#[derive(Clone, PartialEq, prost::Message)]
struct ListRequest {
    #[prost(string, tag = "1")]
    namespace: String,
    #[prost(string, tag = "2")]
    r#type: String,
}

#[derive(Clone, PartialEq, prost::Message)]
struct ListResponse {
    #[prost(message, optional, tag = "1")]
    resource: Option<Resource>,
}

#[derive(Clone, PartialEq, prost::Message)]
struct Resource {
    #[prost(message, optional, tag = "1")]
    metadata: Option<ResourceMetadata>,
    #[prost(message, optional, tag = "2")]
    spec: Option<ResourceSpec>,
}

#[derive(Clone, PartialEq, prost::Message)]
struct ResourceMetadata {
    #[prost(string, tag = "1")]
    namespace: String,
    #[prost(string, tag = "2")]
    r#type: String,
    #[prost(string, tag = "3")]
    id: String,
    #[prost(string, tag = "4")]
    version: String,
    #[prost(map = "string, string", tag = "10")]
    labels: HashMap<String, String>,
    #[prost(map = "string, string", tag = "11")]
    annotations: HashMap<String, String>,
}

#[derive(Clone, PartialEq, prost::Message)]
struct ResourceSpec {
    #[prost(bytes = "vec", tag = "1")]
    proto_spec: Vec<u8>,
    #[prost(string, tag = "2")]
    yaml_spec: String,
}

pub(crate) struct CosiResource {
    pub id: String,
    pub spec: serde_json::Value,
}

pub(crate) async fn list(
    channel: Channel,
    node: &str,
    namespace: &str,
    resource_type: &str,
) -> Result<Vec<CosiResource>, TalosError> {
    let request = targeted(
        node,
        ListRequest {
            namespace: namespace.into(),
            r#type: resource_type.into(),
        },
    )?;
    let mut grpc = tonic::client::Grpc::new(channel);
    grpc.ready()
        .await
        .map_err(|error| TalosError::Upstream(error.to_string()))?;
    let response: tonic::Response<tonic::codec::Streaming<ListResponse>> = grpc
        .server_streaming(
            request,
            PathAndQuery::from_static("/cosi.resource.State/List"),
            tonic_prost::ProstCodec::default(),
        )
        .await?;
    let mut stream = response.into_inner();
    let mut resources = Vec::new();
    while let Some(item) = stream.message().await? {
        let Some(resource) = item.resource else {
            continue;
        };
        let id = resource
            .metadata
            .map(|metadata| metadata.id)
            .unwrap_or_default();
        let Some(spec) = resource.spec else {
            continue;
        };
        let mut value: serde_json::Value =
            serde_yaml::from_str(&spec.yaml_spec).map_err(|error| {
                TalosError::Upstream(format!("invalid COSI resource YAML: {error}"))
            })?;
        if let serde_json::Value::String(nested_yaml) = value {
            value = parse_nested_yaml(&nested_yaml)?;
        }
        resources.push(CosiResource { id, spec: value });
    }
    Ok(resources)
}

fn parse_nested_yaml(yaml: &str) -> Result<serde_json::Value, TalosError> {
    let documents = serde_yaml::Deserializer::from_str(yaml)
        .map(|document| {
            serde::Deserialize::deserialize(document).map_err(|error| {
                TalosError::Upstream(format!("invalid nested COSI resource YAML: {error}"))
            })
        })
        .collect::<Result<Vec<serde_json::Value>, _>>()?;

    match documents.len() {
        0 => Ok(serde_json::Value::Null),
        1 => Ok(documents
            .into_iter()
            .next()
            .expect("document count checked")),
        _ => Ok(serde_json::Value::Array(documents)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_multi_document_yaml_is_preserved() {
        let value = parse_nested_yaml(
            "machine:\n  type: worker\n---\napiVersion: v1alpha1\nkind: ExtensionServiceConfig\n",
        )
        .unwrap();

        let documents = value.as_array().unwrap();
        assert_eq!(documents.len(), 2);
        assert_eq!(documents[0].pointer("/machine/type").unwrap(), "worker");
        assert_eq!(documents[1].get("kind").unwrap(), "ExtensionServiceConfig");
    }
}
