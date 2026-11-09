"""Run with: python3 -m unittest discover -s tuning"""
import json
import os
import subprocess
import sys
import tempfile
import unittest

import decide

HERE = os.path.dirname(os.path.abspath(__file__))


def score(lifts, coverage=1.0, planned=100, recorded=None, state="complete", sha="d1", predictions=400):
    """A `decisionbench score` result with one instruction set and the given lift per candidate."""
    return {
        "run": "r1",
        "state": state,
        "planned_trials": planned,
        "recorded_trials": planned if recorded is None else recorded,
        "dataset_sha256": sha,
        "scores": [
            {"candidate": n, "instructions": "prompt", "instructions_sha256": "h",
             "pooled": {"lift": lift, "coverage": coverage, "hit": predictions, "under": 0, "over": 0, "invalid": 0, "failed": 0}}
            for n, lift in lifts.items()
        ],
    }


SEL = {"cases": ["c1", "c2"], "targets": ["opus"], "repetitions": 1, "retries": 3}


class Baseline(unittest.TestCase):
    def test_noise_is_the_difference_between_runs_and_gates_eligibility(self):
        a = score({"a": 0.10, "b": 0.10, "c": 0.03, "d": 0.00})
        b = score({"a": 0.10, "b": 0.02, "c": 0.03, "d": 0.00})
        base = decide.baseline(a, b, SEL, SEL)
        m = base["models"]
        self.assertAlmostEqual(m["a"]["lift"], 0.10)
        self.assertTrue(m["a"]["eligible"])
        self.assertAlmostEqual(m["b"]["noise"], 0.08)
        self.assertFalse(m["b"]["eligible"], "lift 0.06 is under twice its noise 0.16")
        self.assertTrue(m["c"]["eligible"])
        self.assertFalse(m["d"]["eligible"], "no lift over the fixed answer")
        self.assertAlmostEqual(base["objective"], (0.10 + 0.03) / 2)

    def test_delta_is_half_a_point_or_twice_the_mean_noise_of_eligible_models(self):
        quiet = decide.baseline(score({"a": 0.10, "b": 0.10}), score({"a": 0.10, "b": 0.10}), SEL, SEL)
        self.assertAlmostEqual(quiet["delta"], 0.005)
        noisy = decide.baseline(score({"a": 0.20, "b": 0.20}), score({"a": 0.16, "b": 0.14}), SEL, SEL)
        self.assertAlmostEqual(noisy["delta"], 2 * (0.04 + 0.06) / 2)

    def test_low_coverage_in_calibration_makes_a_model_ineligible(self):
        a = score({"a": 0.10, "b": 0.10})
        b = score({"a": 0.10, "b": 0.10})
        b["scores"][1]["pooled"]["coverage"] = 0.89
        self.assertFalse(decide.baseline(a, b, SEL, SEL)["models"]["b"]["eligible"])

    def test_noise_is_the_spread_of_repeated_answers_when_that_is_larger(self):
        same = score({"a": 0.10, "b": 0.10})
        base_ = decide.baseline(same, same, SEL, SEL, {"a": 0.03})
        self.assertAlmostEqual(base_["models"]["a"]["noise"], 0.03, msg="equal totals, but answers changed")
        self.assertAlmostEqual(base_["models"]["b"]["noise"], 0.0)
        self.assertAlmostEqual(base_["delta"], 2 * 0.03 / 2)
        noisy = decide.baseline(score({"a": 0.20}), score({"a": 0.16}), SEL, SEL, {"a": 0.01})
        self.assertAlmostEqual(noisy["models"]["a"]["noise"], 0.04, msg="the observed difference is larger")

    def test_refuses_runs_that_cannot_be_paired(self):
        with self.assertRaises(SystemExit):
            decide.baseline(score({"a": 0.1}), score({"a": 0.1}, sha="other"), SEL, SEL)
        with self.assertRaises(SystemExit):
            decide.baseline(score({"a": 0.1}), score({"a": 0.1}, state="partial"), SEL, SEL)
        with self.assertRaises(SystemExit):
            decide.baseline(score({"a": 0.1}), score({"b": 0.1}), SEL, SEL)
        with self.assertRaises(SystemExit):
            decide.baseline(score({"a": 0.1}), score({"a": 0.1}, predictions=200), SEL, SEL)
        other = score({"a": 0.1})
        other["scores"][0]["instructions_sha256"] = "different"
        with self.assertRaises(SystemExit):
            decide.baseline(score({"a": 0.1}), other, SEL, SEL)


