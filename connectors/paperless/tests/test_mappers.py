"""Unit tests for the paperless-ngx mappers (content generation and document mapping)."""

from datetime import datetime, timezone

from paperless_connector.mappers import generate_document_content, map_document_to_omni
from paperless_connector.models import PaperlessCustomField, PaperlessDocument, PaperlessNote


def _make_doc(**kwargs: object) -> PaperlessDocument:
    defaults: dict = {
        "id": 1,
        "title": "Test Document",
        "content": "Some OCR text here.",
        "created": datetime(2024, 1, 15, tzinfo=timezone.utc),
        "added": datetime(2024, 1, 16, tzinfo=timezone.utc),
        "modified": datetime(2024, 1, 17, tzinfo=timezone.utc),
        "original_file_name": "test.pdf",
    }
    defaults.update(kwargs)
    return PaperlessDocument(**defaults)  # type: ignore[arg-type]


# ── generate_document_content ───────────────────────────────────────────────


class TestGenerateDocumentContent:
    def test_title_is_heading(self) -> None:
        doc = _make_doc(title="Invoice 2024-01")
        content = generate_document_content(doc)
        assert content.startswith("# Invoice 2024-01")

    def test_includes_metadata_section(self) -> None:
        doc = _make_doc()
        content = generate_document_content(doc)
        assert "## Metadata" in content

    def test_correspondent_in_metadata(self) -> None:
        doc = _make_doc(correspondent_name="ACME Corp")
        content = generate_document_content(doc)
        assert "**Correspondent:** ACME Corp" in content

    def test_document_type_in_metadata(self) -> None:
        doc = _make_doc(document_type_name="Invoice")
        content = generate_document_content(doc)
        assert "**Document Type:** Invoice" in content

    def test_tags_sorted_alphabetically(self) -> None:
        doc = _make_doc(tag_names=["work", "finance", "2024"])
        content = generate_document_content(doc)
        assert "**Tags:** 2024, finance, work" in content

    def test_created_date_formatted(self) -> None:
        doc = _make_doc(created=datetime(2024, 3, 5, tzinfo=timezone.utc))
        content = generate_document_content(doc)
        assert "**Created:** 2024-03-05" in content

    def test_original_file_name_present(self) -> None:
        doc = _make_doc(original_file_name="my_invoice.pdf")
        content = generate_document_content(doc)
        assert "**Original File:** my_invoice.pdf" in content

    def test_content_section_with_ocr_text(self) -> None:
        doc = _make_doc(content="Total amount due: $1,234.56")
        content = generate_document_content(doc)
        assert "## Content" in content
        assert "Total amount due: $1,234.56" in content

    def test_empty_content_no_content_section(self) -> None:
        doc = _make_doc(content="")
        content = generate_document_content(doc)
        assert "## Content" not in content

    def test_custom_fields_rendered(self) -> None:
        doc = _make_doc(
            custom_fields=[
                PaperlessCustomField(name="Invoice Number", value="INV-2024-001"),
                PaperlessCustomField(name="Amount", value="500.00"),
            ]
        )
        content = generate_document_content(doc)
        assert "Invoice Number: INV-2024-001" in content
        assert "Amount: 500.00" in content

    def test_custom_field_with_none_value_omitted(self) -> None:
        doc = _make_doc(
            custom_fields=[
                PaperlessCustomField(name="Optional", value=None),
            ]
        )
        content = generate_document_content(doc)
        assert "Optional:" not in content

    def test_no_optional_fields_omitted_gracefully(self) -> None:
        doc = _make_doc(
            correspondent_name=None,
            document_type_name=None,
            tag_names=[],
            original_file_name=None,
            custom_fields=[],
        )
        content = generate_document_content(doc)
        # Should still have title and metadata heading
        assert "# Test Document" in content
        assert "## Metadata" in content

    def test_truncation_at_max_length(self) -> None:
        from paperless_connector.config import MAX_CONTENT_LENGTH

        long_text = "x" * (MAX_CONTENT_LENGTH + 1000)
        doc = _make_doc(content=long_text)
        content = generate_document_content(doc)
        assert len(content) <= MAX_CONTENT_LENGTH + len("\n... (truncated)")
        assert content.endswith("... (truncated)")

    def test_storage_path_in_metadata(self) -> None:
        doc = _make_doc(storage_path_name="Archive/Finance")
        content = generate_document_content(doc)
        assert "**Storage Path:** Archive/Finance" in content

    def test_archive_serial_number_in_metadata(self) -> None:
        doc = _make_doc(archive_serial_number=42)
        content = generate_document_content(doc)
        assert "**Archive Serial Number:** 42" in content

    def test_archive_serial_number_none_omitted(self) -> None:
        doc = _make_doc(archive_serial_number=None)
        content = generate_document_content(doc)
        assert "Archive Serial Number" not in content

    def test_notes_section_rendered(self) -> None:
        doc = _make_doc(
            notes=[
                PaperlessNote(
                    note="Approved for payment.",
                    created=datetime(2024, 3, 15, 14, 30, tzinfo=timezone.utc),
                    user="admin",
                ),
            ]
        )
        content = generate_document_content(doc)
        assert "## Notes" in content
        assert "admin" in content
        assert "2024-03-15 14:30" in content
        assert "Approved for payment." in content

    def test_notes_without_user_or_date(self) -> None:
        doc = _make_doc(
            notes=[PaperlessNote(note="Just a note.", created=None, user=None)]
        )
        content = generate_document_content(doc)
        assert "## Notes" in content
        assert "Just a note." in content

    def test_no_notes_no_section(self) -> None:
        doc = _make_doc(notes=[])
        content = generate_document_content(doc)
        assert "## Notes" not in content

    def test_full_document_structure(self) -> None:
        """Smoke test: a fully-populated document produces well-structured markdown."""
        doc = _make_doc(
            title="Q4 Financial Report",
            correspondent_name="Finance Dept",
            document_type_name="Report",
            tag_names=["quarterly", "finance"],
            created=datetime(2024, 12, 31, tzinfo=timezone.utc),
            content="Revenue increased by 12% in Q4.",
            custom_fields=[
                PaperlessCustomField(name="Fiscal Year", value="2024"),
            ],
            storage_path_name="Archive/Finance",
            archive_serial_number=7,
            notes=[
                PaperlessNote(
                    note="Reviewed.",
                    created=datetime(2025, 1, 5, 10, 0, tzinfo=timezone.utc),
                    user="reviewer",
                ),
            ],
        )
        content = generate_document_content(doc)

        assert "# Q4 Financial Report" in content
        assert "**Correspondent:** Finance Dept" in content
        assert "**Document Type:** Report" in content
        assert "**Tags:** finance, quarterly" in content
        assert "**Created:** 2024-12-31" in content
        assert "Fiscal Year: 2024" in content
        assert "Revenue increased by 12% in Q4." in content
        assert "**Storage Path:** Archive/Finance" in content
        assert "**Archive Serial Number:** 7" in content
        assert "## Notes" in content
        assert "Reviewed." in content


