use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Per-account IMAP configuration stored in `Source.config`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImapAccountConfig {
    /// Human-readable display name for this account.
    pub display_name: Option<String>,
    /// IMAP server hostname.
    pub host: String,
    /// IMAP server port (default 993 for TLS).
    pub port: u16,
    /// Encryption mode.  Currently only `"tls"` (implicit TLS, a.k.a. SSL)
    /// is supported.  Connections without TLS are not allowed.
    #[serde(default = "default_encryption")]
    pub encryption: String,
    /// Folders that should be indexed (empty = index all).
    #[serde(default)]
    pub folder_allowlist: Vec<String>,
    /// Folders that should never be indexed.
    /// Defaults to the most common names for Trash and Spam folders across
    /// major IMAP providers (Gmail, Outlook, Apple Mail, Dovecot, etc.).
    #[serde(default = "default_folder_denylist")]
    pub folder_denylist: Vec<String>,
    /// Maximum message size in bytes to process (0 = unlimited).
    #[serde(default)]
    pub max_message_size: u64,
    /// Optional URL template for generating webmail links.
    /// Supported placeholders: `{folder}`, `{uid}`, `{message_id}`.
    #[serde(default)]
    pub webmail_url_template: Option<String>,
    /// Whether periodic sync is enabled.
    #[serde(default = "default_true")]
    pub sync_enabled: bool,
}

fn default_encryption() -> String {
    "tls".to_string()
}

fn default_folder_denylist() -> Vec<String> {
    // Common Trash/Spam folder names across major providers.
    // Gmail uses "[Gmail]/Trash" and "[Gmail]/Spam"; Outlook uses
    // "Deleted Items" and "Junk Email"; Apple/Dovecot use "Trash" and "Junk".
    // Users can override this entirely by setting folder_denylist in config.
    vec![
        "Trash".to_string(),
        "Spam".to_string(),
        "Junk".to_string(),
        "Junk Email".to_string(),
        "Deleted Items".to_string(),
        "Deleted Messages".to_string(),
        "[Gmail]/Trash".to_string(),
        "[Gmail]/Spam".to_string(),
    ]
}

fn default_true() -> bool {
    true
}

impl ImapAccountConfig {
    pub fn from_source_config(config: &serde_json::Value) -> Result<Self> {
        serde_json::from_value(config.clone()).context("Failed to parse IMAP account config")
    }

    /// Returns true if the given folder should be indexed.
    pub fn should_index_folder(&self, folder: &str) -> bool {
        // Denylist takes priority.
        if self
            .folder_denylist
            .iter()
            .any(|d| d.eq_ignore_ascii_case(folder))
        {
            return false;
        }
        // If allowlist is non-empty, only listed folders are indexed.
        if !self.folder_allowlist.is_empty() {
            return self
                .folder_allowlist
                .iter()
                .any(|a| a.eq_ignore_ascii_case(folder));
        }
        true
    }
}
