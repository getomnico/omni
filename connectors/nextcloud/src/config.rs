use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Per-source Nextcloud configuration stored in `Source.config`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NextcloudConfig {
    /// Nextcloud server URL (e.g. `https://cloud.example.com`).
    pub server_url: String,
    /// Base path within the user's file tree to sync (default: `/`).
    #[serde(default = "default_base_path")]
    pub base_path: String,
    /// File extensions to include (empty = all). Case-insensitive.
    #[serde(default)]
    pub extension_allowlist: Vec<String>,
    /// File extensions to exclude. Case-insensitive.
    #[serde(default)]
    pub extension_denylist: Vec<String>,
    /// Maximum file size in bytes to download and index (0 = unlimited).
    #[serde(default)]
    pub max_file_size: u64,
    /// Whether periodic sync is enabled.
    #[serde(default = "default_true")]
    pub sync_enabled: bool,
}

fn default_base_path() -> String {
    "/".to_string()
}

fn default_true() -> bool {
    true
}

impl NextcloudConfig {
    pub fn from_source_config(config: &serde_json::Value) -> Result<Self> {
        serde_json::from_value(config.clone()).context("Failed to parse Nextcloud source config")
    }

    /// Returns the full WebDAV files endpoint for the given user.
    ///
    /// Each segment of `base_path` is percent-encoded so that folder names
    /// with spaces or special characters produce valid URLs.
    pub fn webdav_base_url(&self, username: &str) -> String {
        let base = self.server_url.trim_end_matches('/');
        let path = self.base_path.trim_matches('/');
        if path.is_empty() {
            format!("{}/remote.php/dav/files/{}", base, username)
        } else {
            let encoded_path = path
                .split('/')
                .map(|seg| urlencoding::encode(seg))
                .collect::<Vec<_>>()
                .join("/");
            format!(
                "{}/remote.php/dav/files/{}/{}",
                base, username, encoded_path
            )
        }
    }

