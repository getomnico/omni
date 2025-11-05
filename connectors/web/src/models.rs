use anyhow::Result;
use chrono::{DateTime, Utc};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use shared::models::{ConnectorEvent, DocumentMetadata, DocumentPermissions};
use spider::page::Page;
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebPage {
    pub url: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub content: String,
    pub content_hash: String,
    pub last_modified: Option<String>,
    pub etag: Option<String>,
    pub word_count: usize,
}

impl WebPage {
    pub fn from_spider_page(page: &Page) -> Result<Self> {
        let url = page.get_url().to_string();
        let html = page.get_html();

        let document = Html::parse_document(&html);
        let content = Self::extract_main_content(&document)?;
        let content_hash = Self::compute_content_hash(&content);
        let word_count = content.split_whitespace().count();

        let title = Self::extract_title(&document).or_else(|| Self::extract_first_h1(&document));

        let description = Self::extract_description(&document);

        let last_modified = None;
        let etag = None;

        Ok(Self {
            url,
            title,
            description,
            content,
            content_hash,
            last_modified,
            etag,
            word_count,
        })
    }

    fn extract_title(document: &Html) -> Option<String> {
        let title_selector = Selector::parse("title").ok()?;
        document
            .select(&title_selector)
            .next()
            .map(|el| el.text().collect::<String>().trim().to_string())
    }

    fn extract_description(document: &Html) -> Option<String> {
        let meta_selector = Selector::parse("meta[name='description']").ok()?;
        document
            .select(&meta_selector)
            .next()
            .and_then(|el| el.value().attr("content"))
            .map(|s| s.trim().to_string())
    }

    fn extract_main_content(document: &Html) -> Result<String> {
        let selectors_to_try = vec![
            "main",
            "article",
            "[role='main']",
            ".content",
            "#content",
            "body",
        ];

        for selector_str in selectors_to_try {
            if let Ok(selector) = Selector::parse(selector_str) {
                if let Some(element) = document.select(&selector).next() {
                    let text = element.text().collect::<Vec<_>>().join(" ");
                    let cleaned = text
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                        .trim()
                        .to_string();

                    if !cleaned.is_empty() {
                        return Ok(cleaned);
                    }
                }
            }
        }

        Err(anyhow::anyhow!("Could not extract content from page"))
    }

    fn extract_first_h1(document: &Html) -> Option<String> {
        let h1_selector = Selector::parse("h1").ok()?;
        document
            .select(&h1_selector)
            .next()
            .map(|el| el.text().collect::<String>().trim().to_string())
    }

