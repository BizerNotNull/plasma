"""Measure a prebuilt Plasma executable; never include compilation in startup.

Example: python tools/performance.py target/release/plasma-ui.exe --output before.json
Run again with --baseline before.json --max-regression-percent 20 for a regression gate.
"""
import argparse
import json
import math
import os
from pathlib import Path
import platform
import statistics
import subprocess
import sys
import time


def percentile(values, fraction):
    return sorted(values)[max(0, math.ceil(len(values) * fraction) - 1)]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("executable", type=Path)
    parser.add_argument("--runs", type=int, default=7)
    parser.add_argument("--seconds", type=int, default=5)
    parser.add_argument("--workload", choices=("idle", "controls", "modulation", "resize"), default="idle")
    parser.add_argument("--no-audio", action="store_true")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--baseline", type=Path)
    parser.add_argument("--max-regression-percent", type=float, default=20)
    parser.add_argument("--max-first-frame-ms", type=float)
    args = parser.parse_args()
    if args.runs < 3 or not 1 <= args.seconds <= 300 or args.max_regression_percent < 0:
        parser.error("Require runs >= 3, seconds 1..300, nonnegative regression tolerance")
    command = [str(args.executable.resolve()), "--perf", f"--perf-seconds={args.seconds}", f"--perf-workload={args.workload}"]
    if args.no_audio:
        command.append("--perf-no-audio")
    runs = []
    for index in range(args.runs):
        start = time.perf_counter()
        result = subprocess.run(command, capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=args.seconds + 120)
        if result.returncode:
            raise RuntimeError(f"Run {index + 1} failed ({result.returncode}):\n{result.stderr}\n{result.stdout}")
        metrics = {}
        for line in result.stdout.splitlines():
            if line.startswith('{"metric":'):
                row = json.loads(line)
                metrics[row.pop("metric")] = row
        for required in ("first_frame_ms", "render_ms", "event_loop_lateness_ms", "profile"):
            if required not in metrics:
                raise RuntimeError(f"Run {index + 1} missing {required}; stdout={result.stdout!r}")
        runs.append({"metrics": metrics, "process_wall_ms": (time.perf_counter() - start) * 1000, "stderr": result.stderr})
        print(f"Run {index + 1}: first frame {metrics['first_frame_ms']['value']:.2f} ms", flush=True)
    summary = {}
    for metric, row in runs[0]["metrics"].items():
        if "value" in row and isinstance(row["value"], (int, float)):
            values = [run["metrics"][metric]["value"] for run in runs]
            summary[metric] = {"p50": statistics.median(values), "p95": percentile(values, .95), "max": max(values)}
        elif "p95" in row:
            summary[metric] = {"median_run_p95": statistics.median(run["metrics"][metric]["p95"] for run in runs)}
    config = {"workload": args.workload, "no_audio": args.no_audio, "seconds": args.seconds, "profile": runs[0]["metrics"]["profile"]["value"], "slint_backend": os.environ.get("SLINT_BACKEND", "default")}
    report = {"config": config, "platform": platform.platform(), "executable": str(args.executable.resolve()), "runs": runs, "summary": summary, "measurement": "First frame: main entry to renderer AfterRendering, not display scanout or OS process loader. Sequential launches; first launch retained separately, no claim of cold disk cache. Wall time includes workload and shutdown."}
    args.output.write_text(json.dumps(report, indent=2, ensure_ascii=False), encoding="utf-8")
    print(json.dumps(summary, indent=2))
    failed = []
    if args.max_first_frame_ms is not None and summary["first_frame_ms"]["p95"] > args.max_first_frame_ms:
        failed.append("first-frame p95 exceeds absolute budget")
    if args.baseline:
        baseline = json.loads(args.baseline.read_text(encoding="utf-8"))
        if baseline["config"] != config or baseline["platform"] != report["platform"]:
            raise RuntimeError("Baseline configuration/platform differs; compare like for like")
        for metric, key in (("first_frame_ms", "p50"), ("render_ms", "median_run_p95"), ("event_loop_lateness_ms", "median_run_p95")):
            before = baseline["summary"][metric][key]
            after = summary[metric][key]
            # Timer noise below one millisecond is not a meaningful regression.
            limit = max(before * (1 + args.max_regression_percent / 100), before + 1)
            print(f"{metric}/{key}: {before:.3f} -> {after:.3f}")
            if after > limit:
                failed.append(f"{metric} regressed beyond {limit:.3f} ms")
    if failed:
        print("\n".join(failed), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
