use anyhow::{anyhow, Result};
use redis::{AsyncCommands, Client as RedisClient};
use serde_json::json;
use sqlx::{PgPool, Row};
use std::collections::HashSet;
use tracing::{error, info, warn};

use crate::auth::{AuthManager, OAuthCredentials};
use crate::drive::DriveClient;
use crate::models::{DocumentEvent, Source};

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
            "SELECT * FROM sources 
             WHERE source_type = 'google_drive' 
             AND is_active = true 
             AND oauth_credentials IS NOT NULL"
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(sources)
    }

    async fn sync_source(&self, source: &Source) -> Result<()> {
        info!("Syncing source: {} ({})", source.name, source.id);

        let oauth_creds = source.oauth_credentials
            .as_ref()
            .ok_or_else(|| anyhow!("No OAuth credentials found"))?;

        let mut creds: OAuthCredentials = serde_json::from_value(oauth_creds.clone())?;

        self.auth_manager.ensure_valid_token(&mut creds).await?;

        if serde_json::to_value(&creds)? != *oauth_creds {
            self.update_oauth_credentials(&source.id, &creds).await?;
        }

        let synced_files = self.get_synced_files(&source.id).await?;
        let mut current_files = HashSet::new();
        let mut page_token: Option<String> = None;

        loop {
            let response = self.drive_client
                .list_files(&creds.access_token, page_token.as_deref())
                .await?;

            for file in response.files {
                current_files.insert(file.id.clone());

                if self.should_index_file(&file) {
                    match self.drive_client.get_file_content(&creds.access_token, &file).await {
                        Ok(content) => {
                            if !content.is_empty() {
                                let event = DocumentEvent::from_drive_file(source.id.clone(), &file, content);
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
            self.publish_deletion_event(&source.id, deleted_file_id).await?;
        }

        self.update_source_status(&source.id, "completed").await?;
        
        info!("Completed sync for source: {}", source.id);
        Ok(())
    }

    fn should_index_file(&self, file: &crate::models::GoogleDriveFile) -> bool {
        matches!(
            file.mime_type.as_str(),
            "application/vnd.google-apps.document" |
            "application/vnd.google-apps.spreadsheet" |
            "text/plain" | 
            "text/html" | 
            "text/csv"
        )
    }

    async fn get_synced_files(&self, source_id: &str) -> Result<HashSet<String>> {
        let rows = sqlx::query(
            "SELECT external_id FROM documents WHERE source_id = $1"
        )
        .bind(source_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter()
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

        conn.publish::<_, _, ()>("indexer:events", serde_json::to_string(&event_data)?).await?;
        Ok(())
    }

    async fn publish_deletion_event(&self, source_id: &str, document_id: &str) -> Result<()> {
        let mut conn = self.redis_client.get_multiplexed_async_connection().await?;
        
        let event_data = json!({
            "type": "document_deleted",
            "source_id": source_id,
            "document_id": document_id,
        });

        conn.publish::<_, _, ()>("indexer:events", serde_json::to_string(&event_data)?).await?;
        Ok(())
    }

    async fn update_oauth_credentials(&self, source_id: &str, creds: &OAuthCredentials) -> Result<()> {
        sqlx::query(
            "UPDATE sources SET oauth_credentials = $1, updated_at = CURRENT_TIMESTAMP WHERE id = $2"
        )
        .bind(serde_json::to_value(creds)?)
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