"""Typed models for agents, durable agent runs, and agent run logs."""

import json
from dataclasses import dataclass
from datetime import datetime, timedelta
from enum import StrEnum
from typing import Literal, TypeAlias, Optional

from anthropic.types import MessageParam

AgentType = Literal["user", "org"]
ScheduleType = Literal["cron", "interval"]
AgentRunLogMessage: TypeAlias = MessageParam
AgentRunConversation: TypeAlias = list[AgentRunLogMessage]


class RunStatus(StrEnum):
    PENDING = "pending"
    RUNNING = "running"
    COMPLETED = "completed"
    FAILED = "failed"


class AgentRunTriggerType(StrEnum):
    MANUAL = "manual"
    SCHEDULED = "scheduled"


@dataclass(frozen=True)
class Agent:
    id: str
    user_id: str
    name: str
    instructions: str
    agent_type: AgentType
    schedule_type: ScheduleType
    schedule_value: str
    model_id: Optional[str]
    allowed_sources: list[dict[str, object]]  # [{source_id, modes: ["read","write"]}]
    allowed_actions: list[str]  # tool name whitelist for org agents
    is_enabled: bool
    is_deleted: bool
    created_at: datetime
    updated_at: datetime

    @classmethod
    def from_row(cls, row: dict[str, object]) -> "Agent":
        allowed_sources_raw = row.get("allowed_sources", [])
        if isinstance(allowed_sources_raw, str):
            allowed_sources = json.loads(allowed_sources_raw)
        elif isinstance(allowed_sources_raw, list):
            allowed_sources = allowed_sources_raw
        else:
            allowed_sources = []

        allowed_actions_raw = row.get("allowed_actions", [])
        if isinstance(allowed_actions_raw, str):
            allowed_actions = json.loads(allowed_actions_raw)
        elif isinstance(allowed_actions_raw, list):
            allowed_actions = [str(action) for action in allowed_actions_raw]
        else:
            allowed_actions = []

        model_id_raw = row.get("model_id")
        model_id = model_id_raw.strip() if isinstance(model_id_raw, str) else None

        agent_type = str(row["agent_type"])
        schedule_type = str(row["schedule_type"])

        return cls(
            id=str(row["id"]).strip(),
            user_id=str(row["user_id"]).strip(),
            name=str(row["name"]),
            instructions=str(row["instructions"]),
            agent_type="org" if agent_type == "org" else "user",
            schedule_type="cron" if schedule_type == "cron" else "interval",
            schedule_value=str(row["schedule_value"]),
            model_id=model_id,
            allowed_sources=allowed_sources,
            allowed_actions=allowed_actions,
            is_enabled=bool(row["is_enabled"]),
            is_deleted=bool(row["is_deleted"]),
            created_at=row["created_at"],  # type: ignore[assignment]
            updated_at=row["updated_at"],  # type: ignore[assignment]
        )

    def to_dict(self) -> dict[str, object]:
        return {
            "id": self.id,
            "user_id": self.user_id,
            "name": self.name,
            "instructions": self.instructions,
            "agent_type": self.agent_type,
            "schedule_type": self.schedule_type,
            "schedule_value": self.schedule_value,
            "model_id": self.model_id,
            "allowed_sources": self.allowed_sources,
            "allowed_actions": self.allowed_actions,
            "is_enabled": self.is_enabled,
            "is_deleted": self.is_deleted,
            "created_at": self.created_at.isoformat(),
            "updated_at": self.updated_at.isoformat(),
        }


@dataclass(frozen=True)
class AgentRun:
    id: str
    agent_id: str
    status: RunStatus
    trigger_type: AgentRunTriggerType
    started_at: datetime | None
    completed_at: datetime | None
    claim_token: str | None
    lease_expires_at: datetime | None
    heartbeat_at: datetime | None
    attempt_count: int
    max_attempts: int
    summary: str | None = None
    error_message: str | None = None
    created_at: datetime | None = None

    @classmethod
    def from_row(cls, row: dict[str, object]) -> "AgentRun":
        claim_token_raw = row.get("claim_token")
        return cls(
            id=str(row["id"]).strip(),
            agent_id=str(row["agent_id"]).strip(),
            status=RunStatus(str(row["status"])),
            trigger_type=AgentRunTriggerType(str(row.get("trigger_type", "manual"))),
            started_at=row.get("started_at"),  # type: ignore[arg-type]
            completed_at=row.get("completed_at"),  # type: ignore[arg-type]
            claim_token=str(claim_token_raw).strip() if claim_token_raw else None,
            lease_expires_at=row.get("lease_expires_at"),  # type: ignore[arg-type]
            heartbeat_at=row.get("heartbeat_at"),  # type: ignore[arg-type]
            attempt_count=int(row.get("attempt_count", 0)),
            max_attempts=int(row.get("max_attempts", 3)),
            summary=row.get("summary") if isinstance(row.get("summary"), str) else None,
            error_message=(
                row.get("error_message") if isinstance(row.get("error_message"), str) else None
            ),
            created_at=row.get("created_at"),  # type: ignore[arg-type]
        )

    def to_dict(self) -> dict[str, object]:
        return {
            "id": self.id,
            "agent_id": self.agent_id,
            "status": self.status.value,
            "trigger_type": self.trigger_type.value,
            "started_at": self.started_at.isoformat() if self.started_at else None,
            "completed_at": self.completed_at.isoformat() if self.completed_at else None,
            "lease_expires_at": self.lease_expires_at.isoformat()
            if self.lease_expires_at
            else None,
            "heartbeat_at": self.heartbeat_at.isoformat() if self.heartbeat_at else None,
            "attempt_count": self.attempt_count,
            "max_attempts": self.max_attempts,
            "summary": self.summary,
            "error_message": self.error_message,
            "created_at": self.created_at.isoformat() if self.created_at else None,
        }


@dataclass(frozen=True)
class AgentRunLog:
    id: str
    run_id: str
    message_seq_num: int
    message: AgentRunLogMessage
    created_at: datetime

    @classmethod
    def from_row(cls, row: dict[str, object]) -> "AgentRunLog":
        message_raw = row["message"]
        if isinstance(message_raw, str):
            message = json.loads(message_raw)
        else:
            message = message_raw
        return cls(
            id=str(row["id"]).strip(),
            run_id=str(row["run_id"]).strip(),
            message_seq_num=int(row["message_seq_num"]),
            message=message,  # type: ignore[arg-type]
            created_at=row["created_at"],  # type: ignore[assignment]
        )


@dataclass(frozen=True)
class AgentRunClaim:
    run: AgentRun
    claim_token: str


@dataclass(frozen=True)
class AgentRunAlreadyActive:
    run: AgentRun


AgentRunCreateResult: TypeAlias = AgentRun | AgentRunAlreadyActive


@dataclass(frozen=True)
class AgentRunRetryPolicy:
    max_attempts: int
    backoff_delays: tuple[timedelta, ...]

    def delay_for_attempt(self, attempt_count: int) -> timedelta:
        if attempt_count <= 0:
            return timedelta(seconds=0)
        if not self.backoff_delays:
            return timedelta(seconds=0)
        idx = min(attempt_count - 1, len(self.backoff_delays) - 1)
        return self.backoff_delays[idx]


@dataclass(frozen=True)
class AgentExecutionResult:
    summary: str | None
