use std::future::Future;
use std::io;
use std::time::Duration;

use anyhow::{Context, Result};
use async_imap::Client;
use async_imap::types::Fetch;
use futures::StreamExt;
use tokio_native_tls::TlsStream;
use tracing::{debug, info, warn};

use crate::config::ImapAccountConfig;

/// Bounds establishing or tearing down a session (TCP, TLS, LOGIN, LOGOUT).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(60);

/// Bounds a single IMAP command, including draining its response. This is a
/// liveness bound, not a budget for the work itself: a folder may take as long
/// as it needs as long as each command keeps returning.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(300);

/// Bound an IMAP operation in time. Neither `async-imap` nor the TLS stack
/// applies a deadline, so a server that stops responding mid-session parks the
/// sync task forever while it holds the connector's per-source slot. The error
/// is `TimedOut` so `is_connection_error` rebuilds the session, which a future
/// dropped mid-command leaves in an indeterminate protocol state.
async fn with_timeout<F, T, E>(limit: Duration, operation: &str, future: F) -> Result<T>
where
    F: Future<Output = Result<T, E>>,
    E: Into<anyhow::Error>,
{
    match tokio::time::timeout(limit, future).await {
        Ok(result) => result.map_err(Into::into),
        Err(_) => Err(anyhow::Error::new(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("IMAP {} timed out after {}s", operation, limit.as_secs()),
        ))),
    }
}

/// Read-only IMAP session wrapper.
///
/// The session always opens mailboxes with EXAMINE (read-only) so that
/// the connector never changes any message flags or state.
pub struct ImapSession {
    session: async_imap::Session<TlsStream<tokio::net::TcpStream>>,
}

/// Raw message bytes with the associated IMAP UID.
pub struct RawMessage {
    pub uid: u32,
    pub data: Vec<u8>,
    pub flags: Vec<String>,
}

impl ImapSession {
    /// Connect and authenticate, returning a read-only session.
    pub async fn connect(
        config: &ImapAccountConfig,
        username: &str,
        password: &str,
    ) -> Result<Self> {
        with_timeout(
            CONNECT_TIMEOUT,
            "connect",
            Self::connect_inner(config, username, password),
        )
        .await
    }

    async fn connect_inner(
        config: &ImapAccountConfig,
        username: &str,
        password: &str,
    ) -> Result<Self> {
        let addr = format!("{}:{}", config.host, config.port);
        debug!("Connecting to IMAP server: {}", addr);

        let tcp = tokio::net::TcpStream::connect(&addr)
            .await
            .with_context(|| format!("Failed to connect to {}", addr))?;

        let enc = config.encryption.to_ascii_lowercase();
        let tls_stream = match enc.as_str() {
            "tls" | "ssl" => {
                let connector =
                    native_tls::TlsConnector::new().context("Failed to create TLS connector")?;
                let connector = tokio_native_tls::TlsConnector::from(connector);
                connector
                    .connect(&config.host, tcp)
                    .await
                    .context("TLS handshake failed")?
            }
            other => {
                return Err(anyhow::anyhow!(
                    "Unsupported encryption mode '{}'. Use 'tls'.",
                    other
                ));
            }
        };

        let client = Client::new(tls_stream);
        let session = client
            .login(username, password)
            .await
            .map_err(|(e, _)| anyhow::anyhow!("IMAP login failed: {}", e))
            .context("Authentication failed")?;

        info!("IMAP session established for {}", username);
        Ok(Self { session })
    }

    /// List all accessible mailbox folders.
    pub async fn list_folders(&mut self) -> Result<Vec<String>> {
        let folders = with_timeout(COMMAND_TIMEOUT, "LIST", async {
            let names = self
                .session
                .list(Some(""), Some("*"))
                .await
                .context("Failed to list IMAP folders")?;

            let mut folders = Vec::new();
            let mut stream = names;
            while let Some(result) = stream.next().await {
                match result {
                    Ok(name) => folders.push(name.name().to_string()),
                    Err(e) => warn!("Error listing folder: {}", e),
                }
            }
            Ok::<_, anyhow::Error>(folders)
        })
        .await?;

        debug!("Found {} IMAP folders", folders.len());
        Ok(folders)
    }

    /// Open a mailbox in read-only mode via EXAMINE.
    /// Returns (uid_validity, exists_count).
    pub async fn examine_folder(&mut self, folder: &str) -> Result<(u32, u32)> {
        let mailbox = with_timeout(COMMAND_TIMEOUT, "EXAMINE", self.session.examine(folder))
            .await
            .with_context(|| format!("Failed to EXAMINE folder '{}'", folder))?;

        let uid_validity = mailbox.uid_validity.unwrap_or(0);
        let exists = mailbox.exists;
        debug!(
            "Opened '{}' (uidvalidity={}, exists={})",
            folder, uid_validity, exists
        );
        Ok((uid_validity, exists))
    }

