use axum::body::Body;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use roder_core::ActiveDrainJob;

use super::api::action::ActionRequest;
use super::AppState;

pub const INTERNAL_HEADER: &str = "x-roder-internal-hop";
pub const FORWARDED_AUTH_HEADER: &str = "x-roder-forwarded-auth";

pub struct HaState {
    pub coordinator: roder_k8s::NodeCoordinator,
    pub pod_name: String,
    http: reqwest::Client,
}

impl HaState {
    pub fn new(coordinator: roder_k8s::NodeCoordinator, pod_name: String) -> Self {
        Self {
            coordinator,
            pod_name,
            http: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(5))
                .build()
                .expect("static HA HTTP client configuration is valid"),
        }
    }

    async fn peer_url(&self, executor: &str, path: &str) -> Result<String, Response> {
        let peer = self
            .coordinator
            .pod(executor)
            .await
            .map_err(coordination_error)?
            .ok_or_else(|| {
                (
                    StatusCode::GONE,
                    "operation executor is no longer available",
                )
                    .into_response()
            })?;
        Ok(format!("http://{}:8080{path}", peer.ip))
    }
}

pub(crate) async fn forward_action_from_target(
    state: &AppState,
    headers: &HeaderMap,
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
    let cookie = forwarding_cookie(state, headers).await;
    let mut forwarded = ha
        .http
        .post(format!("http://{}:8080/api/action", peer.ip))
        .header(INTERNAL_HEADER, "1")
        .json(request);
    if let Some(auth) = forwarded_auth(state, headers).await {
        forwarded = forwarded.header(FORWARDED_AUTH_HEADER, auth);
    }
    if let Some(cookie) = cookie {
        forwarded = forwarded.header(header::COOKIE.as_str(), cookie);
    }
    Some(match forwarded.send().await {
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
    headers: &HeaderMap,
    executor: Option<&str>,
    method: reqwest::Method,
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
        Err(response) => return Some(response),
    };
    let mut request = ha.http.request(method, url).header(INTERNAL_HEADER, "1");
    if let Some(cookie) = forwarding_cookie(state, headers).await {
        request = request.header(header::COOKIE.as_str(), cookie);
    }
    if let Some(body) = body {
        request = request.json(body);
        if let Some(auth) = forwarded_auth(state, headers).await {
            request = request.header(FORWARDED_AUTH_HEADER, auth);
        }
    }
    Some(match request.send().await {
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
) -> Result<Vec<ActiveDrainJob>, Response> {
    let Some(ha) = state.ha.as_ref() else {
        return Ok(Vec::new());
    };
    if headers.contains_key(INTERNAL_HEADER) {
        return Ok(Vec::new());
    }
    let cookie = forwarding_cookie(state, headers).await;
    let peers = ha.coordinator.pods().await.map_err(coordination_error)?;
    let mut active = Vec::new();
    for peer in peers.into_iter().filter(|peer| peer.name != ha.pod_name) {
        let mut request = ha
            .http
            .get(format!("http://{}:8080/api/drain-active", peer.ip))
            .header(INTERNAL_HEADER, "1");
        if let Some(cookie) = cookie.as_deref() {
            request = request.header(header::COOKIE.as_str(), cookie);
        }
        if let Ok(response) = request.send().await {
            if response.status().is_success() {
                if let Ok(Some(job)) = response.json::<Option<ActiveDrainJob>>().await {
                    active.push(job);
                }
            }
        }
    }
    Ok(active)
}

fn proxy_response(response: reqwest::Response) -> Response {
    let status =
        StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .cloned();
    let set_cookies: Vec<_> = response
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|value| HeaderValue::from_bytes(value.as_bytes()).ok())
        .collect();
    let mut proxied = Response::builder().status(status);
    if let Some(content_type) =
        content_type.and_then(|value| HeaderValue::from_bytes(value.as_bytes()).ok())
    {
        proxied = proxied.header(header::CONTENT_TYPE, content_type);
    }
    let mut proxied = proxied
        .body(Body::from_stream(response.bytes_stream()))
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

async fn forwarding_cookie(state: &AppState, headers: &HeaderMap) -> Option<String> {
    if state.config.dev_mode {
        return None;
    }
    if let Some(caller) = super::api::request_caller(state, headers) {
        if let (Some(tokens), Some(key)) = (
            state.backends.resolved_tokens(&caller.owner).await,
            state.config.session_key,
        ) {
            return Some(format!(
                "{}={}",
                super::session::SESSION_COOKIE,
                super::session::seal_session(&tokens, &key)
            ));
        }
    }
    headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

async fn forwarded_auth(state: &AppState, headers: &HeaderMap) -> Option<String> {
    let caller = super::api::request_caller(state, headers)?;
    let tokens = state.backends.resolved_tokens(&caller.owner).await?;
    let key = state.config.session_key?;
    Some(super::session::seal_forwarded_tokens(&tokens, &key))
}
