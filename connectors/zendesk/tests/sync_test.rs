mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use common::Fixture;
use omni_zendesk_connector::client::ZendeskClient;
use omni_zendesk_connector::config::ZendeskConfig;
use omni_zendesk_connector::credentials::ZendeskCredentials;
use omni_zendesk_connector::models::{CHECKPOINT_VERSION, Phase, ZendeskCheckpoint};
use omni_zendesk_connector::sync::run_sync;
use serde_json::{Value as JsonValue, json};
use shared::models::{SyncStatus, SyncType};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const ARTICLE_UPDATED: i64 = 1_714_644_000; // 2024-05-02T10:00:00Z
const WATERMARK: i64 = 1_717_063_200; // 2024-05-30T10:00:00Z

fn client(server: &MockServer) -> ZendeskClient {
    ZendeskClient::with_base_url(
        server.uri(),
        ZendeskCredentials::ApiToken {
            email: "admin@example.com".to_string(),
            api_token: "tok".to_string(),
        },
    )
    .unwrap()
}

fn config() -> ZendeskConfig {
    ZendeskConfig::from_source_config(&json!({"subdomain": "example"})).unwrap()
}

fn page(key: &str, items: JsonValue) -> JsonValue {
    json!({key: items, "meta": {"has_more": false, "after_cursor": null}})
}

fn agent(
    id: u64,
    name: &str,
    email: &str,
    restriction: Option<&str>,
    suspended: bool,
) -> JsonValue {
    json!({
        "id": id, "name": name, "email": email, "role": "agent",
        "ticket_restriction": restriction, "active": true, "suspended": suspended
    })
}

fn article(id: u64, title: &str, updated: &str, draft: bool, segment: Option<u64>) -> JsonValue {
    json!({
        "id": id, "title": title, "body": format!("<p>Body of {title}</p>"),
        "html_url": format!("https://example.zendesk.com/hc/en-us/articles/{id}"),
        "author_id": 1, "section_id": 9, "locale": "en-us", "draft": draft,
        "user_segment_id": segment, "permission_group_id": 3, "label_names": ["faq"],
        "created_at": "2024-01-01T00:00:00Z", "updated_at": updated
    })
}

fn ticket(id: u64, status: &str) -> JsonValue {
    json!({
        "id": id, "subject": format!("Printer {id} is on fire"), "description": "It smokes.",
        "status": status, "priority": "high", "type": "problem", "tags": ["vip"],
        "group_id": 10, "organization_id": 30, "requester_id": 20, "assignee_id": 2,
        "via": {"channel": "email"},
        "created_at": "2024-05-01T08:00:00Z", "updated_at": "2024-05-02T08:00:00Z"
    })
}

fn export(tickets: JsonValue, after_cursor: &str, end_of_stream: bool) -> JsonValue {
    json!({
        "tickets": tickets,
        "users": [{"id": 20, "name": "Rae Requester", "email": "rae@example.com"}],
        "groups": [{"id": 10, "name": "Support"}],
        "organizations": [{"id": 30, "name": "Acme"}],
        "after_cursor": after_cursor, "end_of_stream": end_of_stream
    })
}

