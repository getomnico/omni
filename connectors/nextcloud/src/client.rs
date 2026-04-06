use anyhow::{Context, Result};
use reqwest::Client;
use tracing::{debug, warn};

use crate::models::DavEntry;

/// HTTP client for Nextcloud WebDAV operations.
pub struct NextcloudClient {
    client: Client,
    username: String,
    password: String,
}

/// PROPFIND XML body requesting comprehensive file metadata.
const PROPFIND_BODY: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<d:propfind xmlns:d="DAV:" xmlns:oc="http://owncloud.org/ns" xmlns:nc="http://nextcloud.org/ns">
  <d:prop>
    <d:getlastmodified/>
    <d:getetag/>
    <d:getcontenttype/>
    <d:getcontentlength/>
    <d:resourcetype/>
    <d:displayname/>
    <d:creationdate/>
    <oc:fileid/>
    <oc:permissions/>
    <oc:size/>
    <oc:owner-id/>
    <oc:owner-display-name/>
    <oc:favorite/>
  </d:prop>
</d:propfind>"#;

impl NextcloudClient {
    pub fn new(username: &str, password: &str) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .connect_timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to build HTTP client");
        Self {
            client,
            username: username.to_string(),
            password: password.to_string(),
        }
    }

    /// List all files recursively under `base_url` via PROPFIND Depth: infinity.
    /// Falls back to Depth: 1 with recursive descent if infinity is rejected.
    pub async fn list_files(&self, base_url: &str) -> Result<Vec<DavEntry>> {
        match self.try_list_all(base_url).await {
            Ok(entries) => Ok(entries),
            Err(_) => {
                debug!("Depth: infinity not supported, falling back to recursive descent");
                let mut all = Vec::new();
                let mut visited = std::collections::HashSet::new();
                self.list_recursive(base_url, &mut all, 0, &mut visited).await?;
                Ok(all)
            }
        }
    }

    /// List all files via PROPFIND Depth: infinity. Returns an error if the
    /// server rejects Depth: infinity (common for large instances).
    pub async fn try_list_all(&self, base_url: &str) -> Result<Vec<DavEntry>> {
        self.propfind(base_url, "infinity").await
    }

    /// List a single directory's immediate children via PROPFIND Depth: 1.
    pub async fn list_directory(&self, url: &str) -> Result<Vec<DavEntry>> {
        self.propfind(url, "1").await
    }

    /// Maximum recursion depth when using Depth:1 fallback to prevent infinite loops
    /// from symlink cycles or misconfigured shares.
    const MAX_RECURSION_DEPTH: usize = 100;

    /// Recursive PROPFIND with Depth: 1.
    fn list_recursive<'a>(
        &'a self,
        url: &'a str,
        out: &'a mut Vec<DavEntry>,
        depth: usize,
        visited: &'a mut std::collections::HashSet<String>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            if depth > Self::MAX_RECURSION_DEPTH {
                warn!("Maximum recursion depth ({}) reached at {}, skipping", Self::MAX_RECURSION_DEPTH, url);
                return Ok(());
            }

            let canonical = extract_path(url).trim_end_matches('/').to_string();
            if !visited.insert(canonical.clone()) {
                warn!("Cycle detected: already visited {}, skipping", url);
                return Ok(());
            }

            let entries = self.propfind(url, "1").await?;
            let parent_path = canonical;

            for entry in entries {
                let entry_path = entry.href.trim_end_matches('/');
                // Skip the parent directory itself
                if entry_path == parent_path {
                    continue;
                }
                if entry.is_collection {
                    // Recurse into subdirectory
                    let child_url = build_child_url(url, &entry.href);
                    self.list_recursive(&child_url, out, depth + 1, visited).await?;
                } else {
                    out.push(entry);
                }
            }
            Ok(())
        })
    }

    /// Execute a PROPFIND request and parse the multistatus XML response.
    async fn propfind(&self, url: &str, depth: &str) -> Result<Vec<DavEntry>> {
        let response = self
            .client
            .request(reqwest::Method::from_bytes(b"PROPFIND").unwrap(), url)
            .basic_auth(&self.username, Some(&self.password))
            .header("Depth", depth)
            .header("Content-Type", "application/xml")
            .body(PROPFIND_BODY)
            .send()
            .await
            .context("PROPFIND request failed")?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("PROPFIND failed with status {}: {}", status, body);
        }

        let xml = response.text().await.context("Failed to read PROPFIND response body")?;
        parse_multistatus(&xml)
    }

    /// Download a file by its WebDAV URL. Returns raw bytes.
    pub async fn download_file(&self, url: &str) -> Result<Vec<u8>> {
        let response = self
            .client
            .get(url)
            .basic_auth(&self.username, Some(&self.password))
            .send()
            .await
            .context("GET file download failed")?;

        if !response.status().is_success() {
            let status = response.status();
            anyhow::bail!("File download failed with status {}", status);
        }

        let bytes = response.bytes().await.context("Failed to read file bytes")?;
        Ok(bytes.to_vec())
    }

    /// Validate credentials by issuing a PROPFIND Depth:0 on the user's root.
    pub async fn validate_credentials(&self, base_url: &str) -> Result<bool> {
        let response = self
            .client
            .request(reqwest::Method::from_bytes(b"PROPFIND").unwrap(), base_url)
            .basic_auth(&self.username, Some(&self.password))
            .header("Depth", "0")
            .header("Content-Type", "application/xml")
            .body(PROPFIND_BODY)
            .send()
            .await
            .context("Credential validation PROPFIND failed")?;

        Ok(response.status().is_success())
    }
}