# ── map_document_to_omni ────────────────────────────────────────────────────


class TestMapDocumentToOmni:
    def test_external_id_format(self) -> None:
        doc = _make_doc(id=42)
        omni_doc = map_document_to_omni(doc, "cid-1", "src-1", "http://paperless.local")
        assert omni_doc.external_id == "paperless:src-1:42"

    def test_title_preserved(self) -> None:
        doc = _make_doc(title="My Document")
        omni_doc = map_document_to_omni(doc, "cid-1", "src-1", "http://paperless.local")
        assert omni_doc.title == "My Document"

    def test_content_id_set(self) -> None:
        doc = _make_doc()
        omni_doc = map_document_to_omni(doc, "content-abc", "src-1", "http://paperless.local")
        assert omni_doc.content_id == "content-abc"

    def test_url_includes_document_id(self) -> None:
        doc = _make_doc(id=7)
        omni_doc = map_document_to_omni(doc, "cid", "src", "http://paperless.example.com")
        assert omni_doc.metadata is not None
        assert omni_doc.metadata.url == "http://paperless.example.com/documents/7/details/"

    def test_url_trailing_slash_normalised(self) -> None:
        doc = _make_doc(id=3)
        omni_doc = map_document_to_omni(doc, "cid", "src", "http://paperless.example.com/")
        assert omni_doc.metadata is not None
        assert omni_doc.metadata.url == "http://paperless.example.com/documents/3/details/"

    def test_author_is_correspondent(self) -> None:
        doc = _make_doc(correspondent_name="Insurance Co")
        omni_doc = map_document_to_omni(doc, "cid", "src", "http://paperless.local")
        assert omni_doc.metadata is not None
        assert omni_doc.metadata.author == "Insurance Co"

    def test_created_at_propagated(self) -> None:
        created = datetime(2024, 6, 1, tzinfo=timezone.utc)
        doc = _make_doc(created=created)
        omni_doc = map_document_to_omni(doc, "cid", "src", "http://paperless.local")
        assert omni_doc.metadata is not None
        assert omni_doc.metadata.created_at == created

    def test_permissions_public(self) -> None:
        doc = _make_doc()
        omni_doc = map_document_to_omni(doc, "cid", "src", "http://paperless.local")
        assert omni_doc.permissions is not None
        assert omni_doc.permissions.public is True

    def test_attributes_contain_source_type(self) -> None:
        doc = _make_doc()
        omni_doc = map_document_to_omni(doc, "cid", "src", "http://paperless.local")
        assert omni_doc.attributes is not None
        assert omni_doc.attributes["source_type"] == "paperless_ngx"

    def test_attributes_contain_paperless_id(self) -> None:
        doc = _make_doc(id=99)
        omni_doc = map_document_to_omni(doc, "cid", "src", "http://paperless.local")
        assert omni_doc.attributes is not None
        assert omni_doc.attributes["paperless_id"] == "99"

    def test_attributes_correspondent_and_type(self) -> None:
        doc = _make_doc(correspondent_name="ACME", document_type_name="Contract")
        omni_doc = map_document_to_omni(doc, "cid", "src", "http://paperless.local")
        assert omni_doc.attributes is not None
        assert omni_doc.attributes["correspondent"] == "ACME"
        assert omni_doc.attributes["document_type"] == "Contract"

    def test_attributes_tags(self) -> None:
        doc = _make_doc(tag_names=["alpha", "beta"])
        omni_doc = map_document_to_omni(doc, "cid", "src", "http://paperless.local")
        assert omni_doc.attributes is not None
        assert omni_doc.attributes["tags"] == "alpha, beta"

    def test_optional_attributes_absent_when_empty(self) -> None:
        doc = _make_doc(
            correspondent_name=None,
            document_type_name=None,
            tag_names=[],
            original_file_name=None,
        )
        omni_doc = map_document_to_omni(doc, "cid", "src", "http://paperless.local")
        assert omni_doc.attributes is not None
        assert "correspondent" not in omni_doc.attributes
        assert "document_type" not in omni_doc.attributes
        assert "tags" not in omni_doc.attributes
        assert "original_file_name" not in omni_doc.attributes

    def test_storage_path_in_metadata_path(self) -> None:
        doc = _make_doc(storage_path_name="Archive/Finance")
        omni_doc = map_document_to_omni(doc, "cid", "src", "http://paperless.local")
        assert omni_doc.metadata is not None
        assert omni_doc.metadata.path == "Archive/Finance"

    def test_archive_serial_number_in_attributes(self) -> None:
        doc = _make_doc(archive_serial_number=123)
        omni_doc = map_document_to_omni(doc, "cid", "src", "http://paperless.local")
        assert omni_doc.attributes is not None
        assert omni_doc.attributes["archive_serial_number"] == "123"

    def test_archive_serial_number_absent_when_none(self) -> None:
        doc = _make_doc(archive_serial_number=None)
        omni_doc = map_document_to_omni(doc, "cid", "src", "http://paperless.local")
        assert omni_doc.attributes is not None
        assert "archive_serial_number" not in omni_doc.attributes

    def test_extra_custom_fields_in_metadata(self) -> None:
        doc = _make_doc(
            custom_fields=[
                PaperlessCustomField(name="Invoice Number", value="INV-001"),
                PaperlessCustomField(name="Empty", value=None),
            ]
        )
        omni_doc = map_document_to_omni(doc, "cid", "src", "http://paperless.local")
        assert omni_doc.metadata is not None
        assert omni_doc.metadata.extra is not None
        assert omni_doc.metadata.extra["custom_fields"] == {"Invoice Number": "INV-001"}

    def test_extra_note_count_in_metadata(self) -> None:
        doc = _make_doc(
            notes=[
                PaperlessNote(note="A", created=None, user=None),
                PaperlessNote(note="B", created=None, user=None),
            ]
        )
        omni_doc = map_document_to_omni(doc, "cid", "src", "http://paperless.local")
        assert omni_doc.metadata is not None
        assert omni_doc.metadata.extra is not None
        assert omni_doc.metadata.extra["note_count"] == 2

    def test_extra_is_none_when_no_extra_data(self) -> None:
        doc = _make_doc(custom_fields=[], notes=[])
        omni_doc = map_document_to_omni(doc, "cid", "src", "http://paperless.local")
        assert omni_doc.metadata is not None
        assert omni_doc.metadata.extra is None

    def test_extra_is_none_when_all_custom_field_values_are_none(self) -> None:
        doc = _make_doc(
            custom_fields=[PaperlessCustomField(name="Optional", value=None)],
            notes=[],
        )
        omni_doc = map_document_to_omni(doc, "cid", "src", "http://paperless.local")
        assert omni_doc.metadata is not None
        assert omni_doc.metadata.extra is None

    def test_no_storage_path_means_no_path(self) -> None:
        doc = _make_doc(storage_path_name=None)
        omni_doc = map_document_to_omni(doc, "cid", "src", "http://paperless.local")
        assert omni_doc.metadata is not None
        assert omni_doc.metadata.path is None
