#!/usr/bin/env python3
"""
Create golden_set.yaml from the MRQA validation split via HuggingFace, and export
context chunks as .txt corpus files for ingestion via the Omni filesystem connector.

The corpus and golden set are sized independently: the corpus is the retrieval
haystack (thousands of docs), while the golden set is a small curated slice of
queries whose source documents are guaranteed to live in that corpus.

Each golden entry contains:
  - query: the question
  - reference_answer: first answer annotation
  - context_chunks: the supporting passage (oracle context)
  - reference_doc_ids: container path of the corpus file holding this passage

No LLM key required. The HuggingFace `datasets` library caches to
~/.cache/huggingface/datasets on first run.

Usage:
    cd services/ai
    uv run python -m evaluation.datasets.generate_golden \
        --corpus-size 5000 --golden-size 20 \
        --output evaluation/datasets/golden_set.yaml
"""

import argparse
import hashlib
from pathlib import Path

import yaml

# Must match the volume mount destination in docker-compose.dev.yml
CORPUS_CONTAINER_PATH = "/data/eval-corpus"


def _corpus_filename(text: str) -> str:
    sha = hashlib.sha1(text.encode()).hexdigest()[:12]
    return f"{sha}.txt"


def _stratified_pick(
    subset_candidates: dict[str, list[dict]], golden_size: int
) -> list[dict]:
    """Pick `golden_size` samples spread evenly across MRQA subsets.

    Deterministic: subsets iterated in sorted order; within each subset picks
    are at evenly-spaced indices (no RNG). Subsets with fewer candidates than
    their allocation contribute everything they have; the shortfall is
    redistributed to later subsets so the total hits `golden_size` when
    possible.
    """
    subsets = sorted(subset_candidates)
    if not subsets:
        return []

    remaining = golden_size
    picks: list[dict] = []
    for idx, subset in enumerate(subsets):
        subsets_left = len(subsets) - idx
        # Ceil division evenly distributes the remainder across the earliest
        # subsets while keeping the total exactly `golden_size`.
        allocation = min(-(-remaining // subsets_left), len(subset_candidates[subset]))
        candidates = subset_candidates[subset]
        if allocation > 0:
            step = len(candidates) / allocation
            picks.extend(candidates[int(j * step)] for j in range(allocation))
            remaining -= allocation

    return picks


def generate(corpus_size: int, golden_size: int, output_path: Path) -> None:
    import shutil

    from datasets import load_dataset

    if golden_size > corpus_size:
        raise ValueError(
            f"--golden-size ({golden_size}) cannot exceed --corpus-size ({corpus_size})"
        )

    print(
        f"Loading MRQA validation split "
        f"(corpus={corpus_size} unique docs, golden={golden_size} queries)..."
    )
    ds = load_dataset(
        "mrqa-workshop/mrqa",
        split="validation",
        trust_remote_code=False,
    )

    corpus_dir = output_path.parent / "corpus"
    if corpus_dir.exists():
        shutil.rmtree(corpus_dir)
    corpus_dir.mkdir(parents=True)

    seen_hashes: set[str] = set()
    # subset name → list of candidate golden samples (every valid MRQA row we
    # encountered while building the corpus). Preserves dataset order so the
    # evenly-spaced pick below is deterministic.
    subset_candidates: dict[str, list[dict]] = {}

    for i, row in enumerate(ds):
        if len(seen_hashes) >= corpus_size:
            break

        answers = row.get("answers") or []
        if not answers:
            continue

        question = row.get("question", "").strip()
        context = row.get("context", "").strip()
        answer = answers[0].strip()

        if not question or not context or not answer:
            continue

        # Truncate context to 800 chars — keeps RAGAS token usage manageable
        # and matches what we expose in the golden set.
        text = context[:800]
        filename = _corpus_filename(text)

        if filename not in seen_hashes:
            (corpus_dir / filename).write_text(text, encoding="utf-8")
            seen_hashes.add(filename)

        subset = row.get("subset", "unknown").lower()
        subset_candidates.setdefault(subset, []).append({
            "id": f"mrqa-{i:05d}",
            "query": question,
            "task_family": "current_state",
            "temporal_type": "none",
            "reference_answer": answer,
            "reference_doc_ids": [f"{CORPUS_CONTAINER_PATH}/{filename}"],
            "context_chunks": [{"text": text}],
            "tags": ["mrqa", subset],
            "difficulty": "medium",
        })

    golden_samples = _stratified_pick(subset_candidates, golden_size)

    # Invariant: every golden query's source document must be in the exported
    # corpus. This is already guaranteed by the iteration order (candidates
    # only collected while their file is being written), but verify explicitly
    # so a future refactor can't silently break the contract.
    for sample in golden_samples:
        for doc_id in sample["reference_doc_ids"]:
            filename = doc_id.rsplit("/", 1)[-1]
            if not (corpus_dir / filename).exists():
                raise RuntimeError(
                    f"Golden sample {sample['id']} references {doc_id} "
                    f"but {corpus_dir / filename} was not exported"
                )

    print(
        f"Exported {len(seen_hashes)} corpus files to {corpus_dir}/ "
        f"(target: {corpus_size})"
    )
    subset_breakdown = ", ".join(
        f"{s}={sum(1 for g in golden_samples if s in g['tags'])}"
        for s in sorted(subset_candidates)
    )
    print(
        f"Collected {len(golden_samples)} golden queries (target: {golden_size}) "
        f"[{subset_breakdown}]"
    )

    output_path.parent.mkdir(parents=True, exist_ok=True)
    header = (
        "# Auto-generated from MRQA validation split via HuggingFace.\n"
        "# To regenerate: cd services/ai && "
        "uv run python -m evaluation.datasets.generate_golden "
        f"--corpus-size {corpus_size} --golden-size {golden_size}\n\n"
    )
    with open(output_path, "w", encoding="utf-8") as f:
        f.write(header)
        yaml.dump(golden_samples, f, allow_unicode=True, default_flow_style=False, sort_keys=False)
    print(f"Written to {output_path}")


def main():
    parser = argparse.ArgumentParser(
        description="Generate RAG golden set and corpus from MRQA via HuggingFace"
    )
    parser.add_argument(
        "--corpus-size",
        type=int,
        default=5000,
        help="Number of unique corpus documents to export (default: 5000)",
    )
    parser.add_argument(
        "--golden-size",
        type=int,
        default=20,
        help="Number of golden queries to emit (default: 20)",
    )
    parser.add_argument(
        "--output",
        default="evaluation/datasets/golden_set.yaml",
        help="Output YAML path",
    )
    args = parser.parse_args()
    generate(args.corpus_size, args.golden_size, Path(args.output))


if __name__ == "__main__":
    main()
