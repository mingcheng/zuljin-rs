use crate::bucket::Bucket;
use crate::bucket::SaveResult;
use axum::Json;
use axum::body::Body;
use axum::extract::{Multipart, Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Shared application state, injected into all handlers via axum's `State` extractor.
#[derive(Clone)]
pub struct AppState {
    pub bucket: Arc<Bucket>,
    pub token: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct UploadResult {
    pub key: String,
    pub size: usize,
    pub content_type: Option<String>,
}

/// Unified JSON response envelope for all API endpoints.
#[derive(Serialize, Deserialize, Debug)]
pub struct ApiResponse<T> {
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<String>,
}

impl<T> ApiResponse<T> {
    pub fn ok(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct FileInfo {
    pub key: String,
    pub size: usize,
    pub content_type: Option<String>,
    pub extension: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct DiskInfo {
    pub path: String,
    pub total: u64,
    pub total_human: String,
    pub available: u64,
    pub available_human: String,
    pub used: u64,
    pub used_human: String,
    pub file_count: u64,
    pub os: String,
    pub arch: String,
}

/// Unified error type returned by all handlers: an HTTP status code paired with a JSON error body.
pub type ApiError = (StatusCode, Json<ApiResponse<()>>);

/// Build an `ApiError` from a status code and human-readable message.
fn err_response(status: StatusCode, msg: impl Into<String>) -> ApiError {
    (
        status,
        Json(ApiResponse {
            success: false,
            data: None,
            error: Some(msg.into()),
        }),
    )
}

/// Validate the `Authorization: Bearer <token>` header against the configured token.
/// If no token is configured on the server, all requests are allowed.
fn verify_token(state: &AppState, headers: &HeaderMap) -> Result<(), ApiError> {
    let expected = match &state.token {
        Some(t) => t,
        None => return Ok(()), // no token configured, skip auth
    };
    debug!("Verifying token for request");
    debug!("Expected token: {}", expected);

    let provided = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    debug!("Provided token: {}", provided.unwrap_or("<none>"));

    match provided {
        // Constant-time comparison to prevent timing attacks
        Some(t) if crate::utils::secure_compare(t, expected) => Ok(()),
        Some(_) => {
            warn!("Authentication failed: invalid token");
            Err(err_response(StatusCode::UNAUTHORIZED, "Invalid token"))
        }
        None => {
            warn!("Authentication failed: missing token");
            Err(err_response(StatusCode::UNAUTHORIZED, "Missing token"))
        }
    }
}

/// Health check endpoint. Returns project name and version.
pub async fn healthz() -> Json<ApiResponse<serde_json::Value>> {
    Json(ApiResponse::ok(serde_json::json!({
        "name": env!("CARGO_PKG_NAME"),
        "version": env!("CARGO_PKG_VERSION"),
    })))
}

/// Handle multipart file upload. Requires token. Supports multiple files in one request.
pub async fn upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Json<ApiResponse<Vec<UploadResult>>>, ApiError> {
    verify_token(&state, &headers)?;
    let bucket = &state.bucket;
    let mut results = Vec::new();

    while let Ok(Some(field)) = multipart.next_field().await {
        let file_name = field.file_name().map(|s| s.to_string());
        let content_type = field.content_type().map(|s| s.to_string());
        let data = field.bytes().await.map_err(|e| {
            err_response(
                StatusCode::BAD_REQUEST,
                format!("Failed to read field: {e}"),
            )
        })?;

        if data.is_empty() {
            continue;
        }

        let size = data.len();
        let SaveResult {
            key,
            content_type: detected_type,
        } = bucket
            .save(data.to_vec(), content_type.as_deref(), file_name.as_deref())
            .map_err(|e| {
                err_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to save file: {e}"),
                )
            })?;

        results.push(UploadResult {
            key: key.clone(),
            size,
            content_type: detected_type.or(content_type),
        });
        info!(
            key = %key,
            size,
            original_name = file_name.as_deref().unwrap_or("-"),
            "File uploaded"
        );
    }

    if results.is_empty() {
        return Err(err_response(StatusCode::BAD_REQUEST, "No file uploaded"));
    }

    Ok(Json(ApiResponse::ok(results)))
}

/// Serve a file by key. Public endpoint, no token required.
pub async fn download(
    State(state): State<AppState>,
    Path(path): Path<String>,
) -> Result<Response, ApiError> {
    debug!(key = %path, "File download requested");
    let content = state
        .bucket
        .get_content(&path)
        .map_err(|e| err_response(StatusCode::NOT_FOUND, format!("File not found: {e}")))?;

    let content_type = mime_guess::from_path(&path)
        .first_or_octet_stream()
        .to_string();

    let mut headers = HeaderMap::new();
    headers.insert(
        "Content-Type",
        HeaderValue::from_str(&content_type)
            .unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );

    Ok((headers, Body::from(content)).into_response())
}

/// Return file metadata (size, content type, extension). Requires token.
pub async fn file_info(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
) -> Result<Json<ApiResponse<FileInfo>>, ApiError> {
    verify_token(&state, &headers)?;
    let meta = state
        .bucket
        .get_meta(&key)
        .map_err(|e| err_response(StatusCode::NOT_FOUND, format!("File not found: {e}")))?;

    Ok(Json(ApiResponse::ok(FileInfo {
        key,
        size: meta.size,
        content_type: meta.content_type,
        extension: meta.extension,
    })))
}

/// Return disk and upload directory statistics. Requires token.
pub async fn disk_info(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ApiResponse<DiskInfo>>, ApiError> {
    verify_token(&state, &headers)?;
    let (total, available) = state.bucket.disk_space().map_err(|e| {
        err_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to get disk space: {e}"),
        )
    })?;

    let (used, file_count) = state.bucket.usage().map_err(|e| {
        err_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to get usage: {e}"),
        )
    })?;

    Ok(Json(ApiResponse::ok(DiskInfo {
        path: state.bucket.path.display().to_string(),
        total,
        total_human: crate::utils::format_size(total),
        available,
        available_human: crate::utils::format_size(available),
        used,
        used_human: crate::utils::format_size(used),
        file_count,
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
    })))
}

#[derive(Serialize, Deserialize, Debug)]
pub struct DeleteResult {
    pub key: String,
}

/// Delete a file by key. Requires token.
pub async fn delete_file(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
) -> Result<Json<ApiResponse<DeleteResult>>, ApiError> {
    verify_token(&state, &headers)?;
    state
        .bucket
        .delete(&key)
        .map_err(|e| err_response(StatusCode::NOT_FOUND, format!("File not found: {e}")))?;

    info!(key = %key, "File deleted");
    Ok(Json(ApiResponse::ok(DeleteResult { key })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::http::{HeaderName, HeaderValue};
    use axum::routing::{delete, get, post};
    use axum_test::TestServer;
    use axum_test::multipart::{MultipartForm, Part};

    fn make_server() -> (TestServer, tempfile::TempDir) {
        make_server_with_token(None)
    }

    fn make_server_with_token(token: Option<&str>) -> (TestServer, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let bucket = Arc::new(Bucket::new(dir.path().to_str().unwrap()).unwrap());
        let state = AppState {
            bucket,
            token: token.map(|t| t.to_string()),
        };
        let app = Router::new()
            .route("/upload", post(upload))
            .route("/get/{*path}", get(download))
            .route("/info/{*path}", get(file_info))
            .route("/disk", get(disk_info))
            .route("/delete/{*path}", delete(delete_file))
            .with_state(state);
        (TestServer::builder().build(app), dir)
    }

    #[tokio::test]
    async fn test_upload_and_download_roundtrip() {
        let (server, _dir) = make_server();

        let content = b"hello world content";
        let form = MultipartForm::new()
            .add_part("file", Part::bytes(content.to_vec()).file_name("test.txt"));
        let res = server.post("/upload").multipart(form).await;
        res.assert_status_ok();

        let body: ApiResponse<Vec<UploadResult>> = res.json();
        assert!(body.success);
        let results = body.data.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].size, content.len());

        let download_res = server.get(&format!("/get/{}", results[0].key)).await;
        download_res.assert_status_ok();
        assert_eq!(download_res.as_bytes().as_ref(), content);
    }

    #[tokio::test]
    async fn test_upload_empty_returns_error() {
        let (server, _dir) = make_server();
        let form =
            MultipartForm::new().add_part("file", Part::bytes(vec![]).file_name("empty.txt"));
        server
            .post("/upload")
            .multipart(form)
            .await
            .assert_status(StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_download_not_found() {
        let (server, _dir) = make_server();
        server
            .get("/get/2026_01_01/nonexistent.txt")
            .await
            .assert_status(StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_upload_multiple_files() {
        let (server, _dir) = make_server();
        let form = MultipartForm::new()
            .add_part("file", Part::bytes(b"file1".to_vec()).file_name("a.txt"))
            .add_part("file", Part::bytes(b"file2".to_vec()).file_name("b.txt"));
        let res = server.post("/upload").multipart(form).await;
        res.assert_status_ok();

        let body: ApiResponse<Vec<UploadResult>> = res.json();
        assert!(body.success);
        assert_eq!(body.data.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn test_file_info() {
        let (server, _dir) = make_server();

        let form = MultipartForm::new().add_part(
            "file",
            Part::bytes(b"info test".to_vec()).file_name("demo.txt"),
        );
        let upload_res: ApiResponse<Vec<UploadResult>> =
            server.post("/upload").multipart(form).await.json();
        let key = &upload_res.data.unwrap()[0].key;

        let res = server.get(&format!("/info/{key}")).await;
        res.assert_status_ok();

        let body: ApiResponse<FileInfo> = res.json();
        assert!(body.success);
        let info = body.data.unwrap();
        assert_eq!(info.key, *key);
        assert_eq!(info.size, 9);
    }

    #[tokio::test]
    async fn test_file_info_not_found() {
        let (server, _dir) = make_server();
        server
            .get("/info/2026_01_01/nonexistent.txt")
            .await
            .assert_status(StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_disk_info() {
        let (server, _dir) = make_server();
        let res = server.get("/disk").await;
        res.assert_status_ok();

        let body: ApiResponse<DiskInfo> = res.json();
        assert!(body.success);
        let info = body.data.unwrap();
        assert!(info.total > 0);
        assert!(info.available > 0);
        assert!(!info.total_human.is_empty());
        assert!(!info.available_human.is_empty());
        assert!(!info.path.is_empty());
        assert_eq!(info.file_count, 0);
        assert_eq!(info.used, 0);
        assert!(!info.os.is_empty());
        assert!(!info.arch.is_empty());
    }

    #[tokio::test]
    async fn test_upload_preserves_content_type() {
        let (server, _dir) = make_server();

        // PNG magic bytes
        let png_data: Vec<u8> = vec![
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52,
        ];
        let form =
            MultipartForm::new().add_part("file", Part::bytes(png_data).file_name("image.png"));
        let res = server.post("/upload").multipart(form).await;
        res.assert_status_ok();

        let body: ApiResponse<Vec<UploadResult>> = res.json();
        let result = &body.data.unwrap()[0];
        assert_eq!(result.content_type.as_deref(), Some("image/png"));
    }

    // --- Token auth tests ---

    #[tokio::test]
    async fn test_upload_requires_token() {
        let (server, _dir) = make_server_with_token(Some("secret123"));
        let form =
            MultipartForm::new().add_part("file", Part::bytes(b"data".to_vec()).file_name("a.txt"));

        // No token -> 401
        server
            .post("/upload")
            .multipart(form)
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_upload_with_valid_token() {
        let (server, _dir) = make_server_with_token(Some("secret123"));
        let form =
            MultipartForm::new().add_part("file", Part::bytes(b"data".to_vec()).file_name("a.txt"));

        let res = server
            .post("/upload")
            .add_header(
                HeaderName::from_static("authorization"),
                HeaderValue::from_static("Bearer secret123"),
            )
            .multipart(form)
            .await;
        res.assert_status_ok();
    }

    #[tokio::test]
    async fn test_upload_with_invalid_token() {
        let (server, _dir) = make_server_with_token(Some("secret123"));
        let form =
            MultipartForm::new().add_part("file", Part::bytes(b"data".to_vec()).file_name("a.txt"));

        server
            .post("/upload")
            .add_header(
                HeaderName::from_static("authorization"),
                HeaderValue::from_static("Bearer wrong"),
            )
            .multipart(form)
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_download_requires_token() {
        let (server, _dir) = make_server_with_token(Some("tok"));

        // Upload with token
        let form = MultipartForm::new()
            .add_part("file", Part::bytes(b"public".to_vec()).file_name("pub.txt"));
        let res = server
            .post("/upload")
            .add_header(
                HeaderName::from_static("authorization"),
                HeaderValue::from_static("Bearer tok"),
            )
            .multipart(form)
            .await;
        let body: ApiResponse<Vec<UploadResult>> = res.json();
        let key = &body.data.unwrap()[0].key;

        // Download without token should succeed (public)
        let res = server.get(&format!("/get/{key}")).await;
        res.assert_status_ok();
        assert_eq!(res.as_bytes().as_ref(), b"public");
    }

    #[tokio::test]
    async fn test_api_info_requires_token() {
        let (server, _dir) = make_server_with_token(Some("tok"));

        server
            .get("/info/2026_01_01/x.txt")
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_api_disk_requires_token() {
        let (server, _dir) = make_server_with_token(Some("tok"));

        server
            .get("/disk")
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_no_token_configured_allows_access() {
        let (server, _dir) = make_server(); // no token
        let res = server.get("/disk").await;
        res.assert_status_ok();
    }

    // --- Delete tests ---

    #[tokio::test]
    async fn test_delete_file() {
        let (server, _dir) = make_server();

        let form = MultipartForm::new().add_part(
            "file",
            Part::bytes(b"to delete".to_vec()).file_name("rm.txt"),
        );
        let body: ApiResponse<Vec<UploadResult>> =
            server.post("/upload").multipart(form).await.json();
        let key = body.data.unwrap()[0].key.clone();

        let res = server.delete(&format!("/delete/{key}")).await;
        res.assert_status_ok();
        let body: ApiResponse<DeleteResult> = res.json();
        assert!(body.success);
        assert_eq!(body.data.unwrap().key, key);

        // Verify file is gone
        server
            .get(&format!("/get/{key}"))
            .await
            .assert_status(StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_delete_not_found() {
        let (server, _dir) = make_server();
        server
            .delete("/delete/2026_01_01/nonexistent.txt")
            .await
            .assert_status(StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_delete_requires_token() {
        let (server, _dir) = make_server_with_token(Some("tok"));
        server
            .delete("/delete/2026_01_01/x.txt")
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
    }
}
