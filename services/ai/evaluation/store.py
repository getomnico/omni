import logging
from pathlib import Path

from db.connection import get_db_pool
from evaluation.models import EvalScore

logger = logging.getLogger(__name__)

_MIGRATIONS_DIR = Path(__file__).parent / "migrations"
_schema_initialized = False


async def ensure_eval_schema() -> None:
    """Run evaluation migrations if not already applied. Safe to call multiple times."""
    global _schema_initialized
    if _schema_initialized:
        return
    pool = await get_db_pool()
    for sql_file in sorted(_MIGRATIONS_DIR.glob("*.sql")):
        sql = sql_file.read_text().strip()
        if sql:
            try:
                await pool.execute(sql)
            except Exception as e:
                err = str(e).lower()
                if not any(x in err for x in ["already exists", "duplicate"]):
                    raise
    _schema_initialized = True

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
