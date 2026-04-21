import uuid
from datetime import datetime
from typing import List, Optional
from pydantic import BaseModel, Field


class EvalScore(BaseModel):
    """Represents a single metric score for a trace."""
    id: str = Field(default_factory=lambda: str(uuid.uuid4()))
    trace_id: str
    metric_name: str
    metric_category: str  # 'retrieval', 'generation', 'temporal', 'operational'
    score: float
    raw_score: Optional[float] = None
    reasoning: Optional[str] = None
    judge_model: Optional[str] = None
    created_at: datetime = Field(default_factory=datetime.utcnow)

class GoldenSample(BaseModel):
    """A test case from the golden dataset."""
    id: str
    query: str
    task_family: Optional[str] = None
    temporal_type: Optional[str] = None
    expected_source_types: Optional[List[str]] = None
    expected_languages: Optional[List[str]] = None
    reference_answer: Optional[str] = None
    reference_doc_ids: Optional[List[str]] = None
    tags: List[str] = Field(default_factory=list)
    difficulty: Optional[str] = None