def base(lifts=None, noise=0.0, delta=None):
    """A baseline.json; `delta` is ignored: judge derives it from the noise, as the real file would."""
    lifts = lifts or {"a": 0.10, "b": 0.10, "x": 0.0}
    models = {n: {"predictions": 400, "lift": v, "noise": noise, "coverage": 1.0, "eligible": n != "x"}
              for n, v in lifts.items()}
    return {"dataset_sha256": "d1", "selection": SEL, "models": models}


def best_of(b, objective, lifts):
    return {"baseline": decide.fingerprint(b), "objective": objective, "lifts": lifts}


class Judge(unittest.TestCase):
    def test_keeps_a_gain_of_at_least_delta(self):
        r = decide.judge(score({"a": 0.13, "b": 0.12}), base(), None, SEL)
        self.assertEqual(r["verdict"], "keep")
        self.assertAlmostEqual(r["gain"], 0.025)

    def test_a_gain_of_exactly_delta_is_kept_and_less_is_not(self):
        self.assertEqual(decide.judge(score({"a": 0.105, "b": 0.105}), base(), None, SEL)["verdict"], "keep")
        self.assertEqual(decide.judge(score({"a": 0.104, "b": 0.104}), base(), None, SEL)["verdict"], "discard")

    def test_the_comparison_is_against_the_best_kept_state_not_the_baseline(self):
        b = base()
        best = best_of(b, 0.14, {"a": 0.14, "b": 0.14})
        r = decide.judge(score({"a": 0.13, "b": 0.12}), b, best, SEL)
        self.assertEqual(r["verdict"], "discard")

    def test_one_model_dropping_past_twice_its_noise_discards_despite_a_higher_mean(self):
        b = base(noise=0.01, delta=0.02)
        r = decide.judge(score({"a": 0.25, "b": 0.07}), b, None, SEL)  # mean 0.16, but b fell by 0.03 > 0.02
        self.assertEqual(r["verdict"], "discard")
        self.assertIn("b", r["worse_models"])
        ok = decide.judge(score({"a": 0.25, "b": 0.081}), b, None, SEL)  # b fell by 0.019, within 0.02
        self.assertEqual(ok["verdict"], "keep")

    def test_the_drop_guard_compares_each_model_with_the_kept_best_not_the_baseline(self):
        b = base(noise=0.01)  # baseline lifts 0.10; guard and delta 0.02
        best = best_of(b, 0.14, {"a": 0.14, "b": 0.14})
        # Mean 0.1825 gains 0.0425 on the best, and b is up 0.015 on the baseline, but down 0.025 on the best.
        r = decide.judge(score({"a": 0.25, "b": 0.115}), b, best, SEL)
        self.assertEqual(r["verdict"], "discard")
        self.assertIn("b", r["worse_models"])

    def test_the_drop_guard_has_a_one_point_floor_when_noise_is_zero(self):
        self.assertEqual(decide.judge(score({"a": 0.30, "b": 0.095}), base(), None, SEL)["verdict"], "keep")
        self.assertEqual(decide.judge(score({"a": 0.30, "b": 0.085}), base(), None, SEL)["verdict"], "discard")

    def test_ineligible_models_do_not_count_even_if_they_are_in_the_run(self):
        r = decide.judge(score({"a": 0.13, "b": 0.13, "x": -0.5}), base(), None, SEL)
        self.assertEqual(r["verdict"], "keep")
        self.assertNotIn("x", r["lifts"])

    def test_unusable_runs_are_rerun_not_scored(self):
        for bad in (
            score({"a": 0.5, "b": 0.5}, state="partial"),
            score({"a": 0.5, "b": 0.5}, recorded=99),
            score({"a": 0.5, "b": 0.5}, coverage=0.89),
        ):
            self.assertEqual(decide.judge(bad, base(), None, SEL)["verdict"], "rerun")

    def test_flipping_an_eligible_flag_changes_what_is_compared(self):
        b = base({"a": 0.10, "b": 0.20, "c": 0.10})
        b["models"]["c"]["eligible"] = True
        b["models"]["b"]["eligible"] = False  # the user dropped b: mean of a and c is 0.10, not 0.133
        r = decide.judge(score({"a": 0.104, "c": 0.104}), b, None, SEL)
        self.assertEqual(r["verdict"], "discard")
        self.assertAlmostEqual(r["previous_objective"], 0.10)
        self.assertEqual(decide.judge(score({"a": 0.12, "c": 0.12}), b, None, SEL)["verdict"], "keep")

    def test_a_best_file_from_another_baseline_is_refused(self):
        old = base({"a": 0.10, "b": 0.10, "x": 0.0})
        stale = best_of(old, 0.5, {"a": 0.5, "b": 0.5})
        edited = base({"a": 0.10, "b": 0.10, "x": 0.0})
        edited["models"]["b"]["eligible"] = False
        edited["models"]["x"]["eligible"] = True
        with self.assertRaises(SystemExit):
            decide.judge(score({"a": 0.2, "x": 0.2}), edited, stale, SEL)

    def test_a_model_that_never_answered_is_not_a_crash(self):
        failed = score({"a": 0.1, "b": 0.1})
        failed["scores"][1]["pooled"].update({"lift": None, "coverage": 0.0})
        base_ok = decide.baseline(score({"a": 0.1, "b": 0.1}), failed, SEL, SEL)
        self.assertFalse(base_ok["models"]["b"]["eligible"])
        mid_loop = score({"a": 0.5, "b": 0.5})
        mid_loop["scores"][1]["pooled"].update({"lift": None, "coverage": None})
        self.assertEqual(decide.judge(mid_loop, base(), None, SEL)["verdict"], "rerun")

    def test_fewer_than_two_eligible_models_is_an_error(self):
        with self.assertRaises(SystemExit):
            decide.judge(score({"a": 0.5}), base({"a": 0.1, "x": 0.0}), None, SEL)

    def test_a_run_with_only_the_eligible_models_is_comparable_to_the_baseline(self):
        r = decide.judge(score({"a": 0.13, "b": 0.13}, planned=200), base(), None, SEL)  # baseline had three models
        self.assertEqual(r["verdict"], "keep")

    def test_other_cases_with_the_same_count_are_an_error(self):
        with self.assertRaises(SystemExit):
            decide.judge(score({"a": 0.5, "b": 0.5}), base(), None, {**SEL, "cases": ["c1", "c9"]})

    def test_a_different_selection_than_the_baseline_is_an_error(self):
        with self.assertRaises(SystemExit):
            decide.judge(score({"a": 0.5, "b": 0.5}, predictions=100), base(), None, SEL)

    def test_a_missing_eligible_model_or_other_dataset_is_an_error(self):
        with self.assertRaises(SystemExit):
            decide.judge(score({"a": 0.5}), base(), None, SEL)
        with self.assertRaises(SystemExit):
            decide.judge(score({"a": 0.5, "b": 0.5}, sha="other"), base(), None, SEL)


