"""Unit tests for document permission checking."""

from tools.permissions import check_document_access


class TestCheckDocumentAccess:
    def test_public_document_accessible_to_anyone(self):
        perms = {"public": True, "users": [], "groups": []}
        assert check_document_access(perms, "alice@example.com") is True

    def test_public_document_accessible_with_no_email(self):
        perms = {"public": True, "users": [], "groups": []}
        assert check_document_access(perms, None) is True

    def test_user_in_users_list(self):
        perms = {"public": False, "users": ["alice@example.com"], "groups": []}
        assert check_document_access(perms, "alice@example.com") is True

    def test_user_not_in_users_list(self):
        perms = {"public": False, "users": ["alice@example.com"], "groups": []}
        assert check_document_access(perms, "bob@example.com") is False

    def test_user_in_groups_list(self):
        perms = {"public": False, "users": [], "groups": ["eng@example.com"]}
        assert check_document_access(perms, "eng@example.com") is True

    def test_user_not_in_groups_list(self):
        perms = {"public": False, "users": [], "groups": ["eng@example.com"]}
        assert check_document_access(perms, "sales@example.com") is False

    def test_case_insensitive_users(self):
        perms = {"public": False, "users": ["Alice@Example.COM"], "groups": []}
        assert check_document_access(perms, "alice@example.com") is True

    def test_case_insensitive_groups(self):
        perms = {"public": False, "users": [], "groups": ["ENG@Example.com"]}
        assert check_document_access(perms, "eng@example.com") is True

    def test_none_permissions_denied(self):
        assert check_document_access(None, "alice@example.com") is False

    def test_empty_permissions_denied(self):
        assert check_document_access({}, "alice@example.com") is False

    def test_no_email_non_public_denied(self):
        perms = {"public": False, "users": ["alice@example.com"], "groups": []}
        assert check_document_access(perms, None) is False

    def test_multiple_users(self):
        perms = {
            "public": False,
            "users": ["alice@example.com", "bob@example.com"],
            "groups": [],
        }
        assert check_document_access(perms, "bob@example.com") is True

    def test_missing_public_key_treated_as_false(self):
        perms = {"users": ["alice@example.com"], "groups": []}
        assert check_document_access(perms, "alice@example.com") is True
        assert check_document_access(perms, "bob@example.com") is False
