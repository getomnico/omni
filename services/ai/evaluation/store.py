import json
from db.connection import get_db_pool
from evaluation.models import EvalTrace, EvalScore
import logging

logger = logging.getLogger(__name__)

async def save_eval_trace(trace: EvalTrace) -> None:
    """Asynchronously save an evaluation trace to the database."""
    try:
        pool = await get_db_pool()
        query = """
            INSERT INTO eval_traces (
                id, query, task_family, temporal_type,
                retrieved_doc_ids, retrieved_scores, retrieval_views,
                fts_result_count, semantic_result_count, retrieval_latency_ms,
                context_chunks, context_token_count, context_truncated,
                chunk_duplication_rate, generated_answer, citations,
                generation_tokens, generation_latency_ms,
                source_types, languages, golden_set_id, is_production,
                user_id, chat_id, created_at
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                $11::jsonb, $12, $13, $14, $15, $16::jsonb,
                $17, $18, $19, $20, $21, $22, $23, $24, $25
            )
        """
        
        context_chunks_json = json.dumps(trace.context_chunks) if trace.context_chunks else None
        citations_json = json.dumps(trace.citations) if trace.citations else None
        
        await pool.execute(
            query,
            trace.id,
            trace.query,
            trace.task_family,
            trace.temporal_type,
            trace.retrieved_doc_ids,
            trace.retrieved_scores,
            trace.retrieval_views,
            trace.fts_result_count,
            trace.semantic_result_count,
            trace.retrieval_latency_ms,
            context_chunks_json,
            trace.context_token_count,
            trace.context_truncated,
            trace.chunk_duplication_rate,
            trace.generated_answer,
            citations_json,
            trace.generation_tokens,
            trace.generation_latency_ms,
            trace.source_types,
            trace.languages,
            trace.golden_set_id,
            trace.is_production,
            trace.user_id,
            trace.chat_id,
            trace.created_at
        )
        logger.debug(f"Saved eval trace {trace.id}")
    except Exception as e:
        logger.error(f"Failed to save eval trace: {e}")

async def save_eval_score(score: EvalScore) -> None:
    """Save a computed evaluation score."""
    try:
        pool = await get_db_pool()
        query = """
            INSERT INTO eval_scores (
                id, trace_id, metric_name, metric_category,
                score, raw_score, reasoning, judge_model, created_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        """
        await pool.execute(
            query,
            score.id,
            score.trace_id,
            score.metric_name,
            score.metric_category,
            score.score,
            score.raw_score,
            score.reasoning,
            score.judge_model,
            score.created_at
        )
        logger.debug(f"Saved eval score {score.metric_name} for trace {score.trace_id}")
    except Exception as e:
        logger.error(f"Failed to save eval score: {e}")