class Cli(unittest.TestCase):
    def run_cli(self, *args):
        return subprocess.run([sys.executable, os.path.join(HERE, "decide.py"), *args],
                              capture_output=True, text=True)

    def test_exit_codes_and_best_file(self):
        with tempfile.TemporaryDirectory() as d:
            paths = {}
            for name, data in {
                "a": score({"a": 0.10, "b": 0.10}), "b": score({"a": 0.10, "b": 0.10}),
                "good": score({"a": 0.15, "b": 0.15}), "bad": score({"a": 0.10, "b": 0.10}),
                "weak": score({"a": 0.0, "b": 0.0}),
            }.items():
                paths[name] = os.path.join(d, name + ".json")
                with open(paths[name], "w") as f:
                    json.dump(data, f)
            run = write_run(d, "run.jsonl")
            baseline, best = os.path.join(d, "baseline.json"), os.path.join(d, "best.json")
            self.assertEqual(self.run_cli("baseline", paths["a"], paths["b"], "--runs", run, run, "-o", baseline).returncode, 0)
            with open(baseline) as f:
                self.assertEqual(json.load(f)["selection"]["retries"], 3, "read from the calibration run's manifest")
            self.assertEqual(self.run_cli("baseline", paths["weak"], paths["weak"], "--runs", run, run, "-o", baseline + "2").returncode, 12)
            # The noise comes from the evidence too: model a changed its answer between the runs.
            changed = [write_run(d, f"changed{i}.jsonl", trials=[trial("c1", c, "a")]) for i, c in enumerate(["low", "high"])]
            r = self.run_cli("baseline", paths["a"], paths["b"], "--runs", *changed, "-o", baseline + "3")
            self.assertEqual(r.returncode, 12, "one changed answer out of one: a's noise now exceeds its lift")
            self.assertGreater(json.loads(r.stdout)["models"]["a"]["noise"], 0, "equal totals, changed answers")
            self.assertEqual(json.loads(r.stdout)["models"]["b"]["noise"], 0)
            judge = ["judge", "--baseline", baseline, "--best", best, "--commit", "c0ffee"]
            self.assertEqual(self.run_cli(*judge, "--run", run, paths["bad"]).returncode, 10)
            self.assertFalse(os.path.exists(best), "a discard must not write the best file")
            self.assertEqual(self.run_cli(*judge, "--run", run, paths["good"]).returncode, 0)
            with open(best) as f:
                kept = json.load(f)
            self.assertAlmostEqual(kept["objective"], 0.15)
            self.assertEqual((kept["commit"], kept["run"], kept["evidence"]), ("c0ffee", "r1", os.path.abspath(run)))
            # Judging the kept run again (a resume after a stop before logging) reports the same keep.
            again = self.run_cli(*judge, "--run", run, paths["good"])
            self.assertEqual(again.returncode, 0, again.stderr)
            self.assertAlmostEqual(json.loads(again.stdout)["gain"], 0.05)
            other_commit = ["judge", "--baseline", baseline, "--best", best, "--commit", "beef"]
            self.assertEqual(self.run_cli(*other_commit, "--run", run, paths["good"]).returncode, 1,
                             "the kept run belongs to another commit")
            # A new run of an equal prompt is no gain over the new best.
            second = os.path.join(d, "second.json")
            with open(second, "w") as f:
                json.dump({**score({"a": 0.15, "b": 0.15}), "run": "r2"}, f)
            self.assertEqual(self.run_cli(*judge, "--run", write_run(d, "r2.jsonl", run_id="r2"), second).returncode, 10)
            self.assertFalse([n for n in os.listdir(d) if n.endswith(".tmp")], "no temp file left behind")

    def test_bad_input_is_a_decide_error_not_a_traceback(self):
        with tempfile.TemporaryDirectory() as d:
            sc = os.path.join(d, "s.json")
            with open(sc, "w") as f:
                json.dump(score({"a": 0.10, "b": 0.10}), f)
            bare = os.path.join(d, "bare.jsonl")
            with open(bare, "w") as f:
                f.write('{"type": "run", "id": "r1"}\n')
            for args in (
                ["baseline", os.path.join(d, "missing.json"), sc, "--runs", bare, bare, "-o", os.path.join(d, "b.json")],
                ["baseline", sc, sc, "--runs", bare, bare, "-o", os.path.join(d, "b.json")],
                ["spent", os.path.join(d, "no-such-dir"), "--budget", "10"],
            ):
                r = self.run_cli(*args)
                self.assertEqual(r.returncode, 1, args)
                self.assertTrue(r.stderr.startswith("decide.py: "), r.stderr)
                self.assertNotIn("Traceback", r.stderr)

    def test_spent_adds_up_every_attempt_in_the_folder_and_checks_the_budget(self):
        with tempfile.TemporaryDirectory() as d:
            def trial(n):
                return json.dumps({"type": "trial", "attempts": [{}] * n})
            with open(os.path.join(d, "1-1.jsonl"), "w") as f:  # calibration: 5 Trials, one retried twice
                f.write("\n".join(['{"type": "run"}', *[trial(1)] * 4, trial(3), '{"type": "end"}']) + "\n")
            with open(os.path.join(d, "2-1.jsonl"), "w") as f:  # a probe; a stop cut its last line
                f.write('{"type": "run"}\n' + trial(1) + '\n{"type": "tri')
            missing = self.run_cli("spent", d, "--budget", "100", "--baseline", os.path.join(d, "none.json"))
            self.assertEqual(missing.returncode, 1, "no baseline yet: calibration is checked with the user")
            self.assertIn("calibrate first", missing.stderr)
            # The next experiment's ceiling: eligible models (a and b, not x) × Cases × repetitions × (1 + retries) = 2 × 2 × 1 × 4.
            baseline = os.path.join(d, "baseline.json")
            with open(baseline, "w") as f:
                json.dump(base(), f)
            ok = self.run_cli("spent", d, "--budget", "24", "--baseline", baseline)
            self.assertEqual(ok.returncode, 0, ok.stderr)
            self.assertEqual(json.loads(ok.stdout), {"requests": 8, "budget": 24, "remaining": 16, "next_run": 16})
            self.assertEqual(self.run_cli("spent", d, "--budget", "23", "--baseline", baseline).returncode, 13,
                             "15 left covers the 4 first attempts but not their retries")

    def test_other_cases_or_another_run_file_are_refused(self):
        with tempfile.TemporaryDirectory() as d:
            sc = os.path.join(d, "s.json")
            with open(sc, "w") as f:
                json.dump(score({"a": 0.10, "b": 0.10}), f)
            run = write_run(d, "run.jsonl")
            other_cases = write_run(d, "other.jsonl", cases=["c1", "c3"])
            wrong_run = write_run(d, "wrong.jsonl", run_id="r2")
            baseline = os.path.join(d, "baseline.json")
            args = ["baseline", sc, sc, "-o", baseline]
            self.assertEqual(self.run_cli(*args, "--runs", run, run).returncode, 0)
            self.assertEqual(self.run_cli(*args, "--runs", run, other_cases).returncode, 1, "same count, other Cases")
            self.assertEqual(self.run_cli(*args, "--runs", run, wrong_run).returncode, 1, "run file of another run")
            judge = ["judge", sc, "--baseline", baseline, "--best", os.path.join(d, "best.json"), "--commit", "c0ffee"]
            self.assertEqual(self.run_cli(*judge, "--run", other_cases).returncode, 1)
            self.assertEqual(self.run_cli(*judge, "--run", write_run(d, "reps.jsonl", repetitions=3)).returncode, 1)
            # Fewer retries in calibration than later would make `spent` size runs too small.
            fewer = write_run(d, "fewer.jsonl", retries=0)
            self.assertEqual(self.run_cli(*args, "--runs", run, fewer).returncode, 1, "calibration runs with other retries")
            self.assertEqual(self.run_cli(*judge, "--run", write_run(d, "more.jsonl", retries=5)).returncode, 1, "other retries than the baseline")
            self.assertEqual(self.run_cli(*judge).returncode, 2, "--run is required")


