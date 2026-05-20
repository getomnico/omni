"""Filter EnterpriseRAG-Bench questions.jsonl down to a subset whose source_types
intersect a target set (default: confluence + jira).

Skips rows with no `source_types` (these are typically the High Level / Info Not
Found categories which don't carry expected_doc_ids).
"""

from __future__ import annotations

import argparse
import json
import logging
import sys
from collections import Counter
from pathlib import Path

logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")
log = logging.getLogger("filter_questions")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--questions",
        type=Path,
        required=True,
        help="path to questions.jsonl from the benchmark release",
    )
    parser.add_argument(
        "--output", type=Path, default=Path(__file__).parent / "questions_subset.jsonl"
    )
    parser.add_argument(
        "--source-types",
        default="confluence,jira",
        help="comma-separated source_types to keep",
    )
    parser.add_argument(
        "--strict",
        action="store_true",
        help="keep only rows whose source_types are a SUBSET of --source-types "
        "(default is intersection, which can include questions whose gold docs "
        "live in source types we don't index — bounds achievable recall)",
    )
    args = parser.parse_args()

    target = {s.strip() for s in args.source_types.split(",") if s.strip()}
    if not target:
        log.error("empty target source_types")
        return 2

    kept_by_type: Counter[str] = Counter()
    skipped_no_source: int = 0
    skipped_no_intersect: int = 0
    total = 0

    with args.questions.open("r", encoding="utf-8") as src, args.output.open(
        "w", encoding="utf-8"
    ) as dst:
        for line in src:
            line = line.rstrip("\n")
            if not line:
                continue
            total += 1
            row = json.loads(line)
            row_sources = row.get("source_types") or []
            if not row_sources:
                skipped_no_source += 1
                continue
            row_set = set(row_sources)
            if args.strict:
                if not row_set.issubset(target):
                    skipped_no_intersect += 1
                    continue
            elif not (row_set & target):
                skipped_no_intersect += 1
                continue
            kept_by_type[row.get("question_type", "<unknown>")] += 1
            dst.write(json.dumps(row, ensure_ascii=False) + "\n")

    log.info(
        "scanned=%d kept=%d skipped_no_source_types=%d skipped_no_intersect=%d",
        total,
        sum(kept_by_type.values()),
        skipped_no_source,
        skipped_no_intersect,
    )
    log.info("kept by question_type:")
    for qtype, n in sorted(kept_by_type.items(), key=lambda kv: (-kv[1], kv[0])):
        log.info("  %5d  %s", n, qtype)
    log.info("wrote %s", args.output)
    return 0


if __name__ == "__main__":
    sys.exit(main())
