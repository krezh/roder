use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::request::Builder as RequestBuilder;
use axum::http::Request;
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client as HyperClient;
use hyper_util::rt::TokioExecutor;
use roder_auth::Identity;
use roder_core::ActiveDrainJob;

use super::api::action::ActionRequest;
use super::AppState;

pub const INTERNAL_HEADER: &str = "x-roder-internal-hop";
pub const FORWARDED_AUTH_HEADER: &str = "x-roder-forwarded-auth";

/// Peer-to-peer port (TLS-terminated mTLS listener alongside the public
/// plain-HTTP listener on 8080). Configured via `RODER_PEER_PORT`, defaulting
/// to 8443.
pub const DEFAULT_PEER_PORT: u16 = 8443;

/// Fixed request body type for peer calls.
type PeerBody = Full<Bytes>;

/// TLS peer client backed by hyper-rustls with the `PinnedVerifier` baked into
/// the rustls `ClientConfig`.
type TlsClient = HyperClient<HttpsConnector<HttpConnector>, PeerBody>;

pub struct HaState {
    pub coordinator: roder_k8s::NodeCoordinator,
    pub pod_name: String,
    peer_port: u16,
    tls_http: TlsClient,
    pub server_config: Arc<rustls::ServerConfig>,
    verifier: Arc<roder_mtls::PinnedVerifier>,
}

impl HaState {
    pub fn new(
        coordinator: roder_k8s::NodeCoordinator,
        pod_name: String,
        peer_port: u16,
        verifier: Arc<roder_mtls::PinnedVerifier>,
        rustls_client: Arc<rustls::ClientConfig>,
        server_config: Arc<rustls::ServerConfig>,
    ) -> Self {
        let https = hyper_rustls::HttpsConnectorBuilder::new()
            .with_tls_config((*rustls_client).clone())
            .https_only()
            .enable_http1()
            .build();
        let tls_http = HyperClient::builder(TokioExecutor::new())
            .pool_idle_timeout(Some(Duration::from_secs(30)))
            .build(https);
        Self {
            coordinator,
            pod_name,
            peer_port,
            tls_http,
            server_config,
            verifier,
        }
    }

    fn peer_scheme(&self) -> &'static str {
        "https"
    }

    fn peer_port(&self) -> u16 {
        self.peer_port
    }

    async fn send_peer(
        &self,
        builder: RequestBuilder,
        body: PeerBody,
    ) -> Result<hyper::Response<Incoming>, hyper_util::client::legacy::Error> {
        let req = builder.body(body).expect("peer request is well-formed");
        self.tls_http.request(req).await
    }

    async fn peer_url(&self, executor: &str, path: &str) -> Result<String, Box<Response>> {
        let peer = self
            .coordinator
            .pod(executor)
            .await
            .map_err(|error| Box::new(coordination_error(error)))?
            .ok_or_else(|| {
                Box::new(
                    (
                        StatusCode::GONE,
                        "operation executor is no longer available",
                    )
                        .into_response(),
                )
            })?;
        Ok(format!(
            "{}://{}:{}{path}",
            self.peer_scheme(),
            peer.ip,
            self.peer_port()
        ))
    }

    /// Refresh destination-bound server pins and the accepted client-cert set.
    pub async fn refresh_fingerprints(&self) -> Result<usize, kube::Error> {
        let pods = self.coordinator.pods().await?;
        let pins: HashMap<String, String> = pods
            .into_iter()
            .filter(|pod| pod.ready)
            .filter_map(|pod| {
                let fingerprint = pod.tls_fingerprint?;
                if fingerprint.len() != 64
                    || !fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit())
                {
                    tracing::warn!(pod = %pod.name, "ignoring malformed peer TLS fingerprint");
                    return None;
                }
                Some((pod.ip, fingerprint.to_ascii_lowercase()))
            })
            .collect();
        let count = pins.len();
        self.verifier.set(pins);
        Ok(count)
    }

    /// Start a background task that refreshes the pinned-fingerprint set every
    /// 15s. Failure to refresh is logged and silent — the cached set stays
    /// authoritative; a stale cache just keeps trusting the old set.
    pub fn spawn_fingerprint_refresh(self: &std::sync::Arc<Self>) {
        let this = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(15));
            tick.tick().await;
            loop {
                tick.tick().await;
                if let Err(error) = this.refresh_fingerprints().await {
                    tracing::warn!("failed to refresh peer TLS fingerprints: {error}");
                }
            }
        });
    }

    /// Peer port this pod listens on for inbound mTLS peer traffic (mirrors
    /// `RODER_PEER_PORT` / `DEFAULT_PEER_PORT`). Used by `main` to bind the
    /// TLS listener in HA mode.
    pub fn peer_listener_port(&self) -> u16 {
        self.peer_port
    }
}