async fn get(server: &MockServer, p: &str, body: JsonValue) {
    Mock::given(method("GET"))
        .and(path(p))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

async fn mock_people(server: &MockServer) {
    get(
        server,
        "/users.json",
        page(
            "users",
            json!([
                {"id": 1, "name": "Ada Admin", "email": "ada@example.com", "role": "admin",
                 "ticket_restriction": null, "active": true, "suspended": false},
                agent(2, "Bob Agent", "Bob@Example.com", None, false),
                agent(3, "Cy Restricted", "cy@example.com", Some("groups"), false),
                agent(4, "Dee Suspended", "dee@example.com", None, true),
                agent(6, "Eli Assigned", "eli@example.com", Some("assigned"), false),
            ]),
        ),
    )
    .await;
    get(
        server,
        "/group_memberships.json",
        page(
            "group_memberships",
            json!([
                {"user_id": 3, "group_id": 10}, {"user_id": 6, "group_id": 10},
                {"user_id": 2, "group_id": 11}
            ]),
        ),
    )
    .await;
}

async fn mock_comments(server: &MockServer, responder: impl Respond + 'static) {
    Mock::given(method("GET"))
        .and(wiremock::matchers::path_regex(
            r"^/tickets/\d+/comments\.json$",
        ))
        .respond_with(responder)
        .mount(server)
        .await;
}

fn comments_body() -> JsonValue {
    json!({
        "comments": [
            {"author_id": 20, "plain_body": "Please help.", "public": true,
             "created_at": "2024-05-01T08:00:00Z", "attachments": []},
            {"author_id": 2, "plain_body": "Escalating to ops.", "public": false,
             "created_at": "2024-05-01T09:00:00Z",
             "attachments": [{"file_name": "invoice.pdf"}]}
        ],
        "users": [{"id": 2, "name": "Bob Agent", "email": "bob@example.com"}],
        "meta": {"has_more": false, "after_cursor": null}
    })
}

async fn mock_full_zendesk() -> MockServer {
    let server = MockServer::start().await;
    mock_people(&server).await;
    get(
        &server,
        "/help_center/articles.json",
        page(
            "articles",
            json!([
                article(1, "Reset password", "2024-05-02T10:00:00Z", false, None),
                article(2, "Unfinished", "2024-05-02T09:00:00Z", true, None),
                article(3, "Agents only", "2024-05-02T08:00:00Z", false, Some(5)),
            ]),
        ),
    )
    .await;
    // The more specific cursor mock is registered first; wiremock uses the first match.
    Mock::given(method("GET"))
        .and(path("/incremental/tickets/cursor.json"))
        .and(query_param("cursor", "c1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(export(json!([]), "c2", true)))
        .mount(&server)
        .await;
    get(
        &server,
        "/incremental/tickets/cursor.json",
        export(
            json!([ticket(1001, "open"), ticket(1002, "deleted")]),
            "c1",
            false,
        ),
    )
    .await;
    mock_comments(
        &server,
        ResponseTemplate::new(200).set_body_json(comments_body()),
    )
    .await;
    server
}

fn of_type<'a>(events: &'a [JsonValue], kind: &str) -> Vec<&'a JsonValue> {
    events.iter().filter(|e| e["type"] == kind).collect()
}

fn doc<'a>(events: &'a [JsonValue], suffix: &str) -> &'a JsonValue {
    events
        .iter()
        .find(|e| {
            e["document_id"]
                .as_str()
                .is_some_and(|id| id.ends_with(suffix))
        })
        .unwrap_or_else(|| panic!("no event for {suffix}"))
}

fn checkpoint(phase: Phase, cursor: Option<&str>, known: Vec<u64>) -> ZendeskCheckpoint {
    ZendeskCheckpoint {
        version: CHECKPOINT_VERSION,
        mode: SyncType::Full,
        cutoff: 1_717_000_000,
        phase,
        tickets_cursor: cursor.map(str::to_string),
        articles_watermark: Some(WATERMARK),
        known_article_ids: known,
        known_group_ids: Vec::new(),
    }
}

fn paths(requests: &[Request]) -> Vec<String> {
    requests.iter().map(|r| r.url.path().to_string()).collect()
}

