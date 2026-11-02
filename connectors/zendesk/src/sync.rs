use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use anyhow::{Context, Result, bail};
use omni_connector_sdk::{
    ConnectorEvent, DocumentAttributes, DocumentMetadata, DocumentPermissions, SyncContext,
    SyncType,
};
use reqwest::StatusCode;
use serde_json::{Value as JsonValue, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tracing::{info, warn};

use crate::client::{ApiError, ZendeskClient, is_auth_rejection};
use crate::config::ZendeskConfig;
use crate::models::{
    Article, ArticlesResponse, CHECKPOINT_VERSION, Comment, GroupMembership, Named, Person, Phase,
    Ticket, TicketExportResponse, User, ZendeskCheckpoint,
};

/// Seconds an incremental article walk re-reads below the watermark, to tolerate clock skew.
const ARTICLE_WATERMARK_OVERLAP_SECS: i64 = 300;
const HEARTBEAT_EVERY_TICKETS: u64 = 25;
/// Per-ticket comment failures that mean "this ticket has no readable comments"; anything else
/// (notably 401/403) would silently index tickets without conversations, so it fails the run.
const COMMENTS_FALLBACK_STATUSES: [StatusCode; 3] = [
    StatusCode::NOT_FOUND,
    StatusCode::GONE,
    StatusCode::UNPROCESSABLE_ENTITY,
];

pub fn agents_group(source_id: &str) -> String {
    format!("zendesk:{source_id}:agents")
}

pub fn zendesk_group(source_id: &str, group_id: u64) -> String {
    format!("zendesk:{source_id}:group:{group_id}")
}

pub fn ticket_document_id(source_id: &str, ticket_id: u64) -> String {
    format!("zendesk:{source_id}:ticket:{ticket_id}")
}

pub fn article_document_id(source_id: &str, article_id: u64) -> String {
    format!("zendesk:{source_id}:article:{article_id}")
}

/// Who may see tickets: unrestricted agents and admins everywhere, group-restricted agents only
/// through their groups. Agents restricted to assigned/requested/organization tickets get nothing.
#[derive(Debug, Default)]
pub struct Directory {
    pub agents: BTreeSet<String>,
    pub group_members: BTreeMap<u64, BTreeSet<String>>,
    pub people: HashMap<u64, Person>,
}

impl Directory {
    pub fn build(users: &[User], memberships: &[GroupMembership]) -> Self {
        let mut directory = Self::default();
        let mut restricted: HashMap<u64, String> = HashMap::new();
        for user in users.iter().filter(|u| u.active && !u.suspended) {
            let Some(email) = user.email.as_ref().map(|e| e.trim().to_lowercase()) else {
                continue;
            };
            directory.people.insert(
                user.id,
                Person {
                    id: user.id,
                    name: user.name.clone(),
                    email: Some(email.clone()),
                },
            );
            match (user.role.as_str(), user.ticket_restriction.as_deref()) {
                ("admin", _) | ("agent", None) => {
                    directory.agents.insert(email);
                }
                ("agent", Some("groups")) => {
                    restricted.insert(user.id, email);
                }
                _ => {}
            }
        }
        for membership in memberships {
            let members = directory
                .group_members
                .entry(membership.group_id)
                .or_default();
            if let Some(email) = restricted.get(&membership.user_id) {
                members.insert(email.clone());
            }
        }
        directory
    }
}

pub fn ticket_permissions(source_id: &str, group_id: Option<u64>) -> DocumentPermissions {
    let mut groups = vec![agents_group(source_id)];
    groups.extend(group_id.map(|id| zendesk_group(source_id, id)));
    DocumentPermissions {
        public: false,
        users: Vec::new(),
        groups,
    }
}

/// Public only when no user segment limits the article; otherwise agents only (fail closed).
pub fn article_permissions(source_id: &str, article: &Article) -> DocumentPermissions {
    let public = article.user_segment_id.is_none() && article.user_segment_ids.is_empty();
    DocumentPermissions {
        public,
        users: Vec::new(),
        groups: if public {
            Vec::new()
        } else {
            vec![agents_group(source_id)]
        },
    }
}

pub async fn run_sync(
    client: &ZendeskClient,
    config: &ZendeskConfig,
    state: Option<ZendeskCheckpoint>,
    ctx: SyncContext,
) -> Result<()> {
    info!(
        source_id = ctx.source_id(),
        sync_run_id = ctx.sync_run_id(),
        mode = %ctx.sync_mode(),
        "Starting Zendesk sync"
    );

    let mut checkpoint = start_checkpoint(&ctx, state)?;

    // shortcut: users and memberships are re-read on every run, resume included, because later
    // phases need names and the group events are idempotent; upgrade if the listings get slow.
    let directory = sync_groups(client, &ctx, &mut checkpoint).await?;
    checkpoint.phase = checkpoint.phase.max(Phase::Articles);
    save(&ctx, &checkpoint).await?;

    if checkpoint.phase == Phase::Articles {
        if !sync_articles(client, config, &ctx, &mut checkpoint, &directory).await? {
            return cancelled(&ctx, &checkpoint).await;
        }
        checkpoint.phase = Phase::Tickets;
        save(&ctx, &checkpoint).await?;
    }

    if checkpoint.phase == Phase::Tickets {
        if config.sync_tickets
            && !sync_tickets(client, config, &ctx, &mut checkpoint, &directory).await?
        {
            return cancelled(&ctx, &checkpoint).await;
        }
        checkpoint.phase = Phase::Done;
        save(&ctx, &checkpoint).await?;
    }

    info!(source_id = ctx.source_id(), "Zendesk sync finished");
    Ok(())
}

fn start_checkpoint(
    ctx: &SyncContext,
    state: Option<ZendeskCheckpoint>,
) -> Result<ZendeskCheckpoint> {
    if let Some(state) = &state
        && state.version != CHECKPOINT_VERSION
    {
        bail!("unsupported Zendesk checkpoint version {}", state.version);
    }
    match state {
        Some(state) if ctx.is_resume() && state.phase != Phase::Done => Ok(state),
        previous => Ok(ZendeskCheckpoint {
            version: CHECKPOINT_VERSION,
            mode: ctx.sync_mode(),
            cutoff: OffsetDateTime::now_utc().unix_timestamp(),
            phase: Phase::Users,
            // A full sync re-exports from the beginning with a fresh cursor.
            tickets_cursor: match ctx.sync_mode() {
                SyncType::Full => None,
                _ => previous.as_ref().and_then(|p| p.tickets_cursor.clone()),
            },
            articles_watermark: previous.as_ref().and_then(|p| p.articles_watermark),
            known_group_ids: previous
                .as_ref()
                .map(|p| p.known_group_ids.clone())
                .unwrap_or_default(),
            known_article_ids: previous.map(|p| p.known_article_ids).unwrap_or_default(),
        }),
    }
}

async fn save(ctx: &SyncContext, checkpoint: &ZendeskCheckpoint) -> Result<()> {
    ctx.save_checkpoint(serde_json::to_value(checkpoint)?).await
}

async fn cancelled(ctx: &SyncContext, checkpoint: &ZendeskCheckpoint) -> Result<()> {
    info!(sync_run_id = ctx.sync_run_id(), "Zendesk sync cancelled");
    save(ctx, checkpoint).await?;
    ctx.cancel().await
}

async fn sync_groups(
    client: &ZendeskClient,
    ctx: &SyncContext,
    checkpoint: &mut ZendeskCheckpoint,
) -> Result<Directory> {
    let users = client.agents_and_admins().await?;
    let memberships = client.group_memberships().await?;
    let directory = Directory::build(&users, &memberships);

    let source_id = ctx.source_id();
    emit_group(
        ctx,
        agents_group(source_id),
        Some("Zendesk agents".into()),
        &directory.agents,
    )
    .await?;
    // Every group seen in the listing is re-emitted, so members that lost the restriction are dropped.
    for (group_id, members) in &directory.group_members {
        emit_group(ctx, zendesk_group(source_id, *group_id), None, members).await?;
    }
    // Groups deleted in Zendesk no longer show up in memberships; empty them so access is revoked.
    let no_members = BTreeSet::new();
    for group_id in &checkpoint.known_group_ids {
        if !directory.group_members.contains_key(group_id) {
            emit_group(ctx, zendesk_group(source_id, *group_id), None, &no_members).await?;
        }
    }
    checkpoint.known_group_ids = directory.group_members.keys().copied().collect();
    Ok(directory)
}

async fn emit_group(
    ctx: &SyncContext,
    group_email: String,
    group_name: Option<String>,
    members: &BTreeSet<String>,
) -> Result<()> {
    ctx.emit_event(ConnectorEvent::GroupMembershipSync {
        sync_run_id: ctx.sync_run_id().to_string(),
        source_id: ctx.source_id().to_string(),
        group_email,
        group_name,
        member_emails: members.iter().cloned().collect(),
    })
    .await
}

/// Returns `false` when the run was cancelled before the phase finished.
async fn sync_articles(
    client: &ZendeskClient,
    config: &ZendeskConfig,
    ctx: &SyncContext,
    checkpoint: &mut ZendeskCheckpoint,
    directory: &Directory,
) -> Result<bool> {
    let full = ctx.sync_mode() == SyncType::Full;
    let stop_below = match ctx.sync_mode() {
        SyncType::Full => None,
        _ => checkpoint
            .articles_watermark
            .map(|w| w - ARTICLE_WATERMARK_OVERLAP_SECS),
    };
    let mut watermark = if full {
        None
    } else {
        checkpoint.articles_watermark
    };
    let mut seen = BTreeSet::new();
    let mut known: BTreeSet<u64> = checkpoint.known_article_ids.iter().copied().collect();

    if config.sync_help_center {
        let mut pager = client.articles_pager();
        'pages: while let Some(page) = pager.next::<ArticlesResponse>(client).await? {
            if ctx.is_cancelled() {
                return Ok(false);
            }
            for article in page.articles {
                let updated = article.updated_at.unix_timestamp();
                if stop_below.is_some_and(|floor| updated < floor) {
                    break 'pages;
                }
                watermark = watermark.max(Some(updated));
                ctx.increment_scanned(1).await?;
                if article.draft {
                    // Unpublishing reduces visibility; full syncs reconcile it below.
                    if !full && known.remove(&article.id) {
                        emit_deleted(ctx, article_document_id(ctx.source_id(), article.id)).await?;
                    }
                    continue;
                }
                seen.insert(article.id);
                emit_article(ctx, &article, directory).await?;
            }
            ctx.heartbeat().await?;
        }
    }

    if full {
        for id in known.difference(&seen) {
            emit_deleted(ctx, article_document_id(ctx.source_id(), *id)).await?;
        }
        checkpoint.known_article_ids = seen.into_iter().collect();
    } else {
        checkpoint.known_article_ids = known.union(&seen).copied().collect();
    }
    checkpoint.articles_watermark = watermark;
    Ok(true)
}

