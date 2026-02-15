import json
from dataclasses import dataclass
from datetime import datetime
from typing import Optional, Dict, Any
from enum import Enum


@dataclass
class User:
    id: str
    email: str
    full_name: Optional[str]
    role: str
    is_active: bool
    created_at: datetime
    updated_at: datetime

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
        )


class ChatRole(str, Enum):
    USER = "user"
    ASSISTANT = "assistant"
    SYSTEM = "system"


@dataclass
class Chat:
    id: str
    user_id: str
    title: Optional[str]
    model_id: Optional[str]
    created_at: datetime
    updated_at: datetime

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
        )

    def to_dict(self) -> dict:
        """Convert to dictionary for JSON serialization"""
        return {
            "id": self.id,
            "user_id": self.user_id,
            "title": self.title,
            "model_id": self.model_id,
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
        return cls(
            id=row["id"].strip(),
            model_provider_id=row["model_provider_id"].strip(),
            model_id=row["model_id"],
            display_name=row["display_name"],
            is_default=row["is_default"],
            is_deleted=row["is_deleted"],
            provider_type=row["provider_type"],
            config=config,
            created_at=row["created_at"],
            updated_at=row["updated_at"],
        )


@dataclass
class ChatMessage:
    id: str
    chat_id: str
    message_seq_num: int
    message: Dict[str, Any]  # Full JSONB message content
    created_at: datetime

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
        )

    def to_dict(self) -> dict:
        """Convert to dictionary for JSON serialization"""
        return {
            "id": self.id,
            "chat_id": self.chat_id,
            "message_seq_num": self.message_seq_num,
            "message": self.message,
            "created_at": self.created_at.isoformat(),
        }
