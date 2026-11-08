#!/usr/bin/env python3
"""Arithmetic for the tuning loop, so the agent never does it by hand (standard library only).

  decide.py baseline SCORE_A SCORE_B --runs RUN_A RUN_B [-o tuning/baseline.json]
  decide.py judge SCORE --run RUN --commit HASH [--baseline tuning/baseline.json] [--best tuning/best.json]
  decide.py misses RUN.jsonl [--limit N]
  decide.py spent DIR --budget N [--baseline tuning/baseline.json]

SCORE_* is the JSON printed by `decisionbench score`; RUN_* is the evidence file it scored. `score` does not say
which Cases it covered, so the evidence manifest is read to make sure that runs compare the same Cases. `judge` exits 0 on keep, 10 on discard,
11 on rerun (the run is not usable), 1 on bad input. `baseline` exits 0, or 12 when fewer than
two models are eligible. `spent` exits 13 when what is left of the budget cannot cover the most the
next experiment can send. See tuning/program.md.
"""
import argparse
import glob
import hashlib
import json
import os
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


def save(path, data):
    """Write JSON through a temp file and a rename, so a crash never leaves a half-written file."""
    tmp = path + ".tmp"
    with open(tmp, "w") as f:
        json.dump(data, f, indent=2)
        f.write("\n")
    os.replace(tmp, path)


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


def selection(path, score):
    """The Cases, targets and repetitions a run planned, read from its evidence manifest."""
    with open(path) as f:
        manifest = json.loads(f.readline())
    if manifest.get("type") != "run":
        fail(f"{path} does not start with a run manifest")
    if manifest["id"] != score["run"]:
        fail(f"{path} is run {manifest['id']}, but the score is of run {score['run']}")
    return {
        "cases": sorted(manifest["cases"]),
        "targets": sorted(manifest["targets"]),
        "repetitions": manifest["settings"]["repetitions"],
    }


def retries(path):
    """The retries a run allowed per Trial, from its evidence manifest; `spent` sizes the next run with it."""
    with open(path) as f:
        return json.loads(f.readline())["settings"]["retries"]


def baseline(a, b, sel_a, sel_b):
    for name, s in (("A", a), ("B", b)):
        if (why := usable(s)):
            fail(f"score {name}: {why}")
    ma, mb = by_model(a), by_model(b)
    ia, ib = {r["instructions_sha256"] for r in a["scores"]}, {r["instructions_sha256"] for r in b["scores"]}
    if ia != ib:
        fail("the two calibration runs must use the same instructions")
    if sel_a != sel_b:
        fail("the two calibration runs must select the same Cases, targets and repetitions")
    if a["dataset_sha256"] != b["dataset_sha256"] or set(ma) != set(mb) or any(size(ma[n]) != size(mb[n]) for n in ma):
        fail("the two runs must use the same dataset, candidates and selection")
    models = {}
    for name in sorted(ma):
        la, lb = ma[name]["lift"], mb[name]["lift"]
        cover = min(ma[name]["coverage"] or 0.0, mb[name]["coverage"] or 0.0)
        if la is None or lb is None:  # no answered row at all: nothing to tune against
            lift, noise, ok = 0.0, 0.0, False
        else:
            lift, noise = (la + lb) / 2, abs(la - lb)
            ok = cover >= MIN_COVERAGE and lift > max(2 * noise, MIN_LIFT)
        models[name] = {"predictions": size(ma[name]), "lift": lift, "noise": noise, "coverage": cover, "eligible": ok}
    result = {"dataset_sha256": a["dataset_sha256"], "selection": sel_a, "models": models}
    result.update(derived(result))
    return result


def derived(base):
    """Objective, delta and the eligible set, always recomputed from the per-model fields, so that
    hand-editing a model's `eligible` flag in baseline.json cannot leave them stale."""
    models = base["models"]
    eligible = sorted(n for n, m in models.items() if m["eligible"])
    noise = sum(models[n]["noise"] for n in eligible) / len(eligible) if eligible else 0.0
    return {
        "eligible_models": eligible,
        "delta": max(MIN_DELTA, 2 * noise),
        "objective": sum(models[n]["lift"] for n in eligible) / len(eligible) if eligible else None,
    }