pub(crate) async fn forward_action_from_target(
    state: &AppState,
    headers: &HeaderMap,
    identity: &Identity,
    target: &str,
    request: &ActionRequest,
) -> Option<Response> {
    let ha = state.ha.as_ref()?;
    if state.config.pod_node_name.as_deref() != Some(target) {
        return None;
    }
    if headers.contains_key(INTERNAL_HEADER) {
        return Some(
            (
                StatusCode::CONFLICT,
                "no off-target Roder executor is available",
            )
                .into_response(),
        );
    }
    let peers = match ha.coordinator.pods().await {
        Ok(peers) => peers,
        Err(error) => return Some(coordination_error(error)),
    };
    let Some(peer) = peers
        .into_iter()
        .filter(|peer| peer.ready && peer.node != target && peer.name != ha.pod_name)
        .min_by(|a, b| a.name.cmp(&b.name))
    else {
        return Some(
            (
                StatusCode::CONFLICT,
                "no off-target Roder executor is available",
            )
                .into_response(),
        );
    };
    let cookie = forwarding_cookie(state, identity).await;
    let body = match serde_json::to_vec(request) {
        Ok(bytes) => PeerBody::from(Bytes::from(bytes)),
        Err(error) => {
            return Some(
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("failed to encode forwarded action: {error}"),
                )
                    .into_response(),
            );
        }
    };
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri(format!(
            "{}://{}:{}/api/action",
            ha.peer_scheme(),
            peer.ip,
            ha.peer_port()
        ))
        .header(INTERNAL_HEADER, HeaderValue::from_static("1"))
        .header(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
    if let Some(forwarded) = forwarded_auth(state, identity).await {
        builder = builder.header(
            FORWARDED_AUTH_HEADER,
            forwarded.parse::<HeaderValue>().unwrap(),
        );
    }
    if let Some(cookie) = cookie.and_then(|c| c.parse::<HeaderValue>().ok()) {
        builder = builder.header(header::COOKIE.as_str(), cookie);
    }
    Some(match ha.send_peer(builder, body).await {
        Ok(response) => proxy_response(response),
        Err(error) => (
            StatusCode::BAD_GATEWAY,
            format!("failed to reach executor: {error}"),
        )
            .into_response(),
    })
}

pub(crate) async fn proxy_to_executor(
    state: &AppState,
    identity: &Identity,
    executor: Option<&str>,
    method: Method,
    path: &str,
    body: Option<&ActionRequest>,
) -> Option<Response> {
    let ha = state.ha.as_ref()?;
    let executor = executor?;
    if executor == ha.pod_name {
        return None;
    }
    let url = match ha.peer_url(executor, path).await {
        Ok(url) => url,
        Err(response) => return Some(*response),
    };
    let cookie = forwarding_cookie(state, identity).await;
    let (body, forwarded) = match body {
        Some(payload) => match serde_json::to_vec(payload) {
            Ok(bytes) => (
                Some(PeerBody::from(Bytes::from(bytes))),
                forwarded_auth(state, identity).await,
            ),
            Err(error) => {
                return Some(
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("failed to encode forwarded action: {error}"),
                    )
                        .into_response(),
                );
            }
        },
        None => (None, None),
    };
    let mut builder = Request::builder()
        .method(method)
        .uri(url)
        .header(INTERNAL_HEADER, HeaderValue::from_static("1"));
    if body.is_some() {
        builder = builder.header(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
    }
    if let Some(forwarded) = forwarded.and_then(|f| f.parse::<HeaderValue>().ok()) {
        builder = builder.header(FORWARDED_AUTH_HEADER, forwarded);
    }
    if let Some(cookie) = cookie.and_then(|c| c.parse::<HeaderValue>().ok()) {
        builder = builder.header(header::COOKIE.as_str(), cookie);
    }
    let body = body.unwrap_or_else(|| PeerBody::from(Bytes::new()));
    Some(match ha.send_peer(builder, body).await {
        Ok(response) => proxy_response(response),
        Err(error) => (
            StatusCode::BAD_GATEWAY,
            format!("operation executor is unavailable: {error}"),
        )
            .into_response(),
    })
}

