import logging
from dataclasses import dataclass

from providers import LLMProvider, LLMProviderEmptyResponseError, TokenUsage

logger = logging.getLogger(__name__)

TITLE_GENERATION_SYSTEM_PROMPT = """You are a helpful assistant that generates concise, descriptive titles for chat conversations.
Based on the first message(s) of a conversation, generate a title that is:
- 3-7 words long
- Descriptive and specific
- Written in title case
- Does not include quotes or special formatting

Just respond with the title text, nothing else."""

TITLE_GENERATION_EMPTY_RESPONSE_PROMPT = (
    "You did not provide a title. Please provide a concise 3-6 word chat title only, "
    "without quotes or punctuation."
)
TITLE_GENERATION_EMPTY_RETRIES = 2


@dataclass
class GeneratedChatTitle:
    title: str
    usage: TokenUsage | None
    used_fallback: bool = False


def _clean_generated_title(title: str) -> str:
    cleaned = title.strip().strip('"').strip("'").strip()
    cleaned = cleaned.rstrip(".!?:;,").strip()
    if len(cleaned) > 100:
        cleaned = cleaned[:97] + "..."
    return cleaned


def _fallback_chat_title(conversation_text: str) -> str:
    first_line = (
        conversation_text.strip().splitlines()[0] if conversation_text.strip() else ""
    )
    if first_line.lower().startswith("user:"):
        first_line = first_line.split(":", 1)[1]
    words = [w.strip("\"'.,!?;:()[]{}") for w in first_line.split()]
    words = [w for w in words if w]
    if not words:
        return "Untitled"
    return " ".join(words[:6])[:100] or "Untitled"


async def generate_title_for_conversation(
    llm_provider: LLMProvider,
    conversation_text: str,
    chat_id: str,
) -> GeneratedChatTitle:
    base_prompt = (
        f"{TITLE_GENERATION_SYSTEM_PROMPT}\n\nConversation:\n{conversation_text}\n\nTitle:"
    )
    prompt = base_prompt

    for attempt in range(TITLE_GENERATION_EMPTY_RETRIES + 1):
        try:
            generated_title, usage = await llm_provider.generate_response(
                prompt=prompt,
                max_tokens=512,
                temperature=0.7,
                top_p=0.9,
            )
            title = _clean_generated_title(generated_title)
            if title:
                return GeneratedChatTitle(title=title, usage=usage)
            logger.warning(
                "Title generation returned empty text for chat %s (attempt %s/%s)",
                chat_id,
                attempt + 1,
                TITLE_GENERATION_EMPTY_RETRIES + 1,
            )
        except LLMProviderEmptyResponseError as e:
            logger.warning(
                "Title generation returned empty provider response for chat %s "
                "(attempt %s/%s): %s",
                chat_id,
                attempt + 1,
                TITLE_GENERATION_EMPTY_RETRIES + 1,
                e,
            )

        if attempt < TITLE_GENERATION_EMPTY_RETRIES:
            prompt = (
                f"{base_prompt}\n\n{TITLE_GENERATION_EMPTY_RESPONSE_PROMPT}\n\nTitle:"
            )

    title = _fallback_chat_title(conversation_text)
    logger.warning("Falling back to deterministic title for chat %s: %s", chat_id, title)
    return GeneratedChatTitle(title=title, usage=None, used_fallback=True)