#[tokio::test]
async fn full_sync_emits_groups_documents_deletes_and_promotes_checkpoint() {
    let server = mock_full_zendesk().await;
    let fixture = Fixture::new().await.unwrap();
    let (source_id, run_id) = fixture.seed_run(SyncType::Full).await.unwrap();
    let ctx = fixture.context(&source_id, &run_id, SyncType::Full, false, Arc::default());

    // Article 99 was indexed by an earlier sync and is gone now.
    // Zendesk group 12 was emitted by an earlier sync and is gone now.
    let mut prior = checkpoint(Phase::Done, Some("stale"), vec![1, 99]);
    prior.known_group_ids = vec![10, 12];
    run_sync(&client(&server), &config(), Some(prior), ctx.clone())
        .await
        .unwrap();
    ctx.complete().await.unwrap();

    let events = fixture.events(&source_id).await.unwrap();
    let agents = format!("zendesk:{source_id}:agents");
    let group = |id: u64| format!("zendesk:{source_id}:group:{id}");

    let groups = of_type(&events, "group_membership_sync");
    let members = |email: &str| {
        groups
            .iter()
            .find(|g| g["group_email"] == email)
            .unwrap_or_else(|| panic!("no group {email}"))["member_emails"]
            .clone()
    };
    assert_eq!(
        members(&agents),
        json!(["ada@example.com", "bob@example.com"])
    );
    assert_eq!(members(&group(10)), json!(["cy@example.com"]));
    assert_eq!(members(&group(11)), json!([]));
    assert_eq!(members(&group(12)), json!([]), "vanished group is revoked");

    let created = of_type(&events, "document_created");
    assert_eq!(
        created.len(),
        3,
        "ticket 1001 plus published articles 1 and 3"
    );

    let ticket_event = doc(&events, ":ticket:1001");
    assert_eq!(ticket_event["type"], "document_created");
    assert_eq!(
        ticket_event["permissions"],
        json!({"public": false, "users": [], "groups": [agents, group(10)]})
    );
    assert_eq!(ticket_event["metadata"]["content_type"], "ticket");
    assert_eq!(ticket_event["metadata"]["mime_type"], "text/markdown");
    assert_eq!(ticket_event["metadata"]["author"], "Rae Requester");
    assert_eq!(
        ticket_event["metadata"]["url"],
        "https://example.zendesk.com/agent/tickets/1001"
    );
    assert_eq!(ticket_event["metadata"]["extra"]["group_id"], 10);
    assert_eq!(ticket_event["metadata"]["extra"]["via_channel"], "email");
    let attributes = &ticket_event["attributes"];
    assert_eq!(attributes["status"], "open");
    assert_eq!(attributes["priority"], "high");
    assert_eq!(attributes["ticket_type"], "problem");
    assert_eq!(attributes["tags"], json!(["vip"]));
    assert_eq!(attributes["assignee"], "bob@example.com");
    assert_eq!(attributes["requester"], "rae@example.com");

    let content = fixture
        .content(ticket_event["content_id"].as_str().unwrap())
        .await
        .unwrap();
    for expected in [
        "# Printer 1001 is on fire",
        "Ticket #1001 · status: open · priority: high · type: problem",
        "Requester: Rae Requester <rae@example.com>",
        "Group: Support · Organization: Acme",
        "Tags: vip",
        "URL: https://example.zendesk.com/agent/tickets/1001",
        "### Bob Agent — 2024-05-01T09:00:00Z [internal note]",
        "Attachments: invoice.pdf",
    ] {
        assert!(
            content.contains(expected),
            "missing {expected:?} in\n{content}"
        );
    }
    assert!(!content.contains("### Rae Requester — 2024-05-01T08:00:00Z ["));

    assert_eq!(doc(&events, ":article:1")["permissions"]["public"], true);
    let restricted = doc(&events, ":article:3");
    assert_eq!(restricted["permissions"]["public"], false);
    assert_eq!(restricted["permissions"]["groups"], json!([agents]));
    assert_eq!(restricted["metadata"]["content_type"], "article");
    assert_eq!(restricted["attributes"]["labels"], json!(["faq"]));
    assert!(!events.iter().any(|e| {
        e["document_id"]
            .as_str()
            .is_some_and(|id| id.ends_with(":article:2"))
    }));
    let article_body = fixture
        .content(doc(&events, ":article:1")["content_id"].as_str().unwrap())
        .await
        .unwrap();
    assert!(article_body.contains("Body of Reset password"));

    let deleted: Vec<&str> = of_type(&events, "document_deleted")
        .iter()
        .map(|e| e["document_id"].as_str().unwrap())
        .collect();
    assert_eq!(deleted.len(), 2);
    assert!(deleted.contains(&format!("zendesk:{source_id}:ticket:1002").as_str()));
    assert!(deleted.contains(&format!("zendesk:{source_id}:article:99").as_str()));

    // A full sync restarts the export from the beginning, ignoring the stale cursor.
    let export_requests: Vec<_> = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.url.path() == "/incremental/tickets/cursor.json")
        .collect();
    assert!(
        export_requests[0]
            .url
            .query()
            .unwrap()
            .contains("start_time=0")
    );
    assert!(
        export_requests[1]
            .url
            .query()
            .unwrap()
            .contains("cursor=c1")
    );

    let promoted: ZendeskCheckpoint = serde_json::from_value(
        fixture
            .sdk_client
            .get_checkpoint(&source_id)
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(promoted.phase, Phase::Done);
    assert_eq!(promoted.tickets_cursor.as_deref(), Some("c2"));
    assert_eq!(promoted.articles_watermark, Some(ARTICLE_UPDATED));
    assert_eq!(promoted.known_article_ids, vec![1, 3]);
    assert_eq!(promoted.known_group_ids, vec![10, 11]);
}

