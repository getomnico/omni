use serde::Deserialize;
use serde_json::json;
use shared::models::{ConnectorEvent, DocumentMetadata, DocumentPermissions};
use std::collections::HashMap;
use time::OffsetDateTime;

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum DateValue {
    Timestamp(i64),
    Text(String),
}

#[derive(Debug, Clone, Deserialize)]
pub struct Transcript {
    pub id: String,
    pub title: Option<String>,
    pub date: Option<DateValue>,
    pub duration: Option<f64>,
    pub organizer_email: Option<String>,
    pub participants: Option<Vec<String>>,
    pub transcript_url: Option<String>,
    pub sentences: Option<Vec<Sentence>>,
    pub summary: Option<Summary>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct Sentence {
    pub speaker_name: Option<String>,
    pub text: Option<String>,
    pub start_time: Option<f64>,
    pub end_time: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct Summary {
    pub keywords: Option<Vec<String>>,
    pub action_items: Option<String>,
    pub outline: Option<String>,
    pub overview: Option<String>,
    pub shorthand_bullet: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GraphQLResponse {
    pub data: Option<TranscriptsData>,
    pub errors: Option<Vec<GraphQLError>>,
}

#[derive(Debug, Deserialize)]
pub struct TranscriptsData {
    pub transcripts: Vec<Transcript>,
}

#[derive(Debug, Deserialize)]
pub struct GraphQLError {
    pub message: String,
}

impl Transcript {
    pub fn external_id(&self) -> String {
        format!("fireflies:transcript:{}", self.id)
    }

    pub fn generate_content(&self) -> String {
        let mut content = String::new();

        let title = self.title.as_deref().unwrap_or("Untitled Meeting");
        content.push_str(&format!("# {}\n", title));

        let mut meta_parts = Vec::new();
        if let Some(date) = &self.date {
            match date {
                DateValue::Timestamp(ms) => {
                    if let Ok(dt) = OffsetDateTime::from_unix_timestamp(ms / 1000) {
                        meta_parts.push(format!("Date: {}", dt));
                    }
                }
                DateValue::Text(s) => meta_parts.push(format!("Date: {}", s)),
            }
        }
        if let Some(duration) = self.duration {
            let mins = (duration / 60.0).round() as u64;
            meta_parts.push(format!("Duration: {} min", mins));
        }
        if let Some(participants) = &self.participants {
            if !participants.is_empty() {
                meta_parts.push(format!("Participants: {}", participants.len()));
            }
        }
        if !meta_parts.is_empty() {
            content.push_str(&format!("{}\n", meta_parts.join(" | ")));
        }

        if let Some(summary) = &self.summary {
            if let Some(overview) = &summary.overview {
                if !overview.is_empty() {
                    content.push_str(&format!("\n## Summary\n{}\n", overview));
                }
            }

            if let Some(action_items) = &summary.action_items {
                if !action_items.is_empty() {
                    content.push_str(&format!("\n## Action Items\n{}\n", action_items));
                }
            }

            if let Some(keywords) = &summary.keywords {
                if !keywords.is_empty() {
                    content.push_str(&format!("\n## Keywords\n{}\n", keywords.join(", ")));
                }
            }
        }

        if let Some(sentences) = &self.sentences {
            if !sentences.is_empty() {
                content.push_str("\n## Transcript\n");
                for sentence in sentences {
                    let speaker = sentence.speaker_name.as_deref().unwrap_or("Unknown");
                    let text = sentence.text.as_deref().unwrap_or("");
                    if text.is_empty() {
                        continue;
                    }
                    let timestamp = sentence
                        .start_time
                        .map(|t| {
                            let secs = t as u64;
                            let mins = secs / 60;
                            let secs = secs % 60;
                            format!("[{:02}:{:02}]", mins, secs)
                        })
                        .unwrap_or_default();
                    content.push_str(&format!("{} {}: {}\n", timestamp, speaker, text));
                }
            }
        }

        content.trim().to_string()
    }

    pub fn to_connector_event(
        &self,
        sync_run_id: String,
        source_id: String,
        content_id: String,
    ) -> ConnectorEvent {
        let document_id = self.external_id();

        let title = self
            .title
            .clone()
            .or_else(|| {
                self.date.as_ref().map(|d| match d {
                    DateValue::Timestamp(ms) => OffsetDateTime::from_unix_timestamp(ms / 1000)
                        .map(|dt| format!("Meeting on {}", dt))
                        .unwrap_or_else(|_| "Untitled Meeting".to_string()),
                    DateValue::Text(s) => format!("Meeting on {}", s),
                })
            })
            .unwrap_or_else(|| "Untitled Meeting".to_string());

        let created_at = self.date.as_ref().and_then(|d| match d {
            DateValue::Timestamp(ms) => OffsetDateTime::from_unix_timestamp(ms / 1000).ok(),
            DateValue::Text(s) => s
                .parse::<i64>()
                .ok()
                .and_then(|ms| OffsetDateTime::from_unix_timestamp(ms / 1000).ok())
                .or_else(|| {
                    OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
                }),
        });

        let url = Some(format!("https://app.fireflies.ai/view/{}", self.id));

        let mut extra = HashMap::new();
        let mut fireflies_extra = HashMap::new();
        if let Some(duration) = self.duration {
            fireflies_extra.insert("duration_seconds".to_string(), json!(duration));
        }
        if let Some(participants) = &self.participants {
            fireflies_extra.insert("participants".to_string(), json!(participants));
        }
        if let Some(transcript_url) = &self.transcript_url {
            fireflies_extra.insert("transcript_url".to_string(), json!(transcript_url));
        }
        if let Some(summary) = &self.summary {
            if let Some(keywords) = &summary.keywords {
                fireflies_extra.insert("keywords".to_string(), json!(keywords));
            }
        }
        extra.insert("fireflies".to_string(), json!(fireflies_extra));

        let metadata = DocumentMetadata {
            title: Some(title),
            author: self.organizer_email.clone(),
            created_at,
            updated_at: created_at,
            content_type: Some("meeting_transcript".to_string()),
            mime_type: Some("text/plain".to_string()),
            size: Some(self.generate_content().len().to_string()),
            url,
            path: None,
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
            attributes: None,
        }
    }
}
