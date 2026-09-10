"""Unit tests: mappers, permissions, checkpoint models, and SOQL builders."""

from __future__ import annotations

from datetime import UTC, datetime

import pytest
from omni_connector import DocumentPermissions

from salesforce_connector.config import SyncRunMode, schema_fingerprint
from salesforce_connector.mappers import (
    attributes_for,
    generate_content,
    map_record_to_document,
)
from salesforce_connector.models import (
    AccessLevel,
    AccountRecord,
    CaseRecord,
    ContactRecord,
    GroupMemberRecord,
    GroupRecord,
    ObjectState,
    OpportunityRecord,
    PeopleState,
    RecordCursor,
    RoleRecord,
    RunProgress,
    SalesforceCheckpoint,
    ShareSnapshot,
    UserRecord,
    direct_role_email,
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


def _dt(value: str) -> datetime:
    return datetime.fromisoformat(value).astimezone(UTC)


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

    def test_owner_grants_user_and_ancestor_roles_only(self) -> None:
        directory = self._directory()
        grants = directory.owner_grants("005000000000001")
        assert grants.users == ("rep@example.com",)
        # Hierarchy grants go to strict ancestor roles only: the owner's own
        # role (peers) and subordinate roles must never receive access.
        assert grants.groups == (
            direct_role_email("00E000000000002"),
            direct_role_email("00E000000000003"),
        )
        assert direct_role_email("00E000000000001") not in grants.groups

    def test_owner_peer_does_not_receive_hierarchy_access(self) -> None:
        directory = self._directory()
        grants = directory.owner_grants("005000000000001")
        # The owner's own role group must not be granted, so peers never see
        # this record through the hierarchy.
        assert direct_role_email("00E000000000001") not in grants.groups
        assert directory.direct_role_members("00E000000000001") == {"rep@example.com"}

    def test_sibling_branch_does_not_receive_hierarchy_access(self) -> None:
        directory = self._directory()
        # Add a sibling role under the VP with its own member.
        directory.roles_by_id["00E000000000004"] = RoleRecord.from_record(
            {"Id": "00E000000000004", "Name": "Ops", "ParentRoleId": "00E000000000003"}
        )
        directory.users_by_id["005000000000004"] = UserRecord.from_record(
            {
                "Id": "005000000000004",
                "Email": "ops@example.com",
                "UserRoleId": "00E000000000004",
                "IsActive": True,
            }
        )
        directory.users_by_role["00E000000000004"] = [directory.users_by_id["005000000000004"]]
        groups = directory.owner_grants("005000000000001").groups
        assert direct_role_email("00E000000000004") not in groups
        # ops is a member of the sibling role only, and the owner's ancestor
        # chain never reaches the sibling branch.
        assert directory.direct_role_members("00E000000000004") == {"ops@example.com"}
        assert "ops@example.com" not in directory.direct_role_members("00E000000000003")

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

    def test_direct_role_group_excludes_descendants(self) -> None:
        directory = self._directory()
        assert directory.direct_role_members("00E000000000002") == {"manager@example.com"}

    def test_salesforce_role_group_resolves_through_related_id(self) -> None:
        directory = self._directory()
        directory.groups_by_id["00G000000000009"] = GroupRecord.from_record(
            {
                "Id": "00G000000000009",
                "Name": "Manager and Subordinates",
                "Type": "RoleAndSubordinates",
                "RelatedId": "00E000000000002",
            }
        )
        assert directory.group_member_emails("00G000000000009") == {
            "manager@example.com",
            "rep@example.com",
        }

    def test_group_cycle_is_safe(self) -> None:
        directory = SalesforceDirectory()
        directory.groups_by_id["00G00000000000A"] = GroupRecord.from_record(
            {"Id": "00G00000000000A", "Name": "A", "Type": "Public"}
        )
        directory.groups_by_id["00G00000000000B"] = GroupRecord.from_record(
            {"Id": "00G00000000000B", "Name": "B", "Type": "Public"}
        )
        directory.group_members_by_id["00G00000000000A"] = [
            GroupMemberRecord.from_record(
                {"Id": "m1", "GroupId": "00G00000000000A", "UserOrGroupId": "00G00000000000B"}
            )
        ]
        directory.group_members_by_id["00G00000000000B"] = [
            GroupMemberRecord.from_record(
                {"Id": "m2", "GroupId": "00G00000000000B", "UserOrGroupId": "00G00000000000A"}
            )
        ]
        assert directory.group_member_emails("00G00000000000A") == set()

    def test_role_cycle_is_safe(self) -> None:
        directory = SalesforceDirectory()
        directory.roles_by_id["00E00000000000A"] = RoleRecord.from_record(
            {"Id": "00E00000000000A", "Name": "A", "ParentRoleId": "00E00000000000B"}
        )
        directory.roles_by_id["00E00000000000B"] = RoleRecord.from_record(
            {"Id": "00E00000000000B", "Name": "B", "ParentRoleId": "00E00000000000A"}
        )
        directory.users_by_id["00500000000000A"] = UserRecord.from_record(
            {
                "Id": "00500000000000A",
                "Email": "a@example.com",
                "UserRoleId": "00E00000000000A",
                "IsActive": True,
            }
        )
        directory.users_by_role["00E00000000000A"] = [directory.users_by_id["00500000000000A"]]
        assert directory._manager_roles("00E00000000000A") == ["00E00000000000B"]
        assert directory.role_and_descendants("00E00000000000A") == {"a@example.com"}

    def test_share_grants_resolve_users_groups_and_roles(self) -> None:
        directory = self._directory()
        from salesforce_connector.models import ShareRecord

        shares = [
            ShareRecord.from_record(
                {
                    "Id": "s1",
                    "AccountId": "001000000000001",
                    "UserOrGroupId": "005000000000003",
                    "AccountAccessLevel": "Read",
                    "RowCause": "Manual",
                },
                "AccountId",
                "AccountAccessLevel",
            ),
            ShareRecord.from_record(
                {
                    "Id": "s2",
                    "AccountId": "001000000000001",
                    "UserOrGroupId": "00G000000000002",
                    "AccountAccessLevel": "Edit",
                    "RowCause": "Rule",
                },
                "AccountId",
                "AccountAccessLevel",
            ),
            ShareRecord.from_record(
                {
                    "Id": "s3",
                    "AccountId": "001000000000001",
                    "UserOrGroupId": "00E000000000003",
                    "AccountAccessLevel": "Read",
                    "RowCause": "Manual",
                },
                "AccountId",
                "AccountAccessLevel",
            ),
        ]
        grants = directory.share_grants(shares)
        assert "peer@example.com" in grants.users
        assert group_email("00G000000000002") in grants.groups
        # Role shares grant the role and everything below it.
        assert role_email("00E000000000003") in grants.groups

    @pytest.mark.parametrize("access_level", ["None"])
    def test_explicit_no_access_shares_are_ignored(self, access_level: str) -> None:
        directory = self._directory()
        from salesforce_connector.models import ShareRecord

        share = ShareRecord.from_record(
            {
                "Id": "s1",
                "AccountId": "001000000000001",
                "UserOrGroupId": "005000000000003",
                "AccountAccessLevel": access_level,
                "RowCause": "Manual",
            },
            "AccountId",
            "AccountAccessLevel",
        )
        grants = directory.share_grants([share])
        assert grants.users == ()
        assert grants.groups == ()

    @pytest.mark.parametrize("access_level", ["Read", "Edit", "All", "ReadWrite"])
    def test_any_non_no_access_level_grants(self, access_level: str) -> None:
        directory = self._directory()
        from salesforce_connector.models import ShareRecord

        share = ShareRecord.from_record(
            {
                "Id": "s1",
                "AccountId": "001000000000001",
                "UserOrGroupId": "005000000000003",
                "AccountAccessLevel": access_level,
                "RowCause": "Manual",
            },
            "AccountId",
            "AccountAccessLevel",
        )
        assert directory.share_grants([share]).users == ("peer@example.com",)

    def test_access_level_none_is_not_a_grant(self) -> None:
        assert AccessLevel.parse("None") is not None
        assert AccessLevel.parse("None").grants_access is False
        assert AccessLevel.parse("Read").grants_access is True


class TestCheckpoint:
    def test_round_trip(self) -> None:
        checkpoint = SalesforceCheckpoint(
            objects={
                "Account": ObjectState(
                    watermark="2024-01-01T00:00:00+00:00",
                    deletion_through="2024-01-02T00:00:00+00:00",
                )
            },
            progress=RunProgress(
                run_id="run-1",
                mode=SyncRunMode.INCREMENTAL,
                window_end="2024-01-03T00:00:00+00:00",
                started_at="2024-01-03T00:00:00+00:00",
                current_object="Contact",
                record_cursor=RecordCursor(
                    last_id="003000000000001",
                    last_system_modstamp="2024-01-01T00:00:00+00:00",
                ),
                records_completed=("Account",),
            ),
            people=PeopleState(
                user_fingerprints={"005000000000001": "fp"},
                active_emails=frozenset({"owner@example.com"}),
                group_emails=frozenset({"g@salesforce.groups"}),
                memberships={"g@salesforce.groups": ("owner@example.com",)},
            ),
            share_snapshot=ShareSnapshot(
                grants={"AccountShare": {"001000000000001": "fp"}},
                captured_at="2024-01-03T00:00:00+00:00",
            ),
        )
        restored = SalesforceCheckpoint.from_mapping(checkpoint.to_json())
        assert restored.objects["Account"].watermark == "2024-01-01T00:00:00+00:00"
        assert restored.objects["Account"].deletion_through == "2024-01-02T00:00:00+00:00"
        assert restored.progress is not None
        assert restored.progress.run_id == "run-1"
        assert restored.progress.records_completed == ("Account",)
        assert restored.progress.record_cursor is not None
        assert restored.progress.record_cursor.last_id == "003000000000001"
        assert restored.people is not None
        assert restored.people.active_emails == frozenset({"owner@example.com"})
        assert restored.share_snapshot is not None
        assert restored.share_snapshot.grants["AccountShare"]["001000000000001"] == "fp"

    def test_legacy_version_is_discarded(self) -> None:
        restored = SalesforceCheckpoint.from_mapping(
            {"version": 1, "watermarks": {"Account": "x"}}
        )
        assert restored.objects == {}
        assert restored.progress is None

    def test_without_progress_strips_run_state(self) -> None:
        checkpoint = SalesforceCheckpoint(
            objects={"Account": ObjectState(watermark="2024-01-01T00:00:00+00:00")},
            progress=RunProgress(
                run_id="run-1",
                mode=SyncRunMode.FULL,
                window_end="2024-01-03T00:00:00+00:00",
                started_at="2024-01-03T00:00:00+00:00",
            ),
        )
        stripped = checkpoint.without_progress()
        assert stripped.progress is None
        assert stripped.objects["Account"].watermark == "2024-01-01T00:00:00+00:00"

    def test_from_none(self) -> None:
        assert SalesforceCheckpoint.from_mapping(None).version == 2

    def test_delta_cursor_requires_both_components(self) -> None:
        assert RecordCursor(last_id="a").is_delta_ready is False
        assert RecordCursor(last_system_modstamp="t").is_delta_ready is False
        assert RecordCursor(last_id="a", last_system_modstamp="t").is_delta_ready is True


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

    def test_delta_scan_is_bounded(self) -> None:
        soql = delta_scan_soql(
            "Account",
            ("Id", "Name"),
            None,
            _dt("2024-01-01T00:00:00+00:00"),
            _dt("2024-01-02T00:00:00+00:00"),
        )
        assert "SystemModstamp >= 2024-01-01T00:00:00Z" in soql
        assert "SystemModstamp <= 2024-01-02T00:00:00Z" in soql
        assert "ORDER BY SystemModstamp ASC, Id ASC" in soql

    def test_delta_scan_resume(self) -> None:
        soql = delta_scan_soql(
            "Account",
            ("Id", "Name"),
            RecordCursor(
                last_id="001000000000005",
                last_system_modstamp="2024-01-01T00:00:00Z",
            ),
            _dt("2024-01-01T00:00:00+00:00"),
            _dt("2024-01-02T00:00:00+00:00"),
        )
        assert "(SystemModstamp > 2024-01-01T00:00:00Z" in soql
        assert "Id > '001000000000005'" in soql

    def test_partial_delta_cursor_restarts_window(self) -> None:
        soql = delta_scan_soql(
            "Account",
            ("Id", "Name"),
            RecordCursor(last_system_modstamp="2024-01-01T00:00:00Z"),
            _dt("2024-01-01T00:00:00+00:00"),
            _dt("2024-01-02T00:00:00+00:00"),
        )
        assert "(SystemModstamp >" not in soql
        assert "SystemModstamp >= 2024-01-01T00:00:00Z" in soql


class TestSchemaFingerprint:
    def test_changes_with_config(self) -> None:
        fp1 = schema_fingerprint(frozenset(), frozenset())
        fp2 = schema_fingerprint(frozenset({"Account"}), frozenset())
        fp3 = schema_fingerprint(frozenset({"Account"}), frozenset({"Account"}))
        assert fp1 != fp2
        assert fp2 != fp3
        assert len(fp1) == 64
