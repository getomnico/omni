"""Typed representations of ClickUp API objects used in permission resolution."""

from dataclasses import dataclass, field
from typing import Any


ROLE_GUEST = 4


@dataclass
class ClickUpMember:
    user_id: str
    username: str
    email: str | None
    role: int  # 1=Owner, 2=Admin, 3=Member, 4=Guest


@dataclass
class ClickUpSpace:
    id: str
    name: str
    private: bool
    members: list[ClickUpMember] = field(default_factory=list)


def parse_member(raw: dict[str, Any]) -> ClickUpMember:
    """Parse a raw ClickUp API member dict into a ClickUpMember."""
    user = raw.get("user", {})
    return ClickUpMember(
        user_id=str(user.get("id", "")),
        username=user.get("username", ""),
        email=user.get("email") or None,
        role=raw.get("role", 0),
    )


def parse_space(raw: dict[str, Any]) -> ClickUpSpace:
    """Parse a raw ClickUp API space dict into a ClickUpSpace."""
    return ClickUpSpace(
        id=str(raw["id"]),
        name=raw.get("name", ""),
        private=raw.get("private", False),
        members=[parse_member(m) for m in raw.get("members", [])],
    )
