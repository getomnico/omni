use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A single entry from a WebDAV PROPFIND response.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DavEntry {
    /// WebDAV href (path or absolute URL).
    pub href: String,
    /// Whether this entry is a collection (directory).
    pub is_collection: bool,
    /// Display name.
    pub display_name: Option<String>,
    /// MIME content type.
    pub content_type: Option<String>,
    /// File size in bytes.
    pub content_length: Option<u64>,
    /// ETag for change detection.
    pub etag: Option<String>,
    /// Last modified timestamp string.
    pub last_modified: Option<String>,
    /// Creation date string.
    pub creation_date: Option<String>,
    /// Nextcloud file ID.
    pub file_id: Option<String>,
    /// OC permission string (e.g. "RGDNVW").
    pub permissions: Option<String>,
    /// oc:size (works for both files and folders).
    pub oc_size: Option<u64>,
    /// Owner user ID.
    pub owner_id: Option<String>,
    /// Owner display name.
    pub owner_display_name: Option<String>,
    /// Whether this file is marked as favorite.
    pub favorite: bool,
}

impl DavEntry {
    /// Extract the file name from the href path.
    pub fn filename(&self) -> String {
        if let Some(name) = &self.display_name {
            if !name.is_empty() {
                return name.clone();
            }
        }
        let path = self.href.trim_end_matches('/');
        let decoded = urlencoding::decode(path.rsplit('/').next().unwrap_or(path))
            .unwrap_or_default()
            .into_owned();
        decoded
    }

    /// Extract the relative path from the href, stripping the WebDAV prefix.
    /// e.g. "/remote.php/dav/files/alice/Documents/report.pdf" → "/Documents/report.pdf"
    ///
    /// Uses the literal `username` (not percent-encoded) to match the hrefs
    /// returned by the server, which mirror the encoding of the request URL.
    pub fn relative_path(&self, username: &str) -> String {
        let prefix = format!("/remote.php/dav/files/{}", username);
        let path = self.href.trim_end_matches('/');
        let decoded = urlencoding::decode(
            path.strip_prefix(&prefix).unwrap_or(path),
        )
        .unwrap_or_default()
        .into_owned();
        if decoded.is_empty() {
            "/".to_string()
        } else {
            decoded
        }
    }

    /// Build a URL to view this file in the Nextcloud web UI.
    pub fn web_url(&self, server_url: &str) -> String {
        let server = server_url.trim_end_matches('/');
        if let Some(ref fid) = self.file_id {
            format!("{}/f/{}", server, fid)
        } else {
            format!("{}{}", server, self.href)
        }
    }

    /// Stable key for change tracking (prefer file_id, fall back to href).
    pub fn file_key(&self) -> String {
        self.file_id
            .clone()
            .unwrap_or_else(|| self.href.clone())
    }

    /// Returns the real etag if available, or a synthetic one derived from
    /// last_modified + content_length for files where the server omits an etag.
    pub fn effective_etag(&self) -> Option<String> {
        if let Some(ref etag) = self.etag {
            return Some(etag.clone());
        }
        match (&self.last_modified, self.content_length.or(self.oc_size)) {
            (Some(lm), Some(size)) => Some(format!("synth:{}:{}", lm, size)),
            (Some(lm), None) => Some(format!("synth:{}", lm)),
            _ => None,
        }
    }

    /// Build the document ID used within Omni. Deterministic and stable.
    pub fn document_id(&self, source_id: &str) -> String {
        let key = self
            .file_id
            .as_deref()
            .unwrap_or(&self.href);
        format!("nextcloud:{}:{}", source_id, urlencoding::encode(key))
    }