#[tokio::test]
async fn incremental_sync_continues_from_cursor_and_stops_at_watermark() {
    let server = MockServer::start().await;
    mock_people(&server).await;
    get(
        &server,
        "/help_center/articles.json",
        page(
            "articles",
            json!([
                article(10, "Fresh", "2024-06-01T10:00:00Z", false, None),
                article(11, "Old", "2024-01-01T10:00:00Z", false, None),
            ]),
        ),
    )
    .await;
    Mock::given(method("GET"))
        .and(path("/incremental/tickets/cursor.json"))
        .and(query_param("cursor", "c1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(export(
            json!([ticket(2001, "solved")]),
            "c9",
            true,
        )))
        .mount(&server)
        .await;
    mock_comments(
        &server,
        ResponseTemplate::new(200).set_body_json(comments_body()),
    )
    .await;

    let fixture = Fixture::new().await.unwrap();
    let (source_id, run_id) = fixture.seed_run(SyncType::Incremental).await.unwrap();
    let ctx = fixture.context(
        &source_id,
        &run_id,
        SyncType::Incremental,
        false,
        Arc::default(),
    );

    // Article 777 vanished, but only full syncs reconcile article deletes.
    let prior = checkpoint(Phase::Done, Some("c1"), vec![777]);
    run_sync(&client(&server), &config(), Some(prior), ctx.clone())
        .await
        .unwrap();

    let events = fixture.events(&source_id).await.unwrap();
    assert!(of_type(&events, "document_deleted").is_empty());
    assert_eq!(doc(&events, ":ticket:2001")["type"], "document_updated");
    doc(&events, ":article:10");
    assert!(!events.iter().any(|e| {
        e["document_id"]
            .as_str()
            .is_some_and(|id| id.ends_with(":article:11"))
    }));

    let requests = server.received_requests().await.unwrap();
    let exports: Vec<_> = requests
        .iter()
        .filter(|r| r.url.path() == "/incremental/tickets/cursor.json")
        .collect();
    assert_eq!(exports.len(), 1);
    let query = exports[0].url.query().unwrap();
    assert!(query.contains("cursor=c1") && !query.contains("start_time"));

    let run = fixture.sync_run(&run_id).await.unwrap();
    let saved: ZendeskCheckpoint = serde_json::from_value(run.checkpoint.unwrap()).unwrap();
    assert_eq!(saved.mode, SyncType::Incremental);
    assert_eq!(saved.tickets_cursor.as_deref(), Some("c9"));
    assert_eq!(saved.articles_watermark, Some(1_717_236_000));
    assert_eq!(saved.known_article_ids, vec![10, 777]);
}

