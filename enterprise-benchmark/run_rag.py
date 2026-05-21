"""run_rag.py — one-shot RAG driver for the EnterpriseRAG-Bench run.

For each benchmark question:
  1. POST /search to Omni searcher (mode=hybrid by default, limit=10)
  2. Read the full text of each retrieved document from the local .txt files
     (matches how Onyx's published BM25 + GPT-5.4 baseline assembles context)
  3. Build a context-stuffed prompt and call an OpenAI-compatible chat
     completions endpoint.
  4. Write {question_id, answer, document_ids} as a JSONL line

Output schema matches EnterpriseRAG-Bench's `answers_<system>.jsonl`.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import logging
import os
import random
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import httpx
from tqdm.asyncio import tqdm_asyncio

logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")
log = logging.getLogger("run_rag")


# OpenAI-compatible inference settings. KIMI_* env vars are still accepted for
# backwards compatibility with earlier exploratory runs.
LLM_MODEL = os.environ.get(
    "OPENAI_COMPAT_MODEL",
    os.environ.get("DEEPSEEK_MODEL", os.environ.get("KIMI_MODEL", "deepseek-v4-pro")),
)
LLM_API_URL = os.environ.get(
    "OPENAI_COMPAT_API_URL",
    os.environ.get(
        "DEEPSEEK_API_URL",
        os.environ.get("KIMI_API_URL", "https://api.deepseek.com/v1"),
    ),
)
LLM_TEMPERATURE = 1.0
LLM_TOP_P = 0.95
# Big enough that thinking tokens + answer don't squeeze each other out.
# Empty-content responses on small max_tokens were the dominant failure mode
# in the dipstick run when this was 1024.
LLM_MAX_TOKENS = int(
    os.environ.get(
        "OPENAI_COMPAT_MAX_TOKENS", os.environ.get("KIMI_MAX_TOKENS", "8192")
    )
)
# Some OpenAI-compatible endpoints support a provider-specific thinking field.
# Leave disabled unless the target provider explicitly accepts it.
LLM_THINKING_ENABLED = (
    os.environ.get(
        "OPENAI_COMPAT_THINKING_ENABLED",
        os.environ.get("KIMI_THINKING_ENABLED", "false"),
    ).lower()
    == "true"
)

# Soft cap on stuffed context, to stay below the model's effective budget once
# the system + question + answer-tokens are accounted for.
DEFAULT_CONTEXT_CHAR_BUDGET = 200_000


ANSWER_GEN_PROMPT = """You are answering a question using a set of retrieved enterprise documents.

Use only information from the documents below. Be concise and factually precise. If the answer is not present in the documents, state that explicitly.

# Documents

{context}

# Question

{question}

