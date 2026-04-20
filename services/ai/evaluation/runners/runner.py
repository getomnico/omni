import asyncio
import logging
import os
import uuid
import yaml
from datetime import datetime
from pathlib import Path
from typing import Optional

logger = logging.getLogger(__name__)
logging.basicConfig(level=logging.INFO)

# ---------------------------------------------------------------------------
# Optional imports
# ---------------------------------------------------------------------------

RAGAS_AVAILABLE = False
try:
    from ragas import evaluate, EvaluationDataset, SingleTurnSample
    from ragas.metrics import faithfulness, context_recall
    from ragas.llms import llm_factory
    from ragas.run_config import RunConfig
    from openai import AsyncOpenAI
    RAGAS_AVAILABLE = True
except Exception as e:
    logger.warning(f"RAGAS initialization skipped: {e}")


from evaluation.config import EvalConfig
from evaluation.models import EvalScore
from evaluation.store import save_eval_score
from evaluation.reporters.console import print_results


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _make_llm(config: EvalConfig):
    api_key = os.environ.get("EVAL_OPENAI_API_KEY")
    if not api_key:
        raise RuntimeError("EVAL_OPENAI_API_KEY is not set")
    base_url = os.environ.get("EVAL_OPENAI_API_BASE")
    # Use a higher max_tokens budget for thinking/reasoning models that consume
    # tokens for chain-of-thought before emitting the actual JSON output.
    max_tokens = int(os.environ.get("EVAL_MAX_TOKENS", "1024"))
    client = AsyncOpenAI(api_key=api_key, base_url=base_url)
    return llm_factory(model=config.judge_model, client=client, max_tokens=max_tokens), config.judge_model


# ---------------------------------------------------------------------------
# Mode 1: Synthetic — read context directly from golden_set.yaml
# ---------------------------------------------------------------------------

async def _load_synthetic_samples(golden_path: Path) -> list[dict]:
    """Return entries from golden_set.yaml that have both reference_answer and context_chunks."""
    if not golden_path.exists():
        logger.error(f"Golden set not found: {golden_path}")
        return []
    with open(golden_path) as f:
        data = yaml.safe_load(f) or []
    usable = [
        e for e in data
        if e.get("reference_answer") and e.get("context_chunks")
    ]

    samples = []
    for e in usable:
        samples.append({
            "id": e["id"],
            "query": e["query"],
            "reference_answer": e["reference_answer"],
            "contexts": [c["text"] for c in e.get("context_chunks", []) if isinstance(c, dict) and "text" in c],
            "response": e["reference_answer"],
            "source": "synthetic",
            "trace_id": str(uuid.uuid4()),
        })

    logger.info(f"Loaded {len(samples)}/{len(data)} usable synthetic samples")
    return samples


# ---------------------------------------------------------------------------
# Shared RAGAS scoring
# ---------------------------------------------------------------------------

async def _score_samples(
    samples: list[dict],
    config: EvalConfig,
    pool=None,
) -> dict[str, float]:
    """Run RAGAS on samples; persist EvalScore rows; return aggregate scores."""
    if not RAGAS_AVAILABLE:
        logger.warning("RAGAS not available — skipping")
        return {}

    llm, judge_model = _make_llm(config)

    ragas_samples = [
        SingleTurnSample(
            user_input=s["query"],
            response=s.get("response") or s.get("reference_answer", ""),
            retrieved_contexts=s["contexts"],
            reference=s["reference_answer"],
        )
        for s in samples
    ]

    dataset = EvaluationDataset(samples=ragas_samples)
    result = evaluate(
        dataset=dataset,
        metrics=[context_recall, faithfulness],
        llm=llm,
        run_config=RunConfig(max_workers=16),
        batch_size=min(len(samples), 16),
    )

    scores_df = result.to_pandas()

    for i, sample in enumerate(samples):
        row = scores_df.iloc[i]
        logger.info(
            "\n--- Sample %s ---\n"
            "  query:     %s\n"
            "  reference: %s\n"
            "  response:  %s\n"
            "  contexts:  %s\n"
            "  faithfulness:   %s\n"
            "  context_recall: %s",
            sample["id"],
            sample["query"],
            sample["reference_answer"],
            sample.get("response", ""),
            sample["contexts"],
            row.get("faithfulness"),
            row.get("context_recall"),
        )

    agg: dict[str, float] = {}

    import math
    for metric_name in ("faithfulness", "context_recall"):
        if metric_name not in scores_df.columns:
            continue
        col = scores_df[metric_name].dropna()
        agg[metric_name] = float(col.mean()) if len(col) > 0 else 0.0

        for i, sample in enumerate(samples):
            row_score = scores_df.iloc[i].get(metric_name)
            if row_score is None or math.isnan(row_score):
                continue
            score_obj = EvalScore(
                id=str(uuid.uuid4()),
                trace_id=sample.get("trace_id") or sample["id"],
                metric_name=metric_name,
                metric_category="generation" if metric_name == "faithfulness" else "retrieval",
                score=float(row_score),
                judge_model=judge_model,
                created_at=datetime.utcnow(),
            )
            await save_eval_score(score_obj)

    return agg


# ---------------------------------------------------------------------------
# Public API
# ---------------------------------------------------------------------------

async def run_evaluation(config: Optional[EvalConfig] = None) -> dict[str, float]:
    """
    Run evaluation in synthetic mode (no DB traces required).

    Returns aggregate scores dict: {"faithfulness": 0.72, "context_recall": 0.55}
    """
    if config is None:
        config = EvalConfig.from_env()

    golden_path = Path(config.golden_set_path)

    samples = await _load_synthetic_samples(golden_path)

    if not samples:
        logger.warning("No usable samples — run generate_golden.py first")
        return {}

    scores = await _score_samples(samples, config)

    thresholds = {
        "faithfulness": config.faithfulness_threshold,
        "context_recall": config.context_recall_threshold,
    }
    print_results(scores, thresholds)
    return scores


if __name__ == "__main__":
    asyncio.run(run_evaluation())
