"""Configuration constants for GitHub connector."""

MAX_COMMENT_COUNT = 100
MAX_CONTENT_LENGTH = 100_000
ITEMS_PER_PAGE = 100
CHECKPOINT_INTERVAL = 50

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