def write_run(directory, name, cases=("c1", "c2"), run_id="r1", repetitions=1, retries=3, trials=()):
    """An evidence file with the manifest line `decide.py` reads, and any Trial lines."""
    path = os.path.join(directory, name)
    with open(path, "w") as f:
        f.write(json.dumps({"type": "run", "id": run_id, "cases": list(cases), "targets": ["opus"],
                            "settings": {"repetitions": repetitions, "retries": retries}}) + "\n")
        for t in trials:
            f.write(json.dumps(t) + "\n")
    return path


def trial(case, choice, candidate="m", label="low", repetition=1):
    """An answered Trial on the opus target."""
    reply = {"answers": {"effort_opus": {"type": "choice", "choice": choice}}}
    return {"type": "trial", "candidate": candidate, "case_id": case, "targets": ["opus"], "labels": {"opus": label},
            "repetition": repetition, "attempts": [{"disposition": "answered", "raw_response": json.dumps(reply)}]}


class Spread(unittest.TestCase):
    def test_answers_that_change_but_cancel_out_still_count_as_noise(self):
        with tempfile.TemporaryDirectory() as d:
            # Both runs score 1 of 2, so their totals match; each Case's answer changed between them.
            a = write_run(d, "a.jsonl", trials=[trial("c1", "low"), trial("c2", "high"), trial("c1", "low", "fixed")])
            b = write_run(d, "b.jsonl", trials=[trial("c1", "high"), trial("c2", "low"), trial("c1", "low", "fixed")])
            s = decide.spread([a, b], 1)
            # Each Case has observations [1, 0]: sample variance 0.5; sqrt(2 * 1.0 / (1 * 2 ** 2)).
            self.assertAlmostEqual(s["m"], 0.5 ** 0.5)
            self.assertEqual(s["fixed"], 0.0, "a model that always gives the same answer has no noise")

    def test_more_repetitions_make_a_run_less_noisy(self):
        with tempfile.TemporaryDirectory() as d:
            ts = [trial("c1", c, repetition=r) for r, c in enumerate(["low", "high", "low"], 1)]
            once = decide.spread([write_run(d, "a.jsonl", trials=ts[:2])], 1)["m"]
            thrice = decide.spread([write_run(d, "b.jsonl", trials=ts * 2)], 3)["m"]
            self.assertLess(thrice, once)

    def test_failed_and_unlabelled_targets(self):
        with tempfile.TemporaryDirectory() as d:
            failed = {**trial("c1", "low"), "attempts": [{"disposition": "transport_error"}]}
            unlabelled = {**trial("c2", "low"), "labels": {}}
            s = decide.spread([write_run(d, "a.jsonl", trials=[trial("c1", "low"), failed, unlabelled])], 1)
            # One labelled prediction, observed as [hit, miss]: sqrt(2 * 0.5 / (1 * 1 ** 2)). Counting the
            # unlabelled target would make it two predictions; skipping the failure would make it no noise.
            self.assertAlmostEqual(s["m"], 1.0, msg="a failed Trial is a miss; an unlabelled target is not counted")


