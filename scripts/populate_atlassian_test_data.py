"""
One-shot helper to populate the Atlassian test instance with dummy content
for end-to-end connector testing.

Reads creds from .env.atlassian (ATLASSIAN_API_TOKEN). Cloud ID and site URL
are baked in; change the constants at the top if you target a different site.

Creates:
  - 4 Confluence pages in the first available space
  - 4 Jira issues in the first available project

If no space / project exists yet, prints instructions to create one via the UI.
The SA token must have product write scopes (write:page:confluence,
write:jira-work) AND the SA itself must be a member of (or have site-admin
role on) the space/project for writes to succeed.
"""

from __future__ import annotations

import os
import sys
from pathlib import Path
from typing import Any

import requests

SITE = "getomnico.atlassian.net"
CLOUD_ID = "c1ddb21f-9135-44c7-8db3-d4e504ae87a2"
ENV_FILE = Path(__file__).resolve().parents[1] / ".env.atlassian"
JIRA_BASE = f"https://api.atlassian.com/ex/jira/{CLOUD_ID}"
CONFLUENCE_BASE = f"https://api.atlassian.com/ex/confluence/{CLOUD_ID}/wiki"


def load_token() -> str:
    if not ENV_FILE.exists():
        sys.exit(f"missing env file: {ENV_FILE}")
    for line in ENV_FILE.read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        k, _, v = line.partition("=")
        if k.strip() == "ATLASSIAN_API_TOKEN":
            return v.strip().strip('"').strip("'")
    sys.exit("ATLASSIAN_API_TOKEN not found in env file")


def jget(session: requests.Session, path: str, **params: Any) -> dict:
    resp = session.get(f"{JIRA_BASE}{path}", params=params or None)
    resp.raise_for_status()
    return resp.json()


def jpost(session: requests.Session, path: str, body: dict) -> dict:
    resp = session.post(f"{JIRA_BASE}{path}", json=body)
    if not resp.ok:
        print(f"  jira POST {path} -> {resp.status_code}: {resp.text[:300]}")
        resp.raise_for_status()
    return resp.json() if resp.text else {}


def cget(session: requests.Session, path: str, **params: Any) -> dict:
    resp = session.get(f"{CONFLUENCE_BASE}{path}", params=params or None)
    resp.raise_for_status()
    return resp.json()


def cpost(session: requests.Session, path: str, body: dict) -> dict:
    resp = session.post(f"{CONFLUENCE_BASE}{path}", json=body)
    if not resp.ok:
        print(f"  confluence POST {path} -> {resp.status_code}: {resp.text[:300]}")
        resp.raise_for_status()
    return resp.json() if resp.text else {}


CONFLUENCE_PAGES = [
    (
        "Onboarding for new engineers",
        "<p>Welcome to the team. Set up your <strong>laptop</strong>, install the <em>development</em> stack, "
        "and join the engineering Slack channel. The on-call rotation guide is linked at the bottom of this page.</p>"
        "<p>Step one: clone the repo. Step two: read the architecture doc.</p>",
    ),
    (
        "Quarterly OKRs Q2 2026",
        "<p>This quarter we are focused on shipping the new search engine, "
        "improving connector reliability to 99.9%, and growing weekly active "
        "users by 20%.</p><ul><li>Search latency p95 under 250ms</li>"
        "<li>Connector uptime SLO: 99.9%</li><li>WAU growth: +20% QoQ</li></ul>",
    ),
    (
        "Architecture overview",
        "<p>Our system is composed of five core services: searcher, indexer, ai, "
        "connector-manager, and web. Each runs as a separate container. "
        "Postgres (ParadeDB) handles full-text search and vector storage.</p>",
    ),
    (
        "Hiring rubric",
        "<p>We evaluate candidates on four axes: technical depth, "
        "communication, ownership, and culture add. Each interviewer rates "
        "1-4 on each axis. Calibration is done weekly.</p>",
    ),
]


