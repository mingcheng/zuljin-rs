use crate::http::{ApiResponse, DeleteResult, DiskInfo, FileInfo, UploadResult};
use reqwest::multipart;
use serde::de::DeserializeOwned;
use std::path::Path;

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

    /// Send a request, parse the `ApiResponse<T>` envelope, and extract the data or error.
    async fn send_and_parse<T: DeserializeOwned>(
        &self,
        req: reqwest::RequestBuilder,
    ) -> Result<T, String> {
        let body: ApiResponse<T> = req
            .send()
            .await
            .map_err(|e| format!("Request failed: {e}"))?
            .json()
            .await
            .map_err(|e| format!("Failed to parse response: {e}"))?;

        if body.success {
            body.data.ok_or_else(|| "Empty response".to_string())
        } else {
            Err(body.error.unwrap_or_else(|| "Unknown error".to_string()))
        }
    }

    async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        let req = self.with_auth(self.http.get(self.url(path)));
        self.send_and_parse(req).await
    }

    pub async fn upload(&self, file_path: &str) -> Result<Vec<UploadResult>, String> {
        let path = Path::new(file_path);
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
            .to_string();

        let data = std::fs::read(path).map_err(|e| format!("Failed to read file: {e}"))?;

        let part = multipart::Part::bytes(data).file_name(file_name);
        let form = multipart::Form::new().part("file", part);

        let req = self
            .with_auth(self.http.post(self.url("/upload")))
            .multipart(form);

        self.send_and_parse(req).await
    }

    /// Download raw file bytes. No token required (public endpoint).
    pub async fn download(&self, key: &str) -> Result<Vec<u8>, String> {
        let resp = self
            .http
            .get(self.url(&format!("/files/{key}")))
            .send()
            .await
            .map_err(|e| format!("Request failed: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("Server returned {}", resp.status()));
        }

        resp.bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| format!("Failed to read response: {e}"))
    }

    pub async fn info(&self, key: &str) -> Result<FileInfo, String> {
        self.get_json(&format!("/api/info/{key}")).await
    }

    pub async fn disk(&self) -> Result<DiskInfo, String> {
        self.get_json("/api/disk").await
    }

    pub async fn delete(&self, key: &str) -> Result<DeleteResult, String> {
        let req = self.with_auth(self.http.delete(self.url(&format!("/api/delete/{key}"))));
        self.send_and_parse(req).await
    }
}