#[tokio::test]
async fn resume_from_tickets_phase_does_not_redo_articles() {
    let server = mock_full_zendesk().await;
    let fixture = Fixture::new().await.unwrap();
    let (source_id, run_id) = fixture.seed_run(SyncType::Full).await.unwrap();
    let ctx = fixture.context(&source_id, &run_id, SyncType::Full, true, Arc::default());

    let saved = checkpoint(Phase::Tickets, Some("c1"), vec![1, 3]);
    run_sync(&client(&server), &config(), Some(saved), ctx)
        .await
        .unwrap();

    let requests = server.received_requests().await.unwrap();
    assert!(
        !paths(&requests)
            .iter()
            .any(|p| p.starts_with("/help_center"))
    );
    let exports: Vec<_> = requests
        .iter()
        .filter(|r| r.url.path() == "/incremental/tickets/cursor.json")
        .collect();
    assert_eq!(exports.len(), 1);
    assert!(exports[0].url.query().unwrap().contains("cursor=c1"));

    let run = fixture.sync_run(&run_id).await.unwrap();
    let saved: ZendeskCheckpoint = serde_json::from_value(run.checkpoint.unwrap()).unwrap();
    assert_eq!(
        saved.cutoff, 1_717_000_000,
        "resume keeps the original cutoff"
    );
    assert_eq!(saved.tickets_cursor.as_deref(), Some("c2"));
    assert_eq!(saved.phase, Phase::Done);
}

struct CancelOnComments(Arc<AtomicBool>);

impl Respond for CancelOnComments {
    fn respond(&self, _: &Request) -> ResponseTemplate {
        self.0.store(true, Ordering::SeqCst);
        ResponseTemplate::new(200).set_body_json(comments_body())
    }
}

#[tokio::test]
async fn cancellation_mid_tickets_saves_checkpoint_and_cancels_run() {
    let server = MockServer::start().await;
    mock_people(&server).await;
    get(
        &server,
        "/help_center/articles.json",
        page("articles", json!([])),
    )
    .await;
    get(
        &server,
        "/incremental/tickets/cursor.json",
        export(
            json!([ticket(1001, "open"), ticket(1003, "open")]),
            "c1",
            false,
        ),
    )
    .await;
    let cancelled = Arc::new(AtomicBool::new(false));
    mock_comments(&server, CancelOnComments(cancelled.clone())).await;

    let fixture = Fixture::new().await.unwrap();
    let (source_id, run_id) = fixture.seed_run(SyncType::Full).await.unwrap();
    let ctx = fixture.context(&source_id, &run_id, SyncType::Full, false, cancelled);
    run_sync(&client(&server), &config(), None, ctx)
        .await
        .unwrap();

    let run = fixture.sync_run(&run_id).await.unwrap();
    assert_eq!(run.status, SyncStatus::Cancelled);
    let saved: ZendeskCheckpoint = serde_json::from_value(run.checkpoint.unwrap()).unwrap();
    assert_eq!(saved.phase, Phase::Tickets);
    assert_eq!(
        saved.tickets_cursor, None,
        "an unfinished page is not committed"
    );

    let events = fixture.events(&source_id).await.unwrap();
    doc(&events, ":ticket:1001");
    assert!(!events.iter().any(|e| {
        e["document_id"]
            .as_str()
            .is_some_and(|id| id.ends_with(":ticket:1003"))
    }));
}

