#!/usr/bin/env python3
"""Arithmetic for the tuning loop, so the agent never does it by hand (standard library only).

  decide.py baseline SCORE_A SCORE_B [-o tuning/baseline.json]
  decide.py judge SCORE [--baseline tuning/baseline.json] [--best tuning/best.json]
  decide.py misses RUN.jsonl [--limit N]

SCORE_* is the JSON printed by `decisionbench score`. `judge` exits 0 on keep, 10 on discard,
11 on rerun (the run is not usable), 1 on bad input. `baseline` exits 0, or 12 when fewer than
two models are eligible. See tuning/program.md.
"""
import argparse
import json
import sys

MIN_LIFT = 0.02  # a model must beat the best fixed answer by this much to be eligible
MIN_DELTA = 0.01  # smallest objective gain that counts as a gain
MIN_COVERAGE = 0.90  # share of Trials that must have a valid reply
EPS = 1e-9  # lifts are floats: a gain of exactly delta must not fail on rounding
LEVELS = ["low", "medium", "high", "xhigh", "max"]


def load(path):
    try:
        with open(path) as f:
            return json.load(f)
    except ValueError as e:
        sys.exit(f"decide.py: {path} is not JSON ({e}); did `run` or `score` fail?")


def fail(message):
    sys.exit(f"decide.py: {message}")


def by_model(score):
    """{candidate: pooled stats} for a score of a run with one instruction set."""
    rows = score["scores"]
    if len({r["instructions_sha256"] for r in rows}) != 1:
        fail("the run must use exactly one instructions file")
    out = {r["candidate"]: r["pooled"] for r in rows}
    if len(out) != len(rows):
        fail("a candidate appears twice in the score")
    return out


def usable(score):
    """Why a score cannot be used, or None."""
    if score["state"] != "complete":
        return f"run state is {score['state']}"
    if score["recorded_trials"] != score["planned_trials"]:
        return "not every planned Trial was recorded"
    return None


def size(pooled):
    """Predictions the model was asked for, answered or not: the selection a score covers."""
    return sum(pooled[k] for k in ("hit", "under", "over", "invalid", "failed"))


def baseline(a, b):
    for name, s in (("A", a), ("B", b)):
        if (why := usable(s)):
            fail(f"score {name}: {why}")
    ma, mb = by_model(a), by_model(b)
    if a["dataset_sha256"] != b["dataset_sha256"] or set(ma) != set(mb) or any(size(ma[n]) != size(mb[n]) for n in ma):
        fail("the two runs must use the same dataset, candidates and selection")
    models = {}
    for name in sorted(ma):
        la, lb = ma[name]["lift"], mb[name]["lift"]
        cover = min(ma[name]["coverage"], mb[name]["coverage"])
        lift, noise = (la + lb) / 2, abs(la - lb)
        models[name] = {
            "predictions": size(ma[name]),
            "lift": lift,
            "noise": noise,
            "coverage": cover,
            "eligible": cover >= MIN_COVERAGE and lift > max(2 * noise, MIN_LIFT),
        }
    eligible = [m for m in models.values() if m["eligible"]]
    noise = sum(m["noise"] for m in eligible) / len(eligible) if eligible else 0.0
    return {
        "dataset_sha256": a["dataset_sha256"],
        "delta": max(MIN_DELTA, 2 * noise),
        "objective": sum(m["lift"] for m in eligible) / len(eligible) if eligible else None,
        "models": models,
    }


