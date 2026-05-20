#!/usr/bin/env python3
"""Analyze benchmark results broken down by question category."""

import json
import argparse
from pathlib import Path
from collections import defaultdict


def load_results(results_path: Path) -> dict:
    with open(results_path) as f:
        return json.load(f)


def load_questions(questions_path: Path) -> dict:
    questions = {}
    with open(questions_path) as f:
        for line in f:
            d = json.loads(line)
            questions[d["question_id"]] = d
    return questions


def get_recall(q: dict) -> float:
    return q.get("document_recall_pct") or 0.0


def analyze_by_category(results: dict, questions: dict):
    """Break down results by question category."""
    cats = defaultdict(
        lambda: {
            "total": 0,
            "correct": 0,
            "correct_pct_sum": 0.0,
            "complete_pct_sum": 0.0,
            "recall_pct_sum": 0.0,
            "recall_dist": {"zero": 0, "low": 0, "med": 0, "perfect": 0},
            "invalid_extra_sum": 0.0,
            "questions": [],
        }
    )

    for r in results["questions"]:
        qid = r["question_id"]
        q = questions.get(qid, {})
        category = q.get("category") or q.get("question_type", "unknown")

        cats[category]["total"] += 1
        if r["answer_correct"]:
            cats[category]["correct"] += 1
        cats[category]["correct_pct_sum"] += (
            r.get("completeness_pct", 0) if r["answer_correct"] else 0
        )
        cats[category]["complete_pct_sum"] += r.get("completeness_pct", 0)
        recall = get_recall(r)
        cats[category]["recall_pct_sum"] += recall
        cats[category]["invalid_extra_sum"] += r.get("invalid_extra_docs") or 0

        if recall == 0:
            cats[category]["recall_dist"]["zero"] += 1
        elif recall < 50:
            cats[category]["recall_dist"]["low"] += 1
        elif recall < 100:
            cats[category]["recall_dist"]["med"] += 1
        else:
            cats[category]["recall_dist"]["perfect"] += 1

        cats[category]["questions"].append(
            {
                "qid": qid,
                "question": q.get("question", ""),
                "recall": recall,
                "correct": r["answer_correct"],
                "complete": r.get("completeness_pct", 0),
            }
        )

    # Print table
    print(
        f"{'Category':<35} {'N':>4} {'Correct%':>8} {'Complete%':>10} {'Recall%':>8} {'Zero%':>7} {'Perfect%':>9} {'InvExtra':>8}"
    )
    print("-" * 105)

    rows = []
    for cat, data in sorted(cats.items()):
        n = data["total"]
        correct_pct = data["correct"] / n * 100
        complete_pct = data["complete_pct_sum"] / n
        recall_pct = data["recall_pct_sum"] / n
        zero_pct = data["recall_dist"]["zero"] / n * 100
        perfect_pct = data["recall_dist"]["perfect"] / n * 100
        inv_extra = data["invalid_extra_sum"] / n
        overall = data["correct_pct_sum"] / n

        rows.append(
            (
                cat,
                n,
                correct_pct,
                complete_pct,
                recall_pct,
                zero_pct,
                perfect_pct,
                inv_extra,
                overall,
                data,
            )
        )

    # Sort by overall score descending
    rows.sort(key=lambda x: x[8], reverse=True)

    for (
        cat,
        n,
        correct_pct,
        complete_pct,
        recall_pct,
        zero_pct,
        perfect_pct,
        inv_extra,
        overall,
        data,
    ) in rows:
        print(
            f"{cat:<35} {n:>4} {correct_pct:>7.1f}% {complete_pct:>9.1f}% {recall_pct:>7.1f}% {zero_pct:>6.1f}% {perfect_pct:>8.1f}% {inv_extra:>8.1f}"
        )

    print("-" * 105)
    # Overall
    agg = results.get("aggregate_stats", {})
    total_n = len(results["questions"])
    print(
        f"{'OVERALL':<35} {total_n:>4} {agg.get('average_correctness_pct', 0):>7.1f}% {agg.get('average_completeness_pct', 0):>9.1f}% {agg.get('average_recall_pct', 0):>7.1f}% {'':>6} {'':>8} {agg.get('average_invalid_extra_docs', 0):>8.1f}"
    )
    print()

    # Print worst categories (highest zero recall)
    print("WORST CATEGORIES (by zero-recall rate):")
    rows_by_zero = sorted(rows, key=lambda x: x[5], reverse=True)
    for (
        cat,
        n,
        correct_pct,
        complete_pct,
        recall_pct,
        zero_pct,
        perfect_pct,
        inv_extra,
        overall,
        data,
    ) in rows_by_zero[:5]:
        print(f"  {cat:<35} zero={zero_pct:.1f}%  n={n}  overall={overall:.1f}%")
    print()

    # Print sample questions from worst category
    worst_cat = rows_by_zero[0][0]
    print(f"Sample zero-recall questions from '{worst_cat}':")
    for q in [q for q in cats[worst_cat]["questions"] if q["recall"] == 0][:3]:
        print(f"  {q['qid']}: {q['question'][:80]}...")


def main():
    parser = argparse.ArgumentParser(
        description="Analyze benchmark results by category"
    )
    parser.add_argument("--results", required=True, help="Results JSON file")
    parser.add_argument("--questions", required=True, help="Questions JSONL file")
    args = parser.parse_args()

    results = load_results(Path(args.results))
    questions = load_questions(Path(args.questions))

    analyze_by_category(results, questions)


if __name__ == "__main__":
    main()
