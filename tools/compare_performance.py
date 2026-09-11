"""Compare saved kernel or API performance CSV output; no third-party packages.

python tools/compare_performance.py before.csv after.csv --max-regression-percent 20
Kernel rows are grouped by scenario/sample_rate/frames across repeated batches.
"""
import argparse
import csv
import json
from pathlib import Path
import statistics
import sys


def load(path):
    groups = {}
    with path.open(encoding="utf-8-sig", newline="") as stream:
        rows = csv.DictReader(stream)
        for row in rows:
            key = (row["scenario"], row.get("sample_rate", ""), row.get("frames", ""))
            if int(row.get("errors", "0")):
                raise ValueError(f"{path}: {key} has errors")
            if not int(row["iterations"]):
                continue
            groups.setdefault(key, []).append(row)
    if not groups:
        raise ValueError(f"No successful measurement rows in {path}")
    return groups


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("baseline", type=Path)
    parser.add_argument("current", type=Path)
    parser.add_argument("--max-regression-percent", type=float, default=20)
    parser.add_argument("--max-budget-percent", type=float, default=50)
    args = parser.parse_args()
    if args.max_regression_percent < 0 or args.max_budget_percent <= 0:
        parser.error("Require nonnegative regression tolerance and positive budget")
    before, after = load(args.baseline), load(args.current)
    if before.keys() != after.keys():
        raise ValueError("Scenario/rate/buffer coverage differs between runs")
    failed = False
    for key, current in after.items():
        old = before[key]
        for field in ("iterations", "warmup"):
            if [row.get(field) for row in old] != [row.get(field) for row in current]:
                raise ValueError(f"{key}: sampling configuration differs")
        b = statistics.median(float(row["p50_us"]) for row in old)
        a = statistics.median(float(row["p50_us"]) for row in current)
        p99 = statistics.median(float(row["p99_us"]) for row in current)
        # A timer-floor-sized difference is not a useful performance failure.
        regressed = a > max(b * (1 + args.max_regression_percent / 100), b + .1)
        over_budget = any(float(row.get("p99_budget_percent", "0")) > args.max_budget_percent or int(row.get("overruns", "0")) > 0 for row in current)
        failed |= regressed or over_budget
        print(json.dumps({"scenario": key, "baseline_p50_us": b, "current_p50_us": a, "current_p99_us": p99, "regressed": regressed, "over_budget": over_budget}))
    return int(failed)


if __name__ == "__main__":
    sys.exit(main())