    /// Generate a combined markdown document with metadata header and extracted content.
    pub fn to_markdown(&self, username: &str, server_url: &str, content_text: &str) -> String {
        let mut md = String::new();

        // Title
        let title = self.filename();
        md.push_str(&format!("# {}\n\n", title));

        // Metadata table
        md.push_str("| Property | Value |\n|---|---|\n");

        let esc = |s: &str| -> String { s.replace('|', "\\|") };

        md.push_str(&format!(
            "| Path | {} |\n",
            esc(&self.relative_path(username))
        ));

        if let Some(ref ct) = self.content_type {
            md.push_str(&format!("| Content Type | {} |\n", esc(ct)));
        }
        if let Some(size) = self.content_length.or(self.oc_size) {
            md.push_str(&format!("| Size | {} bytes |\n", size));
        }
        if let Some(ref lm) = self.last_modified {
            md.push_str(&format!("| Last Modified | {} |\n", esc(lm)));
        }
        if let Some(ref cd) = self.creation_date {
            md.push_str(&format!("| Created | {} |\n", esc(cd)));
        }
        if let Some(ref owner) = self.owner_display_name {
            md.push_str(&format!("| Owner | {} |\n", esc(owner)));
        } else if let Some(ref oid) = self.owner_id {
            md.push_str(&format!("| Owner | {} |\n", esc(oid)));
        }
        if let Some(ref fid) = self.file_id {
            md.push_str(&format!("| File ID | {} |\n", esc(fid)));
        }
        if let Some(ref etag) = self.etag {
            md.push_str(&format!("| ETag | {} |\n", esc(etag)));
        }
        if let Some(ref perms) = self.permissions {
            md.push_str(&format!("| Permissions | {} |\n", esc(perms)));
        }
        if self.favorite {
            md.push_str("| Favorite | Yes |\n");
        }

        let url = self.web_url(server_url);
        md.push_str(&format!("| URL | {} |\n", esc(&url)));

        md.push('\n');

        // Content
        if !content_text.is_empty() {
            md.push_str("## Content\n\n");
            md.push_str(content_text);
            md.push('\n');
        }

        md
    }
}

/// Connector state persisted across sync runs.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NextcloudConnectorState {
    /// Map of file_id (or href) → etag for change detection.
    pub etags: HashMap<String, String>,
    /// Set of known file keys (file_id or href) from the last sync.
    pub known_files: Vec<String>,
}

