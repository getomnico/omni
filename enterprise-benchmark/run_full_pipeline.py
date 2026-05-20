#!/usr/bin/env python3
"""Streaming benchmark pipeline: answer generation + incremental eval.

Usage:
    uv run python run_full_pipeline.py --samples 500 --suffix final

This runs:
  1. Agentic answer generation (run_agentic.py) in background
  2. Periodically runs eval with --resume on the growing answers file
  3. Prints real-time metrics as eval progresses
  4. Outputs final results when complete

The eval script already supports incremental evaluation (--resume) and
writes results after each question, so we just poll and re-trigger.

Output files:
  answers_{system_name}.jsonl
  results_{system_name}.json
"""

from __future__ import annotations

import argparse
import json
import logging
import os
import re
import subprocess
import sys
import time
import urllib.error
import urllib.request
from datetime import datetime
from pathlib import Path

logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")
log = logging.getLogger("run_full_pipeline")

BENCH_DIR = Path(__file__).parent.resolve()
EVAL_DIR = Path("/root/EnterpriseRAG-Bench")
ANSWER_EVAL_DIR = BENCH_DIR / "answer_evaluation"
RUNS_DIR = BENCH_DIR / "runs"


def _slugify_run_name(name: str) -> str:
    slug = re.sub(r"[^a-zA-Z0-9]+", "_", name.strip().lower()).strip("_")
    return slug or "benchmark_run"


def _create_run_dir(args: argparse.Namespace) -> Path:
    if args.run_dir:
        run_dir = args.run_dir
        run_dir.mkdir(parents=True, exist_ok=True)
    else:
        timestamp = datetime.now().strftime("%Y_%m_%d_%H_%M")
        run_dir = RUNS_DIR / f"{_slugify_run_name(args.run_name)}_{timestamp}"
        run_dir.mkdir(parents=True, exist_ok=False)
    return run_dir


def _post_json(url: str, body: dict, timeout: float = 30.0) -> dict:
    request = urllib.request.Request(
        url,
        data=json.dumps(body).encode("utf-8"),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=timeout) as response:
        return json.loads(response.read())


def _get_json(url: str, timeout: float = 10.0) -> dict:
    with urllib.request.urlopen(url, timeout=timeout) as response:
        return json.loads(response.read())


def _retry_check(name: str, attempts: int, delay_seconds: float, check_fn) -> bool:
    last_exc: Exception | None = None
    for attempt in range(1, attempts + 1):
        try:
            check_fn()
            if attempt > 1:
                log.info("%s healthy after %d attempt(s)", name, attempt)
            return True
        except Exception as exc:  # noqa: BLE001 - preflight should log any failure.
            last_exc = exc
            log.warning(
                "%s check failed on attempt %d/%d: %s", name, attempt, attempts, exc
            )
            if attempt < attempts:
                time.sleep(delay_seconds)
    log.error("%s unreachable after %d attempt(s): %s", name, attempts, last_exc)
    return False


def _check_llama_endpoint(args: argparse.Namespace) -> None:
    models_payload = _get_json(args.llama_url.rstrip("/") + "/models")
    models = models_payload.get("data") or models_payload.get("models") or []
    model_ids = {
        model.get("id") or model.get("model") or model.get("name")
        for model in models
        if isinstance(model, dict)
    }
    if args.llama_model and args.llama_model not in model_ids:
        raise RuntimeError(
            f"{args.llama_model!r} not found in llama.cpp model list: {sorted(model_ids)}"
        )
    log.info("llama.cpp endpoint healthy: %s", sorted(model_ids)[:3])


def _check_embedding_endpoint(args: argparse.Namespace) -> None:
    embedding_payload = _post_json(
        args.embedding_url.rstrip("/") + "/embeddings",
        {"model": args.embedding_model, "input": args.semantic_probe_query},
    )
    embedding = embedding_payload["data"][0]["embedding"]
    if len(embedding) != args.embedding_dimensions:
        raise RuntimeError(
            f"returned {len(embedding)} dims, expected {args.embedding_dimensions}"
        )
    log.info("embedding endpoint healthy: dims=%d", len(embedding))