def fingerprint(base):
    """Identifies the baseline a best.json belongs to."""
    basis = {"d": base["dataset_sha256"], "s": base["selection"], "m": {n: [m["eligible"], m["lift"], m["noise"]] for n, m in sorted(base["models"].items())}}
    return hashlib.sha256(json.dumps(basis, sort_keys=True).encode()).hexdigest()


# Keys of best.json that are not part of the verdict.
PROVENANCE = ("baseline", "commit", "run", "evidence")


def judge(score, base, best, sel, commit=None):
    """A verdict for one experiment against the best kept state."""
    d = derived(base)
    eligible = d["eligible_models"]
    if len(eligible) < 2:
        fail("fewer than two eligible models in the baseline")
    if score["dataset_sha256"] != base["dataset_sha256"]:
        fail("the dataset differs from the baseline's")
    if sel != base["selection"]:
        fail("the run selects other Cases, targets or repetitions than the baseline's (is it a train run with the same flags?)")
    if best is not None and best.get("baseline") != fingerprint(base):
        fail("best.json belongs to another baseline; delete it only if you are starting over")
    if best is not None and best.get("run") == score["run"]:
        # This run was kept already (the session stopped before logging it): report that keep again,
        # rather than judge it against its own score.
        if best.get("commit") != commit:
            fail(f"run {score['run']} was kept for commit {best.get('commit')}, not {commit}")
        return {k: v for k, v in best.items() if k not in PROVENANCE}
    if (why := usable(score)):
        return {"verdict": "rerun", "reason": why}
    got = by_model(score)
    if (missing := [n for n in eligible if n not in got]):
        fail(f"eligible models missing from the score: {missing}")
    if (other := {n: size(got[n]) for n in eligible if size(got[n]) != base["models"][n]["predictions"]}):
        fail(f"a different selection than the baseline's (predictions per model: {other})")
    low = {n: got[n]["coverage"] for n in eligible if (got[n]["coverage"] or 0.0) < MIN_COVERAGE}
    if low:
        return {"verdict": "rerun", "reason": f"coverage under {MIN_COVERAGE}: {low}"}
    lifts = {n: got[n]["lift"] for n in eligible}
    if any(v is None for v in lifts.values()):
        return {"verdict": "rerun", "reason": "a model has no answered rows"}
    objective = sum(lifts.values()) / len(lifts)
    prev = best or {"objective": d["objective"], "lifts": {n: base["models"][n]["lift"] for n in eligible}}
    gain = objective - prev["objective"]
    drops = {
        n: lifts[n] - prev["lifts"][n]
        for n in eligible
        if lifts[n] - prev["lifts"][n] < -max(2 * base["models"][n]["noise"], MIN_DELTA) - EPS
    }
    keep = gain >= d["delta"] - EPS and not drops
    return {
        "verdict": "keep" if keep else "discard",
        "objective": objective,
        "previous_objective": prev["objective"],
        "gain": gain,
        "needed": d["delta"],
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
            # `score` counts an answered reply that cannot be read as invalid for every labelled target.
            try:
                answers = json.loads(t["attempts"][-1]["raw_response"])["answers"]
            except (ValueError, KeyError, TypeError):
                answers = {}
            if not isinstance(answers, dict):
                answers = {}
            for target in t["targets"]:
                label = t["labels"].get(target)
                entry = answers.get(f"effort_{target}")
                choice = entry.get("choice") if isinstance(entry, dict) else None
                if label is None:
                    continue
                lo, _, hi = label.partition("-")
                if not isinstance(choice, str) or choice not in LEVEL_INDEX:
                    kind = "invalid"
                elif LEVEL_INDEX[choice] < LEVEL_INDEX[lo]:
                    kind = "under"
                elif LEVEL_INDEX[choice] > LEVEL_INDEX[hi or lo]:
                    kind = "over"
                else:
                    continue
                counts[(target, kind)] = counts.get((target, kind), 0) + 1
                rows.append({
                    "candidate": t["candidate"], "case": t["case_id"], "target": target, "label": label,
                    "predicted": choice if isinstance(choice, str) else None, "kind": kind,
                    "message": t["request"]["state"]["next_message"][:300],
                    "recent_conversation": (t["request"]["state"].get("recent_conversation") or "")[-300:],
                })
    # Trials are written as they finish, so the first rows favour fast models: sample across the whole file.
    step = max(1, -(-len(rows) // limit)) if limit > 0 else 1
    return {"counts": {f"{k[0]} {k[1]}": v for k, v in sorted(counts.items())}, "misses": rows[::step][:limit]}


def spent(directory, budget, base):
    """Requests sent so far (every Attempt in every evidence file in `directory`), and the most the next
    experiment can send: one request per eligible model, Case and repetition, each retried up to the
    calibration's `retries`, as `run` counts its "up to N requests"."""
    if not os.path.isdir(directory):
        fail(f"{directory} is not a folder")
    runs = []
    for path in sorted(glob.glob(os.path.join(directory, "*.jsonl"))):
        attempts = 0
        with open(path) as f:
            for line in f:
                try:
                    t = json.loads(line)
                except ValueError:  # a line cut short by a stop: its Trial is lost, as are Trials in flight
                    continue
                if t.get("type") == "trial":
                    attempts += len(t["attempts"])
        runs.append(attempts)
    total = sum(runs)
    sel = base["selection"]
    next_run = len(derived(base)["eligible_models"]) * len(sel["cases"]) * sel["repetitions"] * (1 + base["retries"])
    return {"requests": total, "budget": budget, "remaining": budget - total, "next_run": next_run}


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="cmd", required=True)
    b = sub.add_parser("baseline")
    b.add_argument("score_a")
    b.add_argument("score_b")
    b.add_argument("--runs", nargs=2, required=True, metavar=("RUN_A", "RUN_B"))
    b.add_argument("-o", "--out", default="tuning/baseline.json")
    j = sub.add_parser("judge")
    j.add_argument("score")
    j.add_argument("--run", required=True)
    j.add_argument("--commit", required=True, help="the commit judged: $(git rev-parse HEAD)")
    j.add_argument("--baseline", default="tuning/baseline.json")
    j.add_argument("--best", default="tuning/best.json")
    m = sub.add_parser("misses")
    m.add_argument("run")
    m.add_argument("--limit", type=int, default=40)
    s = sub.add_parser("spent")
    s.add_argument("dir")
    s.add_argument("--budget", type=int, required=True)
    s.add_argument("--baseline", default="tuning/baseline.json")
    args = p.parse_args()
    try:
        command(args)
    except OSError as e:
        fail(f"{e.filename or e}: {e.strerror or e}")
    except (KeyError, TypeError, IndexError, ValueError, AttributeError) as e:
        fail(f"unexpected input ({type(e).__name__}: {e}); pass a `decisionbench score` result and the evidence file it scored")


def command(args):
    if args.cmd == "baseline":
        a, b = load(args.score_a), load(args.score_b)
        result = baseline(a, b, selection(args.runs[0], a), selection(args.runs[1], b))
        result["retries"] = retries(args.runs[0])
        save(args.out, result)
        print(json.dumps(result, indent=2))
        sys.exit(0 if len(result["eligible_models"]) >= 2 else 12)
    if args.cmd == "judge":
        base = load(args.baseline)
        try:
            best = load(args.best)
        except FileNotFoundError:
            best = None
        score = load(args.score)
        result = judge(score, base, best, selection(args.run, score), args.commit)
        if result["verdict"] == "keep":
            save(args.best, {**result, "baseline": fingerprint(base), "commit": args.commit,
                             "run": score["run"], "evidence": os.path.abspath(args.run)})
        print(json.dumps(result, indent=2))
        sys.exit({"keep": 0, "discard": 10, "rerun": 11}[result["verdict"]])
    if args.cmd == "spent":
        if not os.path.exists(args.baseline):
            fail(f"no {args.baseline}: calibrate first (Setup checks the calibration cost with the user)")
        result = spent(args.dir, args.budget, load(args.baseline))
        print(json.dumps(result))
        sys.exit(13 if result["remaining"] < result["next_run"] else 0)
    print(json.dumps(misses(args.run, args.limit), indent=2))


if __name__ == "__main__":
    main()
