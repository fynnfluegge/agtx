#!/usr/bin/env python3
"""
Print a summary table for a completed benchmark + evaluation run.

Usage:
    python report.py --results swebench_output/agtx_claude_.../results.json \
                     --logs    ../../logs/run_evaluation/<run_id>/
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path


def find_resolved(logs_dir: Path) -> dict[str, bool]:
    """Walk logs dir and collect resolved status per instance_id."""
    resolved = {}
    for report_file in logs_dir.rglob("report.json"):
        data = json.loads(report_file.read_text())
        for instance_id, info in data.items():
            resolved[instance_id] = info.get("resolved", False)
    return resolved


def fmt_duration(seconds: float) -> str:
    m = int(seconds) // 60
    s = int(seconds) % 60
    return f"{m}m {s:02d}s" if m else f"{s}s"


def fmt_tokens(n: int | None) -> str:
    if n is None:
        return "n/a"
    if n >= 1_000_000:
        return f"{n / 1_000_000:.1f}M"
    if n >= 1_000:
        return f"{n / 1_000:.0f}K"
    return str(n)


def fmt_cost(usd: float | None) -> str:
    if usd is None:
        return "n/a"
    return f"${usd:.2f}"


def print_table(rows: list[dict], resolved: dict[str, bool]) -> None:
    # Build display rows
    display = []
    totals = {"duration": 0.0, "cost": 0.0, "tokens": 0, "resolved": 0}

    for r in rows:
        iid = r["instance_id"]
        short = iid.replace("__", "/")
        is_resolved = resolved.get(iid)

        if is_resolved is None:
            status = "⏳ pending"
        elif is_resolved:
            status = "✅ resolved"
            totals["resolved"] += 1
        else:
            status = "❌ failed"

        dur = r.get("duration_seconds") or 0.0
        cost = r.get("cost_usd")
        tokens = r.get("cost_tokens")

        totals["duration"] += dur
        if cost is not None:
            totals["cost"] += cost
        if tokens is not None:
            totals["tokens"] += tokens

        display.append((short, status, fmt_duration(dur), fmt_cost(cost), fmt_tokens(tokens)))

    # Column widths
    headers = ("Instance", "Status", "Duration", "Cost", "Tokens")
    widths = [len(h) for h in headers]
    for row in display:
        for i, cell in enumerate(row):
            widths[i] = max(widths[i], len(cell))

    def sep(left, mid, right, fill="─"):
        parts = [fill * (w + 2) for w in widths]
        return left + mid.join(parts) + right

    def row_line(cells):
        parts = [f" {c:<{widths[i]}} " for i, c in enumerate(cells)]
        return "│" + "│".join(parts) + "│"

    print(sep("┌", "┬", "┐"))
    print(row_line(headers))
    print(sep("├", "┼", "┤"))
    for i, row in enumerate(display):
        print(row_line(row))
        if i < len(display) - 1:
            print(sep("├", "┼", "┤"))
    print(sep("└", "┴", "┘"))

    # Summary line
    n = len(rows)
    cost_str = fmt_cost(totals["cost"] if totals["cost"] else None)
    tokens_str = fmt_tokens(totals["tokens"] if totals["tokens"] else None)
    print(
        f"\n{totals['resolved']}/{n} resolved  ·  "
        f"{fmt_duration(totals['duration'])} total  ·  "
        f"{cost_str} total  ·  {tokens_str} tokens"
    )


def main() -> None:
    parser = argparse.ArgumentParser(description="Print benchmark + evaluation summary table.")
    parser.add_argument("--results", required=True, help="Path to results.json from benchmark run")
    parser.add_argument("--logs", required=True, help="Path to evaluation logs dir (contains report.json files)")
    args = parser.parse_args()

    results_path = Path(args.results)
    logs_path = Path(args.logs)

    if not results_path.exists():
        print(f"results.json not found: {results_path}")
        raise SystemExit(1)
    if not logs_path.exists():
        print(f"Logs dir not found: {logs_path}")
        raise SystemExit(1)

    rows = json.loads(results_path.read_text())
    resolved = find_resolved(logs_path)
    print_table(rows, resolved)


if __name__ == "__main__":
    main()
