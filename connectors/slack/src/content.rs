use anyhow::Result;
use chrono::{DateTime, NaiveDate, Utc};
use std::collections::HashMap;
use tracing::{debug, info};

use crate::models::{MessageGroup, SlackMessage, SlackUser};

pub struct ContentProcessor {
    users: HashMap<String, SlackUser>,
}

impl ContentProcessor {
    pub fn new() -> Self {
        Self {
            users: HashMap::new(),
        }
    }

    pub fn update_users(&mut self, users: Vec<SlackUser>) {
        for user in users {
            self.users.insert(user.id.clone(), user);
        }
        info!("Updated user cache with {} users", self.users.len());
    }

    pub fn resolve_member_emails(&self, user_ids: &[String]) -> Vec<String> {
        let mut emails: Vec<String> = user_ids
            .iter()
            .filter_map(|id| {
                let user = self.users.get(id)?;
                if user.is_bot {
                    return None;
                }
                user.email().map(|e| e.to_string())
            })
            .collect();
        emails.sort();
        emails.dedup();
        emails
    }

    pub fn get_author_name(&self, user_id: &str) -> String {
        self.users
            .get(user_id)
            .map(|user| user.real_name.clone().unwrap_or_else(|| user.name.clone()))
            .unwrap_or_else(|| format!("User {}", user_id))
    }

    pub fn group_messages_by_date(
        &self,
        channel_id: String,
        channel_name: String,
        messages: Vec<SlackMessage>,
    ) -> Result<Vec<MessageGroup>> {
        let mut groups: HashMap<NaiveDate, MessageGroup> = HashMap::new();
        let mut thread_groups: HashMap<String, MessageGroup> = HashMap::new();
        let message_count = messages.len();

        for message in messages {
            // Skip bot messages and system messages
            if message.msg_type != "message" || message.user.is_empty() {
                continue;
            }

            let timestamp = message
                .ts
                .split('.')
                .next()
                .unwrap_or("0")
                .parse::<i64>()
                .unwrap_or(0);

            let datetime =
                DateTime::from_timestamp(timestamp, 0).unwrap_or_else(|| DateTime::<Utc>::MIN_UTC);
            let date = datetime.date_naive();
            let author_name = self.get_author_name(&message.user);

            // Check if this is a thread message
            if let Some(thread_ts) = &message.thread_ts {
                // This is a thread reply
                let thread_key = format!("{}_{}", channel_id, thread_ts);

                let group = thread_groups.entry(thread_key).or_insert_with(|| {
                    MessageGroup::new(
                        channel_id.clone(),
                        channel_name.clone(),
                        date,
                        true,
                        Some(thread_ts.clone()),
                    )
                });

                group.add_message(message, author_name);
            } else {
                // Regular channel message
                let group = groups.entry(date).or_insert_with(|| {
                    MessageGroup::new(channel_id.clone(), channel_name.clone(), date, false, None)
                });

                group.add_message(message, author_name);
            }
        }

        // Collect all groups and split if they're too large
        let mut result = Vec::new();

        // Process daily groups
        for (_, group) in groups {
            result.extend(self.split_group_if_needed(group));
        }

        // Process thread groups
        for (_, group) in thread_groups {
            result.extend(self.split_group_if_needed(group));
        }

        debug!(
            "Grouped {} messages into {} document groups for channel {}",
            message_count,
            result.len(),
            channel_name
        );

        Ok(result)
    }

    fn split_group_if_needed(&self, group: MessageGroup) -> Vec<MessageGroup> {
        if !group.should_split() {
            return vec![group];
        }

        let mut groups = Vec::new();
        let mut current_group = MessageGroup::new(
            group.channel_id.clone(),
            group.channel_name.clone(),
            group.date,
            group.is_thread,
            group.thread_ts.clone(),
        );

        for (message, author) in group.messages {
            current_group.add_message(message, author);

            if current_group.should_split() {
                // Remove the last message that caused the split
                let last_message = current_group.messages.pop().unwrap();

                // Add the completed group
                if !current_group.messages.is_empty() {
                    groups.push(current_group);
                }

                // Start a new group with the message that caused the split
                current_group = MessageGroup::new(
                    group.channel_id.clone(),
                    group.channel_name.clone(),
                    group.date,
                    group.is_thread,
                    group.thread_ts.clone(),
                );
                current_group.add_message(last_message.0, last_message.1);
            }
        }

        // Add the final group if it has messages
        if !current_group.messages.is_empty() {
            groups.push(current_group);
        }

        for (i, group) in groups.iter_mut().enumerate() {
            group.part = Some(i);
        }

        debug!(
            "Split large group into {} smaller groups for channel {}",
            groups.len(),
            group.channel_name
        );

        groups
    }

    pub fn extract_files_from_messages<'a>(
        &self,
        messages: &'a [SlackMessage],
    ) -> Vec<&'a crate::models::SlackFile> {
        let mut files = Vec::new();

        for message in messages {
            if let Some(message_files) = &message.files {
                for file in message_files {
                    // Only process files with downloadable content
                    if file.url_private_download.is_some() {
                        files.push(file);
                    }
                }
            }
        }

        debug!("Found {} downloadable files in messages", files.len());
        files
    }
}
