"""Document permission checking utilities."""

from __future__ import annotations


def check_document_access(permissions: dict | None, user_email: str | None) -> bool:
    """Check if a user has access to a document based on its permissions JSONB.

    Mirrors the searcher's BM25 permission filter logic:
        permissions @@@ 'public:true'
        OR permissions @@@ 'users:{email}'
        OR permissions @@@ 'groups:{email}'

    Fail-closed: if permissions is missing/empty, access is denied.
    """
    if not permissions:
        return False

    if permissions.get("public", False):
        return True

    if not user_email:
        return False

    email_lower = user_email.lower()

    users = permissions.get("users", [])
    if any(u.lower() == email_lower for u in users):
        return True

    groups = permissions.get("groups", [])
    if any(g.lower() == email_lower for g in groups):
        return True

    return False