async fn emit_deleted(ctx: &SyncContext, document_id: String) -> Result<()> {
    ctx.emit_event(ConnectorEvent::DocumentDeleted {
        sync_run_id: ctx.sync_run_id().to_string(),
        source_id: ctx.source_id().to_string(),
        document_id,
    })
    .await
}

async fn emit_article(ctx: &SyncContext, article: &Article, directory: &Directory) -> Result<()> {
    let content_id = match &article.body {
        Some(body) => {
            ctx.extract_and_store_content(
                body.clone().into_bytes(),
                "text/html",
                Some(&format!("{}.html", article.id)),
            )
            .await?
        }
        None => ctx.store_content(&article.title).await?,
    };
    let metadata = DocumentMetadata {
        title: Some(article.title.clone()),
        author: article
            .author_id
            .and_then(|id| directory.people.get(&id))
            .and_then(|p| p.name.clone()),
        created_at: Some(article.created_at),
        updated_at: Some(article.updated_at),
        content_type: Some("article".into()),
        mime_type: Some("text/html".into()),
        size: None,
        url: Some(article.html_url.clone()),
        path: None,
        extra: Some(extra_map(json!({
            "article_id": article.id,
            "section_id": article.section_id,
            "locale": article.locale,
            "label_names": article.label_names,
            "user_segment_id": article.user_segment_id,
            "permission_group_id": article.permission_group_id,
        }))?),
    };
    let attributes: DocumentAttributes = HashMap::from([
        ("locale".to_string(), json!(article.locale)),
        ("labels".to_string(), json!(article.label_names)),
    ]);
    emit_document(
        ctx,
        article_document_id(ctx.source_id(), article.id),
        content_id,
        metadata,
        article_permissions(ctx.source_id(), article),
        attributes,
    )
    .await
}