JIRA_ISSUES = [
    (
        "Investigate slow page load on dashboard",
        "Users report the analytics dashboard takes over 8 seconds to render. "
        "Profile the page, identify which queries are the long pole, and propose a fix.",
    ),
    (
        "Add CSV export to reports",
        "Customers want to export filtered report data to CSV. Add an Export button "
        "to the reports page that streams the current filter set as CSV.",
    ),
    (
        "Migrate to ParadeDB v0.13",
        "Bump the ParadeDB image to v0.13 and run the migration tests. "
        "Verify BM25 index behavior is unchanged.",
    ),
    (
        "Bug: notifications dropdown overlaps the chat panel",
        "When the notifications dropdown is open and a chat panel is visible, "
        "the two overlap on screens narrower than 1280px. Fix the z-index "
        "stacking and add a regression test.",
    ),
]


def populate_confluence(session: requests.Session) -> None:
    """Uses Confluence v1 REST. The classic OAuth scopes
    (`read:confluence-space.summary`, `write:confluence-content`) cover only
    v1; v2 endpoints require granular scopes (`read:space:confluence`,
    `write:page:confluence`). Sticking with v1 keeps the populate compatible
    with the same scope set we use for the connector test instance."""
    print("\n=== Confluence ===")
    try:
        result = cget(session, "/rest/api/space", limit=50)
    except requests.HTTPError as e:
        if e.response is not None and e.response.status_code == 404:
            print(
                "Confluence is unreachable for this service account. "
                "Make sure the SA has Confluence product access at "
                "admin.atlassian.com -> Service accounts -> omni-service-account "
                "-> Apps. Skipping Confluence."
            )
            return
        raise
    spaces = [s for s in result.get("results", []) if s.get("type") != "personal"]
    if not spaces:
        print(
            "No non-personal Confluence spaces found. Create a space via the "
            "UI first (any name will do):"
        )
        print(f"  https://{SITE}/wiki/")
        return
    space = spaces[0]
    print(f"Using space: {space.get('name')} (key={space.get('key')})")

    for title, html in CONFLUENCE_PAGES:
        body = {
            "type": "page",
            "title": title,
            "space": {"key": space["key"]},
            "body": {
                "storage": {
                    "value": html,
                    "representation": "storage",
                }
            },
        }
        try:
            page = cpost(session, "/rest/api/content", body)
            print(f"  + {title}  (id={page.get('id')})")
        except requests.HTTPError as e:
            print(f"  ! {title}  -> failed: {e}")


def populate_jira(session: requests.Session) -> None:
    print("\n=== Jira ===")
    projects = jget(session, "/rest/api/3/project/search", maxResults=10).get("values", [])
    if not projects:
        print("No Jira projects found. Create one via the UI first:")
        print(f"  https://{SITE}/jira/projects")
        return
    project = projects[0]
    project_key = project["key"]
    print(f"Using project: {project.get('name')} (key={project_key})")

    issuetypes = jget(session, f"/rest/api/3/project/{project_key}").get("issueTypes", [])
    if not issuetypes:
        print("  ! project has no issue types — skip")
        return
    pick = next(
        (
            t
            for t in issuetypes
            if t.get("name", "").lower() in ("task", "story", "bug")
        ),
        issuetypes[0],
    )
    issuetype_name = pick["name"]
    print(f"Using issue type: {issuetype_name}")

    for summary, description in JIRA_ISSUES:
        body = {
            "fields": {
                "project": {"key": project_key},
                "summary": summary,
                "issuetype": {"name": issuetype_name},
                "description": {
                    "type": "doc",
                    "version": 1,
                    "content": [
                        {
                            "type": "paragraph",
                            "content": [{"type": "text", "text": description}],
                        }
                    ],
                },
            }
        }
        try:
            issue = jpost(session, "/rest/api/3/issue", body)
            print(f"  + {summary}  (key={issue.get('key')})")
        except requests.HTTPError as e:
            print(f"  ! {summary}  -> failed: {e}")


def main() -> None:
    token = load_token()
    session = requests.Session()
    session.headers.update(
        {
            "Authorization": f"Bearer {token}",
            "Accept": "application/json",
            "Content-Type": "application/json",
        }
    )

    # Sanity check: confirm token is valid against the gateway.
    me = jget(session, "/rest/api/3/myself")
    print(f"Authenticated as: {me.get('displayName')} ({me.get('accountType')})")

    populate_confluence(session)
    populate_jira(session)


if __name__ == "__main__":
    main()
