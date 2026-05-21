#!/usr/bin/env python3
"""Build the GPT-5.4-adjusted full500 result from base + patch artifacts.

This script is intentionally deterministic. It does not call an LLM. It merges
already-generated answer rows and already-generated judgement rows according to
one policy:

    contribution = completeness_pct if answer_correct else 0

Patch rows are selected only when they improve that contribution. For answer
remediation rows, the GPT-5.4 answer text and judgement are used, but the
original DeepSeek document_ids and deterministic retrieval metrics are
preserved. For judgement-only rows, only the quality judgement is updated.

Defaults match the public HuggingFace artifact layout:

    base/
    final/
    patches/

For an older flat canonical run directory, pass explicit --base-* and --patch-*
paths.
"""

from __future__ import annotations

import argparse
import json
from copy import deepcopy
from datetime import UTC, datetime
from pathlib import Path
from typing import Any


DEFAULT_BASE_ANSWERS = "base/answers_omni_agentic_deepseek_v4_pro_full500.jsonl"
DEFAULT_BASE_RESULTS = "base/results_omni_agentic_deepseek_v4_pro_full500.json"
DEFAULT_PATCH12_ANSWERS = (
    "patches/combined_gpt54_high_32/"
    "answers_omni_agentic_gpt54_high_remediation_32.jsonl"
)
DEFAULT_PATCH12_RESULTS = (
    "patches/combined_gpt54_high_32/"
    "results_omni_agentic_gpt54_high_remediation_32.json"
)
DEFAULT_PATCH3_RESULTS = (
    "patches/patch_3_judge_suspicious_gpt54_eval_only_ge70/"
    "results_deepseek_full500_judge_suspicious_ge70_gpt54_judge.json"
)
DEFAULT_OUT_ANSWERS = (
    "final/answers_omni_agentic_deepseek_v4_pro_full500_merged_gpt54_adjusted.jsonl"
)
DEFAULT_OUT_RESULTS = (
    "final/results_omni_agentic_deepseek_v4_pro_full500_merged_gpt54_adjusted.json"
)
DEFAULT_OUT_MANIFEST = "final/merge_manifest_gpt54_adjusted.json"

QUALITY_FIELDS = (
    "answer_correct",
    "correctness_reasoning",
    "completeness_pct",
    "corrected",
)
RETRIEVAL_FIELDS = ("document_recall_pct", "invalid_extra_docs")


def load_json(path: Path) -> dict[str, Any]:
    with path.open("r", encoding="utf-8") as f:
        return json.load(f)


def load_jsonl_by_question_id(path: Path) -> dict[str, dict[str, Any]]:
    rows: dict[str, dict[str, Any]] = {}
    with path.open("r", encoding="utf-8") as f:
        for line_no, line in enumerate(f, start=1):
            line = line.strip()
            if not line:
                continue
            row = json.loads(line)
            question_id = row.get("question_id")
            if not question_id:
                raise ValueError(f"{path}:{line_no} is missing question_id")
            if question_id in rows:
                raise ValueError(
                    f"{path}:{line_no} duplicate question_id={question_id}"
                )
            rows[question_id] = row
    return rows


def load_result_rows(path: Path) -> dict[str, dict[str, Any]]:
    data = load_json(path)
    rows = data.get("questions")
    if not isinstance(rows, list):
        raise ValueError(f"{path} does not contain a questions list")
    return {row["question_id"]: row for row in rows}


def contribution(row: dict[str, Any]) -> float:
    if row.get("answer_correct") is not True:
        return 0.0
    return float(row.get("completeness_pct") or 0.0)


def copy_quality_fields(target: dict[str, Any], source: dict[str, Any]) -> None:
    for field in QUALITY_FIELDS:
        if field in source:
            target[field] = source[field]


def preserve_retrieval_fields(target: dict[str, Any], base: dict[str, Any]) -> None:
    for field in RETRIEVAL_FIELDS:
        if field in base:
            target[field] = base[field]


def compute_aggregate(
    rows: list[dict[str, Any]], skip_count: int = 0
) -> dict[str, Any]:
    total = len(rows)
    correct_count = sum(1 for row in rows if row.get("answer_correct") is True)
    completeness_values = [float(row.get("completeness_pct") or 0.0) for row in rows]
    contributions = [contribution(row) for row in rows]
    recall_values = [
        float(row["document_recall_pct"])
        for row in rows
        if row.get("document_recall_pct") is not None
    ]
    invalid_values = [
        float(row["invalid_extra_docs"])
        for row in rows
        if row.get("invalid_extra_docs") is not None
    ]

    return {
        "total_questions": total,
        "completed_questions": total,
        "skipped_rows": skip_count,
        "num_corrected_questions": sum(1 for row in rows if row.get("corrected")),
        "average_correctness_pct": correct_count * 100.0 / total if total else None,
        "average_completeness_pct": (
            sum(completeness_values) / total if total else None
        ),
        "combined_correctness_completeness_score": (
            sum(contributions) / total if total else None
        ),
        "average_recall_pct": (
            sum(recall_values) / len(recall_values) if recall_values else None
        ),
        "average_invalid_extra_docs": (
            sum(invalid_values) / len(invalid_values) if invalid_values else None
        ),
        "recall_question_count": len(recall_values),
        "invalid_extra_docs_question_count": len(invalid_values),
    }


def compute_question_type_stats(
    rows: list[dict[str, Any]],
) -> dict[str, dict[str, Any]]:
    grouped: dict[str, list[dict[str, Any]]] = {}
    for row in rows:
        grouped.setdefault(row.get("question_type") or "unknown", []).append(row)

    return {
        question_type: compute_aggregate(question_rows)
        for question_type, question_rows in sorted(grouped.items())
    }


