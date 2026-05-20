#!/usr/bin/env python3
"""Analyze benchmark results. Reusable script for failure analysis."""

import json
import argparse
from pathlib import Path


def load_results(results_path: Path) -> dict:
    with open(results_path) as f:
        return json.load(f)


def load_answers(answers_path: Path) -> dict:
    """Load answers as dict keyed by question_id."""
    answers = {}
    with open(answers_path) as f:
        for line in f:
            d = json.loads(line)
            answers[d["question_id"]] = d
    return answers


def load_questions(questions_path: Path) -> dict:
    """Load questions as dict keyed by question_id."""
    questions = {}
    with open(questions_path) as f:
        for line in f:
            d = json.loads(line)
            questions[d["question_id"]] = d
    return questions


def get_recall(q: dict) -> float:
    return q.get("document_recall_pct") or 0.0


def categorize(results: dict) -> dict:
    """Categorize questions by recall bucket."""
    questions = results["questions"]
    return {
        "zero": [q for q in questions if get_recall(q) == 0],
        "low": [q for q in questions if 0 < get_recall(q) < 50],
        "med": [q for q in questions if 50 <= get_recall(q) < 100],
        "perfect": [q for q in questions if get_recall(q) == 100],
    }


def print_summary(results: dict):
    cats = categorize(results)
    total = len(results["questions"])
    print(f"Total questions: {total}")
    print(
        f"  Zero recall (0%):    {len(cats['zero'])} ({len(cats['zero'])/total*100:.1f}%)"
    )
    print(
        f"  Low recall (1-49%):  {len(cats['low'])} ({len(cats['low'])/total*100:.1f}%)"
    )
    print(
        f"  Med recall (50-99%): {len(cats['med'])} ({len(cats['med'])/total*100:.1f}%)"
    )
    print(
        f"  Perfect (100%):      {len(cats['perfect'])} ({len(cats['perfect'])/total*100:.1f}%)"
    )
    print()

    agg = results.get("aggregate_stats", {})
    print(f"Overall: {agg.get('combined_correctness_completeness_score', 0):.1f}%")
    print(f"Correct: {agg.get('average_correctness_pct', 0):.1f}%")
    print(f"Complete: {agg.get('average_completeness_pct', 0):.1f}%")
    print(f"Recall: {agg.get('average_recall_pct', 0):.1f}%")
    print(f"Invalid extras: {agg.get('average_invalid_extra_docs', 0):.1f}")


def analyze_question(qid: str, results: dict, answers: dict, questions: dict):
    """Deep analysis of a single question."""
    # Find result record
    result = next((q for q in results["questions"] if q["question_id"] == qid), None)
    if not result:
        print(f"Question {qid} not found in results")
        return

    answer = answers.get(qid)
    question = questions.get(qid)

    print(f"\n{'='*70}")
    print(f"QUESTION: {qid}")
    print(f"{'='*70}")
    print(f"Text: {question['question']}")
    print()

    if question:
        gold_docs = question.get("expected_doc_ids", [])
        print(f"Gold docs ({len(gold_docs)}):")
        for gd in gold_docs:
            print(f"  - {gd}")
        print()

    if answer:
        our_docs = answer.get("document_ids", [])
        print(f"Our docs ({len(our_docs)}):")
        for d in our_docs[:15]:
            print(f"  - {d}")
        if len(our_docs) > 15:
            print(f"  ... and {len(our_docs)-15} more")
        print()

        # Compute overlap
        if question and gold_docs:
            overlap = set(gold_docs) & set(our_docs)
            missing = set(gold_docs) - set(our_docs)
            extra = set(our_docs) - set(gold_docs)
            print(f"Overlap: {len(overlap)} / {len(gold_docs)}")
            print(f"Missing: {sorted(missing)}")
            print(f"Extra (non-gold): {len(extra)}")

    if result:
        print()
        print(f"Scores:")
        print(f"  Correct: {result['answer_correct']}")
        print(f"  Completeness: {result['completeness_pct']}%")
        print(f"  Recall: {get_recall(result)}%")
        print(f"  Invalid extras: {result.get('invalid_extra_docs', 'N/A')}")

    if answer:
        print()
        print(f"Answer preview: {answer.get('answer', '')[:300]}...")


def main():
    parser = argparse.ArgumentParser(description="Analyze benchmark results")
    parser.add_argument("--results", required=True, help="Results JSON file")
    parser.add_argument("--answers", required=True, help="Answers JSONL file")
    parser.add_argument("--questions", required=True, help="Questions JSONL file")
    parser.add_argument("--summary", action="store_true", help="Print summary only")
    parser.add_argument(
        "--qid", action="append", help="Analyze specific question ID(s)"
    )
    parser.add_argument(
        "--sample", type=int, default=5, help="Sample N from each bucket"
    )
    args = parser.parse_args()

    results = load_results(Path(args.results))
    answers = load_answers(Path(args.answers))
    questions = load_questions(Path(args.questions))

    if args.summary:
        print_summary(results)
        return

    print_summary(results)

    cats = categorize(results)

    # Analyze samples from each bucket
    for bucket_name, bucket in [
        ("Zero recall (0%)", cats["zero"]),
        ("Low recall (1-49%)", cats["low"]),
        ("Med recall (50-99%)", cats["med"]),
        ("Perfect recall (100%)", cats["perfect"]),
    ]:
        print(f"\n{'='*70}")
        print(f"BUCKET: {bucket_name} ({len(bucket)} questions)")
        print(f"{'='*70}")
        for q in bucket[: args.sample]:
            analyze_question(q["question_id"], results, answers, questions)

    # Analyze specific qids if requested
    if args.qid:
        for qid in args.qid:
            analyze_question(qid, results, answers, questions)


if __name__ == "__main__":
    main()
