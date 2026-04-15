use crate::http::{ApiResponse, DeleteResult, DiskInfo, FileInfo, UploadResult};
use reqwest::multipart;
use serde::de::DeserializeOwned;
use std::path::Path;
use tracing::{debug, warn};

/// HTTP client for interacting with the Zuljin server API.
pub struct Client {
    base_url: String,
    token: Option<String>,
    http: reqwest::Client,
}

impl Client {
    pub fn new(server: &str, token: Option<String>) -> Self {
        Client {
            base_url: server.trim_end_matches('/').to_string(),
            token,
            http: reqwest::Client::new(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    /// Attach Bearer token to a request if configured.
    fn with_auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.token {
            Some(t) => req.header("Authorization", format!("Bearer {t}")),
            None => req,
        }
    }

    /// Send a request and return the raw response body as a string.
    async fn send_raw(&self, req: reqwest::RequestBuilder) -> Result<String, String> {
        let resp = req
            .send()
            .await
            .map_err(|e| {
                warn!(error = %e, "HTTP request failed");
                format!("Request failed: {e}")
            })?;
        resp.text()
            .await
            .map_err(|e| format!("Failed to read response: {e}"))
    }

    /// Send a request, parse the `ApiResponse<T>` envelope, and extract the data or error.
    async fn send_and_parse<T: DeserializeOwned>(
        &self,
        req: reqwest::RequestBuilder,
    ) -> Result<T, String> {
        let body: ApiResponse<T> = req
            .send()
            .await
            .map_err(|e| {
                warn!(error = %e, "HTTP request failed");
                format!("Request failed: {e}")
            })?
            .json()
            .await
            .map_err(|e| format!("Failed to parse response: {e}"))?;

        if body.success {
            body.data.ok_or_else(|| "Empty response".to_string())
        } else {
            let err_msg = body.error.unwrap_or_else(|| "Unknown error".to_string());
            warn!(error = %err_msg, "Server returned an error");
            Err(err_msg)
        }
    }

    async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        let req = self.with_auth(self.http.get(self.url(path)));
        self.send_and_parse(req).await
    }

    async fn get_raw(&self, path: &str) -> Result<String, String> {
        let req = self.with_auth(self.http.get(self.url(path)));
        self.send_raw(req).await
    }

    fn build_upload_request(&self, file_path: &str) -> Result<reqwest::RequestBuilder, String> {
        let path = Path::new(file_path);
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
            .to_string();

        let data = std::fs::read(path).map_err(|e| format!("Failed to read file: {e}"))?;

        let part = multipart::Part::bytes(data).file_name(file_name);
        let form = multipart::Form::new().part("file", part);

        Ok(self
            .with_auth(self.http.post(self.url("/upload")))
            .multipart(form))
    }

    pub async fn upload(&self, file_path: &str) -> Result<Vec<UploadResult>, String> {
        debug!(file = %file_path, server = %self.base_url, "Uploading file");
        let req = self.build_upload_request(file_path)?;
        self.send_and_parse(req).await
    }

    pub async fn upload_raw(&self, file_path: &str) -> Result<String, String> {
        debug!(file = %file_path, server = %self.base_url, "Uploading file (raw)");
        let req = self.build_upload_request(file_path)?;
        self.send_raw(req).await
    }

    /// Download raw file bytes. No token required (public endpoint).
    pub async fn download(&self, key: &str) -> Result<Vec<u8>, String> {
        debug!(key = %key, server = %self.base_url, "Downloading file");
        let resp = self
            .http
            .get(self.url(&format!("/get/{key}")))
            .send()
            .await
            .map_err(|e| {
                warn!(key = %key, error = %e, "Download request failed");
                format!("Request failed: {e}")
            })?;

        if !resp.status().is_success() {
            return Err(format!("Server returned {}", resp.status()));
        }

        resp.bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| format!("Failed to read response: {e}"))
    }

    pub async fn info(&self, key: &str) -> Result<FileInfo, String> {
        self.get_json(&format!("/info/{key}")).await
    }

    pub async fn info_raw(&self, key: &str) -> Result<String, String> {
        self.get_raw(&format!("/info/{key}")).await
    }

    pub async fn disk(&self) -> Result<DiskInfo, String> {
        self.get_json("/disk").await
    }

    pub async fn disk_raw(&self) -> Result<String, String> {
        self.get_raw("/disk").await
    }

    pub async fn delete(&self, key: &str) -> Result<DeleteResult, String> {
        debug!(key = %key, server = %self.base_url, "Deleting file");
        let req = self.with_auth(self.http.delete(self.url(&format!("/delete/{key}"))));
        self.send_and_parse(req).await
    }

    pub async fn delete_raw(&self, key: &str) -> Result<String, String> {
        debug!(key = %key, server = %self.base_url, "Deleting file (raw)");
        let req = self.with_auth(self.http.delete(self.url(&format!("/delete/{key}"))));
        self.send_raw(req).await
    }
}
