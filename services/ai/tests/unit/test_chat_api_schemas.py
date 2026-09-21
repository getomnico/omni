from typing import cast

import pytest
from pydantic import ValidationError

from message_types import OmniMentionBlock
from schemas.api import SteeringMessageRequest

MESSAGE_ID = "01J00000000000000000000000"
DOCUMENT_ID = "01J00000000000000000000001"


def request_for(message: dict) -> dict:
    return {"message_id": MESSAGE_ID, "message": message}


def test_steering_message_reuses_shared_user_message_types() -> None:
    request = SteeringMessageRequest.model_validate(
        request_for(
            {
                "role": "user",
                "content": [
                    {"type": "text", "text": "Review these files."},
                    {
                        "type": "document",
                        "source": {
                            "type": "omni_upload",
                            "upload_id": DOCUMENT_ID,
                        },
                    },
                    {
                        "type": "document",
                        "source": {
                            "type": "omni_mention",
                            "document_id": DOCUMENT_ID,
                            "title": "Project brief",
                            "source_type": "drive",
                            "content_type": "text/plain",
                        },
                    },
                ],
            }
        )
    )

    assert request.message["role"] == "user"
    content = request.message["content"]
    assert isinstance(content, list)
    mention = cast(OmniMentionBlock, content[2])
    assert mention["source"]["source_type"] == "drive"


@pytest.mark.parametrize(
    "message",
    [
        {"role": "assistant", "content": "not allowed"},
        {
            "role": "user",
            "content": [{"type": "tool_use", "id": "toolu_1", "name": "search", "input": {}}],
        },
        {
            "role": "user",
            "content": [{"type": "tool_result", "tool_use_id": "toolu_1", "content": "bad"}],
        },
    ],
)
def test_steering_message_rejects_non_user_or_tool_content(message: dict) -> None:
    with pytest.raises(ValidationError):
        SteeringMessageRequest.model_validate(request_for(message))