def run_preflight_checks(args: argparse.Namespace) -> bool:
    """Verify runtime dependencies before spending benchmark tokens."""
    log.info("=" * 60)
    log.info("PREFLIGHT: checking services and semantic search")
    log.info("=" * 60)

    try:
        ai_health = _get_json(args.ai_url.rstrip("/") + "/health")
    except Exception as exc:  # noqa: BLE001 - CLI should report root cause.
        log.error("omni-ai health check failed: %s", exc)
        return False

    if ai_health.get("status") != "healthy":
        log.error("omni-ai unhealthy: %s", ai_health)
        return False
    log.info(
        "omni-ai healthy: llm=%s embedding=%s",
        ai_health.get("llm_model"),
        ai_health.get("embedding_model"),
    )

    try:
        searcher_health = _get_json(args.searcher_url.rstrip("/") + "/health")
    except Exception as exc:  # noqa: BLE001
        log.error("omni-searcher health check failed: %s", exc)
        return False

    if searcher_health.get("status") != "healthy":
        log.error("omni-searcher unhealthy: %s", searcher_health)
        return False
    log.info("omni-searcher healthy")

    if args.llama_url and not args.skip_llama_preflight:
        if not _retry_check(
            "llama.cpp endpoint",
            args.preflight_retries,
            args.preflight_retry_delay,
            lambda: _check_llama_endpoint(args),
        ):
            return False
    else:
        log.info("llama.cpp endpoint preflight skipped")

    if args.embedding_url and not args.skip_embedding_preflight:
        if not _retry_check(
            "embedding endpoint",
            args.preflight_retries,
            args.preflight_retry_delay,
            lambda: _check_embedding_endpoint(args),
        ):
            return False
    else:
        log.info("embedding endpoint preflight skipped")

    try:
        search_payload = _post_json(
            args.searcher_url.rstrip("/") + "/search",
            {
                "query": args.semantic_probe_query,
                "mode": "semantic",
                "limit": args.semantic_probe_limit,
            },
            timeout=90.0,
        )
    except urllib.error.HTTPError as exc:
        detail = exc.read().decode("utf-8", "replace")
        log.error("semantic search probe failed: HTTP %s: %s", exc.code, detail)
        return False
    except Exception as exc:  # noqa: BLE001
        log.error("semantic search probe failed: %s", exc)
        return False

    results = search_payload.get("results") or []
    if not results:
        log.error("semantic search probe returned zero results")
        return False

    log.info("semantic search healthy: %d result(s)", len(results))
    return True


def load_eval_results(results_file: Path) -> dict | None:
    """Load eval results if they exist and are valid JSON."""
    if not results_file.exists():
        return None
    try:
        with results_file.open() as f:
            return json.load(f)
    except (json.JSONDecodeError, OSError):
        return None


def print_metrics(results: dict, prefix: str = "📊") -> None:
    """Print a concise metrics summary from eval results."""
    agg = results.get("aggregate_stats", {})
    total = agg.get("total_questions", 0)
    completed = agg.get("completed_questions", 0)
    overall = agg.get("combined_correctness_completeness_score", 0)
    correctness = agg.get("average_correctness_pct", 0)
    completeness = agg.get("average_completeness_pct", 0)
    recall = agg.get("average_recall_pct", 0)
    invalid = agg.get("average_invalid_extra_docs", 0)

    print(
        f"\n{prefix} {completed}/{total} | "
        f"Overall: {overall:.1f}% | "
        f"Correct: {correctness:.1f}% | "
        f"Complete: {completeness:.1f}% | "
        f"Recall: {recall:.1f}% | "
        f"Invalid: {invalid:.1f}"
    )


def count_answers(answers_file: Path) -> int:
    """Count lines in answers file."""
    if not answers_file.exists():
        return 0
    with answers_file.open() as f:
        return sum(1 for _ in f)