pub(crate) async fn active_jobs(
    state: &AppState,
    headers: &HeaderMap,
    identity: &Identity,
) -> Result<Vec<ActiveDrainJob>, Box<Response>> {
    let Some(ha) = state.ha.as_ref() else {
        return Ok(Vec::new());
    };
    if headers.contains_key(INTERNAL_HEADER) {
        return Ok(Vec::new());
    }
    let cookie = forwarding_cookie(state, identity).await;
    let peers = ha
        .coordinator
        .pods()
        .await
        .map_err(|error| Box::new(coordination_error(error)))?;
    let mut active = Vec::new();
    for peer in peers.into_iter().filter(|peer| peer.name != ha.pod_name) {
        let mut builder = Request::builder()
            .method(Method::GET)
            .uri(format!(
                "{}://{}:{}/api/drain-active",
                ha.peer_scheme(),
                peer.ip,
                ha.peer_port()
            ))
            .header(INTERNAL_HEADER, HeaderValue::from_static("1"));
        if let Some(cookie) = cookie
            .as_deref()
            .and_then(|c| c.parse::<HeaderValue>().ok())
        {
            builder = builder.header(header::COOKIE.as_str(), cookie);
        }
        let response = match ha.send_peer(builder, PeerBody::from(Bytes::new())).await {
            Ok(response) => response,
            Err(error) => {
                tracing::debug!("peer drain-active probe failed: {error}");
                continue;
            }
        };
        if !response.status().is_success() {
            continue;
        }
        match response.into_body().collect().await {
            Ok(collected) => {
                let bytes = collected.to_bytes();
                if let Ok(Some(job)) = serde_json::from_slice::<Option<ActiveDrainJob>>(&bytes) {
                    active.push(job);
                }
            }
            Err(error) => {
                tracing::debug!("peer drain-active body read failed: {error}");
            }
        }
    }
    Ok(active)
}

/// Convert a `hyper::Response<Incoming>` produced by a peer call into an axum
/// `Response` to return to the caller. Preserves the upstream status, the
/// `Content-Type` header, and any `Set-Cookie` headers (which the browser
/// handler chain relies on for session refresh). Other upstream headers are
/// dropped — they were before, too, since forwarding arbitrary headers is a
/// response-splitting surface we don't want to widen.
fn proxy_response(response: hyper::Response<Incoming>) -> Response {
    let status =
        StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| HeaderValue::from_bytes(value.as_bytes()).ok());
    let set_cookies: Vec<HeaderValue> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| HeaderValue::from_bytes(value.as_bytes()).ok())
        .collect();
    let body = Body::new(response.into_body());
    let mut builder = Response::builder().status(status);
    if let Some(content_type) = content_type {
        builder = builder.header(header::CONTENT_TYPE, content_type);
    }
    let mut proxied = builder
        .body(body)
        .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response());
    for cookie in set_cookies {
        proxied.headers_mut().append(header::SET_COOKIE, cookie);
    }
    proxied
}

fn coordination_error(error: impl std::fmt::Display) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        format!("Roder HA coordination is unavailable: {error}"),
    )
        .into_response()
}

async fn forwarding_cookie(state: &AppState, identity: &Identity) -> Option<String> {
    if state.config.dev_mode {
        return None;
    }
    let tokens = state.backends.resolved_tokens(&identity.subject).await?;
    let key = state.config.session_key?;
    Some(format!(
        "{}={}",
        super::session::SESSION_COOKIE,
        super::session::seal_session(&tokens, &key)
    ))
}

async fn forwarded_auth(state: &AppState, identity: &Identity) -> Option<String> {
    let tokens = state.backends.resolved_tokens(&identity.subject).await?;
    let key = state.config.session_key?;
    Some(super::session::seal_forwarded_tokens(&tokens, &key))
}
