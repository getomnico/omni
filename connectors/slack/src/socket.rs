use anyhow::{anyhow, Context, Result};
use dashmap::DashMap;
use futures_util::{SinkExt, StreamExt};
use omni_slack_connector::sync::SyncManager;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use shared::SdkClient;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

const MAX_RECONNECT_DELAY_SECS: u64 = 300;
const DEBOUNCE_SECS: u64 = 600;

// ============================================================================
// Socket Mode protocol types
// ============================================================================

#[derive(Debug, Deserialize)]
struct ConnectionsOpenResponse {
    ok: bool,
    url: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SocketEnvelope {
    envelope_id: Option<String>,
    #[serde(rename = "type")]
    envelope_type: Option<String>,
    payload: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct SocketAck {
    envelope_id: String,
}

// ============================================================================
// SocketModeManager
// ============================================================================

struct ActiveConnection {
    cancel_token: CancellationToken,
}

pub struct SocketModeManager {
    connections: RwLock<std::collections::HashMap<String, ActiveConnection>>,
    /// Per debounce-key: the Instant at which we should fire the sync.
    /// Each new event pushes this forward. A background task sleeps until
    /// the target is stable, then triggers the sync.
    debounce_targets: Arc<DashMap<String, Instant>>,
}

impl SocketModeManager {
    pub fn new() -> Self {
        Self {
            connections: RwLock::new(std::collections::HashMap::new()),
            debounce_targets: Arc::new(DashMap::new()),
        }
    }

    pub async fn is_connected(&self, source_id: &str) -> bool {
        let conns = self.connections.read().await;
        conns
            .get(source_id)
            .map(|c| !c.cancel_token.is_cancelled())
            .unwrap_or(false)
    }

    pub async fn start_connection(
        &self,
        source_id: String,
        app_token: String,
        sdk_client: SdkClient,
        sync_manager: Option<Arc<SyncManager>>,
    ) {
        // Stop existing connection if any
        self.stop_connection(&source_id).await;

        let cancel_token = CancellationToken::new();
        let child_token = cancel_token.child_token();
        let debounce_targets = self.debounce_targets.clone();
        let sid = source_id.clone();

        tokio::spawn(async move {
            socket_mode_loop(
                sid,
                app_token,
                sdk_client,
                child_token,
                debounce_targets,
                sync_manager,
            )
            .await;
        });

        let mut conns = self.connections.write().await;
        conns.insert(source_id, ActiveConnection { cancel_token });
    }

    pub async fn stop_connection(&self, source_id: &str) {
        let mut conns = self.connections.write().await;
        if let Some(conn) = conns.remove(source_id) {
            conn.cancel_token.cancel();
            info!(source_id, "Stopped Socket Mode connection");
        }
    }

    pub async fn stop_all(&self) {
        let mut conns = self.connections.write().await;
        for (source_id, conn) in conns.drain() {
            conn.cancel_token.cancel();
            info!(source_id, "Stopped Socket Mode connection");
        }
    }
}

// ============================================================================
// Socket Mode connection loop
// ============================================================================

async fn socket_mode_loop(
    source_id: String,
    app_token: String,
    sdk_client: SdkClient,
    cancel_token: CancellationToken,
    debounce_targets: Arc<DashMap<String, Instant>>,
    sync_manager: Option<Arc<SyncManager>>,
) {
    let http_client = Client::new();
    let mut backoff_secs = 1u64;

    loop {
        if cancel_token.is_cancelled() {
            info!(source_id, "Socket Mode cancelled, exiting loop");
            return;
        }

        match connect_and_listen(
            &source_id,
            &app_token,
            &sdk_client,
            &http_client,
            &cancel_token,
            &debounce_targets,
            &sync_manager,
        )
        .await
        {
            Ok(()) => {
                info!(source_id, "Socket Mode connection closed cleanly");
                backoff_secs = 1;
            }
            Err(e) => {
                warn!(
                    source_id,
                    error = %e,
                    backoff_secs,
                    "Socket Mode connection error, reconnecting"
                );
            }
        }

        if cancel_token.is_cancelled() {
            return;
        }

        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(backoff_secs)) => {}
            _ = cancel_token.cancelled() => return,
        }

        backoff_secs = (backoff_secs * 2).min(MAX_RECONNECT_DELAY_SECS);
    }
}

async fn connect_and_listen(
    source_id: &str,
    app_token: &str,
    sdk_client: &SdkClient,
    http_client: &Client,
    cancel_token: &CancellationToken,
    debounce_targets: &Arc<DashMap<String, Instant>>,
    sync_manager: &Option<Arc<SyncManager>>,
) -> Result<()> {
    // 1. Get WebSocket URL via apps.connections.open
    let ws_url = get_ws_url(http_client, app_token).await?;
    info!(source_id, "Connecting to Socket Mode WebSocket");

    // 2. Connect WebSocket
    let (ws_stream, _) = tokio_tungstenite::connect_async(&ws_url)
        .await
        .context("Failed to connect to Socket Mode WebSocket")?;

    let (mut ws_sink, mut ws_stream) = ws_stream.split();
    info!(source_id, "Socket Mode WebSocket connected");

    // 3. Read messages until disconnect or cancellation
    loop {
        tokio::select! {
            msg = ws_stream.next() => {
                let msg = match msg {
                    Some(Ok(msg)) => msg,
                    Some(Err(e)) => return Err(e.into()),
                    None => return Ok(()),
                };

                match msg {
                    Message::Text(text) => {
                        if let Err(e) = handle_socket_message(
                            source_id,
                            &text,
                            &mut ws_sink,
                            sdk_client,
                            debounce_targets,
                            sync_manager,
                        ).await {
                            warn!(source_id, error = %e, "Error handling socket message");
                        }
                    }
                    Message::Ping(data) => {
                        if let Err(e) = ws_sink.send(Message::Pong(data)).await {
                            warn!(source_id, error = %e, "Failed to send pong");
                        }
                    }
                    Message::Close(_) => {
                        info!(source_id, "WebSocket close frame received");
                        return Ok(());
                    }
                    _ => {}
                }
            }
            _ = cancel_token.cancelled() => {
                let _ = ws_sink.close().await;
                return Ok(());
            }
        }
    }
}

