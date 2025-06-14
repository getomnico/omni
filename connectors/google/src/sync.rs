use anyhow::{anyhow, Result};
use redis::{AsyncCommands, Client as RedisClient};
use serde_json::json;
use sqlx::{PgPool, Row};
use std::collections::HashSet;
use tracing::{error, info, warn};

use crate::auth::{AuthManager, OAuthCredentials};
use crate::drive::DriveClient;
use crate::models::{DocumentEvent, Source};
use shared::models::SourceType;

pub struct SyncManager {
    pool: PgPool,
    redis_client: RedisClient,
    auth_manager: AuthManager,
    drive_client: DriveClient,
}

impl SyncManager {
    pub async fn new(pool: PgPool, redis_client: RedisClient) -> Result<Self> {
        let client_id = std::env::var("GOOGLE_CLIENT_ID")?;
        let client_secret = std::env::var("GOOGLE_CLIENT_SECRET")?;

        Ok(Self {
            pool,
            redis_client,
            auth_manager: AuthManager::new(client_id, client_secret),
            drive_client: DriveClient::new(),
        })
    }

    pub async fn sync_all_sources(&self) -> Result<()> {
        let sources = self.get_active_sources().await?;

        info!("Found {} active Google Drive sources", sources.len());

        for source in sources {
            if let Err(e) = self.sync_source(&source).await {
                error!("Failed to sync source {}: {}", source.id, e);
                self.update_source_status(&source.id, "failed").await?;
            }
        }

        Ok(())
    }

    async fn get_active_sources(&self) -> Result<Vec<Source>> {
        let sources = sqlx::query_as::<_, Source>(
            "SELECT s.* FROM sources s
             INNER JOIN oauth_credentials oc ON s.id = oc.source_id
             WHERE s.source_type = $1 
             AND s.is_active = true 
             AND oc.provider = 'google'",
        )
        .bind(SourceType::Google)
        .fetch_all(&self.pool)
        .await?;

        Ok(sources)
    }

    async fn sync_source(&self, source: &Source) -> Result<()> {
        info!("Syncing source: {} ({})", source.name, source.id);

        let oauth_creds = self.get_oauth_credentials(&source.id).await?;
        let mut creds: OAuthCredentials = oauth_creds;

        let original_creds = creds.clone();
        self.auth_manager.ensure_valid_token(&mut creds).await?;

        if creds.access_token != original_creds.access_token || creds.refresh_token != original_creds.refresh_token {
            self.update_oauth_credentials(&source.id, &creds).await?;
        }

        let synced_files = self.get_synced_files(&source.id).await?;
        let mut current_files = HashSet::new();
        let mut page_token: Option<String> = None;

        loop {
            let response = self
                .drive_client
                .list_files(&creds.access_token, page_token.as_deref())
                .await?;

            for file in response.files {
                current_files.insert(file.id.clone());

                if self.should_index_file(&file) {
                    match self
                        .drive_client
                        .get_file_content(&creds.access_token, &file)
                        .await
                    {
                        Ok(content) => {
                            if !content.is_empty() {
                                let event = DocumentEvent::from_drive_file(
                                    source.id.clone(),
                                    &file,
                                    content,
                                );
                                self.publish_document_event(&event, "created").await?;
                            }
                        }
                        Err(e) => {
                            warn!("Failed to get content for file {}: {}", file.name, e);
                        }
                    }
                }
            }

            page_token = response.next_page_token;
            if page_token.is_none() {
                break;
            }
        }

        for deleted_file_id in synced_files.difference(&current_files) {
            self.publish_deletion_event(&source.id, deleted_file_id)
                .await?;
        }

        self.update_source_status(&source.id, "completed").await?;

        info!("Completed sync for source: {}", source.id);
        Ok(())
    }

    fn should_index_file(&self, file: &crate::models::GoogleDriveFile) -> bool {
        matches!(
            file.mime_type.as_str(),
            "application/vnd.google-apps.document"
                | "application/vnd.google-apps.spreadsheet"
                | "text/plain"
                | "text/html"
                | "text/csv"
        )
    }

    async fn get_synced_files(&self, source_id: &str) -> Result<HashSet<String>> {
        let rows = sqlx::query("SELECT external_id FROM documents WHERE source_id = $1")
            .bind(source_id)
            .fetch_all(&self.pool)
            .await?;

        Ok(rows
            .into_iter()
            .filter_map(|row| row.try_get::<String, _>("external_id").ok())
            .collect())
    }

    async fn publish_document_event(&self, event: &DocumentEvent, event_type: &str) -> Result<()> {
        let mut conn = self.redis_client.get_multiplexed_async_connection().await?;

        let event_data = json!({
            "type": format!("document_{}", event_type),
            "source_id": event.source_id,
            "document_id": event.document_id,
            "title": event.title,
            "content": event.content,
            "url": event.url,
            "metadata": event.metadata,
            "permissions": event.permissions,
        });

        conn.publish::<_, _, ()>("indexer:events", serde_json::to_string(&event_data)?)
            .await?;
        Ok(())
    }

    async fn publish_deletion_event(&self, source_id: &str, document_id: &str) -> Result<()> {
        let mut conn = self.redis_client.get_multiplexed_async_connection().await?;

        let event_data = json!({
            "type": "document_deleted",
            "source_id": source_id,
            "document_id": document_id,
        });

        conn.publish::<_, _, ()>("indexer:events", serde_json::to_string(&event_data)?)
            .await?;
        Ok(())
    }

    async fn get_oauth_credentials(&self, source_id: &str) -> Result<OAuthCredentials> {
        let row = sqlx::query(
            "SELECT access_token, refresh_token, token_type, expires_at 
             FROM oauth_credentials 
             WHERE source_id = $1 AND provider = 'google'"
        )
        .bind(source_id)
        .fetch_one(&self.pool)
        .await?;

        let expires_at: chrono::DateTime<chrono::Utc> = row.get("expires_at");
        let now = chrono::Utc::now();
        let expires_in = (expires_at - now).num_seconds().max(0);

        Ok(OAuthCredentials {
            access_token: row.get("access_token"),
            refresh_token: row.get("refresh_token"),
            token_type: row.get("token_type"),
            expires_in,
            obtained_at: now.timestamp_millis(),
        })
    }

    async fn update_oauth_credentials(
        &self,
        source_id: &str,
        creds: &OAuthCredentials,
    ) -> Result<()> {
        let expires_at = chrono::DateTime::from_timestamp(creds.obtained_at / 1000, 0)
            .unwrap_or(chrono::Utc::now()) + chrono::Duration::seconds(creds.expires_in);

        sqlx::query(
            "UPDATE oauth_credentials 
             SET access_token = $1, refresh_token = $2, token_type = $3, expires_at = $4, updated_at = CURRENT_TIMESTAMP 
             WHERE source_id = $5 AND provider = 'google'"
        )
        .bind(&creds.access_token)
        .bind(&creds.refresh_token)
        .bind(&creds.token_type)
        .bind(&expires_at)
        .bind(source_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn update_source_status(&self, source_id: &str, status: &str) -> Result<()> {
        sqlx::query(
            "UPDATE sources SET sync_status = $1, last_sync_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP WHERE id = $2"
        )
        .bind(status)
        .bind(source_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}
