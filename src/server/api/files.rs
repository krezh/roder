use std::sync::Arc;

use axum::body::Body;
use axum::extract::Query;
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use base64::Engine;
use roder_k8s::{normalize_file_path, Backend, K8sError};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct FilesQuery {
    namespace: String,
    pod: String,
    container: String,
    path: String,
}

#[derive(Deserialize)]
pub struct FileWriteRequest {
    namespace: String,
    pod: String,
    container: String,
    path: String,
    content: String,
}

#[derive(Deserialize)]
pub struct FileCreateRequest {
    namespace: String,
    pod: String,
    container: String,
    path: String,
    #[serde(default)]
    directory: bool,
}

#[derive(Deserialize)]
pub struct FileDeleteRequest {
    namespace: String,
    pod: String,
    container: String,
    path: String,
}

#[derive(Deserialize)]
pub struct FileUploadRequest {
    namespace: String,
    pod: String,
    container: String,
    path: String,
    content: String,
}

const FILE_TRANSFER_LIMIT: usize = 16 * 1024 * 1024;

fn file_error(error: K8sError) -> Response {
    let status = match error {
        K8sError::Container(_) => StatusCode::UNPROCESSABLE_ENTITY,
        _ => StatusCode::BAD_GATEWAY,
    };
    (status, error.to_string()).into_response()
}

fn validate_target(
    namespace: &str,
    pod: &str,
    container: &str,
    path: &str,
) -> Result<(), (StatusCode, String)> {
    if namespace.is_empty() || pod.is_empty() || container.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "namespace, pod, and container are required".to_string(),
        ));
    }
    normalize_file_path(path)
        .map(|_| ())
        .map_err(|error| (StatusCode::BAD_REQUEST, error.to_string()))
}

fn validate(query: &FilesQuery) -> Result<(), (StatusCode, String)> {
    validate_target(&query.namespace, &query.pod, &query.container, &query.path)
}

fn validate_mutation_target(
    namespace: &str,
    pod: &str,
    container: &str,
    path: &str,
) -> Result<(), (StatusCode, String)> {
    validate_target(namespace, pod, container, path)?;
    if normalize_file_path(path).is_ok_and(|path| path == "/") {
        return Err((
            StatusCode::BAD_REQUEST,
            "the container root cannot be modified".to_string(),
        ));
    }
    Ok(())
}

pub async fn list_files(
    Extension(backend): Extension<Arc<Backend>>,
    Query(query): Query<FilesQuery>,
) -> Response {
    if let Err(error) = validate(&query) {
        return error.into_response();
    }
    match backend
        .list_container_directory(&query.namespace, &query.pod, &query.container, &query.path)
        .await
    {
        Ok(directory) => Json(directory).into_response(),
        Err(error) => file_error(error),
    }
}

pub async fn read_file(
    Extension(backend): Extension<Arc<Backend>>,
    Query(query): Query<FilesQuery>,
) -> Response {
    if let Err(error) = validate(&query) {
        return error.into_response();
    }
    match backend
        .read_container_file(&query.namespace, &query.pod, &query.container, &query.path)
        .await
    {
        Ok(content) => Json(content).into_response(),
        Err(error) => file_error(error),
    }
}

pub async fn download_file(
    Extension(backend): Extension<Arc<Backend>>,
    Query(query): Query<FilesQuery>,
) -> Response {
    if let Err(error) = validate(&query) {
        return error.into_response();
    }
    match backend
        .download_container_file(&query.namespace, &query.pod, &query.container, &query.path)
        .await
    {
        Ok(content) => {
            let disposition = HeaderValue::from_str(&format!(
                "attachment; filename=\"{}\"",
                download_filename(&query.path)
            ))
            .unwrap_or_else(|_| HeaderValue::from_static("attachment"));
            Response::builder()
                .header(CONTENT_TYPE, "application/octet-stream")
                .header(CONTENT_DISPOSITION, disposition)
                .body(Body::from(content))
                .unwrap_or_else(|error| {
                    (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response()
                })
        }
        Err(error) => file_error(error),
    }
}

pub async fn upload_file(
    Extension(backend): Extension<Arc<Backend>>,
    Json(request): Json<FileUploadRequest>,
) -> Response {
    if let Err(error) = validate_mutation_target(
        &request.namespace,
        &request.pod,
        &request.container,
        &request.path,
    ) {
        return error.into_response();
    }
    if request.content.len().saturating_mul(3) / 4 > FILE_TRANSFER_LIMIT {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            "file exceeds the 16 MiB upload limit",
        )
            .into_response();
    }
    let content = match base64::engine::general_purpose::STANDARD.decode(&request.content) {
        Ok(content) if content.len() <= FILE_TRANSFER_LIMIT => content,
        Ok(_) => {
            return (
                StatusCode::PAYLOAD_TOO_LARGE,
                "file exceeds the 16 MiB upload limit",
            )
                .into_response()
        }
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid base64 file content").into_response(),
    };
    match backend
        .upload_container_file(
            &request.namespace,
            &request.pod,
            &request.container,
            &request.path,
            &content,
        )
        .await
    {
        Ok(()) => Json(serde_json::json!({"ok": true})).into_response(),
        Err(error) => file_error(error),
    }
}

