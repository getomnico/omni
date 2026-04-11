use crate::models::{FileSystemFile, FileSystemPermissions, FileSystemSource};
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};
use walkdir::WalkDir;

pub struct FileSystemScanner {
    source: FileSystemSource,
}

impl FileSystemScanner {
    pub fn new(source: FileSystemSource) -> Self {
        Self { source }
    }

    pub async fn scan_directory(&self) -> Result<Vec<FileSystemFile>> {
        info!("Starting filesystem scan for source: {}", self.source.name);
        let mut files = Vec::new();

        if !self.source.base_path.exists() {
            return Err(anyhow::anyhow!(
                "Base path does not exist: {}",
                self.source.base_path.display()
            ));
        }

        if !self.source.base_path.is_dir() {
            return Err(anyhow::anyhow!(
                "Base path is not a directory: {}",
                self.source.base_path.display()
            ));
        }

        let walker = WalkDir::new(&self.source.base_path)
            .follow_links(false)
            .max_depth(100);

        for entry in walker {
            match entry {
                Ok(entry) => {
                    if let Some(file) = self.process_entry(entry).await? {
                        files.push(file);
                    }
                }
                Err(e) => {
                    warn!("Error walking directory: {}", e);
                    continue;
                }
            }
        }

        info!("Completed filesystem scan, found {} files", files.len());
        Ok(files)
    }

    async fn process_entry(&self, entry: walkdir::DirEntry) -> Result<Option<FileSystemFile>> {
        let path = entry.path().to_path_buf();
        let metadata = match entry.metadata() {
            Ok(m) => m,
            Err(e) => {
                warn!("Failed to get metadata for {}: {}", path.display(), e);
                return Ok(None);
            }
        };

        let is_directory = metadata.is_dir();

        // Skip directories for now, we only want files
        if is_directory {
            return Ok(None);
        }

        // Check if file should be included based on filters
        if !self.source.should_include_file(&path) {
            debug!("Skipping file due to filters: {}", path.display());
            return Ok(None);
        }

        // Check file size limit
        if let Some(max_size) = self.source.max_file_size_bytes {
            if metadata.len() > max_size {
                debug!(
                    "Skipping file due to size limit ({} > {}): {}",
                    metadata.len(),
                    max_size,
                    path.display()
                );
                return Ok(None);
            }
        }

        let name = entry.file_name().to_string_lossy().to_string();

        let mime_type = mime_guess::from_path(&path)
            .first_or_octet_stream()
            .to_string();

        let permissions = self.get_file_permissions(&path)?;

        let filesystem_file = FileSystemFile {
            path: path.clone(),
            name,
            size: metadata.len(),
            mime_type,
            created_time: metadata.created().ok(),
            modified_time: metadata.modified().ok(),
            is_directory,
            permissions,
        };

        debug!("Processed file: {}", path.display());
        Ok(Some(filesystem_file))
    }

    /// Get file info from a path directly (used by watcher for real-time events)
    pub async fn get_file_info(&self, path: &PathBuf) -> Result<Option<FileSystemFile>> {
        let metadata = match tokio::fs::metadata(path).await {
            Ok(m) => m,
            Err(e) => {
                warn!("Failed to get metadata for {}: {}", path.display(), e);
                return Ok(None);
            }
        };

        let is_directory = metadata.is_dir();

        // Skip directories
        if is_directory {
            return Ok(None);
        }

        // Check if file should be included based on filters
        if !self.source.should_include_file(path) {
            debug!("Skipping file due to filters: {}", path.display());
            return Ok(None);
        }

        // Check file size limit
        if let Some(max_size) = self.source.max_file_size_bytes {
            if metadata.len() > max_size {
                debug!(
                    "Skipping file due to size limit ({} > {}): {}",
                    metadata.len(),
                    max_size,
                    path.display()
                );
                return Ok(None);
            }
        }

        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        let mime_type = mime_guess::from_path(path)
            .first_or_octet_stream()
            .to_string();

        let permissions = self.get_file_permissions(path)?;

        // Convert std metadata times to SystemTime
        let created_time = metadata.created().ok();
        let modified_time = metadata.modified().ok();

        let file = FileSystemFile {
            path: path.clone(),
            name,
            size: metadata.len(),
            mime_type,
            created_time,
            modified_time,
            is_directory,
            permissions,
        };

        debug!("Got file info: {}", path.display());
        Ok(Some(file))
    }