impl NextcloudConnectorState {
    pub fn from_connector_state(state: &Option<serde_json::Value>) -> Self {
        state
            .as_ref()
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default()
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entry() -> DavEntry {
        DavEntry {
            href: "/remote.php/dav/files/alice/Documents/report.pdf".to_string(),
            is_collection: false,
            display_name: Some("report.pdf".to_string()),
            content_type: Some("application/pdf".to_string()),
            content_length: Some(102400),
            etag: Some("abc123".to_string()),
            last_modified: Some("Wed, 20 Jul 2022 05:12:23 GMT".to_string()),
            creation_date: Some("2022-01-01T00:00:00+00:00".to_string()),
            file_id: Some("42".to_string()),
            permissions: Some("RGDNVW".to_string()),
            oc_size: Some(102400),
            owner_id: Some("alice".to_string()),
            owner_display_name: Some("Alice Smith".to_string()),
            favorite: true,
        }
    }

    #[test]
    fn test_filename_from_display_name() {
        let entry = sample_entry();
        assert_eq!(entry.filename(), "report.pdf");
    }

    #[test]
    fn test_filename_from_href() {
        let mut entry = sample_entry();
        entry.display_name = None;
        assert_eq!(entry.filename(), "report.pdf");
    }

    #[test]
    fn test_filename_url_encoded() {
        let entry = DavEntry {
            href: "/remote.php/dav/files/alice/My%20File%20(2).pdf".to_string(),
            display_name: None,
            ..Default::default()
        };
        assert_eq!(entry.filename(), "My File (2).pdf");
    }

    #[test]
    fn test_relative_path() {
        let entry = sample_entry();
        assert_eq!(entry.relative_path("alice"), "/Documents/report.pdf");
    }

    #[test]
    fn test_relative_path_root() {
        let entry = DavEntry {
            href: "/remote.php/dav/files/alice/".to_string(),
            ..Default::default()
        };
        assert_eq!(entry.relative_path("alice"), "/");
    }

    #[test]
    fn test_web_url_with_file_id() {
        let entry = sample_entry();
        assert_eq!(
            entry.web_url("https://cloud.example.com"),
            "https://cloud.example.com/f/42"
        );
    }

    #[test]
    fn test_web_url_without_file_id() {
        let mut entry = sample_entry();
        entry.file_id = None;
        assert_eq!(
            entry.web_url("https://cloud.example.com"),
            "https://cloud.example.com/remote.php/dav/files/alice/Documents/report.pdf"
        );
    }

    #[test]
    fn test_document_id_deterministic() {
        let entry = sample_entry();
        let id1 = entry.document_id("src-1");
        let id2 = entry.document_id("src-1");
        assert_eq!(id1, id2);
        assert!(id1.starts_with("nextcloud:src-1:"));
    }

    #[test]
    fn test_to_markdown() {
        let entry = sample_entry();
        let md = entry.to_markdown("alice", "https://cloud.example.com", "Hello world");
        assert!(md.starts_with("# report.pdf\n"));
        assert!(md.contains("| Path | /Documents/report.pdf |"));
        assert!(md.contains("| Content Type | application/pdf |"));
        assert!(md.contains("| Size | 102400 bytes |"));
        assert!(md.contains("| Owner | Alice Smith |"));
        assert!(md.contains("| Favorite | Yes |"));
        assert!(md.contains("| URL | https://cloud.example.com/f/42 |"));
        assert!(md.contains("## Content\n\nHello world"));
    }

    #[test]
    fn test_to_markdown_minimal() {
        let entry = DavEntry {
            href: "/remote.php/dav/files/bob/notes.txt".to_string(),
            ..Default::default()
        };
        let md = entry.to_markdown("bob", "https://nc.local", "Some text");
        assert!(md.starts_with("# notes.txt\n"));
        assert!(md.contains("## Content\n\nSome text"));
    }

    #[test]
    fn test_connector_state_round_trip() {
        let mut state = NextcloudConnectorState::default();
        state.etags.insert("42".into(), "abc".into());
        state.known_files.push("42".into());

        let json = state.to_json();
        let restored = NextcloudConnectorState::from_connector_state(&Some(json));
        assert_eq!(restored.etags.get("42").unwrap(), "abc");
        assert_eq!(restored.known_files, vec!["42".to_string()]);
    }

    #[test]
    fn test_connector_state_from_none() {
        let state = NextcloudConnectorState::from_connector_state(&None);
        assert!(state.etags.is_empty());
        assert!(state.known_files.is_empty());
    }

    #[test]
    fn test_file_key_prefers_file_id() {
        let entry = sample_entry();
        assert_eq!(entry.file_key(), "42");
    }

    #[test]
    fn test_file_key_falls_back_to_href() {
        let mut entry = sample_entry();
        entry.file_id = None;
        assert_eq!(
            entry.file_key(),
            "/remote.php/dav/files/alice/Documents/report.pdf"
        );
    }

    #[test]
    fn test_effective_etag_returns_real_etag() {
        let entry = sample_entry();
        assert_eq!(entry.effective_etag().as_deref(), Some("abc123"));
    }

    #[test]
    fn test_effective_etag_synthetic_fallback() {
        let entry = DavEntry {
            href: "/file.txt".into(),
            etag: None,
            last_modified: Some("Thu, 01 Jan 2024 00:00:00 GMT".into()),
            content_length: Some(1024),
            ..Default::default()
        };
        let etag = entry.effective_etag().unwrap();
        assert!(etag.starts_with("synth:"));
        assert!(etag.contains("1024"));
    }

    #[test]
    fn test_effective_etag_none_when_no_metadata() {
        let entry = DavEntry {
            href: "/file.txt".into(),
            ..Default::default()
        };
        assert!(entry.effective_etag().is_none());
    }
}
