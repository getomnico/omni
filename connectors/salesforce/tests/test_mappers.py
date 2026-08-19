"""Unit tests: mappers, permissions, checkpoint models, and SOQL builders."""

from __future__ import annotations

import pytest
from omni_connector import DocumentPermissions

from salesforce_connector.config import schema_fingerprint
from salesforce_connector.mappers import (
    attributes_for,
    generate_content,
    map_record_to_document,
)
from salesforce_connector.models import (
    AccountRecord,
    CaseRecord,
    ContactRecord,
    GroupMemberRecord,
    GroupRecord,
    OpportunityRecord,
    RecordCursor,
    RoleRecord,
    SalesforceCheckpoint,
    UserRecord,
    group_email,
    role_email,
)
from salesforce_connector.pagination import delta_scan_soql, full_scan_soql
from salesforce_connector.permissions import (
    SalesforceDirectory,
    build_directory,
)
from tests.conftest import _account_payload, _case_payload, _contact_payload


def _parse_account() -> AccountRecord:
    return AccountRecord.from_record(_account_payload())


class TestRecordParsing:
    def test_account_parsing(self) -> None:
        record = _parse_account()
        assert record.id == "001000000000001"
        assert record.name == "Acme Corp"
        assert record.industry == "Technology"
        assert record.annual_revenue == 1000000
        assert record.owner_id == "005000000000001"
        assert record.created_date is not None
        assert record.system_modstamp is not None

    def test_contact_parses_nested_account(self) -> None:
        record = ContactRecord.from_record(_contact_payload())
        assert record.account_name == "Acme Corp"
        assert record.account_id == "001000000000001"

    def test_missing_id_fails_loudly(self) -> None:
        with pytest.raises(ValueError):
            AccountRecord.from_record({"Name": "No Id"})


class TestAttributes:
    def test_account_attributes(self) -> None:
        record = _parse_account()
        attrs = attributes_for("Account", record, owner_email="owner@example.com")
        assert attrs["source_type"] == "salesforce"
        assert attrs["object_type"] == "Account"
        assert attrs["salesforce_id"] == "001000000000001"
        assert attrs["owner_email"] == "owner@example.com"
        assert attrs["industry"] == "Technology"
        assert attrs["type"] == "Customer"
        assert attrs["account_name"] == "Acme Corp"
        assert attrs["billing_country"] == "US"
        assert attrs["annual_revenue"] == 1000000
        assert attrs["created_date"] is not None

    def test_case_attributes_include_operator_keys(self) -> None:
        record = CaseRecord.from_record(_case_payload())
        attrs = attributes_for("Case", record, owner_email=None)
        assert attrs["status"] == "New"
        assert attrs["priority"] == "High"
        assert attrs["account_name"] == "Acme Corp"
        assert "owner_email" not in attrs  # None values are dropped

    def test_opportunity_attributes(self) -> None:
        record = OpportunityRecord.from_record(
            {
                "Id": "006000000000001",
                "Name": "Big Deal",
                "StageName": "Prospecting",
                "Amount": 50000,
                "SystemModstamp": "2024-06-10T11:00:00.000+0000",
            }
        )
        attrs = attributes_for("Opportunity", record, owner_email=None)
        assert attrs["stage"] == "Prospecting"
        assert attrs["amount"] == 50000


class TestContent:
    def test_content_includes_title_and_fields(self) -> None:
        record = _parse_account()
        content = generate_content("Account", record)
        assert "Salesforce Account" in content
        assert "Title: Acme Corp" in content
        assert "Industry: Technology" in content
        assert "Annual Revenue: 1000000" in content
        # Structural fields are not dumped into content.
        assert "OwnerId" not in content
        assert "SystemModstamp" not in content


class TestDocumentMapping:
    def test_document_shape(self) -> None:
        record = _parse_account()
        doc = map_record_to_document(
            object_type="Account",
            record=record,
            content_id="content-1",
            instance_url="https://acme.my.salesforce.com",
            owner_email="owner@example.com",
            permissions=DocumentPermissions(
                public=False, users=["owner@example.com"], groups=["g1"]
            ),
            attributes={"object_type": "Account", "owner_email": "owner@example.com"},
        )
        assert doc.external_id == "Account:001000000000001"
        assert doc.title == "Acme Corp"
        assert doc.metadata is not None
        assert doc.metadata.url == "https://acme.my.salesforce.com/001000000000001"
        assert doc.metadata.author == "owner@example.com"
        assert doc.permissions is not None
        assert doc.permissions.users == ["owner@example.com"]
        assert doc.attributes is not None
        assert doc.attributes["owner_email"] == "owner@example.com"


