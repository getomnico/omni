from typing import Dict

try:
    from tabulate import tabulate
    _HAS_TABULATE = True
except ImportError:
    _HAS_TABULATE = False


def format_scores_table(
    scores: Dict[str, float],
    thresholds: Dict[str, float],
) -> str:
    """Return a formatted table string for terminal output."""
    rows = []
    for metric, score in sorted(scores.items()):
        threshold = thresholds.get(metric)
        if threshold is not None:
            status = "PASS" if score >= threshold else "FAIL"
            thresh_str = f"{threshold:.2f}"
        else:
            status = "—"
            thresh_str = "—"
        rows.append([metric, f"{score:.4f}", thresh_str, status])

    headers = ["Metric", "Score", "Threshold", "Status"]

    if _HAS_TABULATE:
        return tabulate(rows, headers=headers, tablefmt="simple")

    # Fallback without tabulate
    col_widths = [max(len(h), max((len(r[i]) for r in rows), default=0)) for i, h in enumerate(headers)]
    sep = "  ".join("-" * w for w in col_widths)
    header_line = "  ".join(h.ljust(col_widths[i]) for i, h in enumerate(headers))
    lines = [header_line, sep]
    for row in rows:
        lines.append("  ".join(cell.ljust(col_widths[i]) for i, cell in enumerate(row)))
    return "\n".join(lines)


def print_results(scores: Dict[str, float], thresholds: Dict[str, float]) -> None:
    """Print evaluation results to stdout."""
    print()
    print("=" * 60)
    print("EVALUATION RESULTS")
    print("=" * 60)
    print(format_scores_table(scores, thresholds))
    print()

    passed = all(scores.get(m, 0) >= t for m, t in thresholds.items())
    if passed:
        print("Overall: PASS — all metrics above threshold")
    else:
        failing = [m for m, t in thresholds.items() if scores.get(m, 0) < t]
        print(f"Overall: FAIL — metrics below threshold: {', '.join(failing)}")
    print()