# Answer
"""


@dataclass(frozen=True)
class Question:
    question_id: str
    text: str


@dataclass(frozen=True)
class Answer:
    question_id: str
    answer: str
    document_ids: list[str]


def _parse_question(row: dict[str, Any]) -> Question:
    return Question(question_id=row["question_id"], text=row["question"])


def _build_doc_index(data_dir: Path) -> dict[str, Path]:
    """Walk the corpus dir and build a dsid → path map.

    Filenames are `dsid_<32hex>_<semantic-name>.txt`. The dsid token is the
    first 37 chars (5 + 32). We accept any depth — files live under nested
    subdirs (e.g. slack/vendors/, gmail subdirs).
    """
    index: dict[str, Path] = {}
    for p in data_dir.rglob("*.txt"):
        name = p.name
        if not name.startswith("dsid_") or len(name) < 37:
            continue
        dsid = name[:37]
        # If a dsid appears twice somehow, last write wins; not expected here.
        index[dsid] = p
    return index


async def _retrieve(
    client: httpx.AsyncClient,
    base_url: str,
    q_text: str,
    mode: str,
    limit: int,
) -> list[str]:
    body = {"query": q_text, "mode": mode, "limit": limit}
    resp = await client.post(f"{base_url}/search", json=body, timeout=60.0)
    resp.raise_for_status()
    payload = resp.json()
    seen: set[str] = set()
    out: list[str] = []
    for r in payload.get("results", []):
        ext = r["document"]["external_id"]
        if ext in seen:
            continue
        seen.add(ext)
        out.append(ext)
    return out


def _build_context(
    doc_ids: list[str],
    doc_index: dict[str, Path],
    char_budget: int,
) -> str:
    """Concatenate retrieved doc bodies until char budget is reached."""
    parts: list[str] = []
    used = 0
    for i, dsid in enumerate(doc_ids):
        path = doc_index.get(dsid)
        if path is None:
            log.warning("dsid not found in doc index: %s", dsid)
            continue
        try:
            text = path.read_text(encoding="utf-8", errors="ignore")
        except OSError as e:
            log.warning("failed to read %s: %s", path, e)
            continue
        block = f"## Document {i + 1} (dsid: {dsid})\n\n{text}\n"
        remaining = char_budget - used
        if remaining <= 0:
            break
        if len(block) > remaining:
            parts.append(block[:remaining])
            break
        parts.append(block)
        used += len(block)
    return "\n".join(parts)


LLM_MAX_ATTEMPTS = int(os.environ.get("LLM_MAX_ATTEMPTS", "6"))
LLM_RETRY_BASE_DELAY = float(os.environ.get("LLM_RETRY_BASE_DELAY", "2.0"))
# Hard cap on any single retry sleep, regardless of what Retry-After or the
# exponential backoff says. Some providers return very large Retry-After values,
# which turns one transient 429 into a stalled benchmark. Cap defensively.
LLM_RETRY_MAX_DELAY = float(os.environ.get("LLM_RETRY_MAX_DELAY", "30.0"))


async def _generate_answer(
    client: httpx.AsyncClient,
    api_key: str,
    prompt: str,
) -> str:
    body: dict[str, Any] = {
        "model": LLM_MODEL,
        "messages": [{"role": "user", "content": prompt}],
        "temperature": LLM_TEMPERATURE,
        "top_p": LLM_TOP_P,
        "max_tokens": LLM_MAX_TOKENS,
    }
    if LLM_THINKING_ENABLED:
        body["thinking"] = {"type": "enabled"}

    url = f"{LLM_API_URL.rstrip('/')}/chat/completions"
    headers = {"Authorization": f"Bearer {api_key}"}
    last_status: int | None = None
    last_text: str | None = None

    for attempt in range(LLM_MAX_ATTEMPTS):
        try:
            resp = await client.post(url, json=body, headers=headers, timeout=180.0)
        except (httpx.TransportError, httpx.TimeoutException) as e:
            if attempt == LLM_MAX_ATTEMPTS - 1:
                raise
            delay = LLM_RETRY_BASE_DELAY * (2**attempt) + random.uniform(0, 0.5)
            log.warning("LLM transport error %s, retry in %.1fs", e, delay)
            await asyncio.sleep(delay)
            continue

        if resp.status_code == 200:
            return resp.json()["choices"][0]["message"]["content"].strip()

        last_status = resp.status_code
        last_text = resp.text
        if resp.status_code == 429 or 500 <= resp.status_code < 600:
            retry_after = resp.headers.get("Retry-After")
            try:
                base = (
                    float(retry_after)
                    if retry_after
                    else LLM_RETRY_BASE_DELAY * (2**attempt)
                )
            except ValueError:
                base = LLM_RETRY_BASE_DELAY * (2**attempt)
            base = min(base, LLM_RETRY_MAX_DELAY)
            delay = base + random.uniform(0, 0.5)
            log.warning(
                "LLM HTTP %s (attempt %d/%d), retry in %.1fs",
                resp.status_code,
                attempt + 1,
                LLM_MAX_ATTEMPTS,
                delay,
            )
            await asyncio.sleep(delay)
            continue

        resp.raise_for_status()

    raise RuntimeError(
        f"LLM failed after {LLM_MAX_ATTEMPTS} attempts (last status={last_status}): {last_text}"
    )


async def _process_question(
    sem: asyncio.Semaphore,
    search_client: httpx.AsyncClient,
    llm_client: httpx.AsyncClient,
    api_key: str,
    base_url: str,
    q: Question,
    mode: str,
    limit: int,
    doc_index: dict[str, Path],
    char_budget: int,
) -> Answer | None:
    async with sem:
        try:
            doc_ids = await _retrieve(search_client, base_url, q.text, mode, limit)
        except Exception as e:
            log.error("retrieval failed for %s: %s", q.question_id, e)
            return None

        if not doc_ids:
            log.warning("no docs retrieved for %s", q.question_id)
            return Answer(
                question_id=q.question_id,
                answer="The answer is not available in the provided documents.",
                document_ids=[],
            )

        context = _build_context(doc_ids, doc_index, char_budget)
        prompt = ANSWER_GEN_PROMPT.format(context=context, question=q.text)

        try:
            answer = await _generate_answer(llm_client, api_key, prompt)
        except Exception as e:
            log.error("LLM call failed for %s: %s", q.question_id, e)
            return None

        return Answer(question_id=q.question_id, answer=answer, document_ids=doc_ids)


def _load_existing_ids(out_path: Path) -> set[str]:
    if not out_path.exists():
        return set()
    seen: set[str] = set()
    with out_path.open("r", encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            try:
                row = json.loads(line)
            except json.JSONDecodeError:
                continue
            qid = row.get("question_id")
            if qid:
                seen.add(qid)
    return seen


async def main_async(args: argparse.Namespace) -> int:
    api_key = (
        os.environ.get("OPENAI_COMPAT_API_KEY")
        or os.environ.get("DEEPSEEK_API_KEY")
        or os.environ.get("KIMI_API_KEY")
    )
    if not api_key:
        log.error("OPENAI_COMPAT_API_KEY or DEEPSEEK_API_KEY env var is required")
        return 2

    questions: list[Question] = []
    with args.questions.open("r", encoding="utf-8") as f:
        for line in f:
            line = line.rstrip("\n")
            if not line:
                continue
            questions.append(_parse_question(json.loads(line)))
    log.info("loaded %d questions", len(questions))

    if args.sample is not None and args.sample < len(questions):
        questions = questions[: args.sample]
        log.info("sampled first %d questions", len(questions))

    args.output_dir.mkdir(parents=True, exist_ok=True)
    out_path = args.output_dir / f"answers_{args.system_name}.jsonl"

    existing = _load_existing_ids(out_path) if args.resume else set()
    if existing:
        questions = [q for q in questions if q.question_id not in existing]
        log.info(
            "resume: skipping %d already-answered, %d remaining",
            len(existing),
            len(questions),
        )

    if not questions:
        log.info("nothing to do")
        return 0

    log.info("building doc index from %s ...", args.data_dir)
    doc_index = _build_doc_index(args.data_dir)
    log.info("indexed %d documents", len(doc_index))

    base_url = args.searcher_url.rstrip("/")
    sem = asyncio.Semaphore(args.concurrency)

    async with (
        httpx.AsyncClient() as search_client,
        httpx.AsyncClient() as llm_client,
    ):
        coros = [
            _process_question(
                sem,
                search_client,
                llm_client,
                api_key,
                base_url,
                q,
                args.mode,
                args.limit,
                doc_index,
                args.context_chars,
            )
            for q in questions
        ]
        with out_path.open("a", encoding="utf-8") as out:
            for fut in tqdm_asyncio.as_completed(
                coros, total=len(coros), desc=args.mode
            ):
                ans = await fut
                if ans is None:
                    continue
                out.write(
                    json.dumps(
                        {
                            "question_id": ans.question_id,
                            "answer": ans.answer,
                            "document_ids": ans.document_ids,
                        }
                    )
                    + "\n"
                )
                out.flush()

    log.info("wrote answers to %s", out_path)
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--questions",
        type=Path,
        default=Path(__file__).parent / "data" / "questions.jsonl",
    )
    parser.add_argument(
        "--data-dir",
        type=Path,
        default=Path(__file__).parent / "data",
        help="root of the unzipped corpus (per-source-type subdirs of .txt files)",
    )
    parser.add_argument(
        "--searcher-url",
        default=os.environ.get(
            "BENCH_SEARCHER_URL",
            f"http://localhost:{os.environ.get('SEARCHER_PORT', '3001')}",
        ),
    )
    parser.add_argument(
        "--mode", choices=["fulltext", "semantic", "hybrid"], default="hybrid"
    )
    parser.add_argument("--limit", type=int, default=10)
    parser.add_argument(
        "--context-chars",
        type=int,
        default=DEFAULT_CONTEXT_CHAR_BUDGET,
        help="soft char cap on the stuffed context block",
    )
    parser.add_argument(
        "--concurrency",
        type=int,
        default=10,
        help="Parallel in-flight LLM calls. Tune this to provider rate limits.",
    )
    parser.add_argument(
        "--sample",
        type=int,
        default=None,
        help="if set, take only the first N questions (for dipstick runs)",
    )
    parser.add_argument(
        "--system-name",
        default="omni_hybrid_deepseek_v4_pro",
        help="suffix for the answers file: answers_<system_name>.jsonl",
    )
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=Path(__file__).parent / "answer_evaluation",
    )
    parser.add_argument(
        "--resume",
        action="store_true",
        help="skip questions already present in the answers file",
    )
    args = parser.parse_args()

    return asyncio.run(main_async(args))


if __name__ == "__main__":
    sys.exit(main())