async fn get_ws_url(http_client: &Client, app_token: &str) -> Result<String> {
    let response = http_client
        .post("https://slack.com/api/apps.connections.open")
        .header("Authorization", format!("Bearer {}", app_token))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .send()
        .await
        .context("Failed to call apps.connections.open")?;

    let body: ConnectionsOpenResponse = response
        .json()
        .await
        .context("Failed to parse apps.connections.open response")?;

    if !body.ok {
        return Err(anyhow!(
            "apps.connections.open failed: {}",
            body.error.unwrap_or_else(|| "unknown error".into())
        ));
    }

    body.url
        .ok_or_else(|| anyhow!("apps.connections.open returned ok but no URL"))
}

// ============================================================================
// Message handling
// ============================================================================

async fn handle_socket_message<S>(
    source_id: &str,
    text: &str,
    ws_sink: &mut S,
    sdk_client: &SdkClient,
    debounce_targets: &Arc<DashMap<String, Instant>>,
    sync_manager: &Option<Arc<SyncManager>>,
) -> Result<()>
where
    S: SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    let envelope: SocketEnvelope =
        serde_json::from_str(text).context("Failed to parse socket envelope")?;

    let envelope_type = envelope.envelope_type.as_deref().unwrap_or("");

    // Acknowledge the envelope (required by Socket Mode protocol)
    if let Some(envelope_id) = &envelope.envelope_id {
        let ack = serde_json::to_string(&SocketAck {
            envelope_id: envelope_id.clone(),
        })?;
        ws_sink
            .send(Message::Text(ack.into()))
            .await
            .context("Failed to send ack")?;
        debug!(
            source_id,
            envelope_id, envelope_type, "Acknowledged envelope"
        );
    }

    match envelope_type {
        "hello" => {
            info!(source_id, "Socket Mode hello received");
        }
        "disconnect" => {
            info!(
                source_id,
                "Socket Mode disconnect requested by Slack, will reconnect"
            );
            return Err(anyhow!("Slack requested disconnect"));
        }
        "events_api" => {
            if let Some(payload) = &envelope.payload {
                handle_event(
                    source_id,
                    payload,
                    sdk_client,
                    debounce_targets,
                    sync_manager,
                )
                .await;
            }
        }
        _ => {
            debug!(source_id, envelope_type, "Ignoring envelope type");
        }
    }

    Ok(())
}

async fn handle_event(
    source_id: &str,
    payload: &serde_json::Value,
    _sdk_client: &SdkClient,
    debounce_targets: &Arc<DashMap<String, Instant>>,
    sync_manager: &Option<Arc<SyncManager>>,
) {
    let event_type = payload
        .pointer("/event/type")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    debug!(source_id, event_type, "Received Slack event");

    if event_type != "message" {
        info!(source_id, event_type, "Ignoring non-message event type");
        return;
    }

    let channel_id = match payload.pointer("/event/channel").and_then(|v| v.as_str()) {
        Some(id) => id,
        None => {
            warn!(source_id, "Message event missing channel field");
            return;
        }
    };

    let sync_manager = match sync_manager {
        Some(sm) => sm,
        None => {
            debug!(
                source_id,
                "No sync manager available, skipping realtime sync"
            );
            return;
        }
    };

    // Trailing-edge debounce: each event resets the timer. The sync fires
    // only after DEBOUNCE_SECS of quiet. Different channels debounce
    // independently.
    let debounce_key = format!("{}:{}", source_id, channel_id);
    let fire_at = Instant::now() + Duration::from_secs(DEBOUNCE_SECS);
    let is_new = !debounce_targets.contains_key(&debounce_key);
    debounce_targets.insert(debounce_key.clone(), fire_at);

    if is_new {
        // First event for this key — spawn the debounce task
        let targets = debounce_targets.clone();
        let sm = sync_manager.clone();
        let sid = source_id.to_string();
        let cid = channel_id.to_string();
        let key = debounce_key;

        tokio::spawn(async move {
            loop {
                let target = match targets.get(&key) {
                    Some(t) => *t,
                    None => return,
                };

                let now = Instant::now();
                if now >= target {
                    break;
                }
                tokio::time::sleep(target - now).await;
            }

            targets.remove(&key);

            info!(
                source_id = sid.as_str(),
                channel_id = cid.as_str(),
                "Debounce expired, starting realtime sync"
            );

            if let Err(e) = sm.sync_realtime_event(&sid, &cid).await {
                error!(
                    source_id = sid.as_str(),
                    channel_id = cid.as_str(),
                    error = %e,
                    "Realtime sync failed"
                );
            }
        });
    } else {
        debug!(source_id, channel_id, "Debounce timer reset for channel");
    }
}
