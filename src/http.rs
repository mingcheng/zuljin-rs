use crate::bucket::Bucket;
use axum::Json;
use axum::body::Body;
use axum::extract::{Multipart, Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Serialize, Deserialize, Debug)]
pub struct UploadResult {
    pub key: String,
    pub size: usize,
    pub content_type: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ApiResponse<T: Serialize> {
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<String>,
}

pub type SharedBucket = Arc<Bucket>;

type UploadError = (StatusCode, Json<ApiResponse<()>>);

fn err_response(status: StatusCode, msg: impl Into<String>) -> UploadError {
    (
        status,
        Json(ApiResponse {
            success: false,
            data: None,
            error: Some(msg.into()),
        }),
    )
}

pub async fn show_form() -> Html<&'static str> {
    Html(
        r#"
        <!doctype html>
        <html>
            <head><title>Zuljin Upload</title></head>
            <body>
                <form action="/upload" method="post" enctype="multipart/form-data">
                    <label>
                        Upload file:
                        <input type="file" name="file" multiple>
                    </label>
                    <input type="submit" value="Upload files">
                </form>
            </body>
        </html>
        "#,
    )
}

pub async fn upload(
    State(bucket): State<SharedBucket>,
    mut multipart: Multipart,
) -> Result<Json<ApiResponse<Vec<UploadResult>>>, UploadError> {
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

        let key = bucket
            .save(data.to_vec(), content_type.as_deref(), file_name.as_deref())
            .map_err(|e| {
                err_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to save file: {e}"),
                )
            })?;

        let detected_type = bucket.get_meta(&key).ok().and_then(|m| m.content_type);
        results.push(UploadResult {
            key,
            size: data.len(),
            content_type: detected_type.or(content_type),
        });
    }

    if results.is_empty() {
        return Err(err_response(StatusCode::BAD_REQUEST, "No file uploaded"));
    }

    Ok(Json(ApiResponse {
        success: true,
        data: Some(results),
        error: None,
    }))
}

pub async fn download(
    State(bucket): State<SharedBucket>,
    Path(path): Path<String>,
) -> Result<Response, (StatusCode, String)> {
    let content = bucket
        .get_content(&path)
        .map_err(|e| (StatusCode::NOT_FOUND, format!("File not found: {e}")))?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::routing::{get, post};
    use axum_test::TestServer;
    use axum_test::multipart::{MultipartForm, Part};

    fn make_server() -> (TestServer, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let bucket = Arc::new(Bucket::new(dir.path().to_str().unwrap()).unwrap());
        let app = Router::new()
            .route("/", get(show_form))
            .route("/upload", post(upload))
            .route("/files/{*path}", get(download))
            .with_state(bucket);
        (TestServer::builder().build(app), dir)
    }

    #[tokio::test]
    async fn test_show_form_returns_html() {
        let (server, _dir) = make_server();
        let res = server.get("/").await;
        res.assert_status_ok();
        let text = res.text();
        assert!(text.contains("<form"));
        assert!(text.contains("Upload file"));
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

        let download_res = server.get(&format!("/files/{}", results[0].key)).await;
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
            .get("/files/2026/01/nonexistent.txt")
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
}
