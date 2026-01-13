"""Tests for HubSpot object-to-document mapping."""

from hubspot_connector.mappers import (
    _contact_title,
    _parse_timestamp,
    generate_content,
    map_hubspot_object_to_document,
)


class TestMapHubSpotObjectToDocument:
    """Tests for the main mapping function."""

    def test_contact_mapping(self, mock_hubspot_contact):
        """Test mapping a contact to a document."""
        doc = map_hubspot_object_to_document(
            "contacts",
            mock_hubspot_contact,
            "content-id-123",
            portal_id="12345",
        )

        assert doc.external_id == "contacts:123"
        assert doc.title == "John Doe"
        assert doc.content_id == "content-id-123"
        assert doc.attributes["object_type"] == "contacts"
        assert doc.attributes["source_type"] == "hubspot"
        assert doc.attributes["hubspot_id"] == "123"
        assert doc.permissions.public is True
        assert doc.metadata.author == "owner-456"
        assert doc.metadata.mime_type == "text/plain"

    def test_mapping_without_portal_id(self, mock_hubspot_contact):
        """Test mapping without portal_id (URL should be None)."""
        doc = map_hubspot_object_to_document(
            "contacts",
            mock_hubspot_contact,
            "content-id-123",
            portal_id=None,
        )

        assert doc.metadata.url is None


class TestGetTitle:
    """Tests for title generation."""

    def test_contact_title_full_name(self):
        """Test contact title with first and last name."""
        properties = {
            "firstname": "Jane",
            "lastname": "Smith",
            "email": "jane@example.com",
        }
        title = _contact_title(properties)
        assert title == "Jane Smith"

    def test_contact_title_unknown(self):
        """Test contact title when no info available."""
        properties = {}
        title = _contact_title(properties)
        assert title == "Unknown Contact"


class TestParseTimestamp:
    """Tests for timestamp parsing."""

    def test_parse_iso_timestamp(self):
        """Test parsing ISO 8601 timestamp."""
        result = _parse_timestamp("2024-01-15T10:30:00Z")
        assert result is not None
        assert result.year == 2024
        assert result.month == 1
        assert result.day == 15

    def test_parse_milliseconds_timestamp(self):
        """Test parsing milliseconds since epoch."""
        result = _parse_timestamp("1705315800000")
        assert result is not None
        assert result.year == 2024

    def test_parse_none_timestamp(self):
        """Test parsing None returns None."""
        result = _parse_timestamp(None)
        assert result is None


class TestGenerateContent:
    """Tests for content generation."""

    def test_contact_content(self, mock_hubspot_contact):
        """Test generating content for a contact."""
        content = generate_content("contacts", mock_hubspot_contact)

        assert "HubSpot Contacts" in content
        assert "Title: John Doe" in content
        assert "john.doe@example.com" in content
        assert "Acme Corp" in content