class TestPermissions:
    def _directory(self) -> SalesforceDirectory:
        users = [
            UserRecord.from_record(
                {
                    "Id": "005000000000001",
                    "Name": "Rep",
                    "Email": "rep@example.com",
                    "UserRoleId": "00E000000000001",
                    "IsActive": True,
                }
            ),
            UserRecord.from_record(
                {
                    "Id": "005000000000002",
                    "Name": "Manager",
                    "Email": "manager@example.com",
                    "UserRoleId": "00E000000000002",
                    "IsActive": True,
                }
            ),
            UserRecord.from_record(
                {
                    "Id": "005000000000003",
                    "Name": "Peer",
                    "Email": "peer@example.com",
                    "UserRoleId": "00E000000000003",
                    "IsActive": True,
                }
            ),
        ]
        groups = [
            GroupRecord.from_record({"Id": "00G000000000001", "Name": "Queue", "Type": "Queue"}),
            GroupRecord.from_record({"Id": "00G000000000002", "Name": "Execs", "Type": "Public"}),
        ]
        members = [
            GroupMemberRecord.from_record(
                {"Id": "m1", "GroupId": "00G000000000001", "UserOrGroupId": "005000000000001"}
            ),
            # Execs group contains the manager and the rep's role.
            GroupMemberRecord.from_record(
                {"Id": "m2", "GroupId": "00G000000000002", "UserOrGroupId": "005000000000002"}
            ),
            GroupMemberRecord.from_record(
                {"Id": "m3", "GroupId": "00G000000000002", "UserOrGroupId": "00E000000000002"}
            ),
        ]
        roles = [
            RoleRecord.from_record(
                {"Id": "00E000000000001", "Name": "Rep", "ParentRoleId": "00E000000000002"}
            ),
            RoleRecord.from_record(
                {"Id": "00E000000000002", "Name": "Manager", "ParentRoleId": "00E000000000003"}
            ),
            RoleRecord.from_record({"Id": "00E000000000003", "Name": "VP"}),
        ]
        return build_directory(users, groups, members, roles)

    def test_owner_grants_user_and_hierarchy(self) -> None:
        directory = self._directory()
        grants = directory.owner_grants("005000000000001")
        assert grants.users == ("rep@example.com",)
        # Hierarchy chain: Rep role -> Manager role -> VP role.
        assert grants.groups == (
            role_email("00E000000000001"),
            role_email("00E000000000002"),
            role_email("00E000000000003"),
        )

    def test_owner_grants_without_hierarchy(self) -> None:
        directory = self._directory()
        grants = directory.owner_grants("005000000000001", include_hierarchy=False)
        assert grants.groups == ()

    def test_queue_owner_grants_queue_group(self) -> None:
        directory = self._directory()
        grants = directory.owner_grants("00G000000000001")
        assert grants.groups == (group_email("00G000000000001"),)
        assert grants.users == ()

    def test_group_membership_expansion_includes_direct_role_members(self) -> None:
        directory = self._directory()
        members = directory.group_member_emails("00G000000000002")
        # The Execs group holds the manager directly and the Manager role; a
        # role in a group contributes its direct members (rep is in the child
        # Rep role and is not a direct member of the Manager role).
        assert members == {"manager@example.com"}

    def test_role_group_includes_descendants(self) -> None:
        directory = self._directory()
        # Manager role: manager + rep (Rep role reports to Manager).
        assert directory.role_and_descendants("00E000000000002") == {
            "manager@example.com",
            "rep@example.com",
        }

    def test_share_grants_resolve_users_groups_and_roles(self) -> None:
        directory = self._directory()
        from salesforce_connector.models import ShareRecord

        shares = [
            ShareRecord.from_record(
                {
                    "Id": "s1",
                    "AccountId": "001000000000001",
                    "UserOrGroupId": "005000000000003",
                    "AccessLevel": "Read",
                    "RowCause": "Manual",
                },
                "AccountId",
            ),
            ShareRecord.from_record(
                {
                    "Id": "s2",
                    "AccountId": "001000000000001",
                    "UserOrGroupId": "00G000000000002",
                    "AccessLevel": "Edit",
                    "RowCause": "Rule",
                },
                "AccountId",
            ),
            ShareRecord.from_record(
                {
                    "Id": "s3",
                    "AccountId": "001000000000001",
                    "UserOrGroupId": "00E000000000003",
                    "AccessLevel": "Read",
                    "RowCause": "Manual",
                },
                "AccountId",
            ),
        ]
        grants = directory.share_grants(shares)
        assert "peer@example.com" in grants.users
        assert group_email("00G000000000002") in grants.groups
        # Role shares grant the role and everything below it.
        assert role_email("00E000000000003") in grants.groups

    def test_none_access_shares_are_ignored(self) -> None:
        directory = self._directory()
        from salesforce_connector.models import ShareRecord

        share = ShareRecord.from_record(
            {
                "Id": "s1",
                "AccountId": "001000000000001",
                "UserOrGroupId": "005000000000003",
                "AccessLevel": "None",
                "RowCause": "Manual",
            },
            "AccountId",
        )
        grants = directory.share_grants([share])
        assert grants.users == ()
        assert grants.groups == ()


