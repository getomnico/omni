"""Vision-capability resolution for chat models.

Provider APIs offer no portable "does this model accept images" flag, and
model-ID substring lists go stale with every release, so capability is
resolved without name matching:

1. The per-connection ``visionMode`` override wins outright
   (``on`` / ``off``).
2. Provider families whose adapters always accept images (Anthropic-style
   Claude, Gemini) resolve to vision-capable.
3. OpenAI-compatible endpoints are asked: OpenRouter publishes per-model
   input modalities and Ollama exposes model capabilities; endpoints that
   offer neither get a one-shot live probe (a tiny image ping — any
   completion proves vision, a clear 4xx image rejection proves text-only).
   Only truly unanswerable cases resolve as NOT vision-capable (images
   degrade to the workspace-pointer path) until an admin opts in with
   ``visionMode: on``.
"""

from __future__ import annotations

# Vision override modes for the provider config visionMode key.
#   - auto: detect (family knowledge or endpoint metadata; else off)
#   - on:   force vision on (e.g. a vLLM-served vision model)
#   - off:  force vision off
VISION_MODE_AUTO = "auto"
VISION_MODE_ON = "on"
VISION_MODE_OFF = "off"


def is_valid_vision_mode(mode: str) -> bool:
    return mode in (VISION_MODE_AUTO, VISION_MODE_ON, VISION_MODE_OFF)


def parse_vision_mode(raw: object) -> str:
    """Extract the vision override from a provider config dict value.

    Missing or unrecognized values defer to auto (the lenient default, so a
    hand-edited config value can't break chat).
    """
    if not isinstance(raw, str):
        return VISION_MODE_AUTO
    mode = raw.strip().lower()
    return mode if is_valid_vision_mode(mode) else VISION_MODE_AUTO


# Provider families whose wire format always accepts image blocks, so
# capability follows from the adapter, not the model id.
_VISION_FAMILY_PROVIDER_TYPES = frozenset(
    {
        "anthropic",
        "gemini",
        "bedrock",
        "vertex_ai",
    }
)

# Providers that must be asked (endpoint metadata) before images are sent.
_METADATA_PROVIDER_TYPES = frozenset({"openai_compatible"})


def static_vision(provider_type: str) -> bool | None:
    """Resolve vision from provider-type knowledge alone.

    Returns True/False when decidable without contacting the endpoint, or
    None when the endpoint must be asked (openai_compatible).
    """
    if provider_type in _VISION_FAMILY_PROVIDER_TYPES:
        return True
    if provider_type in _METADATA_PROVIDER_TYPES:
        return None
    return False


def effective_vision(
    vision_mode_raw: object,
    provider_type: str,
    endpoint_supports_vision: bool | None = None,
) -> bool | None:
    """Resolve whether image inputs may be sent to this model.

    The override wins: on/off are absolute. Otherwise auto (or any
    unrecognized value) defers to provider-family knowledge, then to the
    endpoint's own metadata (``endpoint_supports_vision``; None means the
    endpoint offers none). The result is None only when a metadata provider
    could not be probed and no override is set — callers must treat None as
    "not vision-capable".
    """
    mode = parse_vision_mode(vision_mode_raw)
    if mode == VISION_MODE_ON:
        return True
    if mode == VISION_MODE_OFF:
        return False
    static = static_vision(provider_type)
    if static is not None:
        return static
    return endpoint_supports_vision