def start_eval(
    answers_file: Path,
    results_file: Path,
    parallelism: int,
    no_correction: bool,
    env: dict,
) -> subprocess.Popen:
    """Start eval with --resume in background. Returns the process handle."""
    cmd = [
        str(EVAL_DIR / ".venv/bin/python"),
        "-m",
        "src.scripts.answer_evaluation.metrics_based_eval",
        "--answers-file",
        str(answers_file),
        "--results-file",
        str(results_file),
        "--parallelism",
        str(parallelism),
        "--resume",
    ]
    if no_correction:
        cmd.append("--no-correction")

    return subprocess.Popen(
        cmd,
        cwd=EVAL_DIR,
        env={**os.environ, **env},
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )


def wait_for_eval(eval_proc: subprocess.Popen, timeout: float = 600.0) -> bool:
    """Wait for eval to finish, returning True on success."""
    try:
        stdout, _ = eval_proc.communicate(timeout=timeout)
        if eval_proc.returncode != 0:
            log.warning("Eval failed:\n%s", stdout[-2000:] if stdout else "")
            return False
        return True
    except subprocess.TimeoutExpired:
        log.warning("Eval timed out after %.0fs, killing...", timeout)
        eval_proc.kill()
        return False


def stop_process(proc: subprocess.Popen | None, name: str) -> None:
    if proc is None or proc.poll() is not None:
        return
    log.warning("Stopping %s after failed preflight", name)
    proc.terminate()
    try:
        proc.wait(timeout=10)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait(timeout=10)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--samples", type=int, default=500)
    parser.add_argument("--suffix", required=True)
    parser.add_argument(
        "--run-name",
        default=None,
        help=(
            "Descriptive run name used for enterprise-benchmark/runs/<name>_YYYY_MM_DD_HH_MM. "
            "Defaults to --suffix."
        ),
    )
    parser.add_argument(
        "--run-dir",
        type=Path,
        default=None,
        help="Explicit run artifact directory. Mostly useful for resume/debug runs.",
    )
    parser.add_argument(
        "--questions",
        type=Path,
        default=BENCH_DIR / "data" / "questions.jsonl",
        help="Questions JSONL file to pass to run_agentic.py",
    )
    parser.add_argument(
        "--system-name",
        default=None,
        help=(
            "Full benchmark system name. Defaults to "
            "omni_agentic_deepseek_<suffix> for backward compatibility."
        ),
    )
    parser.add_argument("--concurrency", type=int, default=4)
    parser.add_argument("--timeout", type=float, default=300.0)
    parser.add_argument("--eval-parallelism", type=int, default=12)
    parser.add_argument(
        "--ai-url",
        default=os.environ.get(
            "BENCH_AI_URL",
            f"http://localhost:{os.environ.get('AI_SERVICE_PORT', '3003')}",
        ),
    )
    parser.add_argument(
        "--searcher-url",
        default=os.environ.get(
            "BENCH_SEARCHER_URL",
            f"http://localhost:{os.environ.get('SEARCHER_PORT', '3001')}",
        ),
    )
    parser.add_argument(
        "--embedding-url",
        default=os.environ.get("BENCH_EMBEDDING_URL"),
        help=(
            "Host-reachable OpenAI-compatible embedding base URL for preflight. "
            "If omitted, the direct endpoint check is skipped."
        ),
    )
    parser.add_argument(
        "--embedding-model",
        default=os.environ.get("BENCH_EMBEDDING_MODEL", "BAAI/bge-large-en-v1.5"),
    )
    parser.add_argument("--embedding-dimensions", type=int, default=1024)
    parser.add_argument(
        "--llama-url",
        default=os.environ.get("BENCH_LLAMA_URL"),
        help=(
            "Host-reachable OpenAI-compatible llama.cpp base URL for preflight. "
            "If omitted, the direct endpoint check is skipped."
        ),
    )
    parser.add_argument(
        "--llama-model",
        default=os.environ.get("BENCH_LLAMA_MODEL", "Qwen3.6-27B-IQ4_XS-mtp.gguf"),
    )
    parser.add_argument(
        "--semantic-probe-query",
        default="healthcare chat rollout monthly active users in:gmail",
    )
    parser.add_argument("--semantic-probe-limit", type=int, default=3)
    parser.add_argument("--skip-preflight", action="store_true")
    parser.add_argument(
        "--skip-embedding-preflight",
        action="store_true",
        help=(
            "Skip only the direct embedding endpoint probe. The semantic /search "
            "probe still runs unless --skip-preflight is set."
        ),
    )
    parser.add_argument(
        "--skip-llama-preflight",
        action="store_true",
        help="Skip only the optional direct llama.cpp endpoint probe.",
    )
    parser.add_argument("--preflight-retries", type=int, default=3)
    parser.add_argument("--preflight-retry-delay", type=float, default=2.0)
    parser.add_argument(
        "--preflight-interval-answers",
        type=int,
        default=5,
        help=(
            "Run health/semantic preflight again after this many new answers. "
            "Set 0 to disable periodic checks."
        ),
    )
    parser.add_argument("--judge-model", default="deepseek-v4-pro")
    parser.add_argument("--judge-base-url", default="https://api.deepseek.com/v1")
    parser.add_argument("--judge-api-key-env", default="DEEPSEEK_API_KEY")
    parser.add_argument(
        "--eval-batch",
        type=int,
        default=5,
        help="Trigger eval every N new answers (default: 5)",
    )
    parser.add_argument("--no-correction", action="store_true")
    parser.add_argument("--skip-gen", action="store_true")
    parser.add_argument("--skip-eval", action="store_true")
    args = parser.parse_args()

    args.run_name = args.run_name or args.suffix
    run_dir = _create_run_dir(args)
    system_name = args.system_name or f"omni_agentic_{run_dir.name}"
    answers_file = run_dir / f"answers_{system_name}.jsonl"
    results_file = run_dir / f"results_{system_name}.json"
    chat_map_file = run_dir / "question_chat_map.json"
    run_metadata_file = run_dir / "run_metadata.json"

    run_metadata_file.write_text(
        json.dumps(
            {
                "run_name": args.run_name,
                "run_dir": str(run_dir),
                "system_name": system_name,
                "questions": str(args.questions),
                "samples": args.samples,
                "concurrency": args.concurrency,
                "timeout": args.timeout,
                "judge_model": args.judge_model,
                "ai_url": args.ai_url,
                "searcher_url": args.searcher_url,
                "llama_url": args.llama_url,
                "skip_llama_preflight": args.skip_llama_preflight,
                "embedding_url": args.embedding_url,
                "skip_embedding_preflight": args.skip_embedding_preflight,
                "preflight_interval_answers": args.preflight_interval_answers,
                "created_at": datetime.now().isoformat(timespec="seconds"),
            },
            indent=2,
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )

    # ── Setup eval env ───────────────────────────────────────────────────────
    env = {
        "LLM_PROVIDER": "openai_compat",
        "LLM_API_KEY": os.environ.get(args.judge_api_key_env, ""),
        "LLM_BASE_URL": args.judge_base_url,
        "LLM_MODEL_NAME": args.judge_model,
    }
    env_file = Path("/root/EnterpriseRAG-Bench/.env")
    if not env["LLM_API_KEY"] and env_file.exists():
        with env_file.open() as f:
            for line in f:
                if line.startswith(f"{args.judge_api_key_env}="):
                    env["LLM_API_KEY"] = line.strip().split("=", 1)[1]
                    break

    if not env["LLM_API_KEY"]:
        log.error("%s not found", args.judge_api_key_env)
        return 1

    if not args.skip_preflight and not run_preflight_checks(args):
        return 1

    # ── Step 1: Start answer generation ──────────────────────────────────────
    gen_proc = None
    if not args.skip_gen:
        log.info("=" * 60)
        log.info("STEP 1: Starting Answer Generation (%d questions)", args.samples)
        log.info("Run artifacts: %s", run_dir)
        log.info("=" * 60)

        gen_cmd = [
            sys.executable,
            str(BENCH_DIR / "run_agentic.py"),
            "--questions",
            str(args.questions),
            "--ai-url",
            args.ai_url,
            "--concurrency",
            str(args.concurrency),
            "--timeout",
            str(args.timeout),
            "--sample",
            str(args.samples),
            "--system-name",
            system_name,
            "--output-dir",
            str(run_dir),
            "--chat-map",
            str(chat_map_file),
        ]
        log.info("Starting: %s", " ".join(gen_cmd))
        gen_proc = subprocess.Popen(
            gen_cmd,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )
    else:
        log.info("Skipping generation")

    if args.skip_eval:
        if gen_proc:
            gen_proc.wait()
        return 0

    # ── Step 2: Streaming eval loop ──────────────────────────────────────────
    log.info("=" * 60)
    log.info("STEP 2: Streaming Eval (trigger every %d new answers)", args.eval_batch)
    log.info("=" * 60)

    last_answer_count = 0
    last_eval_completed = 0
    last_preflight_answer_count = 0
    eval_proc: subprocess.Popen | None = None
    poll_interval = 15  # Check every 15 seconds

    while True:
        time.sleep(poll_interval)

        # Check if generation is done
        gen_done = gen_proc is None or gen_proc.poll() is not None
        current_answers = count_answers(answers_file)

        if current_answers == 0:
            if gen_done:
                log.info("Generation finished but no answers generated")
                break
            continue

        # Load current eval results
        results = load_eval_results(results_file)
        current_evaluated = 0
        if results:
            current_evaluated = results.get("aggregate_stats", {}).get(
                "completed_questions", 0
            )

        # Print progress
        print(
            f"\n⏳ Answers: {current_answers}/{args.samples} | "
            f"Evaluated: {current_evaluated} | "
            f"Gen running: {not gen_done}"
            f"{' | Eval running' if eval_proc and eval_proc.poll() is None else ''}"
        )

        if (
            not args.skip_preflight
            and args.preflight_interval_answers > 0
            and current_answers - last_preflight_answer_count
            >= args.preflight_interval_answers
            and not gen_done
        ):
            log.info(
                "Running periodic preflight after %d answer(s)...",
                current_answers,
            )
            if not run_preflight_checks(args):
                stop_process(gen_proc, "answer generation")
                stop_process(eval_proc, "eval")
                return 1
            last_preflight_answer_count = current_answers

        if results and current_evaluated > last_eval_completed:
            print_metrics(results)
            last_eval_completed = current_evaluated

        # Check if an eval process finished
        if eval_proc is not None:
            if eval_proc.poll() is not None:
                # Eval finished
                eval_success = wait_for_eval(eval_proc, timeout=1.0)
                eval_proc = None
                last_answer_count = current_answers

                if eval_success:
                    results = load_eval_results(results_file)
                    if results:
                        new_completed = results.get("aggregate_stats", {}).get(
                            "completed_questions", 0
                        )
                        if new_completed > last_eval_completed:
                            print_metrics(results, prefix="🎯")
                            last_eval_completed = new_completed
                else:
                    log.warning("Eval run failed, will retry")
            else:
                # Eval still running — don't start another one
                continue

        # Trigger eval every eval_batch new answers, or when gen is done
        new_answers = current_answers - last_answer_count
        should_trigger = (new_answers >= args.eval_batch) or (
            gen_done and new_answers > 0
        )

        if should_trigger:
            log.info(
                "Starting eval for %d answers (%d new since last run)...",
                current_answers,
                new_answers,
            )
            eval_proc = start_eval(
                answers_file,
                results_file,
                args.eval_parallelism,
                args.no_correction,
                env,
            )
            continue

        # Exit when generation is done AND all answers are evaluated
        if gen_done and current_evaluated >= current_answers:
            log.info("Generation complete and all answers evaluated")
            break

        if gen_done and current_answers >= args.samples:
            # One final eval to catch any remaining
            eval_proc = start_eval(
                answers_file,
                results_file,
                args.eval_parallelism,
                args.no_correction,
                env,
            )
            wait_for_eval(eval_proc)
            break

    # ── Final results ────────────────────────────────────────────────────────
    results = load_eval_results(results_file)
    if results:
        print("\n" + "=" * 60)
        print("FINAL RESULTS")
        print("=" * 60)
        print_metrics(results, prefix="🏆")
        print(f"\nAnswers:  {answers_file}")
        print(f"Results:  {results_file}")
        print(f"Run dir:  {run_dir}")

    return 0


if __name__ == "__main__":
    sys.exit(main())
