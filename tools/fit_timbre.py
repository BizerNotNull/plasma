"""Bounded LLM-guided parameter fitting against the real offline Plasma voice."""

from __future__ import annotations

import argparse
import json
import math
import os
from pathlib import Path
import shutil
import subprocess
import sys
import urllib.error
import urllib.parse
import urllib.request

import soundfile as sf

from timbre_audio import analyze, compare, load_target

ROOT = Path(__file__).resolve().parents[1]
RATE = 48000


def dump(path, value):
    path.write_text(
        json.dumps(value, ensure_ascii=False, indent=2, allow_nan=False) + "\n",
        encoding="utf-8",
    )


def run_renderer(renderer, request, request_path, wav_path):
    dump(request_path, request)
    result = subprocess.run(
        [str(renderer), str(request_path), str(wav_path)],
        capture_output=True,
        text=True,
        timeout=120,
    )
    if result.returncode:
        raise ValueError("renderer rejected candidate: " + result.stderr.strip()[:2000])
    audio, rate = sf.read(wav_path, dtype="float64", always_2d=True)
    if rate != request["sample_rate"]:
        raise ValueError("renderer returned wrong sample rate")
    return audio


def endpoint(base):
    parsed = urllib.parse.urlsplit(base)
    if parsed.username or parsed.password or parsed.query or parsed.fragment:
        raise ValueError("base URL must not contain credentials, query or fragment")
    if not parsed.hostname or (
        parsed.scheme != "https"
        and not (
            parsed.scheme == "http"
            and parsed.hostname in {"localhost", "127.0.0.1", "::1"}
        )
    ):
        raise ValueError("base URL requires HTTPS (HTTP allowed only on loopback)")
    return base.rstrip("/") + "/chat/completions"


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        # Never forward API credentials to a redirect destination.
        raise ValueError("provider redirects are not allowed; use the final base URL")