    /// Fetch all current UIDs in the open folder.
    ///
    /// Used for both new-message detection (`server_uids − indexed_uids`) and
    /// deletion detection (`indexed_uids − server_uids`) in a single IMAP
    /// round trip.
    pub async fn fetch_all_uids(&mut self) -> Result<Vec<u32>> {
        let uids = with_timeout(
            COMMAND_TIMEOUT,
            "UID SEARCH",
            self.session.uid_search("ALL"),
        )
        .await
        .context("Failed to UID SEARCH ALL for deletion check")?;

        let mut sorted: Vec<u32> = uids.into_iter().collect();
        sorted.sort_unstable();
        Ok(sorted)
    }

    /// Fetch raw RFC 2822 message bytes for a batch of UIDs.
    pub async fn fetch_messages(&mut self, uids: &[u32]) -> Result<Vec<RawMessage>> {
        if uids.is_empty() {
            return Ok(vec![]);
        }

        let uid_set = uids
            .iter()
            .map(|u| u.to_string())
            .collect::<Vec<_>>()
            .join(",");

        with_timeout(COMMAND_TIMEOUT, "UID FETCH", async {
            let fetches = self
                .session
                .uid_fetch(&uid_set, "(FLAGS BODY.PEEK[])")
                .await
                .with_context(|| format!("Failed to UID FETCH for UIDs: {}", uid_set))?;

            let mut messages = Vec::new();
            let mut stream = fetches;
            while let Some(result) = stream.next().await {
                match result {
                    Ok(fetch) => {
                        if let Some(raw) = extract_raw_body(&fetch) {
                            let uid = fetch.uid.unwrap_or(0);
                            if uid > 0 {
                                messages.push(RawMessage {
                                    uid,
                                    data: raw,
                                    flags: extract_flags(&fetch),
                                });
                            }
                        }
                    }
                    Err(e) => warn!("Error fetching message: {}", e),
                }
            }

            Ok::<_, anyhow::Error>(messages)
        })
        .await
    }

    /// Fetch only the FLAGS for a batch of UIDs — no message bodies downloaded.
    ///
    /// Used by the flag-change detection pass to detect read/flagged/etc.
    /// changes on already-indexed messages in a single lightweight round trip.
    /// Returns `(uid, flags)` pairs for every UID the server responded to.
    pub async fn fetch_flags_only(&mut self, uids: &[u32]) -> Result<Vec<(u32, Vec<String>)>> {
        if uids.is_empty() {
            return Ok(vec![]);
        }

        let uid_set = uids
            .iter()
            .map(|u| u.to_string())
            .collect::<Vec<_>>()
            .join(",");

        with_timeout(COMMAND_TIMEOUT, "UID FETCH FLAGS", async {
            let fetches = self
                .session
                .uid_fetch(&uid_set, "FLAGS")
                .await
                .with_context(|| format!("Failed to UID FETCH FLAGS for UIDs: {}", uid_set))?;

            let mut result = Vec::new();
            let mut stream = fetches;
            while let Some(fetch_result) = stream.next().await {
                match fetch_result {
                    Ok(fetch) => {
                        if let Some(uid) = fetch.uid {
                            if uid > 0 {
                                result.push((uid, extract_flags(&fetch)));
                            }
                        }
                    }
                    Err(e) => warn!("Error fetching flags for UID batch: {}", e),
                }
            }

            Ok::<_, anyhow::Error>(result)
        })
        .await
    }

    /// Gracefully log out.  LOGOUT failures are non-fatal: the underlying
    /// TCP connection is closed by Rust's drop semantics regardless, so any
    /// error here is only relevant for observability.
    pub async fn logout(mut self) {
        let logout = with_timeout(CONNECT_TIMEOUT, "LOGOUT", self.session.logout()).await;
        if let Err(e) = logout {
            warn!("IMAP LOGOUT failed (connection will be dropped): {}", e);
        }
    }
}

fn extract_raw_body(fetch: &Fetch) -> Option<Vec<u8>> {
    fetch.body().map(|b| b.to_vec())
}

fn extract_flags(fetch: &Fetch) -> Vec<String> {
    use async_imap::types::Flag;
    fetch
        .flags()
        .map(|flag| match flag {
            Flag::Seen => "\\Seen".to_string(),
            Flag::Answered => "\\Answered".to_string(),
            Flag::Flagged => "\\Flagged".to_string(),
            Flag::Deleted => "\\Deleted".to_string(),
            Flag::Draft => "\\Draft".to_string(),
            Flag::Recent => "\\Recent".to_string(),
            Flag::MayCreate => "\\*".to_string(),
            Flag::Custom(s) => s.to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    /// Unit tests that don't require a live IMAP server are in models.rs.
    /// Integration tests requiring a real server would be in tests/.
    #[test]
    fn test_uid_set_formatting() {
        let uids: Vec<u32> = vec![1, 2, 5, 10];
        let uid_set = uids
            .iter()
            .map(|u| u.to_string())
            .collect::<Vec<_>>()
            .join(",");
        assert_eq!(uid_set, "1,2,5,10");
    }
}