def judge(score, base, best):
    """A verdict for one experiment against the best kept state."""
    eligible = sorted(n for n, m in base["models"].items() if m["eligible"])
    if score["dataset_sha256"] != base["dataset_sha256"]:
        fail("the dataset differs from the baseline's")
    if (why := usable(score)):
        return {"verdict": "rerun", "reason": why}
    got = by_model(score)
    if (missing := [n for n in eligible if n not in got]):
        fail(f"eligible models missing from the score: {missing}")
    if (other := {n: size(got[n]) for n in eligible if size(got[n]) != base["models"][n]["predictions"]}):
        fail(f"a different selection than the baseline's (predictions per model: {other})")
    low = {n: got[n]["coverage"] for n in eligible if got[n]["coverage"] < MIN_COVERAGE}
    if low:
        return {"verdict": "rerun", "reason": f"coverage under {MIN_COVERAGE}: {low}"}
    lifts = {n: got[n]["lift"] for n in eligible}
    objective = sum(lifts.values()) / len(lifts)
    prev = best or {"objective": base["objective"], "lifts": {n: base["models"][n]["lift"] for n in eligible}}
    gain = objective - prev["objective"]
    drops = {
        n: lifts[n] - prev["lifts"][n]
        for n in eligible
        if lifts[n] - prev["lifts"][n] < -max(2 * base["models"][n]["noise"], MIN_DELTA) - EPS
    }
    keep = gain >= base["delta"] - EPS and not drops
    return {
        "verdict": "keep" if keep else "discard",
        "objective": objective,
        "previous_objective": prev["objective"],
        "gain": gain,
        "needed": base["delta"],
        "lifts": lifts,
        "worse_models": drops,
    }


LEVEL_INDEX = {name: i for i, name in enumerate(LEVELS)}


def misses(path, limit):
    """Missed train Trials from an evidence file, for finding what to change in the prompt."""
    rows, counts = [], {}
    with open(path) as f:
        for line in f:
            t = json.loads(line)
            if t.get("type") != "trial" or not t["attempts"] or t["attempts"][-1]["disposition"] != "answered":
                continue
            try:
                answers = json.loads(t["attempts"][-1]["raw_response"])["answers"]
            except (ValueError, KeyError, TypeError):
                continue
            for target in t["targets"]:
                label = t["labels"].get(target)
                choice = (answers.get(f"effort_{target}") or {}).get("choice")
                if label is None:
                    continue
                lo, _, hi = label.partition("-")
                if choice not in LEVEL_INDEX:
                    kind = "invalid"
                elif LEVEL_INDEX[choice] < LEVEL_INDEX[lo]:
                    kind = "under"
                elif LEVEL_INDEX[choice] > LEVEL_INDEX[hi or lo]:
                    kind = "over"
                else:
                    continue
                counts[(target, kind)] = counts.get((target, kind), 0) + 1
                rows.append({
                    "case": t["case_id"], "target": target, "label": label, "predicted": choice, "kind": kind,
                    "message": t["request"]["state"]["next_message"][:300],
                    "recent_conversation": (t["request"]["state"].get("recent_conversation") or "")[-300:],
                })
    return {"counts": {f"{k[0]} {k[1]}": v for k, v in sorted(counts.items())}, "misses": rows[:limit]}


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="cmd", required=True)
    b = sub.add_parser("baseline")
    b.add_argument("score_a")
    b.add_argument("score_b")
    b.add_argument("-o", "--out", default="tuning/baseline.json")
    j = sub.add_parser("judge")
    j.add_argument("score")
    j.add_argument("--baseline", default="tuning/baseline.json")
    j.add_argument("--best", default="tuning/best.json")
    m = sub.add_parser("misses")
    m.add_argument("run")
    m.add_argument("--limit", type=int, default=40)
    args = p.parse_args()

    if args.cmd == "baseline":
        result = baseline(load(args.score_a), load(args.score_b))
        with open(args.out, "w") as f:
            json.dump(result, f, indent=2)
            f.write("\n")
        print(json.dumps(result, indent=2))
        sys.exit(0 if sum(m["eligible"] for m in result["models"].values()) >= 2 else 12)
    if args.cmd == "judge":
        base = load(args.baseline)
        try:
            best = load(args.best)
        except FileNotFoundError:
            best = None
        result = judge(load(args.score), base, best)
        if result["verdict"] == "keep":
            with open(args.best, "w") as f:
                json.dump({"objective": result["objective"], "lifts": result["lifts"]}, f, indent=2)
                f.write("\n")
        print(json.dumps(result, indent=2))
        sys.exit({"keep": 0, "discard": 10, "rerun": 11}[result["verdict"]])
    print(json.dumps(misses(args.run, args.limit), indent=2))


if __name__ == "__main__":
    main()
