use omni_zendesk_connector::client::{ZendeskClient, is_auth_rejection};
use omni_zendesk_connector::connector::validate_credentials;
use omni_zendesk_connector::credentials::ZendeskCredentials;
use serde_json::json;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client(server: &MockServer) -> ZendeskClient {
    ZendeskClient::with_base_url(
        server.uri(),
        ZendeskCredentials::ApiToken {
            email: "agent@example.com".to_string(),
            api_token: "tok".to_string(),
        },
    )
    .unwrap()
}

fn me(id: Option<u64>, role: &str) -> serde_json::Value {
    json!({"user": {"id": id, "email": "agent@example.com", "role": role}})
}

fn user(id: u64) -> serde_json::Value {
    json!({
        "id": id, "name": "Agent", "email": "agent@example.com", "role": "agent",
        "ticket_restriction": null, "active": true, "suspended": false
    })
}

#[tokio::test]
async fn sends_access_token_as_bearer_auth() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/users/me.json"))
        .and(header("authorization", "Bearer oauth-tok"))
        .respond_with(ResponseTemplate::new(200).set_body_json(me(Some(1), "admin")))
        .expect(1)
        .mount(&server)
        .await;
    let client = ZendeskClient::with_base_url(
        server.uri(),
        ZendeskCredentials::Bearer {
            access_token: "oauth-tok".to_string(),
        },
    )
    .unwrap();

    let result = validate_credentials(&client).await.unwrap();
    assert_eq!(result["authenticated"], true);
}

#[test]
fn parses_credential_kinds() {
    let bearer: ZendeskCredentials = serde_json::from_value(json!({"access_token": "x"})).unwrap();
    assert!(matches!(bearer, ZendeskCredentials::Bearer { .. }));
    let api: ZendeskCredentials =
        serde_json::from_value(json!({"email": "a@example.com", "api_token": "t"})).unwrap();
    assert!(matches!(api, ZendeskCredentials::ApiToken { .. }));
    assert!(serde_json::from_value::<ZendeskCredentials>(json!({})).is_err());
}

#[test]
fn rejects_blank_credentials_without_echoing_them() {
    let blank_secret = ZendeskCredentials::Bearer {
        access_token: "  ".to_string(),
    };
    let err = ZendeskClient::with_base_url("http://localhost".to_string(), blank_secret)
        .err()
        .unwrap()
        .to_string();
    assert!(err.contains("access_token"));

    let blank_email = ZendeskCredentials::ApiToken {
        email: " ".to_string(),
        api_token: "secret-value".to_string(),
    };
    let err = ZendeskClient::with_base_url("http://localhost".to_string(), blank_email)
        .err()
        .unwrap()
        .to_string();
    assert!(err.contains("email") && !err.contains("secret-value"));
}

#[tokio::test]
async fn sends_api_token_as_basic_auth() {
    let server = MockServer::start().await;
    // base64("agent@example.com/token:tok")
    Mock::given(method("GET"))
        .and(path("/users/me.json"))
        .and(header(
            "authorization",
            "Basic YWdlbnRAZXhhbXBsZS5jb20vdG9rZW46dG9r",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(me(Some(1), "admin")))
        .expect(1)
        .mount(&server)
        .await;

    let result = validate_credentials(&client(&server)).await.unwrap();
    assert_eq!(result["authenticated"], true);
    assert_eq!(result["role"], "admin");
}

#[tokio::test]
async fn only_admins_are_authenticated() {
    for (id, role, expected) in [
        (Some(1), "admin", true),
        (Some(2), "agent", false),
        (Some(3), "end-user", false),
        (None, "end-user", false),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/users/me.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(me(id, role)))
            .mount(&server)
            .await;
        let result = validate_credentials(&client(&server)).await.unwrap();
        assert_eq!(result["authenticated"], expected, "{role} {id:?}");
    }
}

#[tokio::test]
async fn rate_limit_is_retried_after_retry_after() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/users/me.json"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "0"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/users/me.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(me(Some(1), "agent")))
        .mount(&server)
        .await;

    let response = client(&server).me().await.unwrap();
    assert_eq!(response.user.id, Some(1));
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn unauthorized_is_permanent_and_hides_the_body() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/users/me.json"))
        .respond_with(ResponseTemplate::new(401).set_body_string("BODY-MARKER"))
        .mount(&server)
        .await;

    let error = client(&server).me().await.err().unwrap();
    assert!(is_auth_rejection(&error));
    let text = error.to_string();
    assert!(text.contains("GET /users/me.json") && text.contains("401"));
    assert!(!text.contains("BODY-MARKER"));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);

    let result = validate_credentials(&client(&server)).await.unwrap();
    assert_eq!(result["authenticated"], false);
}

#[tokio::test]
async fn other_client_errors_keep_a_bounded_body() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/users/me.json"))
        .respond_with(ResponseTemplate::new(400).set_body_string("x".repeat(1000)))
        .mount(&server)
        .await;
    let error = client(&server).me().await.err().unwrap().to_string();
    assert!(error.contains("400") && error.len() < 300);
}

#[tokio::test]
async fn cursor_pagination_follows_after_cursor_not_server_urls() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/users.json"))
        .and(query_param("page[after]", "c1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "users": [user(2)],
            "meta": {"has_more": false, "after_cursor": null}
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/users.json"))
        .and(query_param("page[size]", "100"))
        .and(query_param("role[]", "admin"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "users": [user(1)],
            "meta": {"has_more": true, "after_cursor": "c1"},
            "links": {"next": "http://evil.invalid/api/v2/users.json?page[after]=c1"}
        })))
        .mount(&server)
        .await;

    let users = client(&server).agents_and_admins().await.unwrap();
    assert_eq!(users.iter().map(|u| u.id).collect::<Vec<_>>(), vec![1, 2]);
}

#[tokio::test]
async fn repeated_cursor_fails_instead_of_looping() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/users.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "users": [user(1)],
            "meta": {"has_more": true, "after_cursor": "same"}
        })))
        .mount(&server)
        .await;

    let error = client(&server).agents_and_admins().await.err().unwrap();
    assert!(error.to_string().contains("repeated a pagination cursor"));
}

#[tokio::test]
async fn redirects_are_not_followed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/users/me.json"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("Location", "http://evil.invalid/steal"),
        )
        .mount(&server)
        .await;
    let error = client(&server).me().await.err().unwrap();
    assert!(error.to_string().contains("302"));
}

#[tokio::test]
async fn missing_required_fields_fail() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/users.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "users": [{"id": 1, "role": "agent"}],
            "meta": {"has_more": false, "after_cursor": null}
        })))
        .mount(&server)
        .await;
    assert!(client(&server).agents_and_admins().await.is_err());
}