#[tokio::test]
async fn comment_fetch_failure_falls_back_to_the_description() {
    let server = MockServer::start().await;
    mock_people(&server).await;
    get(
        &server,
        "/help_center/articles.json",
        page("articles", json!([])),
    )
    .await;
    get(
        &server,
        "/incremental/tickets/cursor.json",
        export(json!([ticket(1001, "open")]), "c1", true),
    )
    .await;
    mock_comments(&server, ResponseTemplate::new(404)).await;

    let fixture = Fixture::new().await.unwrap();
    let (source_id, run_id) = fixture.seed_run(SyncType::Full).await.unwrap();
    let ctx = fixture.context(&source_id, &run_id, SyncType::Full, false, Arc::default());
    run_sync(&client(&server), &config(), None, ctx)
        .await
        .unwrap();

    let events = fixture.events(&source_id).await.unwrap();
    let content = fixture
        .content(doc(&events, ":ticket:1001")["content_id"].as_str().unwrap())
        .await
        .unwrap();
    assert!(content.contains("## Description\nIt smokes."));
}

#[tokio::test]
async fn repeated_export_cursor_fails_the_run() {
    let server = MockServer::start().await;
    mock_people(&server).await;
    get(
        &server,
        "/help_center/articles.json",
        page("articles", json!([])),
    )
    .await;
    get(
        &server,
        "/incremental/tickets/cursor.json",
        export(json!([]), "same", false),
    )
    .await;

    let fixture = Fixture::new().await.unwrap();
    let (source_id, run_id) = fixture.seed_run(SyncType::Full).await.unwrap();
    let ctx = fixture.context(&source_id, &run_id, SyncType::Full, false, Arc::default());
    let error = run_sync(&client(&server), &config(), None, ctx)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("repeated a cursor"));
}

#[tokio::test]
async fn comment_fetch_forbidden_fails_the_run() {
    let server = MockServer::start().await;
    mock_people(&server).await;
    get(
        &server,
        "/help_center/articles.json",
        page("articles", json!([])),
    )
    .await;
    get(
        &server,
        "/incremental/tickets/cursor.json",
        export(json!([ticket(1001, "open")]), "c1", true),
    )
    .await;
    mock_comments(&server, ResponseTemplate::new(403)).await;

    let fixture = Fixture::new().await.unwrap();
    let (source_id, run_id) = fixture.seed_run(SyncType::Full).await.unwrap();
    let ctx = fixture.context(&source_id, &run_id, SyncType::Full, false, Arc::default());
    let error = run_sync(&client(&server), &config(), None, ctx)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("403"));
    let events = fixture.events(&source_id).await.unwrap();
    assert!(of_type(&events, "document_created").is_empty());
}

#[tokio::test]
async fn rejected_stored_cursor_restarts_the_export() {
    let server = MockServer::start().await;
    mock_people(&server).await;
    get(
        &server,
        "/help_center/articles.json",
        page("articles", json!([])),
    )
    .await;
    Mock::given(method("GET"))
        .and(path("/incremental/tickets/cursor.json"))
        .and(query_param("cursor", "c1"))
        .respond_with(ResponseTemplate::new(422))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/incremental/tickets/cursor.json"))
        .and(query_param("start_time", "0"))
        .respond_with(ResponseTemplate::new(200).set_body_json(export(
            json!([ticket(3001, "open")]),
            "c7",
            true,
        )))
        .mount(&server)
        .await;
    mock_comments(
        &server,
        ResponseTemplate::new(200).set_body_json(comments_body()),
    )
    .await;

    let fixture = Fixture::new().await.unwrap();
    let (source_id, run_id) = fixture.seed_run(SyncType::Incremental).await.unwrap();
    let ctx = fixture.context(
        &source_id,
        &run_id,
        SyncType::Incremental,
        false,
        Arc::default(),
    );
    let prior = checkpoint(Phase::Done, Some("c1"), vec![]);
    run_sync(&client(&server), &config(), Some(prior), ctx)
        .await
        .unwrap();

    let events = fixture.events(&source_id).await.unwrap();
    assert_eq!(doc(&events, ":ticket:3001")["type"], "document_updated");
    let run = fixture.sync_run(&run_id).await.unwrap();
    let saved: ZendeskCheckpoint = serde_json::from_value(run.checkpoint.unwrap()).unwrap();
    assert_eq!(saved.tickets_cursor.as_deref(), Some("c7"));
}