/// Returns `false` when the run was cancelled before the export finished.
async fn sync_tickets(
    client: &ZendeskClient,
    config: &ZendeskConfig,
    ctx: &SyncContext,
    checkpoint: &mut ZendeskCheckpoint,
    directory: &Directory,
) -> Result<bool> {
    let mut seen_cursors = HashSet::new();
    let mut processed = 0u64;
    let mut first_request = true;
    loop {
        if ctx.is_cancelled() {
            return Ok(false);
        }
        let page = match client
            .export_tickets(checkpoint.tickets_cursor.as_deref())
            .await
        {
            Err(error)
                if std::mem::take(&mut first_request)
                    && checkpoint.tickets_cursor.is_some()
                    && error
                        .downcast_ref::<ApiError>()
                        .is_some_and(|e| e.status.is_client_error())
                    && !is_auth_rejection(&error) =>
            {
                // Upserts are idempotent, so re-exporting everything is safe.
                warn!("Stored ticket export cursor was rejected, restarting the export: {error}");
                checkpoint.tickets_cursor = None;
                client.export_tickets(None).await?
            }
            other => {
                first_request = false;
                other?
            }
        };
        let TicketExportResponse {
            tickets,
            users,
            groups,
            organizations,
            after_cursor,
            end_of_stream,
        } = page;
        let mut people = directory.people.clone();
        people.extend(users.into_iter().map(|p| (p.id, p)));
        let lookup = Lookup {
            people,
            groups: groups.into_iter().map(|g| (g.id, g)).collect(),
            organizations: organizations.into_iter().map(|o| (o.id, o)).collect(),
        };

        for ticket in tickets {
            if ctx.is_cancelled() {
                return Ok(false);
            }
            processed += 1;
            if processed.is_multiple_of(HEARTBEAT_EVERY_TICKETS) {
                ctx.heartbeat().await?;
            }
            ctx.increment_scanned(1).await?;
            if ticket.status == "deleted" {
                emit_deleted(ctx, ticket_document_id(ctx.source_id(), ticket.id)).await?;
                continue;
            }
            emit_ticket(client, config, ctx, &ticket, &lookup).await?;
        }

        match after_cursor {
            Some(next) => {
                if !end_of_stream && !seen_cursors.insert(next.clone()) {
                    bail!("Zendesk ticket export repeated a cursor");
                }
                checkpoint.tickets_cursor = Some(next);
            }
            None if !end_of_stream => bail!("Zendesk ticket export returned no after_cursor"),
            None => {}
        }
        save(ctx, checkpoint).await?;
        if end_of_stream {
            return Ok(true);
        }
    }
}

