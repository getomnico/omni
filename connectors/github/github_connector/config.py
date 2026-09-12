"""Configuration constants for GitHub connector."""

MAX_COMMENT_COUNT = 100
MAX_CONTENT_LENGTH = 100_000
ITEMS_PER_PAGE = 100
CHECKPOINT_INTERVAL = 50

# Toolsets exposed through the bundled github-mcp-server. Deliberately
# curated instead of "all": gists, notifications, projects, orgs, dependabot,
# code/secret security, advisories, stargazers and copilot toolsets are either
# not useful for Omni agents or broaden the risk surface without a matching
# Omni use case. Actions is included for CI-triage workflows; write tools
# inside every toolset still require chat approval via mode=write.
MCP_TOOLSETS = [
    "context",
    "repos",
    "git",
    "issues",
    "labels",
    "pull_requests",
    "discussions",
    "actions",
    "users",
]

GITHUB_MCP_COMMAND = "github-mcp-server"

DISCUSSIONS_QUERY = """
query($owner: String!, $name: String!, $cursor: String) {
  repository(owner: $owner, name: $name) {
    discussions(first: 100, after: $cursor, orderBy: {field: UPDATED_AT, direction: DESC}) {
      pageInfo {
        hasNextPage
        endCursor
      }
      nodes {
        number
        title
        body
        url
        createdAt
        updatedAt
        author { login }
        category { name }
        answerChosenAt
        labels(first: 10) { nodes { name } }
        comments(first: 100) {
          nodes {
            body
            createdAt
            author { login }
          }
        }
      }
    }
  }
}
"""
