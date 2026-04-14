import uuid
from datetime import datetime
from enum import Enum
from typing import List, Optional, Any, Dict
from pydantic import BaseModel, Field

class TaskFamily(str, Enum):
    CURRENT_STATE = "current_state"
    HISTORICAL_STATE = "historical_state"
    TIMELINE_CHANGE = "timeline_change"
    SUMMARY_BRIEFING = "summary_briefing"
    ROOT_CAUSE = "root_cause"
    CROSS_DOCUMENT = "cross_document"
    UNKNOWN = "unknown"

class TemporalType(str, Enum):
    CURRENT = "current"
    HISTORICAL = "historical"
    EVOLUTION = "evolution"
    RECENT = "recent"
    NONE = "none"

class EvalTrace(BaseModel):
    """Represents a single execution trace of the RAG pipeline."""
    id: str = Field(default_factory=lambda: str(uuid.uuid4()))
    query: str
    task_family: Optional[str] = None
    temporal_type: Optional[str] = None
    
    # Retrieval Phase
    retrieved_doc_ids: List[str] = Field(default_factory=list)
    retrieved_scores: List[float] = Field(default_factory=list)
    retrieval_views: List[str] = Field(default_factory=list)
    fts_result_count: Optional[int] = None
    semantic_result_count: Optional[int] = None
    retrieval_latency_ms: Optional[int] = None
    
    # Context Assembly
    context_chunks: Optional[List[Dict[str, Any]]] = None
    context_token_count: Optional[int] = None
    context_truncated: bool = False
    chunk_duplication_rate: Optional[float] = None
    
    # Generation Phase
    generated_answer: Optional[str] = None
    citations: Optional[List[Dict[str, Any]]] = None
    generation_tokens: Optional[int] = None
    generation_latency_ms: Optional[int] = None
    
    # Metadata
    source_types: List[str] = Field(default_factory=list)
    languages: List[str] = Field(default_factory=list)
    golden_set_id: Optional[str] = None
    is_production: bool = False
    user_id: Optional[str] = None
    chat_id: Optional[str] = None
    created_at: datetime = Field(default_factory=datetime.utcnow)

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