fn download_filename(path: &str) -> String {
    let name = path
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("download");
    let sanitized: String = name
        .chars()
        .map(|character| match character {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '-' | '_' => character,
            _ => '_',
        })
        .collect();
    if sanitized.is_empty() {
        "download".to_string()
    } else {
        sanitized
    }
}

pub async fn write_file(
    Extension(backend): Extension<Arc<Backend>>,
    Json(request): Json<FileWriteRequest>,
) -> Response {
    if let Err(error) = validate_mutation_target(
        &request.namespace,
        &request.pod,
        &request.container,
        &request.path,
    ) {
        return error.into_response();
    }
    if request.content.len() > 1024 * 1024 {
        return (StatusCode::PAYLOAD_TOO_LARGE, "file content exceeds 1 MiB").into_response();
    }
    match backend
        .write_container_file(
            &request.namespace,
            &request.pod,
            &request.container,
            &request.path,
            request.content.as_bytes(),
        )
        .await
    {
        Ok(()) => Json(serde_json::json!({"ok": true})).into_response(),
        Err(error) => file_error(error),
    }
}

pub async fn create_file(
    Extension(backend): Extension<Arc<Backend>>,
    Json(request): Json<FileCreateRequest>,
) -> Response {
    if let Err(error) = validate_mutation_target(
        &request.namespace,
        &request.pod,
        &request.container,
        &request.path,
    ) {
        return error.into_response();
    }
    match backend
        .create_container_entry(
            &request.namespace,
            &request.pod,
            &request.container,
            &request.path,
            request.directory,
        )
        .await
    {
        Ok(()) => Json(serde_json::json!({"ok": true})).into_response(),
        Err(error) => file_error(error),
    }
}

pub async fn delete_file(
    Extension(backend): Extension<Arc<Backend>>,
    Json(request): Json<FileDeleteRequest>,
) -> Response {
    if let Err(error) = validate_mutation_target(
        &request.namespace,
        &request.pod,
        &request.container,
        &request.path,
    ) {
        return error.into_response();
    }
    match backend
        .delete_container_entry(
            &request.namespace,
            &request.pod,
            &request.container,
            &request.path,
        )
        .await
    {
        Ok(()) => Json(serde_json::json!({"ok": true})).into_response(),
        Err(error) => file_error(error),
    }
}

#[cfg(test)]
mod tests {
    use super::{download_filename, file_error, validate, validate_mutation_target, FilesQuery};
    use axum::http::StatusCode;
    use roder_k8s::K8sError;

    #[test]
    fn query_requires_a_target_and_absolute_path() {
        let valid = FilesQuery {
            namespace: "default".into(),
            pod: "web".into(),
            container: "app".into(),
            path: "/var/log".into(),
        };
        assert!(validate(&valid).is_ok());
        assert!(validate(&FilesQuery {
            path: "var/log".into(),
            ..valid
        })
        .is_err());
    }

    #[test]
    fn mutations_reject_the_container_root() {
        assert!(validate_mutation_target("default", "web", "app", "/").is_err());
        assert!(validate_mutation_target("default", "web", "app", "/tmp/file").is_ok());
    }

    #[test]
    fn download_names_are_safe_header_values() {
        assert_eq!(download_filename("/tmp/report.txt"), "report.txt");
        assert_eq!(download_filename("/tmp/a \"report\".txt"), "a__report_.txt");
        assert_eq!(download_filename("/"), "download");
    }

    #[test]
    fn container_failures_are_not_gateway_errors() {
        assert_eq!(
            file_error(K8sError::Container("permission denied".into())).status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            file_error(K8sError::Api("connection failed".into())).status(),
            StatusCode::BAD_GATEWAY
        );
    }
}
