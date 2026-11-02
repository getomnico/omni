use std::collections::BTreeSet;

use omni_zendesk_connector::models::{Article, GroupMembership, User};
use omni_zendesk_connector::sync::{Directory, article_permissions, ticket_permissions};
use serde_json::json;

fn user(id: u64, role: &str, restriction: Option<&str>, active: bool, suspended: bool) -> User {
    serde_json::from_value(json!({
        "id": id, "name": format!("User {id}"), "email": format!("User{id}@Example.com"),
        "role": role, "ticket_restriction": restriction, "active": active, "suspended": suspended
    }))
    .unwrap()
}

fn membership(user_id: u64, group_id: u64) -> GroupMembership {
    GroupMembership { user_id, group_id }
}

fn emails(ids: &[u64]) -> BTreeSet<String> {
    ids.iter()
        .map(|id| format!("user{id}@example.com"))
        .collect()
}

#[test]
fn agents_group_holds_admins_and_unrestricted_agents_only() {
    let users = [
        user(1, "admin", None, true, false),
        user(2, "agent", None, true, false),
        user(3, "agent", Some("groups"), true, false),
        user(4, "agent", Some("assigned"), true, false),
        user(5, "agent", Some("requested"), true, false),
        user(6, "agent", Some("organization"), true, false),
        user(7, "agent", None, true, true),
        user(8, "agent", None, false, false),
        user(9, "end-user", None, true, false),
    ];
    let directory = Directory::build(&users, &[]);
    assert_eq!(directory.agents, emails(&[1, 2]));
}

#[test]
fn users_without_an_email_are_skipped() {
    let mut no_email = user(1, "admin", None, true, false);
    no_email.email = None;
    assert!(Directory::build(&[no_email], &[]).agents.is_empty());
}

#[test]
fn group_members_are_only_active_groups_restricted_agents() {
    let users = [
        user(1, "admin", None, true, false),
        user(3, "agent", Some("groups"), true, false),
        user(4, "agent", Some("groups"), true, false),
        user(5, "agent", Some("groups"), true, true),
        user(6, "agent", Some("assigned"), true, false),
        user(9, "end-user", Some("groups"), true, false),
    ];
    let memberships = [
        membership(1, 10),
        membership(3, 10),
        membership(4, 11),
        membership(5, 10),
        membership(6, 10),
        membership(9, 10),
    ];
    let directory = Directory::build(&users, &memberships);
    assert_eq!(directory.group_members[&10], emails(&[3]));
    assert_eq!(directory.group_members[&11], emails(&[4]));
}

#[test]
fn ticket_permissions_are_agents_plus_the_ticket_group() {
    let with_group = ticket_permissions("src", Some(10));
    assert!(!with_group.public && with_group.users.is_empty());
    assert_eq!(
        with_group.groups,
        vec!["zendesk:src:agents", "zendesk:src:group:10"]
    );
    assert_eq!(
        ticket_permissions("src", None).groups,
        vec!["zendesk:src:agents"]
    );
}

fn article(segment: Option<u64>, segments: &[u64]) -> Article {
    serde_json::from_value(json!({
        "id": 1, "title": "T", "body": "<p>b</p>", "html_url": "https://example.zendesk.com/hc/1",
        "locale": "en-us", "draft": false, "user_segment_id": segment,
        "user_segment_ids": segments, "created_at": "2024-01-01T00:00:00Z",
        "updated_at": "2024-01-01T00:00:00Z"
    }))
    .unwrap()
}

#[test]
fn article_is_public_only_without_any_user_segment() {
    assert!(article_permissions("src", &article(None, &[])).public);

    let segment = article_permissions("src", &article(Some(5), &[]));
    assert!(!segment.public);
    assert_eq!(segment.groups, vec!["zendesk:src:agents"]);

    // Enterprise plans may restrict through user_segment_ids alone.
    assert!(!article_permissions("src", &article(None, &[7, 8])).public);
}
