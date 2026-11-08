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


class Baseline(unittest.TestCase):
    def test_noise_is_the_difference_between_runs_and_gates_eligibility(self):
        a = score({"a": 0.10, "b": 0.10, "c": 0.03, "d": 0.00})
        b = score({"a": 0.10, "b": 0.02, "c": 0.03, "d": 0.00})
        base = decide.baseline(a, b)
        m = base["models"]
        self.assertAlmostEqual(m["a"]["lift"], 0.10)
        self.assertTrue(m["a"]["eligible"])
        self.assertAlmostEqual(m["b"]["noise"], 0.08)
        self.assertFalse(m["b"]["eligible"], "lift 0.06 is under twice its noise 0.16")
        self.assertTrue(m["c"]["eligible"])
        self.assertFalse(m["d"]["eligible"], "no lift over the fixed answer")
        self.assertAlmostEqual(base["objective"], (0.10 + 0.03) / 2)

    def test_delta_is_one_point_or_twice_the_mean_noise_of_eligible_models(self):
        quiet = decide.baseline(score({"a": 0.10, "b": 0.10}), score({"a": 0.10, "b": 0.10}))
        self.assertAlmostEqual(quiet["delta"], 0.01)
        noisy = decide.baseline(score({"a": 0.20, "b": 0.20}), score({"a": 0.16, "b": 0.14}))
        self.assertAlmostEqual(noisy["delta"], 2 * (0.04 + 0.06) / 2)

    def test_low_coverage_in_calibration_makes_a_model_ineligible(self):
        a = score({"a": 0.10, "b": 0.10})
        b = score({"a": 0.10, "b": 0.10})
        b["scores"][1]["pooled"]["coverage"] = 0.89
        self.assertFalse(decide.baseline(a, b)["models"]["b"]["eligible"])

    def test_refuses_runs_that_cannot_be_paired(self):
        with self.assertRaises(SystemExit):
            decide.baseline(score({"a": 0.1}), score({"a": 0.1}, sha="other"))
        with self.assertRaises(SystemExit):
            decide.baseline(score({"a": 0.1}), score({"a": 0.1}, state="partial"))
        with self.assertRaises(SystemExit):
            decide.baseline(score({"a": 0.1}), score({"b": 0.1}))
        with self.assertRaises(SystemExit):
            decide.baseline(score({"a": 0.1}), score({"a": 0.1}, predictions=200))


def base(lifts=None, noise=0.0, delta=0.01):
    lifts = lifts or {"a": 0.10, "b": 0.10, "x": 0.0}
    models = {n: {"predictions": 400, "lift": v, "noise": noise, "coverage": 1.0, "eligible": n != "x"}
              for n, v in lifts.items()}
    eligible = [v for n, v in lifts.items() if n != "x"]
    return {"dataset_sha256": "d1", "planned_trials": 100, "delta": delta,
            "objective": sum(eligible) / len(eligible), "models": models}


