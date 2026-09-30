#!/usr/bin/env python3
"""Resource-budget diff for CI (#105).

Compares the numbers a `test_resources` run just measured
(`<crate>/target/resource-report/*.json`) against the committed snapshots
(`<crate>/snapshots/resources/*.json`) and, with --base-ref, against the
snapshots on the base branch too. Prints a Markdown table (appended to
$GITHUB_STEP_SUMMARY when set) and exits 1 if any metric grew beyond its
scenario's tolerance relative to the committed snapshot.

Usage:
  scripts/resource-diff/resource_diff.py [--base-ref origin/main] crate...
"""

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

METRICS = [
    "cpu_insns",
    "mem_bytes",
    "read_entries",
    "write_entries",
    "read_bytes",
    "write_bytes",
]
SHORT = {
    "cpu_insns": "CPU",
    "mem_bytes": "Mem",
    "read_entries": "R#",
    "write_entries": "W#",
    "read_bytes": "RB",
    "write_bytes": "WB",
}


def load_dir(path: Path) -> dict:
    out = {}
    if path.is_dir():
        for f in sorted(path.glob("*.json")):
            out[f.stem] = json.loads(f.read_text())
    return out


def load_from_ref(root: Path, ref: str, rel: str) -> dict:
    """Snapshots as committed on `ref`, or {} if the dir doesn't exist there."""
    try:
        names = subprocess.run(
            ["git", "ls-tree", "--name-only", f"{ref}:{rel}"],
            cwd=root,
            check=True,
            capture_output=True,
            text=True,
        ).stdout.split()
    except subprocess.CalledProcessError:
        return {}
    out = {}
    for name in names:
        if name.endswith(".json"):
            blob = subprocess.run(
                ["git", "show", f"{ref}:{rel}/{name}"],
                cwd=root,
                check=True,
                capture_output=True,
                text=True,
            ).stdout
            out[name[:-5]] = json.loads(blob)
    return out


def pct(base: int, cur: int) -> float:
    if base == 0:
        return 0.0 if cur == 0 else 100.0
    return (cur - base) * 100.0 / base


def cell(base, cur: int, tol: float) -> tuple:
    """Formatted cell and whether it is a regression."""
    if base is None:
        return f"{cur:,} (new)", False
    change = pct(base, cur)
    regressed = cur > base and cur > base * (1 + tol / 100.0)
    if cur == base:
        return f"{cur:,}", False
    mark = " ❌" if regressed else ""
    return f"{cur:,} ({change:+.1f}%){mark}", regressed


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base-ref", help="also show deltas against this git ref")
    parser.add_argument("crates", nargs="+")
    args = parser.parse_args()

    root = Path(
        subprocess.run(
            ["git", "rev-parse", "--show-toplevel"],
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()
    )

    lines = ["### Resource budgets", ""]
    failed = False
    for crate in args.crates:
        rel = f"{crate}/snapshots/resources"
        committed = load_dir(root / rel)
        current = load_dir(root / crate / "target" / "resource-report")
        base = load_from_ref(root, args.base_ref, rel) if args.base_ref else {}
        if not current:
            lines += [f"**{crate}**: no measurements (did the resource tests run?)", ""]
            failed = True
            continue

        mode = next(iter(current.values()))["mode"]
        lines.append(f"**{crate}** ({mode} execution; cell = current vs committed snapshot)")
        lines.append("")
        header = "| Scenario | " + " | ".join(SHORT[m] for m in METRICS) + " |"
        if base:
            header += f" CPU vs `{args.base_ref}` |"
        lines.append(header)
        lines.append("|---" * (len(METRICS) + 1 + (1 if base else 0)) + "|")

        for name, snap in sorted(current.items()):
            tol = snap.get("tolerance_pct", 5.0)
            ref = committed.get(name)
            cells = []
            for m in METRICS:
                text, bad = cell(
                    ref["metrics"][m] if ref else None, snap["metrics"][m], tol
                )
                failed |= bad
                cells.append(text)
            row = f"| `{name}` | " + " | ".join(cells) + " |"
            if base:
                b = base.get(name)
                row += (
                    f" {pct(b['metrics']['cpu_insns'], snap['metrics']['cpu_insns']):+.1f}% |"
                    if b
                    else " new |"
                )
            lines.append(row)
        missing = sorted(set(committed) - set(current))
        if missing:
            lines.append("")
            lines.append("Snapshots with no measurement this run: " + ", ".join(missing))
        lines.append("")

    lines.append(
        "Legend: R#/W# = read/write ledger entries, RB/WB = read/write bytes. "
        "❌ = above the scenario's tolerance (default 5%)."
    )
    text = "\n".join(lines)
    print(text)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a", encoding="utf-8") as fh:
            fh.write(text + "\n")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
