"""Dataclasses for the agents and agent_runs tables."""

import json
from dataclasses import dataclass, field
from datetime import datetime
from typing import Literal, Optional

AgentType = Literal["user", "org"]
ScheduleType = Literal["cron", "interval"]
RunStatus = Literal["pending", "running", "completed", "failed"]


@dataclass
class Agent:
    id: str
    user_id: str
    name: str
    instructions: str
    agent_type: AgentType
    schedule_type: ScheduleType
    schedule_value: str
    model_id: Optional[str]
    allowed_sources: list[dict]  # [{source_id, modes: ["read","write"]}]
    allowed_actions: list[str]  # tool name whitelist for org agents
    is_enabled: bool
    is_deleted: bool
    created_at: datetime
    updated_at: datetime

    @classmethod
    def from_row(cls, row: dict) -> "Agent":
        allowed_sources = row.get("allowed_sources", [])
        if isinstance(allowed_sources, str):
            allowed_sources = json.loads(allowed_sources)

        allowed_actions = row.get("allowed_actions", [])
        if isinstance(allowed_actions, str):
            allowed_actions = json.loads(allowed_actions)

        model_id = row.get("model_id")
        if model_id:
            model_id = model_id.strip()

        return cls(
            id=row["id"].strip(),
            user_id=row["user_id"].strip(),
            name=row["name"],
            instructions=row["instructions"],
            agent_type=row["agent_type"],
            schedule_type=row["schedule_type"],
            schedule_value=row["schedule_value"],
            model_id=model_id,
            allowed_sources=allowed_sources,
            allowed_actions=allowed_actions,
            is_enabled=row["is_enabled"],
            is_deleted=row["is_deleted"],
            created_at=row["created_at"],
            updated_at=row["updated_at"],
        )

    def to_dict(self) -> dict:
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


@dataclass
class AgentRun:
    id: str
    agent_id: str
    status: RunStatus
    started_at: Optional[datetime]
    completed_at: Optional[datetime]
    execution_log: list[dict] = field(default_factory=list)
    summary: Optional[str] = None
    error_message: Optional[str] = None
    created_at: Optional[datetime] = None

    @classmethod
    def from_row(cls, row: dict) -> "AgentRun":
        execution_log = row.get("execution_log", [])
        if isinstance(execution_log, str):
            execution_log = json.loads(execution_log)

        return cls(
            id=row["id"].strip(),
            agent_id=row["agent_id"].strip(),
            status=row["status"],
            started_at=row.get("started_at"),
            completed_at=row.get("completed_at"),
            execution_log=execution_log,
            summary=row.get("summary"),
            error_message=row.get("error_message"),
            created_at=row.get("created_at"),
        )

    def to_dict(self, include_execution_log: bool = True) -> dict:
        d = {
            "id": self.id,
            "agent_id": self.agent_id,
            "status": self.status,
            "started_at": self.started_at.isoformat() if self.started_at else None,
            "completed_at": (
                self.completed_at.isoformat() if self.completed_at else None
            ),
            "summary": self.summary,
            "error_message": self.error_message,
            "created_at": self.created_at.isoformat() if self.created_at else None,
        }
        if include_execution_log:
            d["execution_log"] = self.execution_log
        return d
