use infer::Infer;
use serde::{Deserialize, Serialize};
use std::fs;
use std::fs::File;
use std::io::Read;

/// Max bytes to read for magic-byte type detection.
const TYPE_DETECT_BYTES: usize = 8192;

#[derive(Debug, Serialize, Deserialize)]
pub struct FileMeta {
    pub(crate) path: String,
    pub(crate) size: usize,
    pub(crate) content_type: Option<String>,
    pub(crate) extension: Option<String>,
}

impl FileMeta {
    pub fn new(path: &str) -> std::io::Result<Self> {
        let path = fs::canonicalize(path)?;

        let mut result = FileMeta {
            path: path.to_str().unwrap().to_string(),
            size: 0,
            content_type: None,
            extension: None,
        };

        result.detect()?;
        Ok(result)
    }

    fn detect(&mut self) -> std::io::Result<()> {
        let mut f = File::open(&self.path)?;
        self.size = f.metadata()?.len() as usize;

        let mut buffer = vec![0u8; TYPE_DETECT_BYTES.min(self.size)];
        f.read_exact(&mut buffer)?;

        let infer = Infer::new();
        if let Some(file_type) = infer.get(&buffer) {
            self.content_type = Some(file_type.mime_type().to_string());
            self.extension = Some(file_type.extension().to_string());
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;

    #[test]
    fn test_new_with_text_file() {
        let dir = tempfile::tempdir().unwrap();
        let temp_path = dir.path().join("test.txt");
        File::create(&temp_path)
            .unwrap()
            .write_all(b"Hello, world!")
            .unwrap();

        let meta = FileMeta::new(temp_path.to_str().unwrap()).unwrap();
        assert_eq!(meta.size, 13);
        assert!(meta.content_type.is_none());
    }

    #[test]
    fn test_new_with_png_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let temp_path = dir.path().join("image.png");
        let png_data = png_header();
        File::create(&temp_path)
            .unwrap()
            .write_all(&png_data)
            .unwrap();

        let meta = FileMeta::new(temp_path.to_str().unwrap()).unwrap();
        assert_eq!(meta.size, png_data.len());
        assert_eq!(meta.content_type.as_deref(), Some("image/png"));
        assert_eq!(meta.extension.as_deref(), Some("png"));
    }

    #[test]
    fn test_new_nonexistent_file() {
        let result = FileMeta::new("/tmp/definitely_does_not_exist_12345.xyz");
        assert!(result.is_err());
    }

    fn png_header() -> Vec<u8> {
        vec![
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00,
            0x00, 0x90, 0x77, 0x53, 0xDE,
        ]
    }
}
