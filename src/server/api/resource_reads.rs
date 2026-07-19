//! Generic per-resource read endpoints: a single object's detail, its
//! ownership tree, and what the current identity is allowed to do to it.

use std::sync::Arc;

use axum::extract::Query;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use roder_k8s::Backend;
use serde::Deserialize;

use super::{bad_gateway, ns_filter};

#[derive(Deserialize)]
pub struct DetailQuery {
    key: String,
    namespace: Option<String>,
    name: String,
}

pub async fn detail(
    Extension(b): Extension<Arc<Backend>>,
    Query(q): Query<DetailQuery>,
) -> Response {
    let ns = ns_filter(&q.namespace);
    match b.detail(&q.key, ns, &q.name).await {
        Ok(d) => Json(d).into_response(),
        Err(e) => bad_gateway(e),
    }
}

#[derive(Deserialize)]
pub struct ResourceTreeQuery {
    key: String,
    namespace: Option<String>,
    name: String,
}

/// Full recursive ownership tree for a Kustomization/HelmRelease, resolved
/// server-side in one shot (see `Backend::resource_tree`).
pub async fn resource_tree(
    Extension(b): Extension<Arc<Backend>>,
    Query(q): Query<ResourceTreeQuery>,
) -> Response {
    let ns = ns_filter(&q.namespace);
    match b.resource_tree(&q.key, ns, &q.name).await {
        Ok(tree) => Json(tree).into_response(),
        Err(e) => bad_gateway(e),
    }
}

#[derive(Deserialize)]
pub struct PermQuery {
    key: String,
    namespace: Option<String>,
}

/// Which mutations the current identity may perform (drives button visibility).
pub async fn permissions(
    Extension(b): Extension<Arc<Backend>>,
    Query(q): Query<PermQuery>,
) -> Response {
    let ns = ns_filter(&q.namespace);
    let patch = b.can("patch", &q.key, ns).await;
    let delete = b.can("delete", &q.key, ns).await;
    Json(serde_json::json!({ "patch": patch, "delete": delete })).into_response()
}

#[derive(Deserialize)]
pub struct AccessReviewQuery {
    namespace: Option<String>,
}

/// RBAC access review: which verbs the current identity may perform across
/// every known resource kind, given OIDC passthrough.
pub async fn access_review(
    Extension(b): Extension<Arc<Backend>>,
    Query(q): Query<AccessReviewQuery>,
) -> Response {
    let ns = ns_filter(&q.namespace);
    Json(b.access_review(ns).await).into_response()
}
