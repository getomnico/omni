"""Vision-capability resolution for chat models.

Ported from Windshift's `internal/llm/vision_capability.go`: provider APIs
offer no portable "does this model accept images" signal, so capability is
resolved from (1) an explicit per-connection override (`vision_mode` key in
the provider config), then (2) a conservative curated model-ID map. Vision
inputs are only attached when resolution says the model can see them;
otherwise image uploads degrade to the workspace-pointer text path.
"""

from __future__ import annotations

# Vision override modes for the provider_config vision_mode key.
#   - auto: defer to the model's resolved capability (curated map)
#   - on:   force vision on (e.g. a local/custom model id the map can't recognize)
#   - off:  force vision off (a model the map wrongly marks capable)
VISION_MODE_AUTO = "auto"
VISION_MODE_ON = "on"
VISION_MODE_OFF = "off"


def is_valid_vision_mode(mode: str) -> bool:
    return mode in (VISION_MODE_AUTO, VISION_MODE_ON, VISION_MODE_OFF)


def parse_vision_mode(raw: object) -> str:
    """Extract the vision override from a provider config dict value.

    Missing or unrecognized values defer to auto (same leniency as Windshift's
    ProviderConfigVisionMode).
    """
    if not isinstance(raw, str):
        return VISION_MODE_AUTO
    mode = raw.strip().lower()
    return mode if is_valid_vision_mode(mode) else VISION_MODE_AUTO


# Vision support uses a conservative case-insensitive model-ID map. Provider
# config can override the map per connection.
_VISION_MODEL_SUBSTRINGS = (
    # OpenAI
    "gpt-4o", "gpt-4.1", "gpt-4-turbo", "gpt-4-vision", "gpt-5", "chatgpt-4o",
    "o1", "o3", "o4-mini",
    # Anthropic (all Claude 3+ accept images)
    "claude-3", "claude-sonnet-4", "claude-opus-4", "claude-haiku-4", "claude-4",
    # Google Gemini (multimodal from 1.5 onward)
    "gemini-1.5", "gemini-2", "gemini-3",
    # xAI Grok
    "grok-2-vision", "grok-3", "grok-4",
    # Meta Llama vision
    "llama-3.2", "llama-4",
    # Generic vision markers used across vendors
    "pixtral", "llava", "vision", "-vl-", "-vl",
)


def curated_vision_capable(model_id: str | None) -> bool:
    """Report whether the curated map recognizes the model id as vision-capable."""
    id = (model_id or "").strip().lower()
    if not id:
        return False
    return any(sub in id for sub in _VISION_MODEL_SUBSTRINGS)


def effective_vision(
    vision_mode_raw: object,
    provider_type: str,
    model_id: str,
) -> bool:
    """Resolve whether image inputs may be sent to this model.

    The override wins: on/off are absolute; auto (or any unrecognized value)
    defers to the curated map. provider_type is accepted for future
    per-provider rules; the map is currently keyed purely by model id.
    """
    del provider_type
    mode = parse_vision_mode(vision_mode_raw)
    if mode == VISION_MODE_ON:
        return True
    if mode == VISION_MODE_OFF:
        return False
    return curated_vision_capable(model_id)