/// Build an absolute URL for a child entry, handling relative vs absolute hrefs.
pub(crate) fn build_child_url(parent_url: &str, child_href: &str) -> String {
    if child_href.starts_with("http://") || child_href.starts_with("https://") {
        return child_href.to_string();
    }
    // child_href is a path like /remote.php/dav/files/user/path/
    // Extract scheme + host from parent_url
    if let Some(idx) = parent_url.find("://") {
        if let Some(slash_idx) = parent_url[idx + 3..].find('/') {
            let origin = &parent_url[..idx + 3 + slash_idx];
            return format!("{}{}", origin, child_href);
        }
    }
    // Fallback: just use parent + child
    format!("{}/{}", parent_url.trim_end_matches('/'), child_href.trim_start_matches('/'))
}

/// Extract path component from a URL (e.g. "http://host/path" → "/path").
pub(crate) fn extract_path(url: &str) -> &str {
    if let Some(idx) = url.find("://") {
        if let Some(slash_idx) = url[idx + 3..].find('/') {
            return &url[idx + 3 + slash_idx..];
        }
    }
    url
}

/// Parse a WebDAV multistatus XML response into a list of `DavEntry` structs.
pub fn parse_multistatus(xml: &str) -> Result<Vec<DavEntry>> {
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;

    let mut reader = Reader::from_str(xml);

    let mut entries: Vec<DavEntry> = Vec::new();
    let mut current: Option<DavEntry> = None;
    let mut text_buf = String::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => {
                let name_bytes = e.name();
                let name_str = std::str::from_utf8(name_bytes.as_ref()).unwrap_or("");
                let local_name = strip_namespace(name_str);
                text_buf.clear();

                if local_name == "response" {
                    current = Some(DavEntry::default());
                }
                // Handle <d:collection> (non-self-closing variant)
                if local_name == "collection" {
                    if let Some(ref mut entry) = current {
                        entry.is_collection = true;
                    }
                }
            }
            Ok(Event::Empty(ref e)) => {
                let name_bytes = e.name();
                let name_str = std::str::from_utf8(name_bytes.as_ref()).unwrap_or("");
                let local_name = strip_namespace(name_str);
                // <d:collection/> inside <d:resourcetype>
                if local_name == "collection" {
                    if let Some(ref mut entry) = current {
                        entry.is_collection = true;
                    }
                }
                // Self-closing element; no matching End event follows.
            }
            Ok(Event::Text(ref e)) => {
                text_buf.push_str(&e.decode().unwrap_or_default());
            }
            Ok(Event::End(ref e)) => {
                let name_bytes = e.name();
                let name_str = std::str::from_utf8(name_bytes.as_ref()).unwrap_or("");
                let local_name = strip_namespace(name_str);

                if let Some(ref mut entry) = current {
                    let val = text_buf.trim();
                    match local_name {
                        "href" => entry.href = val.to_string(),
                        "displayname" => entry.display_name = non_empty(val),
                        "getcontenttype" => entry.content_type = non_empty(val),
                        "getcontentlength" => {
                            entry.content_length = val.parse().ok();
                        }
                        "getetag" => {
                            entry.etag = non_empty(val.trim_matches('"'));
                        }
                        "getlastmodified" => {
                            entry.last_modified = non_empty(val);
                        }
                        "creationdate" => {
                            entry.creation_date = non_empty(val);
                        }
                        "fileid" => entry.file_id = non_empty(val),
                        "permissions" => {
                            entry.permissions = non_empty(val);
                        }
                        "size" => entry.oc_size = val.parse().ok(),
                        "owner-id" => {
                            entry.owner_id = non_empty(val);
                        }
                        "owner-display-name" => {
                            entry.owner_display_name = non_empty(val);
                        }
                        "favorite" => {
                            entry.favorite = val == "1";
                        }
                        _ => {}
                    }
                }

                if local_name == "response" {
                    if let Some(entry) = current.take() {
                        entries.push(entry);
                    }
                }

                text_buf.clear();
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                warn!("XML parse error: {}", e);
                break;
            }
            _ => {}
        }
    }

    Ok(entries)
}