    /// Whether a given filename should be indexed based on extension filters.
    pub fn should_index_file(&self, filename: &str) -> bool {
        let ext = match filename.rfind('.') {
            Some(pos) => filename[pos + 1..].to_ascii_lowercase(),
            // No file extension: allow unless an allowlist is active.
            None => return self.extension_allowlist.is_empty(),
        };

        if self
            .extension_denylist
            .iter()
            .any(|d| d.eq_ignore_ascii_case(&ext))
        {
            return false;
        }
        if !self.extension_allowlist.is_empty() {
            return self
                .extension_allowlist
                .iter()
                .any(|a| a.eq_ignore_ascii_case(&ext));
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_config_from_json_defaults() {
        let cfg_json = json!({
            "server_url": "https://cloud.example.com"
        });
        let cfg: NextcloudConfig = serde_json::from_value(cfg_json).unwrap();
        assert_eq!(cfg.server_url, "https://cloud.example.com");
        assert_eq!(cfg.base_path, "/");
        assert!(cfg.sync_enabled);
        assert!(cfg.extension_allowlist.is_empty());
        assert!(cfg.extension_denylist.is_empty());
        assert_eq!(cfg.max_file_size, 0);
    }

    #[test]
    fn test_webdav_base_url_root() {
        let cfg = NextcloudConfig {
            server_url: "https://cloud.example.com".into(),
            base_path: "/".into(),
            extension_allowlist: vec![],
            extension_denylist: vec![],
            max_file_size: 0,
            sync_enabled: true,
        };
        assert_eq!(
            cfg.webdav_base_url("alice"),
            "https://cloud.example.com/remote.php/dav/files/alice"
        );
    }

    #[test]
    fn test_webdav_base_url_subpath() {
        let cfg = NextcloudConfig {
            server_url: "https://cloud.example.com/".into(),
            base_path: "/Documents/Work".into(),
            extension_allowlist: vec![],
            extension_denylist: vec![],
            max_file_size: 0,
            sync_enabled: true,
        };
        assert_eq!(
            cfg.webdav_base_url("bob"),
            "https://cloud.example.com/remote.php/dav/files/bob/Documents/Work"
        );
    }

    #[test]
    fn test_should_index_file_no_filters() {
        let cfg = NextcloudConfig {
            server_url: "https://x.com".into(),
            base_path: "/".into(),
            extension_allowlist: vec![],
            extension_denylist: vec![],
            max_file_size: 0,
            sync_enabled: true,
        };
        assert!(cfg.should_index_file("report.pdf"));
        assert!(cfg.should_index_file("image.png"));
        assert!(cfg.should_index_file("noext"));
    }

    #[test]
    fn test_should_index_file_allowlist() {
        let cfg = NextcloudConfig {
            server_url: "https://x.com".into(),
            base_path: "/".into(),
            extension_allowlist: vec!["pdf".into(), "docx".into()],
            extension_denylist: vec![],
            max_file_size: 0,
            sync_enabled: true,
        };
        assert!(cfg.should_index_file("report.pdf"));
        assert!(cfg.should_index_file("doc.DOCX"));
        assert!(!cfg.should_index_file("image.png"));
    }

    #[test]
    fn test_should_index_file_denylist() {
        let cfg = NextcloudConfig {
            server_url: "https://x.com".into(),
            base_path: "/".into(),
            extension_allowlist: vec![],
            extension_denylist: vec!["tmp".into(), "log".into()],
            max_file_size: 0,
            sync_enabled: true,
        };
        assert!(cfg.should_index_file("report.pdf"));
        assert!(!cfg.should_index_file("debug.log"));
        assert!(!cfg.should_index_file("cache.TMP"));
    }

    #[test]
    fn test_should_index_file_denylist_beats_allowlist() {
        let cfg = NextcloudConfig {
            server_url: "https://x.com".into(),
            base_path: "/".into(),
            extension_allowlist: vec!["pdf".into()],
            extension_denylist: vec!["pdf".into()],
            max_file_size: 0,
            sync_enabled: true,
        };
        assert!(!cfg.should_index_file("report.pdf"));
    }

    #[test]
    fn test_should_index_file_extensionless() {
        // No filters: extensionless file allowed
        let no_filters = NextcloudConfig {
            server_url: "https://x.com".into(),
            base_path: "/".into(),
            extension_allowlist: vec![],
            extension_denylist: vec![],
            max_file_size: 0,
            sync_enabled: true,
        };
        assert!(no_filters.should_index_file("Makefile"));

        // Allowlist active: extensionless file excluded (no extension to match)
        let with_allow = NextcloudConfig {
            extension_allowlist: vec!["pdf".into()],
            ..no_filters.clone()
        };
        assert!(!with_allow.should_index_file("Makefile"));

        // Denylist active: extensionless file allowed (no extension to deny)
        let with_deny = NextcloudConfig {
            extension_denylist: vec!["tmp".into()],
            ..no_filters.clone()
        };
        assert!(with_deny.should_index_file("Makefile"));
    }

    #[test]
    fn test_webdav_base_url_encodes_spaces() {
        let cfg = NextcloudConfig {
            server_url: "https://cloud.example.com".into(),
            base_path: "/My Documents/Work Files".into(),
            extension_allowlist: vec![],
            extension_denylist: vec![],
            max_file_size: 0,
            sync_enabled: true,
        };
        assert_eq!(
            cfg.webdav_base_url("alice"),
            "https://cloud.example.com/remote.php/dav/files/alice/My%20Documents/Work%20Files"
        );
    }

    #[test]
    fn test_webdav_base_url_plain_segments_unchanged() {
        // Segments without special characters should pass through unmodified.
        let cfg = NextcloudConfig {
            server_url: "https://cloud.example.com".into(),
            base_path: "/Documents/Work".into(),
            extension_allowlist: vec![],
            extension_denylist: vec![],
            max_file_size: 0,
            sync_enabled: true,
        };
        assert_eq!(
            cfg.webdav_base_url("alice"),
            "https://cloud.example.com/remote.php/dav/files/alice/Documents/Work"
        );
    }
}