struct Lookup {
    people: HashMap<u64, Person>,
    groups: HashMap<u64, Named>,
    organizations: HashMap<u64, Named>,
}

async fn emit_ticket(
    client: &ZendeskClient,
    config: &ZendeskConfig,
    ctx: &SyncContext,
    ticket: &Ticket,
    lookup: &Lookup,
) -> Result<()> {
    let (comments, comment_users) = match client.ticket_comments(ticket.id).await {
        Ok(fetched) => fetched,
        Err(error)
            if error
                .downcast_ref::<ApiError>()
                .is_some_and(|e| COMMENTS_FALLBACK_STATUSES.contains(&e.status)) =>
        {
            warn!(ticket_id = ticket.id, "Skipping comments: {error}");
            (Vec::new(), Vec::new())
        }
        Err(error) => return Err(error),
    };

    let person = |id: u64| {
        comment_users
            .iter()
            .find(|p| p.id == id)
            .or_else(|| lookup.people.get(&id))
    };
    let requester = ticket.requester_id.and_then(person);
    let assignee = ticket.assignee_id.and_then(person);
    let content = render_ticket(config, ticket, lookup, &comments, &person)?;
    let content_id = ctx.store_content(&content).await?;

    let url = config.agent_ticket_url(ticket.id);
    let metadata = DocumentMetadata {
        title: Some(ticket_title(ticket)),
        author: requester.and_then(|p| p.name.clone()),
        created_at: Some(ticket.created_at),
        updated_at: Some(ticket.updated_at),
        content_type: Some("ticket".into()),
        mime_type: Some("text/markdown".into()),
        size: None,
        url: Some(url),
        path: None,
        extra: Some(extra_map(json!({
            "ticket_id": ticket.id,
            "status": ticket.status,
            "priority": ticket.priority,
            "type": ticket.ticket_type,
            "tags": ticket.tags,
            "group_id": ticket.group_id,
            "organization_id": ticket.organization_id,
            "via_channel": ticket.via.as_ref().and_then(|v| v.channel.clone()),
            "requester_id": ticket.requester_id,
            "assignee_id": ticket.assignee_id,
        }))?),
    };
    let mut attributes: DocumentAttributes = HashMap::from([
        ("status".to_string(), json!(ticket.status)),
        ("tags".to_string(), json!(ticket.tags)),
    ]);
    let optional = [
        ("priority", ticket.priority.clone()),
        ("ticket_type", ticket.ticket_type.clone()),
        ("assignee", assignee.and_then(lowercase_email)),
        ("requester", requester.and_then(lowercase_email)),
    ];
    for (key, value) in optional {
        if let Some(value) = value {
            attributes.insert(key.to_string(), json!(value));
        }
    }

    emit_document(
        ctx,
        ticket_document_id(ctx.source_id(), ticket.id),
        content_id,
        metadata,
        ticket_permissions(ctx.source_id(), ticket.group_id),
        attributes,
    )
    .await
}

