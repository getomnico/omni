"""Memory mode resolution.

Org default is a **ceiling**, not just a fallback: users can never exceed
the mode the org admin has enabled. If a user picks a higher mode than the
org allows, they are capped down to the org setting.

Priority:
  1. user_mode capped by org (if set and within ceiling)
  2. org_default (inherited when the user has no override)
  3. 'off' (hard factory default)
"""

VALID_MODES = {"off", "chat", "full"}

_MODE_RANK = {"off": 0, "chat": 1, "full": 2}


def resolve_memory_mode(
    user_mode: str | None,
    org_default: str | None,
) -> str:
    """Return the effective memory mode for a request.

    Rules:
      - `org_default` is the maximum allowed mode for any user in the org.
      - If the user has no override (`None`), they inherit `org_default`.
      - If the user's override is higher than `org_default`, it is capped down.
      - Invalid values are treated as 'off' defensively.
    """
    org = org_default if org_default in VALID_MODES else "off"

    if user_mode is None:
        return org

    if user_mode not in VALID_MODES:
        return "off"

    if _MODE_RANK[user_mode] <= _MODE_RANK[org]:
        return user_mode
    return org
