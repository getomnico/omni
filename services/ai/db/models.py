import json
from dataclasses import dataclass
from datetime import datetime
from enum import Enum
from typing import Any
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError

from crypto import decrypt_config
from memory import MemoryMode


class DoclingQualityPreset(str, Enum):
    FAST = "fast"
    BALANCED = "balanced"
    QUALITY = "quality"


@dataclass(frozen=True)
class GlobalConfiguration:
    docling_enabled: bool = False
    docling_quality_preset: DoclingQualityPreset = DoclingQualityPreset.BALANCED
    memory_mode_default: MemoryMode = MemoryMode.OFF
    memory_llm_id: str | None = None

    @classmethod
    def from_rows(cls, rows: list[dict[str, Any]]) -> "GlobalConfiguration":
        values = {row["key"]: row.get("value") for row in rows}

        docling_enabled = _read_configuration_bool(values.get("docling_enabled"))
        raw_preset = _read_configuration_string(
            values.get("docling_quality_preset"), "preset"
        )
        raw_memory_mode = _read_configuration_string(
            values.get("memory_mode_default"), "mode"
        )
        memory_llm_id = _read_configuration_string(values.get("memory_llm_id"))

        try:
            docling_quality_preset = (
                DoclingQualityPreset(raw_preset)
                if raw_preset
                else DoclingQualityPreset.BALANCED
            )
        except ValueError as exc:
            raise ValueError(
                f"Invalid docling_quality_preset configuration: {raw_preset}"
            ) from exc

        memory_mode_default = MemoryMode.parse(raw_memory_mode)
        if raw_memory_mode and memory_mode_default is None:
            raise ValueError(
                f"Invalid memory_mode_default configuration: {raw_memory_mode}"
            )

        return cls(
            docling_enabled=docling_enabled if docling_enabled is not None else False,
            docling_quality_preset=docling_quality_preset,
            memory_mode_default=memory_mode_default or MemoryMode.OFF,
            memory_llm_id=memory_llm_id,
        )


def _read_configuration_string(raw: Any, *alternate_keys: str) -> str | None:
    if isinstance(raw, str):
        return raw
    if isinstance(raw, dict):
        for key in ("value", *alternate_keys):
            value = raw.get(key)
            if isinstance(value, str):
                return value
    return None


def _read_configuration_bool(raw: Any) -> bool | None:
    if isinstance(raw, bool):
        return raw
    if isinstance(raw, dict):
        value = raw.get("enabled")
        if isinstance(value, bool):
            return value
    return None


@dataclass(frozen=True)
class UserConfiguration:
    memory_mode: MemoryMode | None = None
    timezone: str | None = None

    @classmethod
    def from_rows(cls, rows: list[dict[str, Any]]) -> "UserConfiguration | None":
        if not rows:
            return None

        values = {row["key"]: row.get("value") for row in rows}
        timezone = (
            _read_configuration_string(values["timezone"], "timezone")
            if "timezone" in values
            else None
        )
        if timezone:
            try:
                ZoneInfo(timezone)
            except ZoneInfoNotFoundError as exc:
                raise ValueError(f"Invalid user timezone configuration: {timezone}") from exc

        raw_memory_mode = (
            _read_configuration_string(values["memory_mode"], "mode")
            if "memory_mode" in values
            else None
        )
        memory_mode = MemoryMode.parse(raw_memory_mode)
        if raw_memory_mode and memory_mode is None:
            raise ValueError(f"Invalid user memory_mode configuration: {raw_memory_mode}")

        return cls(memory_mode=memory_mode, timezone=timezone)


@dataclass
class User:
    id: str
    email: str
    full_name: str | None
    role: str
    is_active: bool
    created_at: datetime
    updated_at: datetime
    configuration: UserConfiguration | None = None

    @property
    def timezone(self) -> str | None:
        return self.configuration.timezone if self.configuration else None

    @classmethod
    def from_row(cls, row: dict) -> "User":
        return cls(
            id=row["id"],
            email=row["email"],
            full_name=row.get("full_name"),
            role=row["role"],
            is_active=row["is_active"],
            created_at=row["created_at"],
            updated_at=row["updated_at"],
            configuration=row.get("configuration"),
        )


class ChatRole(str, Enum):
    USER = "user"
    ASSISTANT = "assistant"
    SYSTEM = "system"


@dataclass
class Chat:
    id: str
    user_id: str
    title: str | None
    model_id: str | None
    created_at: datetime
    updated_at: datetime
    agent_id: str | None = None

    @classmethod
    def from_row(cls, row: dict) -> "Chat":
        """Create Chat from database row"""
        model_id = row.get("model_id")
        if model_id:
            model_id = model_id.strip()
        return cls(
            id=row["id"],
            user_id=row["user_id"],
            title=row.get("title"),
            model_id=model_id,
            created_at=row["created_at"],
            updated_at=row["updated_at"],
            agent_id=row.get("agent_id"),
        )

    def to_dict(self) -> dict:
        """Convert to dictionary for JSON serialization"""
        return {
            "id": self.id,
            "user_id": self.user_id,
            "title": self.title,
            "model_id": self.model_id,
            "agent_id": self.agent_id,
            "created_at": self.created_at.isoformat(),
            "updated_at": self.updated_at.isoformat(),
        }


@dataclass
class ModelRecord:
    id: str
    model_provider_id: str
    model_id: str
    display_name: str
    is_default: bool
    is_secondary: bool
    is_deleted: bool
    provider_type: str
    config: dict
    created_at: datetime
    updated_at: datetime

    @classmethod
    def from_row(cls, row: dict) -> "ModelRecord":
        config = row["config"]
        if isinstance(config, str):
            config = json.loads(config)
        config = decrypt_config(config)
        return cls(
            id=row["id"].strip(),
            model_provider_id=row["model_provider_id"].strip(),
            model_id=row["model_id"],
            display_name=row["display_name"],
            is_default=row["is_default"],
            is_secondary=row["is_secondary"],
            is_deleted=row["is_deleted"],
            provider_type=row["provider_type"],
            config=config,
            created_at=row["created_at"],
            updated_at=row["updated_at"],
        )


@dataclass
class Source:
    id: str
    name: str
    source_type: str
    is_active: bool
    is_deleted: bool

    @classmethod
    def from_row(cls, row: dict) -> "Source":
        return cls(
            id=row["id"],
            name=row["name"],
            source_type=row["source_type"],
            is_active=row["is_active"],
            is_deleted=row["is_deleted"],
        )


@dataclass
class ChatMessage:
    id: str
    chat_id: str
    message_seq_num: int
    message: dict[str, Any]  # Full JSONB message content
    created_at: datetime
    parent_id: str | None = None

    @classmethod
    def from_row(cls, row: dict) -> "ChatMessage":
        """Create ChatMessage from database row"""
        if isinstance(row["message"], str):
            row["message"] = json.loads(row["message"])
        return cls(
            id=row["id"],
            chat_id=row["chat_id"],
            message_seq_num=row["message_seq_num"],
            message=row["message"],
            created_at=row["created_at"],
            parent_id=row.get("parent_id"),
        )

    def to_dict(self) -> dict:
        """Convert to dictionary for JSON serialization"""
        return {
            "id": self.id,
            "chat_id": self.chat_id,
            "message_seq_num": self.message_seq_num,
            "message": self.message,
            "parent_id": self.parent_id,
            "created_at": self.created_at.isoformat(),
        }