    fn get_file_permissions(&self, path: &PathBuf) -> Result<FileSystemPermissions> {
        let metadata = fs::metadata(path)
            .with_context(|| format!("Failed to get metadata for {}", path.display()))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = metadata.permissions().mode();

            // Check owner permissions (assuming we're the owner for simplicity)
            let readable = (mode & 0o400) != 0;
            let writable = (mode & 0o200) != 0;
            let executable = (mode & 0o100) != 0;

            Ok(FileSystemPermissions {
                readable,
                writable,
                executable,
            })
        }

        #[cfg(windows)]
        {
            let readonly = metadata.permissions().readonly();

            Ok(FileSystemPermissions {
                readable: true, // Assume readable if we can access it
                writable: !readonly,
                executable: false, // Windows doesn't have simple executable bit
            })
        }
    }

    pub async fn read_file_content(&self, file: &FileSystemFile) -> Result<String> {
        if file.is_directory {
            return Ok(String::new());
        }

        const MAX_CONTENT_SIZE: u64 = 50 * 1024 * 1024; // 50MB
        if file.size > MAX_CONTENT_SIZE {
            warn!(
                "File too large to read content: {} ({}MB)",
                file.path.display(),
                file.size / 1024 / 1024
            );
            return Ok(String::new());
        }

        // Source code and config files should not be indexed
        if is_source_code_or_config(&file.path) {
            debug!("Skipping source/config file: {}", file.path.display());
            return Ok(String::new());
        }

        let data = std::fs::read(&file.path)
            .with_context(|| format!("Failed to read file: {}", file.path.display()))?;

        let filename = file.path.file_name().and_then(|n| n.to_str());

        match shared::content_extractor::extract_content(&data, &file.mime_type, filename) {
            Ok(content) => {
                if !content.is_empty() {
                    debug!(
                        "Extracted {} bytes from {}",
                        content.len(),
                        file.path.display()
                    );
                }
                Ok(content)
            }
            Err(e) => {
                warn!(
                    "Failed to extract content from {}: {}",
                    file.path.display(),
                    e
                );
                Ok(String::new())
            }
        }
    }
}

/// Returns true for source code and config file extensions that should not be indexed.
fn is_source_code_or_config(path: &Path) -> bool {
    let ext = match path.extension().and_then(|e| e.to_str()) {
        Some(e) => e.to_lowercase(),
        None => return false,
    };

    matches!(
        ext.as_str(),
        "rs" | "py"
            | "js"
            | "ts"
            | "jsx"
            | "tsx"
            | "go"
            | "java"
            | "c"
            | "cpp"
            | "h"
            | "hpp"
            | "rb"
            | "sh"
            | "bash"
            | "zsh"
            | "fish"
            | "pl"
            | "pm"
            | "php"
            | "swift"
            | "kt"
            | "scala"
            | "clj"
            | "ex"
            | "exs"
            | "erl"
            | "hs"
            | "lua"
            | "r"
            | "m"
            | "mm"
            | "cs"
            | "fs"
            | "vb"
            | "dart"
            | "zig"
            | "nim"
            | "v"
            | "sql"
            | "xml"
            | "yaml"
            | "yml"
            | "toml"
            | "json"
            | "ini"
            | "conf"
            | "cfg"
            | "hocon"
            | "properties"
            | "html"
            | "htm"
            | "css"
            | "scss"
            | "sass"
            | "less"
    )
}

