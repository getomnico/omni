#!/usr/bin/env python3
"""Check answer file quality: doc IDs, answers, no-submit rate."""

import json
import argparse
from collections import Counter


def main():
    parser = argparse.ArgumentParser(description="Check answer file quality")
    parser.add_argument("--answers", required=True, help="Answers JSONL file")
    args = parser.parse_args()

    answers = []
    with open(args.answers) as f:
        for line in f:
            answers.append(json.loads(line))

    print(f"Total answers: {len(answers)}")

    # Doc ID distribution
    doc_counts = Counter(len(a.get("document_ids", [])) for a in answers)
    print(f"\nDoc ID distribution:")
    for count in sorted(doc_counts.keys()):
        print(
            f"  {count:3d} docs: {doc_counts[count]:4d} answers ({doc_counts[count]/len(answers)*100:.1f}%)"
        )

    # No-submit rate
    no_submit = [a for a in answers if "agent did not submit" in a.get("answer", "")]
    print(
        f"\nNo submit_answer: {len(no_submit)} ({len(no_submit)/len(answers)*100:.1f}%)"
    )

    # Zero doc IDs
    zero_docs = [a for a in answers if not a.get("document_ids")]
    print(f"Zero doc IDs: {len(zero_docs)} ({len(zero_docs)/len(answers)*100:.1f}%)")

    # Has actual text
    has_text = [
        a
        for a in answers
        if a.get("answer") and "agent did not submit" not in a["answer"]
    ]
    print(f"Has answer text: {len(has_text)} ({len(has_text)/len(answers)*100:.1f}%)")


if __name__ == "__main__":
    main()