class Misses(unittest.TestCase):
    def trial(self, case, label, reply):
        return {"type": "trial", "candidate": "m", "case_id": case, "targets": ["opus"], "labels": {"opus": label},
                "request": {"state": {"next_message": f"message of {case}", "recent_conversation": "user: earlier"}},
                "attempts": [{"disposition": "answered", "raw_response": json.dumps(reply)}]}

    def test_classifies_under_over_invalid_and_skips_hits_and_failures(self):
        def answer(choice):
            return {"answers": {"effort_opus": {"type": "choice", "choice": choice}}}

        lines = [
            {"type": "run"},
            self.trial("c1", "medium-high", answer("high")),
            self.trial("c2", "medium-high", answer("low")),
            self.trial("c3", "low", answer("xhigh")),
            self.trial("c4", "low", answer("bogus")),
            self.trial("c6", "low", {"answers": {"effort_opus": {"choice": ["low"]}}}),
            self.trial("c7", "low", {"answers": "oops"}),
            {**self.trial("c8", "low", {}), "attempts": [{"disposition": "answered", "raw_response": "not json"}]},
            {**self.trial("c5", "low", answer("low")), "attempts": [{"disposition": "transport_error"}]},
        ]
        with tempfile.NamedTemporaryFile("w", suffix=".jsonl", delete=False) as f:
            f.write("\n".join(json.dumps(x) for x in lines))
        result = decide.misses(f.name, 10)
        os.unlink(f.name)
        self.assertEqual([(m["case"], m["kind"]) for m in result["misses"]],
                         [("c2", "under"), ("c3", "over"), ("c4", "invalid"),
                          ("c6", "invalid"), ("c7", "invalid"), ("c8", "invalid")])
        self.assertEqual(result["misses"][0]["recent_conversation"], "user: earlier")
        self.assertEqual(result["counts"], {"opus invalid": 4, "opus over": 1, "opus under": 1})


if __name__ == "__main__":
    unittest.main()
