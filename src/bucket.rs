use crate::meta::FileMeta;
use chrono::Local;
use infer::Infer;
use std::path::{Path, PathBuf};
use std::{env, fs};

pub struct Bucket {
    pub path: PathBuf,
}

/// Resolve the best file extension from content bytes, an optional MIME type hint,
/// and an optional original filename. Priority:
///   1. Magic-byte detection via `infer`
///   2. MIME type hint (e.g. from the multipart Content-Type header)
///   3. Original filename extension
///   4. Fallback to "bin"
fn resolve_extension(data: &[u8], mime_hint: Option<&str>, original_name: Option<&str>) -> String {
    // 1. Magic-byte detection
    if let Some(file_type) = Infer::new().get(data) {
        return file_type.extension().to_string();
    }

    // 2. MIME type hint -> extension
    if let Some(ext) = mime_hint
        .and_then(mime_guess::get_mime_extensions_str)
        .and_then(|exts| exts.first())
    {
        return ext.to_string();
    }

    // 3. Original filename extension
    if let Some(ext) = original_name
        .map(Path::new)
        .and_then(|p| p.extension())
        .and_then(|e| e.to_str())
    {
        return ext.to_string();
    }

    "bin".to_string()
}

impl Bucket {
    pub fn new(path: &str) -> std::io::Result<Self> {
        let path = if Path::new(path).is_absolute() {
            PathBuf::from(path)
        } else {
            env::current_dir()?.join(path)
        };

        if !path.exists() {
            fs::create_dir_all(&path)?;
        } else if !path.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Path is not a directory",
            ));
        }

        Ok(Bucket { path })
    }

    /// Resolve a key to its full filesystem path, returning an error if it doesn't exist.
    fn resolve(&self, key: &str) -> std::io::Result<PathBuf> {
        let full_path = self.path.join(key);
        if !full_path.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "File not found",
            ));
        }
        Ok(full_path)
    }

    /// Save file content and return the generated key (`YYYY/mm/<timestamp>.<ext>`).
    pub fn save(
        &self,
        data: Vec<u8>,
        mime_hint: Option<&str>,
        original_name: Option<&str>,
    ) -> std::io::Result<String> {
        let now = Local::now();

        // Determine file extension using content, MIME hint, and original name
        let ext = resolve_extension(&data, mime_hint, original_name);

        // Generate key with date-based directories and timestamp filename
        let key = format!(
            "{}/{}/{}.{}",
            now.format("%Y"),
            now.format("%m"),
            now.timestamp(),
            ext
        );

        let full_path = self.path.join(&key);
        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent)?;
        }

        fs::write(&full_path, &data)?;
        Ok(key)
    }

    pub fn get_meta(&self, key: &str) -> std::io::Result<FileMeta> {
        let full_path = self.resolve(key)?;
        FileMeta::new(full_path.to_str().unwrap())
    }

    pub fn get_content(&self, key: &str) -> std::io::Result<Vec<u8>> {
        let full_path = self.resolve(key)?;
        fs::read(full_path)
    }

    /// Get available disk space for the bucket path.
    pub fn available_space(&self) -> std::io::Result<u64> {
        #[cfg(unix)]
        {
            use std::ffi::CString;
            use std::mem;

            let c_path = CString::new(self.path.to_str().unwrap()).map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid path")
            })?;

            unsafe {
                let mut stat: libc::statvfs = mem::zeroed();
                if libc::statvfs(c_path.as_ptr(), &mut stat) == 0 {
                    Ok(stat.f_bavail as u64 * stat.f_frsize as u64)
                } else {
                    Err(std::io::Error::last_os_error())
                }
            }
        }

        #[cfg(not(unix))]
        {
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "disk space query not supported on this platform",
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal PNG header for type-detection tests.
    const PNG_HEADER: [u8; 16] = [
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52,
    ];

    fn make_temp_bucket() -> (Bucket, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let bucket = Bucket::new(dir.path().to_str().unwrap()).unwrap();
        (bucket, dir)
    }

    // --- resolve_extension ---

    #[test]
    fn test_resolve_ext_magic_bytes_wins() {
        assert_eq!(
            resolve_extension(&PNG_HEADER, Some("text/csv"), Some("data.csv")),
            "png"
        );
    }

    #[test]
    fn test_resolve_ext_mime_hint() {
        assert_eq!(resolve_extension(b"hello", Some("text/csv"), None), "csv");
    }

    #[test]
    fn test_resolve_ext_original_name() {
        assert_eq!(resolve_extension(b"hello", None, Some("notes.md")), "md");
    }

    #[test]
    fn test_resolve_ext_fallback() {
        assert_eq!(resolve_extension(b"hello", None, None), "bin");
    }

    // --- Bucket ---

    #[test]
    fn test_new_creates_directory() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub_bucket");
        assert!(!sub.exists());
        Bucket::new(sub.to_str().unwrap()).unwrap();
        assert!(sub.exists());
    }

    #[test]
    fn test_new_rejects_file_path() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("not_a_dir");
        fs::write(&file_path, b"x").unwrap();
        assert!(Bucket::new(file_path.to_str().unwrap()).is_err());
    }

    #[test]
    fn test_save_key_format() {
        let (bucket, _dir) = make_temp_bucket();
        let key = bucket
            .save(b"hello".to_vec(), None, Some("test.txt"))
            .unwrap();
        let parts: Vec<&str> = key.split('/').collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0].len(), 4); // YYYY
        assert_eq!(parts[1].len(), 2); // mm
        assert!(parts[2].ends_with(".txt"));
    }

    #[test]
    fn test_save_and_get_content() {
        let (bucket, _dir) = make_temp_bucket();
        let data = b"test content".to_vec();
        let key = bucket.save(data.clone(), None, Some("hello.txt")).unwrap();
        assert_eq!(bucket.get_content(&key).unwrap(), data);
    }

    #[test]
    fn test_save_with_mime_hint() {
        let (bucket, _dir) = make_temp_bucket();
        let key = bucket
            .save(b"col1,col2\na,b\n".to_vec(), Some("text/csv"), None)
            .unwrap();
        assert!(key.ends_with(".csv"));
    }

    #[test]
    fn test_get_content_not_found() {
        let (bucket, _dir) = make_temp_bucket();
        assert!(bucket.get_content("nonexistent/file.txt").is_err());
    }

    #[test]
    fn test_get_meta() {
        let (bucket, _dir) = make_temp_bucket();
        let data = b"some bytes".to_vec();
        let key = bucket.save(data.clone(), None, Some("demo.bin")).unwrap();
        assert_eq!(bucket.get_meta(&key).unwrap().size, data.len());
    }

    #[test]
    fn test_get_meta_not_found() {
        let (bucket, _dir) = make_temp_bucket();
        assert!(bucket.get_meta("no/such/file.bin").is_err());
    }

    #[test]
    fn test_available_space() {
        let (bucket, _dir) = make_temp_bucket();
        assert!(bucket.available_space().unwrap() > 0);
    }
}
