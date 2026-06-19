from routers.chat import _extract_text_for_title


def test_extract_text_for_title_from_string():
    assert _extract_text_for_title("  summarize this  ") == "summarize this"


def test_extract_text_for_title_from_text_blocks_with_uploads():
    content = [
        {"type": "document", "source": {"type": "omni_upload", "upload_id": "upload_1"}},
        {"type": "text", "text": "Please inspect this archive."},
    ]

    assert _extract_text_for_title(content) == "Please inspect this archive."


def test_extract_text_for_title_returns_none_for_attachment_only_message():
    content = [
        {"type": "document", "source": {"type": "omni_upload", "upload_id": "upload_1"}},
    ]

    assert _extract_text_for_title(content) is None
