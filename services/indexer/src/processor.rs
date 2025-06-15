use shared::models::{ConnectorEvent, DocumentMetadata, DocumentPermissions};
use crate::AppState;
use anyhow::Result;
use shared::db::repositories::DocumentRepository;
use shared::models::Document;
use shared::CONNECTOR_EVENTS_CHANNEL;
use tokio::time::{interval, Duration};
use tokio_stream::StreamExt;
use tracing::{error, info, warn};

pub struct EventProcessor {
    pub state: AppState,
    pub channel: String,
}

impl EventProcessor {
    pub fn new(state: AppState) -> Self {
        Self {
            state,
            channel: CONNECTOR_EVENTS_CHANNEL.to_string(),
        }
    }

    pub async fn start(&self) -> Result<()> {
        info!("Starting event processor for channel: {}", self.channel);

        let mut pubsub = self.state.redis_client.get_async_pubsub().await?;
        pubsub.subscribe(&self.channel).await?;

        let mut stream = pubsub.on_message();
        let mut interval = interval(Duration::from_secs(30));

        loop {
            tokio::select! {
                msg = stream.next() => {
                    match msg {
                        Some(msg) => {
                            let payload: String = msg.get_payload()?;
                            if let Err(e) = self.process_event(&payload).await {
                                error!("Failed to process event: {}, Error: {}", payload, e);
                            }
                        }
                        None => {
                            error!("Redis pubsub stream ended");
                            break;
                        }
                    }
                }
                _ = interval.tick() => {
                    info!("Event processor heartbeat");
                }
            }
        }

        warn!("Event processor stopped");
        Ok(())
    }

    async fn process_event(&self, payload: &str) -> Result<()> {
        let event: ConnectorEvent = serde_json::from_str(payload)?;

        match event {
            ConnectorEvent::DocumentCreated {
                source_id,
                document_id,
                content,
                metadata,
                permissions,
            } => {
                self.handle_document_created(
                    source_id,
                    document_id,
                    content,
                    metadata,
                    permissions,
                )
                .await?;
            }
            ConnectorEvent::DocumentUpdated {
                source_id,
                document_id,
                content,
                metadata,
                permissions,
            } => {
                self.handle_document_updated(
                    source_id,
                    document_id,
                    content,
                    metadata,
                    permissions,
                )
                .await?;
            }
            ConnectorEvent::DocumentDeleted {
                source_id,
                document_id,
            } => {
                self.handle_document_deleted(source_id, document_id).await?;
            }
        }

        Ok(())
    }

    async fn handle_document_created(
        &self,
        source_id: String,
        document_id: String,
        content: String,
        metadata: DocumentMetadata,
        permissions: DocumentPermissions,
    ) -> Result<()> {
        info!(
            "Processing document created: {} from source {}",
            document_id, source_id
        );

        let now = sqlx::types::time::OffsetDateTime::now_utc();
        let metadata_json = serde_json::to_value(&metadata)?;
        let permissions_json = serde_json::to_value(&permissions)?;

        let document = Document {
            id: ulid::Ulid::new().to_string(),
            source_id: source_id.clone(),
            external_id: document_id.clone(),
            title: metadata.title.unwrap_or_else(|| "Untitled".to_string()),
            content: Some(content.clone()),
            content_type: None,
            file_size: None,
            file_extension: None,
            url: None,
            parent_id: None,
            metadata: metadata_json,
            permissions: permissions_json,
            created_at: now,
            updated_at: now,
            last_indexed_at: now,
        };

        let repo = DocumentRepository::new(self.state.db_pool.pool());
        let created = repo.create(document).await?;

        repo.update_search_vector(&created.id).await?;
        repo.mark_as_indexed(&created.id).await?;

        info!("Document created successfully: {}", document_id);
        Ok(())
    }

    async fn handle_document_updated(
        &self,
        source_id: String,
        document_id: String,
        content: String,
        metadata: DocumentMetadata,
        permissions: Option<DocumentPermissions>,
    ) -> Result<()> {
        info!(
            "Processing document updated: {} from source {}",
            document_id, source_id
        );

        let repo = DocumentRepository::new(self.state.db_pool.pool());

        if let Some(mut document) = repo.find_by_external_id(&source_id, &document_id).await? {
            let now = sqlx::types::time::OffsetDateTime::now_utc();
            let metadata_json = serde_json::to_value(&metadata)?;
            let doc_id = document.id.clone();

            document.title = metadata.title.unwrap_or(document.title);
            document.content = Some(content);
            document.metadata = metadata_json;
            if let Some(perms) = permissions {
                document.permissions = serde_json::to_value(&perms)?;
            }
            document.updated_at = now;

            repo.update(&doc_id, document).await?;
            repo.update_search_vector(&doc_id).await?;
            repo.mark_as_indexed(&doc_id).await?;

            info!("Document updated successfully: {}", document_id);
        } else {
            warn!(
                "Document not found for update: {} from source {}",
                document_id, source_id
            );
        }

        Ok(())
    }

    async fn handle_document_deleted(&self, source_id: String, document_id: String) -> Result<()> {
        info!(
            "Processing document deleted: {} from source {}",
            document_id, source_id
        );

        let repo = DocumentRepository::new(self.state.db_pool.pool());

        if let Some(document) = repo.find_by_external_id(&source_id, &document_id).await? {
            repo.delete(&document.id).await?;
            info!("Document deleted successfully: {}", document_id);
        } else {
            warn!(
                "Document not found for deletion: {} from source {}",
                document_id, source_id
            );
        }

        Ok(())
    }
}