// Sideloaded users are not normalised like the directory is.
fn lowercase_email(person: &Person) -> Option<String> {
    person.email.as_ref().map(|e| e.to_lowercase())
}

fn ticket_title(ticket: &Ticket) -> String {
    ticket
        .subject
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .map_or_else(|| format!("Ticket #{}", ticket.id), str::to_owned)
}

fn describe<'a>(id: Option<u64>, person: &impl Fn(u64) -> Option<&'a Person>) -> String {
    let Some(id) = id else {
        return "-".into();
    };
    match person(id) {
        Some(Person {
            name,
            email: Some(email),
            ..
        }) => format!("{} <{email}>", name.as_deref().unwrap_or("-")),
        Some(Person {
            name: Some(name), ..
        }) => name.clone(),
        _ => format!("User {id}"),
    }
}

fn render_ticket<'a>(
    config: &ZendeskConfig,
    ticket: &Ticket,
    lookup: &Lookup,
    comments: &[Comment],
    person: &impl Fn(u64) -> Option<&'a Person>,
) -> Result<String> {
    let named = |id: Option<u64>, map: &HashMap<u64, Named>| {
        id.and_then(|id| map.get(&id))
            .map_or_else(|| "-".to_string(), |n| n.name.clone())
    };
    let mut out = format!(
        "# {}\nTicket #{} · status: {} · priority: {} · type: {}\n",
        ticket_title(ticket),
        ticket.id,
        ticket.status,
        ticket.priority.as_deref().unwrap_or("-"),
        ticket.ticket_type.as_deref().unwrap_or("-"),
    );
    out += &format!(
        "Requester: {} · Assignee: {} · Group: {} · Organization: {}\n",
        describe(ticket.requester_id, person),
        describe(ticket.assignee_id, person),
        named(ticket.group_id, &lookup.groups),
        named(ticket.organization_id, &lookup.organizations),
    );
    out += &format!(
        "Tags: {}\nURL: {}\n",
        ticket.tags.join(", "),
        config.agent_ticket_url(ticket.id)
    );

    if comments.is_empty() {
        if let Some(description) = &ticket.description {
            out += &format!("\n## Description\n{description}\n");
        }
        return Ok(out);
    }
    out += "\n## Conversation\n";
    for comment in comments {
        let author = match person(comment.author_id).and_then(|p| p.name.as_deref()) {
            Some(name) => name.to_string(),
            None => format!("User {}", comment.author_id),
        };
        let note = if comment.public {
            ""
        } else {
            " [internal note]"
        };
        out += &format!(
            "### {author} — {}{note}\n",
            comment.created_at.format(&Rfc3339)?
        );
        if let Some(body) = &comment.plain_body {
            out += &format!("{body}\n");
        }
        if !comment.attachments.is_empty() {
            let names: Vec<&str> = comment
                .attachments
                .iter()
                .map(|a| a.file_name.as_str())
                .collect();
            out += &format!("Attachments: {}\n", names.join(", "));
        }
    }
    Ok(out)
}

async fn emit_document(
    ctx: &SyncContext,
    document_id: String,
    content_id: String,
    metadata: DocumentMetadata,
    permissions: DocumentPermissions,
    attributes: DocumentAttributes,
) -> Result<()> {
    let sync_run_id = ctx.sync_run_id().to_string();
    let source_id = ctx.source_id().to_string();
    let attributes = Some(attributes);
    let event = match ctx.sync_mode() {
        SyncType::Full => ConnectorEvent::DocumentCreated {
            sync_run_id,
            source_id,
            document_id,
            content_id,
            metadata,
            permissions,
            attributes,
        },
        _ => ConnectorEvent::DocumentUpdated {
            sync_run_id,
            source_id,
            document_id,
            content_id,
            metadata,
            permissions: Some(permissions),
            attributes,
        },
    };
    ctx.emit_event(event).await?;
    ctx.increment_updated(1).await
}

fn extra_map(value: JsonValue) -> Result<HashMap<String, JsonValue>> {
    serde_json::from_value(value).context("document extra metadata is not an object")
}