def propose(url, key, model, system, feedback, timeout):
    body = json.dumps(
        {
            "model": model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": json.dumps(feedback, allow_nan=False)},
            ],
            "response_format": {"type": "json_object"},
        },
        allow_nan=False,
    ).encode()
    headers = {"Content-Type": "application/json"}
    if key:
        headers["Authorization"] = "Bearer " + key
    request = urllib.request.Request(url, body, headers, method="POST")
    try:
        with urllib.request.build_opener(NoRedirect).open(
            request, timeout=timeout
        ) as response:
            raw = response.read(65537)
        if len(raw) > 65536:
            raise ValueError("provider response exceeds 64 KiB")
        payload = json.loads(raw)
        content = payload["choices"][0]["message"]["content"]
        proposal = json.loads(content)
        # Reject both explicit NaN/Infinity and overflow such as 1e400 before logging.
        json.dumps(proposal, allow_nan=False)
    except urllib.error.HTTPError as exc:
        # Provider error bodies can contain private request or credential data.
        raise ValueError(
            f"provider HTTP {exc.code}; check endpoint, model and credentials"
        ) from None
    except (KeyError, IndexError, TypeError, json.JSONDecodeError) as exc:
        raise ValueError(
            "provider must return a JSON object in choices[0].message.content"
        ) from exc
    if not isinstance(proposal, dict) or set(proposal) != {"patch", "gate", "reason"}:
        raise ValueError("proposal must contain exactly patch, gate, reason")
    if not isinstance(proposal["reason"], str) or len(proposal["reason"]) > 2000:
        raise ValueError("reason must be a string of at most 2000 characters")
    return proposal


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("sample", type=Path)
    parser.add_argument(
        "--output", type=Path, required=True, help="new run directory (must not exist)"
    )
    parser.add_argument(
        "--renderer", type=Path, help="compiled plasma-render executable"
    )
    parser.add_argument(
        "--iterations",
        type=int,
        default=12,
        help="maximum LLM requests; 0 = local baseline only",
    )
    parser.add_argument("--model", default=os.getenv("OPENAI_MODEL"))
    parser.add_argument(
        "--base-url", default=os.getenv("OPENAI_BASE_URL", "https://api.openai.com/v1")
    )
    parser.add_argument("--api-key-env", default="OPENAI_API_KEY")
    parser.add_argument(
        "--frequency", type=float, help="known fundamental in Hz; overrides estimate"
    )
    parser.add_argument(
        "--gate",
        type=float,
        help="known note-off time in seconds after trimmed onset; otherwise fitted",
    )
    parser.add_argument("--seed", type=int, default=42)
    parser.add_argument("--timeout", type=float, default=90)
    args = parser.parse_args()
    if (
        not 0 <= args.iterations <= 100
        or not 0 <= args.seed < 2**64
        or not math.isfinite(args.timeout)
        or args.timeout <= 0
    ):
        parser.error(
            "iterations must be 0..100, seed a u64, timeout finite and positive"
        )
    if args.iterations and not args.model:
        parser.error("--model or OPENAI_MODEL is required for LLM fitting")
    url = endpoint(args.base_url) if args.iterations else None
    key = os.getenv(args.api_key_env, "")
    if (
        args.iterations
        and not key
        and urllib.parse.urlsplit(url).hostname not in {"localhost", "127.0.0.1", "::1"}
    ):
        parser.error(f"set {args.api_key_env} for the remote provider")
    renderer = args.renderer or ROOT / "src/harness/target/release" / (
        "plasma-render.exe" if os.name == "nt" else "plasma-render"
    )
    renderer = renderer.resolve()
    if not renderer.is_file():
        parser.error(
            "renderer missing; run cargo build --release --manifest-path src/harness/Cargo.toml"
        )
    target, metadata = load_target(args.sample, RATE)
    description = analyze(target, RATE)
    frequency = (
        args.frequency
        if args.frequency is not None
        else description.get("frequency_hz")
    )
    if frequency is None or not math.isfinite(frequency) or not 20 <= frequency <= 5000:
        parser.error(
            "no reliable fundamental in 20..5000 Hz; specify --frequency for a single note"
        )
    duration = len(target) / RATE
    gate = args.gate if args.gate is not None else duration * 0.75
    if not math.isfinite(gate) or not 0 < gate <= duration:
        parser.error("gate must be >0 and <= trimmed sample duration")
    schema_result = subprocess.run(
        [str(renderer), "--describe"],
        check=True,
        capture_output=True,
        text=True,
        timeout=30,
    )
    schema = json.loads(schema_result.stdout)
    args.output.mkdir(parents=True, exist_ok=False)
    output = args.output.resolve()
    sf.write(output / "target.wav", target, RATE, subtype="FLOAT")
    request = {
        "patch": schema["default_patch"],
        "sample_rate": RATE,
        "frequency": frequency,
        "duration": duration,
        "gate": gate,
        "seed": args.seed,
    }
    baseline = run_renderer(
        renderer, request, output / "baseline.json", output / "baseline.wav"
    )
    best_score = compare(target, baseline, RATE)
    best = {
        "request": request,
        "score": best_score,
        "analysis": analyze(baseline, RATE),
    }
    dump(output / "best.json", request)
    shutil.copyfile(output / "baseline.wav", output / "best.wav")
    dump(
        output / "run.json",
        {
            "sample": str(args.sample.resolve()),
            "metadata": metadata,
            "target": description,
            "frequency": frequency,
            "duration": duration,
            "fixed_gate": args.gate,
            "seed": args.seed,
            "model": args.model,
            "base_url": args.base_url if args.iterations else None,
            "iterations": args.iterations,
            "schema": schema,
        },
    )
    system = (
        "You fit a single-note audio sample using the real Plasma three-oscillator synthesizer. "
        "Only numeric audio descriptors are supplied, not audio. Minimize score.total; lower is better. "
        "Return ONLY JSON with exactly {patch: full patch object, gate: note-off seconds, reason: short explanation}. "
        "All 36 controls use native units in the given order, NOT normalized values. "
        "Keep all fields, arrays and route dimensions exactly as schema; unison must be integer. "
        "routes[0] is unipolar ENV and routes[1] bipolar LFO, signed normalized depths [-1,1]. "
        "ADSR also controls final amplitude. Fit waveform/harmonic mix/filter, then envelope, detune and modulation. "
        "Use best-so-far, observed loss components and recent failures; do not assume a candidate improved before evaluation. "
        "Never emit code, shell commands, paths or network instructions. "
        "Gate must be >0 and <= duration; if fixed_gate is set, preserve it exactly. "
        "Patch schema and default: " + json.dumps(schema)
    )
    recent = []
    successes = 0
    print(
        f"baseline loss={best_score['total']:.6f}; frequency={frequency:.3f} Hz; duration={duration:.3f}s",
        flush=True,
    )
    if args.iterations:
        print(
            "Sending numeric target/candidate descriptors and patches only to "
            + args.base_url,
            flush=True,
        )
    with (output / "history.jsonl").open("w", encoding="utf-8") as history:
        history.write(json.dumps({"iteration": 0, **best}, allow_nan=False) + "\n")
        history.flush()
        for iteration in range(1, args.iterations + 1):
            event = {"iteration": iteration}
            try:
                proposal = propose(
                    url,
                    key,
                    args.model,
                    system,
                    {
                        "target": description,
                        "frequency": frequency,
                        "duration": duration,
                        "fixed_gate": args.gate,
                        "best": best,
                        "recent": recent,
                    },
                    args.timeout,
                )
                event["proposal"] = proposal
                candidate_gate = proposal["gate"]
                if (
                    isinstance(candidate_gate, bool)
                    or not isinstance(candidate_gate, (int, float))
                    or not math.isfinite(candidate_gate)
                    or not 0 < candidate_gate <= duration
                ):
                    raise ValueError("gate must be finite, >0 and <= duration")
                if args.gate is not None and candidate_gate != args.gate:
                    raise ValueError("candidate changed fixed --gate")
                candidate = {
                    **request,
                    "patch": proposal["patch"],
                    "gate": candidate_gate,
                }
                wav_path = output / f"candidate-{iteration:03d}.wav"
                audio = run_renderer(
                    renderer,
                    candidate,
                    output / f"candidate-{iteration:03d}.json",
                    wav_path,
                )
                score = compare(target, audio, RATE)
                event.update(score=score, analysis=analyze(audio, RATE))
                successes += 1
                if score["total"] < best["score"]["total"]:
                    best = {
                        "request": candidate,
                        "score": score,
                        "analysis": event["analysis"],
                    }
                    dump(output / "best.json", candidate)
                    shutil.copyfile(wav_path, output / "best.wav")
                print(
                    f"iteration {iteration}: loss={score['total']:.6f}, best={best['score']['total']:.6f}",
                    flush=True,
                )
            except (ValueError, OSError, subprocess.SubprocessError) as exc:
                event["error"] = str(exc)[:2000]
                print(
                    f"iteration {iteration}: {event['error']}",
                    file=sys.stderr,
                    flush=True,
                )
            history.write(json.dumps(event, ensure_ascii=False, allow_nan=False) + "\n")
            history.flush()
            feedback_event = dict(event)
            if "error" in feedback_event:
                # Local OS/subprocess exceptions may include private filesystem paths.
                feedback_event["error"] = (
                    "Proposal could not be evaluated. Check JSON, parameter ranges, array dimensions and gate; local history contains details."
                )
            recent = (recent + [feedback_event])[-3:]
    dump(
        output / "result.json",
        {
            "best_score": best["score"],
            "baseline_score": best_score,
            "evaluated_proposals": successes,
            "requests": args.iterations,
            "improved": best["score"]["total"] < best_score["total"],
            "best_request": "best.json",
            "best_audio": "best.wav",
        },
    )
    print(f"Saved {output / 'best.wav'} and best.json", flush=True)
    if args.iterations and not successes:
        raise ValueError(
            "no LLM proposal was evaluated successfully; baseline retained; inspect history.jsonl"
        )


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        print(f"error: {error}", file=sys.stderr)
        sys.exit(1)