class Judge(unittest.TestCase):
    def test_keeps_a_gain_of_at_least_delta(self):
        r = decide.judge(score({"a": 0.13, "b": 0.12}), base(), None)
        self.assertEqual(r["verdict"], "keep")
        self.assertAlmostEqual(r["gain"], 0.025)

    def test_a_gain_of_exactly_delta_is_kept_and_less_is_not(self):
        self.assertEqual(decide.judge(score({"a": 0.11, "b": 0.11}), base(), None)["verdict"], "keep")
        self.assertEqual(decide.judge(score({"a": 0.109, "b": 0.109}), base(), None)["verdict"], "discard")

    def test_the_comparison_is_against_the_best_kept_state_not_the_baseline(self):
        best = {"objective": 0.14, "lifts": {"a": 0.14, "b": 0.14}}
        r = decide.judge(score({"a": 0.13, "b": 0.12}), base(), best)
        self.assertEqual(r["verdict"], "discard")

    def test_one_model_dropping_past_twice_its_noise_discards_despite_a_higher_mean(self):
        b = base(noise=0.01, delta=0.02)
        r = decide.judge(score({"a": 0.25, "b": 0.07}), b, None)  # mean 0.16, but b fell by 0.03 > 0.02
        self.assertEqual(r["verdict"], "discard")
        self.assertIn("b", r["worse_models"])
        ok = decide.judge(score({"a": 0.25, "b": 0.081}), b, None)  # b fell by 0.019, within 0.02
        self.assertEqual(ok["verdict"], "keep")

    def test_the_drop_guard_has_a_one_point_floor_when_noise_is_zero(self):
        self.assertEqual(decide.judge(score({"a": 0.30, "b": 0.095}), base(), None)["verdict"], "keep")
        self.assertEqual(decide.judge(score({"a": 0.30, "b": 0.085}), base(), None)["verdict"], "discard")

    def test_ineligible_models_do_not_count_even_if_they_are_in_the_run(self):
        r = decide.judge(score({"a": 0.13, "b": 0.13, "x": -0.5}), base(), None)
        self.assertEqual(r["verdict"], "keep")
        self.assertNotIn("x", r["lifts"])

    def test_unusable_runs_are_rerun_not_scored(self):
        for bad in (
            score({"a": 0.5, "b": 0.5}, state="partial"),
            score({"a": 0.5, "b": 0.5}, recorded=99),
            score({"a": 0.5, "b": 0.5}, coverage=0.89),
        ):
            self.assertEqual(decide.judge(bad, base(), None)["verdict"], "rerun")

    def test_a_run_with_only_the_eligible_models_is_comparable_to_the_baseline(self):
        r = decide.judge(score({"a": 0.13, "b": 0.13}, planned=200), base(), None)  # baseline had three models
        self.assertEqual(r["verdict"], "keep")

    def test_a_different_selection_than_the_baseline_is_an_error(self):
        with self.assertRaises(SystemExit):
            decide.judge(score({"a": 0.5, "b": 0.5}, predictions=100), base(), None)

    def test_a_missing_eligible_model_or_other_dataset_is_an_error(self):
        with self.assertRaises(SystemExit):
            decide.judge(score({"a": 0.5}), base(), None)
        with self.assertRaises(SystemExit):
            decide.judge(score({"a": 0.5, "b": 0.5}, sha="other"), base(), None)


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
            baseline, best = os.path.join(d, "baseline.json"), os.path.join(d, "best.json")
            self.assertEqual(self.run_cli("baseline", paths["a"], paths["b"], "-o", baseline).returncode, 0)
            self.assertEqual(self.run_cli("baseline", paths["weak"], paths["weak"], "-o", baseline + "2").returncode, 12)
            judge = ["judge", "--baseline", baseline, "--best", best]
            self.assertEqual(self.run_cli(*judge, paths["bad"]).returncode, 10)
            self.assertFalse(os.path.exists(best), "a discard must not write the best file")
            self.assertEqual(self.run_cli(*judge, paths["good"]).returncode, 0)
            with open(best) as f:
                self.assertAlmostEqual(json.load(f)["objective"], 0.15)
            self.assertEqual(self.run_cli(*judge, paths["good"]).returncode, 10, "no further gain over the new best")


class Misses(unittest.TestCase):
    def trial(self, case, label, reply):
        return {"type": "trial", "case_id": case, "targets": ["opus"], "labels": {"opus": label},
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
            {**self.trial("c5", "low", answer("low")), "attempts": [{"disposition": "transport_error"}]},
        ]
        with tempfile.NamedTemporaryFile("w", suffix=".jsonl", delete=False) as f:
            f.write("\n".join(json.dumps(x) for x in lines))
        result = decide.misses(f.name, 10)
        os.unlink(f.name)
        self.assertEqual([(m["case"], m["kind"]) for m in result["misses"]],
                         [("c2", "under"), ("c3", "over"), ("c4", "invalid")])
        self.assertEqual(result["misses"][0]["recent_conversation"], "user: earlier")
        self.assertEqual(result["counts"], {"opus invalid": 1, "opus over": 1, "opus under": 1})


if __name__ == "__main__":
    unittest.main()
