import asyncio
import os
import yaml
import json
from pathlib import Path
import logging

logger = logging.getLogger(__name__)
logging.basicConfig(level=logging.INFO)

# --- RAGAS + LLM SETUP --------------------------------------------------------

RAGAS_AVAILABLE = False

try:
    from ragas import evaluate, EvaluationDataset, SingleTurnSample
    from ragas.metrics import faithfulness, context_recall
    from ragas.llms import llm_factory
    from openai import OpenAI

    base_url = os.environ.get("OPENAI_API_BASE")
    model_name = os.environ.get("OPENAI_MODEL_NAME", "gpt-4o-mini")
    api_key = os.environ.get("OPENAI_API_KEY")

    if not api_key:
        raise ValueError("OPENAI_API_KEY is not set")

    client = OpenAI(api_key=api_key, base_url=base_url)

    # RAGAS LLM wrapper (works with OpenRouter)
    llm = llm_factory(model=model_name, client=client)

    RAGAS_AVAILABLE = True

except Exception as e:
    logger.warning(f"RAGAS initialization failed: {e}")
    RAGAS_AVAILABLE = False


# --- ENV LOADING --------------------------------------------------------------

try:
    from dotenv import load_dotenv

    env_path = Path(__file__).parent.parent.parent.parent / ".env"
    if env_path.exists():
        load_dotenv(dotenv_path=env_path)
    else:
        load_dotenv()
except ImportError:
    pass


# --- DB -----------------------------------------------------------------------

from db.connection import get_db_pool


# --- HELPERS ------------------------------------------------------------------

def normalize_query(q: str) -> str:
    if not q:
        return ""
    return q.strip().lower().rstrip("?")


async def load_golden_set():
    dataset_path = Path(__file__).parent.parent / "datasets" / "golden_set.yaml"

    if not dataset_path.exists():
        logger.error(f"Golden dataset not found at {dataset_path}")
        return []

    with open(dataset_path, "r") as f:
        return yaml.safe_load(f) or []


async def fetch_latest_traces(pool, limit=100):
    query = """
        SELECT id, query, retrieved_doc_ids, generated_answer, context_chunks
        FROM eval_traces
        ORDER BY created_at DESC
        LIMIT $1
    """
    return await pool.fetch(query, limit)


def safe_parse_chunks(raw_chunks):
    """Robust parsing for stored JSON chunks"""
    if not raw_chunks:
        return []

    try:
        if isinstance(raw_chunks, str):
            chunks = json.loads(raw_chunks)
        else:
            chunks = raw_chunks

        return [
            c["text"]
            for c in chunks
            if isinstance(c, dict) and "text" in c
        ]
    except Exception as e:
        logger.warning(f"Failed to parse context_chunks: {e}")
        return []


# --- MAIN EVALUATION ----------------------------------------------------------

async def run_evaluation():
    logger.info("Starting RAG evaluation runner")

    golden_data = await load_golden_set()
    logger.info(f"Loaded {len(golden_data)} golden test cases")

    if not golden_data:
        logger.warning("Golden dataset is empty")
        return

    pool = await get_db_pool()
    traces = await fetch_latest_traces(pool)

    if not traces:
        logger.warning("No traces found in database")
        return

    dataset_dict = {
        "user_input": [],
        "response": [],
        "retrieved_contexts": [],
        "reference": [],
    }

    matched_count = 0

    for test_case in golden_data:
        query = test_case.get("query")
        reference = test_case.get("reference_answer")

        if not query or not reference:
            continue

        norm_query = normalize_query(query)

        trace = next(
            (t for t in traces if normalize_query(t["query"]) == norm_query),
            None,
        )

        if not trace:
            continue

        contexts = safe_parse_chunks(trace.get("context_chunks"))

        if not contexts:
            contexts = trace.get("retrieved_doc_ids") or []

        dataset_dict["user_input"].append(query)
        dataset_dict["response"].append(trace.get("generated_answer") or "")
        dataset_dict["retrieved_contexts"].append(contexts)
        dataset_dict["reference"].append(reference)

        matched_count += 1

    logger.info(f"Matched {matched_count} golden queries")

    if matched_count == 0:
        logger.warning("No matching traces found — run pipeline first")
        return

    if not RAGAS_AVAILABLE:
        logger.warning("RAGAS not available — skipping evaluation")
        return

    samples = [
        SingleTurnSample(
            user_input=dataset_dict["user_input"][i],
            response=dataset_dict["response"][i],
            retrieved_contexts=dataset_dict["retrieved_contexts"][i],
            reference=dataset_dict["reference"][i],
        )
        for i in range(len(dataset_dict["user_input"]))
    ]

    eval_dataset = EvaluationDataset(samples=samples)

    logger.info("Running RAGAS evaluation (context_recall, faithfulness)")

    try:
        result = evaluate(
            dataset=eval_dataset,
            metrics=[
                context_recall,
                faithfulness,
            ],
            llm=llm,
        )

        print("\n" + "-" * 60)
        print("EVALUATION RESULTS")
        print("-" * 60)
        print(result)
        print("-" * 60)

    except Exception as e:
        logger.exception(f"RAGAS evaluation failed: {e}")


# --- ENTRYPOINT ---------------------------------------------------------------

if __name__ == "__main__":
    asyncio.run(run_evaluation())