/// Convert an empty string to `None`, or return `Some(s)` for non-empty strings.
fn non_empty(s: &str) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

/// Strip XML namespace prefix from a tag name.
/// e.g. "d:getlastmodified" → "getlastmodified", "oc:fileid" → "fileid"
fn strip_namespace(tag: &str) -> &str {
    tag.rsplit(':').next().unwrap_or(tag)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_multistatus_basic() {
        let xml = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:oc="http://owncloud.org/ns" xmlns:nc="http://nextcloud.org/ns">
  <d:response>
    <d:href>/remote.php/dav/files/alice/</d:href>
    <d:propstat>
      <d:prop>
        <d:resourcetype><d:collection/></d:resourcetype>
        <d:displayname>alice</d:displayname>
      </d:prop>
    </d:propstat>
  </d:response>
  <d:response>
    <d:href>/remote.php/dav/files/alice/document.pdf</d:href>
    <d:propstat>
      <d:prop>
        <d:resourcetype/>
        <d:displayname>document.pdf</d:displayname>
        <d:getcontenttype>application/pdf</d:getcontenttype>
        <d:getcontentlength>102400</d:getcontentlength>
        <d:getetag>"abc123"</d:getetag>
        <d:getlastmodified>Wed, 20 Jul 2022 05:12:23 GMT</d:getlastmodified>
        <d:creationdate>2022-01-01T00:00:00+00:00</d:creationdate>
        <oc:fileid>42</oc:fileid>
        <oc:permissions>RGDNVW</oc:permissions>
        <oc:size>102400</oc:size>
        <oc:owner-id>alice</oc:owner-id>
        <oc:owner-display-name>Alice Smith</oc:owner-display-name>
        <oc:favorite>1</oc:favorite>
        <nc:has-preview>true</nc:has-preview>
      </d:prop>
    </d:propstat>
  </d:response>
</d:multistatus>"#;

        let entries = parse_multistatus(xml).unwrap();
        assert_eq!(entries.len(), 2);

        // First entry: collection (directory)
        assert!(entries[0].is_collection);
        assert_eq!(entries[0].href, "/remote.php/dav/files/alice/");

        // Second entry: file
        assert!(!entries[1].is_collection);
        assert_eq!(entries[1].href, "/remote.php/dav/files/alice/document.pdf");
        assert_eq!(entries[1].content_type.as_deref(), Some("application/pdf"));
        assert_eq!(entries[1].content_length, Some(102400));
        assert_eq!(entries[1].etag.as_deref(), Some("abc123"));
        assert_eq!(entries[1].file_id.as_deref(), Some("42"));
        assert_eq!(entries[1].owner_id.as_deref(), Some("alice"));
        assert_eq!(
            entries[1].owner_display_name.as_deref(),
            Some("Alice Smith")
        );
        assert!(entries[1].favorite);
        assert_eq!(
            entries[1].last_modified.as_deref(),
            Some("Wed, 20 Jul 2022 05:12:23 GMT")
        );
        assert_eq!(entries[1].permissions.as_deref(), Some("RGDNVW"));
    }

    #[test]
    fn test_parse_multistatus_empty() {
        let xml = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:">
</d:multistatus>"#;
        let entries = parse_multistatus(xml).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn test_strip_namespace() {
        assert_eq!(strip_namespace("d:getlastmodified"), "getlastmodified");
        assert_eq!(strip_namespace("oc:fileid"), "fileid");
        assert_eq!(strip_namespace("resourcetype"), "resourcetype");
    }

    #[test]
    fn test_build_child_url_absolute() {
        let result = build_child_url(
            "https://cloud.example.com/remote.php/dav/files/alice/",
            "https://cloud.example.com/remote.php/dav/files/alice/docs/",
        );
        assert_eq!(
            result,
            "https://cloud.example.com/remote.php/dav/files/alice/docs/"
        );
    }

    #[test]
    fn test_build_child_url_relative() {
        let result = build_child_url(
            "https://cloud.example.com/remote.php/dav/files/alice/",
            "/remote.php/dav/files/alice/docs/",
        );
        assert_eq!(
            result,
            "https://cloud.example.com/remote.php/dav/files/alice/docs/"
        );
    }

    #[test]
    fn test_extract_path() {
        assert_eq!(
            extract_path("https://cloud.example.com/remote.php/dav/files/alice/"),
            "/remote.php/dav/files/alice/"
        );
        assert_eq!(
            extract_path("http://localhost:8080/foo"),
            "/foo"
        );
        assert_eq!(extract_path("/just/a/path"), "/just/a/path");
    }

    #[test]
    fn test_parse_multistatus_empty_properties_become_none() {
        // Servers can return empty property tags. These should become None,
        // not Some(""), to avoid breaking downstream fallback logic.
        let xml = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:oc="http://owncloud.org/ns" xmlns:nc="http://nextcloud.org/ns">
  <d:response>
    <d:href>/remote.php/dav/files/alice/mystery</d:href>
    <d:propstat>
      <d:prop>
        <d:resourcetype/>
        <d:displayname></d:displayname>
        <d:getcontenttype></d:getcontenttype>
        <oc:fileid></oc:fileid>
        <d:getetag></d:getetag>
        <oc:owner-id></oc:owner-id>
      </d:prop>
    </d:propstat>
  </d:response>
</d:multistatus>"#;

        let entries = parse_multistatus(xml).unwrap();
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(e.href, "/remote.php/dav/files/alice/mystery");
        assert_eq!(e.display_name, None, "empty displayname should be None");
        assert_eq!(e.content_type, None, "empty content_type should be None");
        assert_eq!(e.file_id, None, "empty file_id should be None");
        assert_eq!(e.etag, None, "empty etag should be None");
        assert_eq!(e.owner_id, None, "empty owner_id should be None");
    }
}