def write_jsonl(path: Path, rows: list[dict[str, Any]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as f:
        for row in rows:
            f.write(json.dumps(row, ensure_ascii=False) + "\n")


def write_json(path: Path, data: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as f:
        json.dump(data, f, indent=2, ensure_ascii=False)
        f.write("\n")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base-answers", default=DEFAULT_BASE_ANSWERS)
    parser.add_argument("--base-results", default=DEFAULT_BASE_RESULTS)
    parser.add_argument("--patch12-answers", default=DEFAULT_PATCH12_ANSWERS)
    parser.add_argument("--patch12-results", default=DEFAULT_PATCH12_RESULTS)
    parser.add_argument("--patch3-results", default=DEFAULT_PATCH3_RESULTS)
    parser.add_argument("--out-answers", default=DEFAULT_OUT_ANSWERS)
    parser.add_argument("--out-results", default=DEFAULT_OUT_RESULTS)
    parser.add_argument("--out-manifest", default=DEFAULT_OUT_MANIFEST)
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    base_answers_path = Path(args.base_answers)
    base_results_path = Path(args.base_results)
    patch12_answers_path = Path(args.patch12_answers)
    patch12_results_path = Path(args.patch12_results)
    patch3_results_path = Path(args.patch3_results)

    base_answers = load_jsonl_by_question_id(base_answers_path)
    base_results_data = load_json(base_results_path)
    base_results = {row["question_id"]: row for row in base_results_data["questions"]}
    patch12_answers = load_jsonl_by_question_id(patch12_answers_path)
    patch12_results = load_result_rows(patch12_results_path)
    patch3_results = load_result_rows(patch3_results_path)

    if set(base_answers) != set(base_results):
        raise ValueError("base answers and base results have different question IDs")

    merged_answers = {
        question_id: deepcopy(row) for question_id, row in base_answers.items()
    }
    merged_results = {
        question_id: deepcopy(row) for question_id, row in base_results.items()
    }

    selected_patch12: list[str] = []
    selected_patch3: list[str] = []

    for question_id, patch_row in patch12_results.items():
        if question_id not in merged_results:
            continue
        current_row = merged_results[question_id]
        if contribution(patch_row) > contribution(current_row):
            base_result_row = base_results[question_id]
            copy_quality_fields(current_row, patch_row)
            preserve_retrieval_fields(current_row, base_result_row)

            if question_id in patch12_answers:
                merged_answers[question_id]["answer"] = patch12_answers[
                    question_id
                ].get("answer", merged_answers[question_id].get("answer"))
                merged_answers[question_id]["document_ids"] = base_answers[
                    question_id
                ].get("document_ids", [])
            selected_patch12.append(question_id)

    for question_id, patch_row in patch3_results.items():
        if question_id not in merged_results:
            continue
        current_row = merged_results[question_id]
        if contribution(patch_row) > contribution(current_row):
            base_result_row = base_results[question_id]
            copy_quality_fields(current_row, patch_row)
            preserve_retrieval_fields(current_row, base_result_row)
            selected_patch3.append(question_id)

    ordered_question_ids = [
        row["question_id"] for row in base_results_data["questions"]
    ]
    answer_order = list(base_answers.keys())
    merged_result_rows = [
        merged_results[question_id] for question_id in ordered_question_ids
    ]
    merged_answer_rows = [merged_answers[question_id] for question_id in answer_order]
    aggregate_stats = compute_aggregate(merged_result_rows)

    results_data = {
        "system_name": "omni_agentic_deepseek_v4_pro_full500_merged_gpt54_adjusted",
        "created_utc": datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "source_result_files": [
            str(base_results_path),
            str(patch12_results_path),
            str(patch3_results_path),
        ],
        "source_answer_files": [
            str(base_answers_path),
            str(patch12_answers_path),
        ],
        "aggregate_stats": aggregate_stats,
        "question_type_stats": compute_question_type_stats(merged_result_rows),
        "questions": merged_result_rows,
        "canonical_run_label": base_results_data.get(
            "canonical_run_label", "omni_agentic_deepseek_v4pro_full500"
        ),
        "canonicalized_utc": base_results_data.get("canonicalized_utc"),
        "merge_manifest": args.out_manifest,
    }

    manifest = {
        "created_utc": results_data["created_utc"],
        "base_answers": str(base_answers_path),
        "base_results": str(base_results_path),
        "patch12_answers": str(patch12_answers_path),
        "patch12_results": str(patch12_results_path),
        "patch3_results": str(patch3_results_path),
        "out_answers": args.out_answers,
        "out_results": args.out_results,
        "policy": (
            "Select GPT-5.4 answer/judgement or judgement-only rows only when "
            "they improve completeness_pct if answer_correct else 0. Preserve "
            "DeepSeek document_ids and deterministic retrieval metrics."
        ),
        "selected_patch12_answer_and_judgement_rows": selected_patch12,
        "selected_patch3_judgement_only_rows": selected_patch3,
        "changed_rows_total": len(set(selected_patch12) | set(selected_patch3)),
        "changed_answer_rows": len(selected_patch12),
        "changed_judgement_only_rows": len(selected_patch3),
        "changed_document_id_rows": 0,
        "aggregate_stats": aggregate_stats,
    }

    write_jsonl(Path(args.out_answers), merged_answer_rows)
    write_json(Path(args.out_results), results_data)
    write_json(Path(args.out_manifest), manifest)

    print(json.dumps(aggregate_stats, indent=2))
    print(
        f"selected patch12 rows: {len(selected_patch12)}; "
        f"selected patch3 rows: {len(selected_patch3)}; "
        f"changed rows total: {manifest['changed_rows_total']}"
    )


if __name__ == "__main__":
    main()