class TestCheckpoint:
    def test_round_trip(self) -> None:
        checkpoint = SalesforceCheckpoint(
            records_synced={"Account": True},
            record_cursors={
                "Contact": RecordCursor(
                    last_id="003000000000001",
                    last_system_modstamp="2024-01-01T00:00:00+00:00",
                )
            },
            watermarks={"Account": "2024-01-01T00:00:00+00:00"},
            deleted_through="2024-01-02T00:00:00+00:00",
        )
        restored = SalesforceCheckpoint.from_mapping(checkpoint.to_json())
        assert restored.records_synced == {"Account": True}
        assert restored.watermarks == checkpoint.watermarks
        assert restored.deleted_through == "2024-01-02T00:00:00+00:00"
        assert restored.record_cursors["Contact"].last_id == "003000000000001"

    def test_version_mismatch_returns_fresh(self) -> None:
        restored = SalesforceCheckpoint.from_mapping(
            {"version": 999, "watermarks": {"Account": "x"}}
        )
        assert restored.watermarks == {}

    def test_from_none(self) -> None:
        assert SalesforceCheckpoint.from_mapping(None).version == 1


class TestSoqlBuilders:
    def test_full_scan(self) -> None:
        soql = full_scan_soql("Account", ("Id", "Name"), None)
        assert soql == "SELECT Id, Name FROM Account ORDER BY Id LIMIT 2000"

    def test_full_scan_resume(self) -> None:
        soql = full_scan_soql(
            "Account",
            ("Id", "Name"),
            RecordCursor(last_id="001000000000005"),
        )
        assert "WHERE Id > '001000000000005'" in soql

    def test_delta_scan(self) -> None:
        soql = delta_scan_soql("Account", ("Id", "Name"), None, "2024-01-01T00:00:00+00:00")
        assert "SystemModstamp >= 2024-01-01T00:00:00+00:00" in soql
        assert "ORDER BY SystemModstamp ASC, Id ASC" in soql

    def test_delta_scan_resume(self) -> None:
        soql = delta_scan_soql(
            "Account",
            ("Id", "Name"),
            RecordCursor(
                last_id="001000000000005",
                last_system_modstamp="2024-01-01T00:00:00+00:00",
            ),
            "2024-01-01T00:00:00+00:00",
        )
        assert "(SystemModstamp > 2024-01-01T00:00:00+00:00" in soql
        assert "Id > '001000000000005'" in soql


class TestSchemaFingerprint:
    def test_changes_with_config(self) -> None:
        fp1 = schema_fingerprint(frozenset(), frozenset())
        fp2 = schema_fingerprint(frozenset({"Account"}), frozenset())
        fp3 = schema_fingerprint(frozenset({"Account"}), frozenset({"Account"}))
        assert fp1 != fp2
        assert fp2 != fp3
        assert len(fp1) == 64
