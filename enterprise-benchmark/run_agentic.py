"""run_agentic.py — Agentic-loop driver for the EnterpriseRAG-Bench run.

For each benchmark question:
  1. Insert a fresh chat row + a single user message in the omni DB
     (omni-ai's stream_chat reads its initial message thread from chat_messages.)
  2. Open an SSE stream to omni-ai's GET /chat/{chat_id}/stream
  3. Watch for the assistant `save_message` event whose content_blocks include
     a tool_use named `submit_answer`; capture its `input` dict
  4. Write {question_id, answer, document_ids} as a JSONL line

Requires omni-ai to be running with BENCHMARK_MODE=true so the SubmitAnswerHandler
is registered. Tools available to the agent: search_documents, read_document,
submit_answer (per services/ai/routers/chat.py:_build_registry).

We bypass omni-web — no auth, no message persistence to orchestrate. omni-ai's
loop runs entirely in memory across iterations; the `save_message` SSE events
are informational and we just parse them for the final tool input.

Output schema matches EnterpriseRAG-Bench's `answers_<system>.jsonl`.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import logging
import os
import sys
from collections.abc import AsyncGenerator
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import httpx
import psycopg
import ulid
from tqdm.asyncio import tqdm_asyncio

logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")
log = logging.getLogger("run_agentic")


SUBMIT_ANSWER_TOOL = "submit_answer"


@dataclass(frozen=True)
class Question:
    question_id: str
    text: str


@dataclass(frozen=True)
class Answer:
    question_id: str
    answer: str
    document_ids: list[str]
    chat_id: str


@dataclass
class RetrievalTrace:
    """Audit trail of every search + its results for a single question."""

    question_id: str
    chat_id: str
    searches: list[dict]
    events: list[dict]
    read_doc_ids: list[str]
    cited_doc_ids: list[str]
    # doc_id -> best_score (keeps highest score seen)
    doc_scores: dict[str, float]


def _parse_question(row: dict[str, Any]) -> Question:
    return Question(question_id=row["question_id"], text=row["question"])


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


def _get_bench_user_id(conn: psycopg.Connection, user_email: str | None = None) -> str:
    """Find the benchmark user id (load_corpus.py creates a single user row).

    If multiple users exist, prefer the one whose email looks like a bench user.
    If user_email is provided, match exactly.
    """
    with conn.cursor() as cur:
        if user_email:
            cur.execute("SELECT id FROM users WHERE email = %s LIMIT 1", (user_email,))
            row = cur.fetchone()
            if row:
                return row[0].strip()
            # Create user if not found
            cur.execute(
                "INSERT INTO users (id, email, first_name, last_name) VALUES (gen_random_uuid()::text, %s, 'BM25', 'Bench') RETURNING id",
                (user_email,),
            )
            row = cur.fetchone()
            return row[0].strip()

        cur.execute(
            """
            SELECT id FROM users
             ORDER BY created_at ASC
             LIMIT 1
            """
        )
        row = cur.fetchone()
        if not row:
            raise RuntimeError(
                "No users in the omni_benchmark DB; run load_corpus.py preflight first"
            )
        return row[0].strip()


def _get_default_model_id(conn: psycopg.Connection) -> str | None:
    with conn.cursor() as cur:
        cur.execute("SELECT id FROM models WHERE is_default = TRUE LIMIT 1")
        row = cur.fetchone()
        return row[0].strip() if row else None


def _create_chat_with_message(
    conn: psycopg.Connection,
    user_id: str,
    model_id: str | None,
    question_text: str,
) -> str:
    """Insert a chat row + a single user message, return chat_id."""
    chat_id = str(ulid.ULID())
    msg_id = str(ulid.ULID())
    user_msg = {"role": "user", "content": question_text}

    with conn.cursor() as cur:
        cur.execute(
            """
            INSERT INTO chats (id, user_id, model_id, title)
            VALUES (%s, %s, %s, %s)
            """,
            (chat_id, user_id, model_id, question_text[:80]),
        )
        cur.execute(
            """
            INSERT INTO chat_messages
                (id, chat_id, message_seq_num, message, content_text, parent_id)
            VALUES (%s, %s, 1, %s::jsonb, %s, NULL)
            """,
            (msg_id, chat_id, json.dumps(user_msg), question_text),
        )
    conn.commit()
    return chat_id


async def _iter_sse_events(
    response: httpx.Response,
) -> AsyncGenerator[tuple[str, str], None]:
    """Yield (event_name, data_str) tuples from an SSE stream.

    SSE messages are separated by a blank line. Within a message, lines may be
    'event: <name>' and 'data: <payload>'. omni-ai always emits exactly one
    `event:` and one `data:` line per message, so this minimal parser is enough.
    """
    event_name: str | None = None
    data_lines: list[str] = []
    async for line in response.aiter_lines():
        if line == "":
            if event_name is not None:
                yield event_name, "\n".join(data_lines)
            event_name = None
            data_lines = []
        elif line.startswith("event:"):
            event_name = line[len("event:") :].strip()
        elif line.startswith("data:"):
            data_lines.append(line[len("data:") :].lstrip())
    # Flush any trailing event without a terminating blank line
    if event_name is not None and data_lines:
        yield event_name, "\n".join(data_lines)


def _extract_submit_answer(assistant_message: dict[str, Any]) -> dict[str, Any] | None:
    """Return the submit_answer tool_use input dict if present, else None."""
    if assistant_message.get("role") != "assistant":
        return None
    for block in assistant_message.get("content") or []:
        if not isinstance(block, dict):
            continue
        if block.get("type") == "tool_use" and block.get("name") == SUBMIT_ANSWER_TOOL:
            tool_input = block.get("input")
            if isinstance(tool_input, dict):
                return tool_input
            if isinstance(tool_input, str):
                try:
                    return json.loads(tool_input)
                except json.JSONDecodeError:
                    return None
    return None


def _persist_message(
    conn: psycopg.Connection,
    chat_id: str,
    seq_num: int,
    message: dict,
    content_text: str,
) -> None:
    """Insert a message into chat_messages exactly like omni-web does."""
    with conn.cursor() as cur:
        cur.execute(
            """
            INSERT INTO chat_messages
                (id, chat_id, message_seq_num, message, content_text, parent_id)
            VALUES (%s, %s, %s, %s::jsonb, %s, NULL)
            """,
            (str(ulid.ULID()), chat_id, seq_num, json.dumps(message), content_text),
        )
    conn.commit()


import re

_DSID_RE = re.compile(r"dsid_[a-f0-9]{32}")
_ULID_RE = re.compile(r"\b[0-9A-HJKMNP-TV-Z]{26}\b")
_SCORE_RE = re.compile(r"\[Relevance Score:\s*([0-9.eE+-]+)\]")
_SOURCE_RE = re.compile(r"benchmark://[^/]+/(dsid_[a-f0-9]{32})__")


def _ordered_unique(values: list[str]) -> list[str]:
    seen: set[str] = set()
    out: list[str] = []
    for value in values:
        if value and value not in seen:
            seen.add(value)
            out.append(value)
    return out


def _extract_dsid(value: Any) -> str | None:
    if not isinstance(value, str):
        return None
    match = _DSID_RE.search(value)
    return match.group(0) if match else None


def _event_from_tool_use(
    block: dict[str, Any], seq_num: int | None = None
) -> dict[str, Any]:
    name = block.get("name")
    tool_input = block.get("input") if isinstance(block.get("input"), dict) else {}
    event: dict[str, Any] = {"type": "tool_use", "name": name}
    if seq_num is not None:
        event["message_seq_num"] = seq_num
    if name == "search_documents":
        event.update(
            {
                "query": tool_input.get("query"),
                "document_id": tool_input.get("document_id"),
                "limit": tool_input.get("limit"),
            }
        )
    elif name == "read_document":
        doc_ref = tool_input.get("id")
        event.update(
            {
                "id": doc_ref,
                "document_id": _extract_dsid(doc_ref),
                "name": tool_input.get("name"),
                "start_line": tool_input.get("start_line"),
                "end_line": tool_input.get("end_line"),
            }
        )
    elif name == SUBMIT_ANSWER_TOOL:
        event.update(
            {
                "answer_chars": len(str(tool_input.get("answer") or "")),
                "document_ids": _normalize_submitted_doc_ids(
                    tool_input.get("document_ids")
                ),
            }
        )
    else:
        event["input"] = tool_input
    return event


def _normalize_submitted_doc_ids(value: Any) -> list[str]:
    if value is None:
        return []
    if isinstance(value, str):
        candidates = _DSID_RE.findall(value) + _ULID_RE.findall(value)
    elif isinstance(value, list):
        candidates = []
        for item in value:
            dsid = _extract_dsid(item)
            if dsid:
                candidates.append(dsid)
            elif isinstance(item, str) and _ULID_RE.fullmatch(item.strip()):
                candidates.append(item.strip())
    else:
        return []
    return _ordered_unique(candidates)[:10]


def _extract_search_results(tool_result: dict) -> dict[str, float]:
    """Extract {doc_id: score} from a search_result tool result block.

    The dsid_* ID is embedded in the 'source' URL field.
    The score is in a text block like '[Relevance Score: 0.123456]'.
    """
    results: dict[str, float] = {}
    content = tool_result.get("content", [])
    if not isinstance(content, list):
        return results

    for block in content:
        if not isinstance(block, dict) or block.get("type") != "search_result":
            continue

        # Extract doc ID from source URL
        source = block.get("source", "")
        m = _DSID_RE.search(source)
        if not m:
            continue
        doc_id = m.group(0)

        # Extract score from content text blocks
        score = 0.0
        for child in block.get("content", []):
            if isinstance(child, dict) and child.get("type") == "text":
                text = child.get("text", "")
                sm = _SCORE_RE.search(text)
                if sm:
                    try:
                        score = float(sm.group(1))
                    except ValueError:
                        pass
                    break  # score found

        # Keep highest score for each doc
        if doc_id not in results or score > results[doc_id]:
            results[doc_id] = score

    return results


def _extract_search_result_events(tool_result: dict) -> list[dict[str, Any]]:
    """Extract raw search result rows for the retrieval trace."""
    events: list[dict[str, Any]] = []
    content = tool_result.get("content", [])
    if not isinstance(content, list):
        return events

    for rank, block in enumerate(content, start=1):
        if not isinstance(block, dict) or block.get("type") != "search_result":
            continue
        source = block.get("source", "")
        source_match = _SOURCE_RE.search(source)
        doc_id = source_match.group(1) if source_match else _extract_dsid(source)

        score = 0.0
        document_ulid = None
        snippets: list[str] = []
        for child in block.get("content", []):
            if isinstance(child, dict) and child.get("type") == "text":
                text = str(child.get("text") or "")
                sm = _SCORE_RE.search(text)
                if sm:
                    try:
                        score = float(sm.group(1))
                    except ValueError:
                        score = 0.0
                if text.startswith("[Document ID:"):
                    document_ulid = (
                        text.removeprefix("[Document ID:").removesuffix("]").strip()
                    )
                elif not text.startswith("[") and len(snippets) < 3:
                    snippets.append(text[:500])

        if doc_id:
            events.append(
                {
                    "type": "search_result",
                    "rank": rank,
                    "document_id": doc_id,
                    "internal_document_id": document_ulid,
                    "title": block.get("title"),
                    "source": source,
                    "score": score,
                    "snippets": snippets,
                }
            )
    return events


async def _stream_one_question(
    client: httpx.AsyncClient,
    base_url: str,
    chat_id: str,
    db_dsn: str,
    timeout_seconds: float,
) -> tuple[dict[str, Any] | None, RetrievalTrace]:
    """Open the SSE stream, persist conversation, and capture submit_answer.

    Returns (submit_answer_input | None, retrieval_trace).
    """
    url = f"{base_url.rstrip('/')}/chat/{chat_id}/stream"
    trace = RetrievalTrace(
        question_id="",
        chat_id=chat_id,
        searches=[],
        events=[],
        read_doc_ids=[],
        cited_doc_ids=[],
        doc_scores={},
    )

    # Track the last assistant message text for fallback when no submit_answer
    last_assistant_text = ""

    # message_seq_num 1 is the initial user message already inserted
    next_seq = 2

    try:
        async with client.stream(
            "GET",
            url,
            timeout=httpx.Timeout(timeout_seconds, connect=10.0),
        ) as resp:
            resp.raise_for_status()
            async for event_name, data in _iter_sse_events(resp):
                if event_name == "save_message":
                    try:
                        msg = json.loads(data)
                    except json.JSONDecodeError:
                        continue

                    # Persist assistant message to DB
                    await asyncio.to_thread(
                        _persist_message_blocking,
                        db_dsn,
                        chat_id,
                        next_seq,
                        msg,
                        _extract_text(msg),
                    )
                    next_seq += 1

                    # Update last assistant text (for fallback when no submit_answer)
                    if msg.get("role") == "assistant":
                        text = _extract_assistant_text(msg)
                        if text:
                            last_assistant_text = text

                    if msg.get("role") == "assistant":
                        for block in msg.get("content") or []:
                            if not isinstance(block, dict):
                                continue
                            if block.get("type") != "tool_use":
                                continue
                            trace.events.append(
                                _event_from_tool_use(block, seq_num=next_seq - 1)
                            )
                            if block.get("name") == "read_document":
                                tool_input = block.get("input")
                                if isinstance(tool_input, dict):
                                    doc_ref = tool_input.get("id")
                                    if isinstance(doc_ref, str) and doc_ref:
                                        trace.read_doc_ids.append(doc_ref)

                    # Check for submit_answer after recording the raw tool event.
                    submitted = _extract_submit_answer(msg)
                    if submitted is not None:
                        return submitted, trace

                elif event_name == "message":
                    # Raw SSE streaming event — only capture search results
                    try:
                        msg = json.loads(data)
                    except json.JSONDecodeError:
                        continue

                    # Capture search result doc IDs + scores from tool_result blocks
                    if isinstance(msg, dict) and msg.get("type") == "tool_result":
                        search_results = _extract_search_results(msg)
                        for doc_id, score in search_results.items():
                            if (
                                doc_id not in trace.doc_scores
                                or score > trace.doc_scores[doc_id]
                            ):
                                trace.doc_scores[doc_id] = score
                        trace.events.extend(_extract_search_result_events(msg))

                elif event_name == "end_of_stream":
                    break
                elif event_name == "error":
                    log.warning("stream error event for chat %s: %s", chat_id, data)
                    break
    except (httpx.HTTPError, httpx.TimeoutException) as e:
        log.error("stream failed for chat %s: %s", chat_id, e)

    # Fallback: if no submit_answer, use last assistant text as the answer
    if last_assistant_text:
        log.info("fallback: using last assistant text as answer for %s", chat_id)
        return {"answer": last_assistant_text}, trace

    return None, trace


async def _process_question(
    sem: asyncio.Semaphore,
    client: httpx.AsyncClient,
    db_dsn: str,
    user_id: str,
    model_id: str | None,
    base_url: str,
    timeout_seconds: float,
    q: Question,
    trace_out: Path | None,
) -> Answer | None:
    async with sem:
        # DB writes happen in a background thread so they don't block the loop.
        try:
            chat_id = await asyncio.to_thread(
                _create_chat_blocking, db_dsn, user_id, model_id, q.text
            )
        except Exception as e:
            log.error("DB setup failed for %s: %s", q.question_id, e)
            return None

        submitted, trace = await _stream_one_question(
            client, base_url, chat_id, db_dsn, timeout_seconds
        )
        trace.question_id = q.question_id

        submitted_doc_ids_raw = (
            _normalize_submitted_doc_ids(submitted.get("document_ids"))
            if submitted is not None
            else []
        )
        submitted_doc_ids = await asyncio.to_thread(
            _resolve_doc_ids_blocking, db_dsn, submitted_doc_ids_raw
        )
        resolved_read_doc_ids = await asyncio.to_thread(
            _resolve_doc_ids_blocking, db_dsn, _ordered_unique(trace.read_doc_ids)
        )

        # Sort retrieved docs by score for trace/debugging. Benchmark-scored
        # document_ids should be the model's cited evidence docs, not every
        # related/read/retrieved document.
        sorted_docs = sorted(trace.doc_scores.items(), key=lambda x: x[1], reverse=True)
        harvested_doc_ids = [doc_id for doc_id, _ in sorted_docs]
        if submitted_doc_ids:
            final_doc_ids = _ordered_unique(submitted_doc_ids)[:10]
        elif submitted is None:
            # No structured submission means no reliable citation intent. Keep
            # retrieval candidates in the trace, but do not pollute benchmark
            # scoring with tangential fallback docs.
            final_doc_ids = []
        else:
            final_doc_ids = []

        log.info(
            "%s: %d unique retrieved docs, %d submitted docs, %d read docs",
            q.question_id,
            len(trace.doc_scores),
            len(submitted_doc_ids),
            len(resolved_read_doc_ids),
        )

        # Write trace to sidecar file
        if trace_out is not None:
            trace_line = json.dumps(
                {
                    "question_id": trace.question_id,
                    "chat_id": trace.chat_id,
                    "doc_scores": trace.doc_scores,
                    "submitted_doc_ids_raw": submitted_doc_ids_raw,
                    "submitted_doc_ids": submitted_doc_ids,
                    "read_doc_ids": resolved_read_doc_ids,
                    "final_doc_ids": final_doc_ids,
                    "events": trace.events,
                }
            )
            await asyncio.to_thread(_append_trace_blocking, trace_out, trace_line)

        if submitted is None:
            log.warning("no submit_answer for %s (chat=%s)", q.question_id, chat_id)
            return Answer(
                question_id=q.question_id,
                answer="The agent did not submit an answer.",
                document_ids=final_doc_ids,
                chat_id=chat_id,
            )

        answer_text = submitted.get("answer") or ""

        return Answer(
            question_id=q.question_id,
            answer=str(answer_text),
            document_ids=final_doc_ids,
            chat_id=chat_id,
        )


def _resolve_doc_ids_blocking(db_dsn: str, doc_ids: list[str]) -> list[str]:
    with psycopg.connect(db_dsn) as conn:
        return _resolve_doc_ids(conn, doc_ids)


def _resolve_doc_ids(conn: psycopg.Connection, doc_ids: list[str]) -> list[str]:
    """Map internal Omni ULIDs back to external dsid_* IDs for the eval.

    The eval expects gold doc IDs (dsid_*). The agent sometimes returns
    internal Omni IDs (01KR*); resolve those via the documents table.
    Pass through anything that already looks like an external ID.
    """
    resolved: list[str] = []
    with conn.cursor() as cur:
        for doc_id in doc_ids:
            if doc_id.startswith("dsid_"):
                resolved.append(doc_id)
                continue
            # Try as internal Omni ID
            cur.execute(
                "SELECT external_id FROM documents WHERE id = %s",
                (doc_id,),
            )
            row = cur.fetchone()
            if row and row[0]:
                resolved.append(row[0])
            else:
                # Fallback: pass through unresolved
                resolved.append(doc_id)
    return resolved


def _extract_text(msg: dict) -> str:
    """Best-effort text extraction for content_text column."""
    content = msg.get("content", "")
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        parts = []
        for block in content:
            if isinstance(block, dict):
                if block.get("type") == "text":
                    parts.append(block.get("text", ""))
                elif block.get("type") == "tool_use":
                    parts.append(f"[tool_use: {block.get('name', '?')}]")
                elif block.get("type") == "tool_result":
                    parts.append("[tool_result]")
        return "\n".join(parts)
    return str(content)


def _extract_assistant_text(msg: dict) -> str:
    """Extract plain text from an assistant message (excluding tool_use blocks)."""
    if msg.get("role") != "assistant":
        return ""
    content = msg.get("content", "")
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        parts = []
        for block in content:
            if isinstance(block, dict) and block.get("type") == "text":
                parts.append(block.get("text", ""))
        return "\n".join(parts)
    return str(content)


def _persist_message_blocking(
    db_dsn: str,
    chat_id: str,
    seq_num: int,
    message: dict,
    content_text: str,
) -> None:
    with psycopg.connect(db_dsn) as conn:
        _persist_message(conn, chat_id, seq_num, message, content_text)


def _append_trace_blocking(trace_path: Path, line: str) -> None:
    with trace_path.open("a", encoding="utf-8") as f:
        f.write(line + "\n")


def _create_chat_blocking(
    db_dsn: str, user_id: str, model_id: str | None, question_text: str
) -> str:
    with psycopg.connect(db_dsn) as conn:
        return _create_chat_with_message(conn, user_id, model_id, question_text)


async def main_async(args: argparse.Namespace) -> int:
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
    chat_map_path = args.chat_map or (args.output_dir / "question_chat_map.json")

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

    db_dsn = (
        f"host={args.db_host} port={args.db_port} dbname={args.db_name} "
        f"user={args.db_user} password={args.db_password}"
    )

    # Resolve user + default model once.
    with psycopg.connect(db_dsn) as conn:
        user_id = _get_bench_user_id(conn, args.user_email)
        model_id = _get_default_model_id(conn)
    log.info("using user_id=%s model_id=%s", user_id, model_id)

    sem = asyncio.Semaphore(args.concurrency)

    trace_path = args.output_dir / f"retrieval_trace_{args.system_name}.jsonl"

    async with httpx.AsyncClient() as client:
        coros = [
            _process_question(
                sem,
                client,
                db_dsn,
                user_id,
                model_id,
                args.ai_url,
                args.timeout,
                q,
                trace_path,
            )
            for q in questions
        ]
        chat_map: dict[str, str] = {}
        if args.resume and chat_map_path.exists():
            try:
                chat_map = json.loads(chat_map_path.read_text(encoding="utf-8"))
            except json.JSONDecodeError:
                log.warning("ignoring invalid existing chat map: %s", chat_map_path)
        with out_path.open("a", encoding="utf-8") as out:
            for fut in tqdm_asyncio.as_completed(
                coros, total=len(coros), desc="agentic"
            ):
                ans = await fut
                if ans is None:
                    continue
                chat_map[ans.question_id] = ans.chat_id
                chat_map_path.write_text(
                    json.dumps(chat_map, indent=2, sort_keys=True) + "\n",
                    encoding="utf-8",
                )
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
        "--ai-url",
        default=os.environ.get(
            "BENCH_AI_URL",
            f"http://localhost:{os.environ.get('AI_SERVICE_PORT', '3003')}",
        ),
    )
    parser.add_argument("--db-host", default=os.environ.get("DB_HOST", "localhost"))
    parser.add_argument(
        "--db-port", type=int, default=int(os.environ.get("DB_PORT", "5432"))
    )
    parser.add_argument(
        "--db-name", default=os.environ.get("DB_NAME", "omni_benchmark")
    )
    parser.add_argument("--db-user", default=os.environ.get("DB_USER", "omni_bench"))
    parser.add_argument(
        "--db-password",
        default=os.environ.get("DB_PASSWORD", "omni_bench_password"),
    )
    parser.add_argument(
        "--concurrency",
        type=int,
        default=4,
        help=(
            "Parallel agentic loops in flight. Each loop can make multiple LLM "
            "calls through omni-ai (search → read → submit), so keep this "
            "aligned with your searcher and provider rate limits."
        ),
    )
    parser.add_argument(
        "--timeout",
        type=float,
        default=600.0,
        help="Per-question stream timeout in seconds (loop can take a while).",
    )
    parser.add_argument(
        "--sample",
        type=int,
        default=None,
        help="If set, take only the first N questions (for dipstick runs).",
    )
    parser.add_argument(
        "--system-name",
        default="omni_agentic_deepseek_v4_pro",
        help="Suffix for the answers file: answers_<system_name>.jsonl",
    )
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=Path(__file__).parent / "answer_evaluation",
    )
    parser.add_argument(
        "--chat-map",
        type=Path,
        default=None,
        help="Path to write a JSON object mapping question_id to chat_id.",
    )
    parser.add_argument(
        "--resume",
        action="store_true",
        help="Skip questions already present in the answers file.",
    )
    parser.add_argument(
        "--user-email",
        default=os.environ.get("BENCH_USER_EMAIL", "bench@omni.local"),
        help="User email for chat attribution (default: bench@omni.local)",
    )
    args = parser.parse_args()

    return asyncio.run(main_async(args))


if __name__ == "__main__":
    sys.exit(main())