#[cfg(test)]
mod tests {
    use super::FileSystemScanner;
    use crate::models::{FileSystemFile, FileSystemPermissions, FileSystemSource};
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn create_text_file(dir: &TempDir, name: &str, content: &str) -> PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, content).unwrap();
        path
    }

    fn create_docx_file(dir: &TempDir, name: &str, text: &str) -> PathBuf {
        let path = dir.path().join(name);
        let docx = docx_rs::Docx::new()
            .add_paragraph(docx_rs::Paragraph::new().add_run(docx_rs::Run::new().add_text(text)));
        let file = std::fs::File::create(&path).unwrap();
        docx.build().pack(file).unwrap();
        path
    }

    fn make_source(dir: &TempDir) -> FileSystemSource {
        FileSystemSource {
            id: "test-source".to_string(),
            name: "Test Source".to_string(),
            base_path: dir.path().to_path_buf(),
            scan_interval_seconds: 300,
            file_extensions: None,
            exclude_patterns: None,
            max_file_size_bytes: None,
        }
    }

    fn make_file(path: PathBuf, mime_type: &str) -> FileSystemFile {
        let metadata = std::fs::metadata(&path).unwrap();
        FileSystemFile {
            name: path.file_name().unwrap().to_string_lossy().to_string(),
            size: metadata.len(),
            mime_type: mime_type.to_string(),
            created_time: metadata.created().ok(),
            modified_time: metadata.modified().ok(),
            is_directory: false,
            permissions: FileSystemPermissions {
                readable: true,
                writable: true,
                executable: false,
            },
            path,
        }
    }

    #[tokio::test]
    async fn test_scan_directory_discovers_files() {
        let dir = TempDir::new().unwrap();
        create_text_file(&dir, "notes.txt", "some text");
        create_text_file(&dir, "readme.md", "# Hello");
        create_docx_file(&dir, "doc.docx", "word content");

        let source = make_source(&dir);
        let scanner = FileSystemScanner::new(source);
        let files = scanner.scan_directory().await.unwrap();

        assert_eq!(files.len(), 3);
    }

    #[tokio::test]
    async fn test_scan_directory_extension_filter() {
        let dir = TempDir::new().unwrap();
        create_text_file(&dir, "notes.txt", "text content");
        create_text_file(&dir, "readme.md", "markdown content");
        create_text_file(&dir, "code.rs", "fn main() {}");

        let mut source = make_source(&dir);
        source.file_extensions = Some(vec!["txt".to_string()]);

        let scanner = FileSystemScanner::new(source);
        let files = scanner.scan_directory().await.unwrap();

        assert_eq!(files.len(), 1);
        assert!(files[0].name.ends_with(".txt"));
    }

    #[tokio::test]
    async fn test_scan_directory_exclude_patterns() {
        let dir = TempDir::new().unwrap();
        create_text_file(&dir, "notes.txt", "text content");

        // Create a subdirectory with a file
        let sub = dir.path().join("hidden");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("secret.txt"), "secret").unwrap();

        let mut source = make_source(&dir);
        source.exclude_patterns = Some(vec!["hidden".to_string()]);

        let scanner = FileSystemScanner::new(source);
        let files = scanner.scan_directory().await.unwrap();

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name, "notes.txt");
    }

    #[tokio::test]
    async fn test_scan_directory_size_limit() {
        let dir = TempDir::new().unwrap();
        create_text_file(&dir, "small.txt", "small");

        let big_path = dir.path().join("big.txt");
        std::fs::write(&big_path, vec![b'x'; 2000]).unwrap();

        let mut source = make_source(&dir);
        source.max_file_size_bytes = Some(1000);

        let scanner = FileSystemScanner::new(source);
        let files = scanner.scan_directory().await.unwrap();

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name, "small.txt");
    }

    #[tokio::test]
    async fn test_read_file_content_text() {
        let dir = TempDir::new().unwrap();
        let path = create_text_file(&dir, "notes.txt", "Hello from scanner");
        let file = make_file(path, "text/plain");

        let source = make_source(&dir);
        let scanner = FileSystemScanner::new(source);
        let content = scanner.read_file_content(&file).await.unwrap();

        assert_eq!(content, "Hello from scanner");
    }

    #[tokio::test]
    async fn test_read_file_content_docx() {
        let dir = TempDir::new().unwrap();
        let path = create_docx_file(&dir, "doc.docx", "Scanner DOCX test");
        let file = make_file(
            path,
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        );

        let source = make_source(&dir);
        let scanner = FileSystemScanner::new(source);
        let content = scanner.read_file_content(&file).await.unwrap();

        assert!(
            content.contains("Scanner DOCX test"),
            "Expected DOCX content via scanner, got: '{}'",
            content
        );
    }

    #[tokio::test]
    async fn test_read_file_content_source_code_returns_empty() {
        let dir = TempDir::new().unwrap();
        let path = create_text_file(&dir, "main.rs", "fn main() {}");
        let file = make_file(path, "text/plain");

        let source = make_source(&dir);
        let scanner = FileSystemScanner::new(source);
        let content = scanner.read_file_content(&file).await.unwrap();

        assert!(
            content.is_empty(),
            "Expected empty for source code, got: '{}'",
            content
        );
    }

    #[tokio::test]
    async fn test_read_file_content_directory_returns_empty() {
        let dir = TempDir::new().unwrap();

        let file = FileSystemFile {
            path: dir.path().to_path_buf(),
            name: "testdir".to_string(),
            size: 0,
            mime_type: "inode/directory".to_string(),
            created_time: None,
            modified_time: None,
            is_directory: true,
            permissions: FileSystemPermissions {
                readable: true,
                writable: true,
                executable: true,
            },
        };

        let source = make_source(&dir);
        let scanner = FileSystemScanner::new(source);
        let content = scanner.read_file_content(&file).await.unwrap();

        assert!(content.is_empty());
    }
}