    fn compute_content_hash(content: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    fn url_to_document_id(url: &str) -> String {
        use base64::Engine;
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(url)
    }

    fn extract_path_from_url(url: &str) -> String {
        url::Url::parse(url)
            .ok()
            .and_then(|u| Some(u.path().to_string()))
            .unwrap_or_else(|| "/".to_string())
    }

    fn extract_domain_from_url(url: &str) -> Option<String> {
        url::Url::parse(url)
            .ok()
            .and_then(|u| u.host_str().map(|s| s.to_string()))
    }

    pub fn to_connector_event(
        &self,
        sync_run_id: String,
        source_id: String,
        content_id: String,
    ) -> ConnectorEvent {
        let document_id = Self::url_to_document_id(&self.url);

        let mut extra = HashMap::new();
        if let Some(domain) = Self::extract_domain_from_url(&self.url) {
            extra.insert("domain".to_string(), serde_json::json!(domain));
        }
        extra.insert("word_count".to_string(), serde_json::json!(self.word_count));
        extra.insert(
            "content_hash".to_string(),
            serde_json::json!(self.content_hash),
        );
        if let Some(etag) = &self.etag {
            extra.insert("etag".to_string(), serde_json::json!(etag));
        }

        let updated_at = self
            .last_modified
            .as_ref()
            .and_then(|s| chrono::DateTime::parse_from_rfc2822(s).ok())
            .map(|dt| dt.with_timezone(&Utc));

        let metadata = DocumentMetadata {
            title: self.title.clone(),
            author: None,
            created_at: None,
            updated_at: updated_at
                .map(|dt| {
                    sqlx::types::time::OffsetDateTime::from_unix_timestamp(dt.timestamp()).ok()
                })
                .flatten(),
            mime_type: Some("text/html".to_string()),
            size: Some(self.content.len().to_string()),
            url: Some(self.url.clone()),
            path: Some(Self::extract_path_from_url(&self.url)),
            extra: Some(extra),
        };

        let permissions = DocumentPermissions {
            public: true,
            users: vec![],
            groups: vec![],
        };

        ConnectorEvent::DocumentCreated {
            sync_run_id,
            source_id,
            document_id,
            content_id,
            metadata,
            permissions,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageSyncState {
    pub last_modified: Option<String>,
    pub etag: Option<String>,
    pub content_hash: String,
    pub last_synced: DateTime<Utc>,
}

impl PageSyncState {
    pub fn new(page: &WebPage) -> Self {
        Self {
            last_modified: page.last_modified.clone(),
            etag: page.etag.clone(),
            content_hash: page.content_hash.clone(),
            last_synced: Utc::now(),
        }
    }

    pub fn has_changed(&self, page: &WebPage) -> bool {
        if let Some(new_etag) = &page.etag {
            if let Some(old_etag) = &self.etag {
                return new_etag != old_etag;
            }
        }

        if let Some(new_modified) = &page.last_modified {
            if let Some(old_modified) = &self.last_modified {
                return new_modified != old_modified;
            }
        }

        self.content_hash != page.content_hash
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_main_content() {
        let html = r#"
            <!DOCTYPE html>
            <html>
            <head><title>Test Page</title></head>
            <body>
                <nav>Navigation</nav>
                <main>
                    <h1>Main Title</h1>
                    <p>This is the main content.</p>
                </main>
                <footer>Footer</footer>
            </body>
            </html>
        "#;

        let document = Html::parse_document(html);
        let content = WebPage::extract_main_content(&document).unwrap();

        assert!(content.contains("Main Title"));
        assert!(content.contains("main content"));
        assert!(!content.contains("Navigation"));
        assert!(!content.contains("Footer"));
    }

    #[test]
    fn test_compute_content_hash() {
        let content1 = "Hello World";
        let content2 = "Hello World";
        let content3 = "Different Content";

        let hash1 = WebPage::compute_content_hash(content1);
        let hash2 = WebPage::compute_content_hash(content2);
        let hash3 = WebPage::compute_content_hash(content3);

        assert_eq!(hash1, hash2);
        assert_ne!(hash1, hash3);
    }

    #[test]
    fn test_extract_path_from_url() {
        assert_eq!(
            WebPage::extract_path_from_url("https://example.com/docs/intro"),
            "/docs/intro"
        );
        assert_eq!(WebPage::extract_path_from_url("https://example.com/"), "/");
    }

    #[test]
    fn test_extract_domain_from_url() {
        assert_eq!(
            WebPage::extract_domain_from_url("https://docs.example.com/path"),
            Some("docs.example.com".to_string())
        );
    }

    #[test]
    fn test_page_sync_state_has_changed() {
        let page1 = WebPage {
            url: "https://example.com".to_string(),
            title: Some("Test".to_string()),
            description: None,
            content: "Content".to_string(),
            content_hash: "hash1".to_string(),
            last_modified: Some("Mon, 01 Jan 2024 00:00:00 GMT".to_string()),
            etag: Some("etag1".to_string()),
            word_count: 1,
        };

        let state = PageSyncState::new(&page1);

        let page2 = WebPage {
            etag: Some("etag2".to_string()),
            ..page1.clone()
        };
        assert!(state.has_changed(&page2));

        let page3 = WebPage {
            last_modified: Some("Tue, 02 Jan 2024 00:00:00 GMT".to_string()),
            ..page1.clone()
        };
        assert!(state.has_changed(&page3));

        let page4 = WebPage {
            content_hash: "hash2".to_string(),
            ..page1.clone()
        };
        assert!(state.has_changed(&page4));

        assert!(!state.has_changed(&page1));
    }
}
